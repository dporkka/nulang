//! Python Interpreter Bridge for Nulang.
//!
//! Manages the Python interpreter lifecycle, GIL acquisition, module imports,
//! and Python object reference counting via a global registry.
//!
//! All Python operations acquire the GIL through `Python::with_gil` ensuring
//! thread-safe interaction with the CPython interpreter.
//!
//! # Registry Design
//!
//! Python objects are stored in a global `Mutex`-protected registry. Each
//! object is assigned a `PythonObjectId` (a u64 index), which is cheap to
//! pass across the FFI boundary. The `PyObject` handle is a refcounted
//! pointer — cloning it merely increments the Python reference count.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// NaN tag for Python object references stored in Nulang `Value`s.
/// See `crate::vm::Value` for the tagging scheme.
pub const TAG_PYTHON: u64 = 0x7FFE000000000000;

// ---------------------------------------------------------------------------
// PythonObjectId — opaque handle to a registered Python object
// ---------------------------------------------------------------------------

/// An opaque handle to a Python object stored in the global registry.
///
/// This is a cheap `Copy` type (just a `u64`) that can be freely passed
/// across the Nulang FFI boundary. The actual `PyObject` lives in the
/// `PYTHON_REGISTRY` and is reference-counted by both the registry and
/// any active GIL scopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PythonObjectId(pub u64);

impl PythonObjectId {
    /// Create a new `PythonObjectId` from a raw `u64` value.
    pub fn new(id: u64) -> Self {
        PythonObjectId(id)
    }
}

// ---------------------------------------------------------------------------
// PythonRegistry — thread-safe storage for Python objects
// ---------------------------------------------------------------------------

/// Thread-safe registry for storing Python object references.
///
/// Uses a `Vec<Option<PyObject>>` as a slab allocator — removed entries
/// become `None` slots. A future optimization could reuse these slots
/// via a free-list.
pub struct PythonRegistry {
    /// Indexed storage of Python objects. `None` entries represent
    /// freed slots.
    objects: Vec<Option<PyObject>>,
    /// Monotonically increasing counter for the next object ID.
    next_id: u64,
}

impl PythonRegistry {
    /// Create a new, empty registry.
    pub fn new() -> Self {
        PythonRegistry {
            objects: Vec::new(),
            next_id: 0,
        }
    }

    /// Insert a Python object into the registry and return its handle.
    ///
    /// The object is stored as a `PyObject` (a refcounted handle to the
    /// Python object). Cloning the handle is cheap — it merely increments
    /// the Python reference count.
    pub fn insert(&mut self, obj: PyObject) -> PythonObjectId {
        let id = self.next_id;
        self.next_id += 1;

        // Grow the vector if needed, otherwise insert
        if (id as usize) < self.objects.len() {
            self.objects[id as usize] = Some(obj);
        } else {
            // Pad with None if somehow there's a gap (shouldn't happen with
            // monotonic IDs, but defensively handle it)
            while self.objects.len() < id as usize {
                self.objects.push(None);
            }
            self.objects.push(Some(obj));
        }
        PythonObjectId(id)
    }

    /// Retrieve a clone of the `PyObject` handle for the given ID.
    ///
    /// Returns `None` if the ID is invalid or the object has been removed.
    /// The returned `PyObject` can be used within a `Python::with_gil`
    /// scope to access the underlying Python object.
    pub fn get(&self, id: PythonObjectId) -> Option<PyObject> {
        self.objects.get(id.0 as usize)?.as_ref().cloned()
    }

    /// Remove a Python object from the registry.
    ///
    /// After removal, the registry slot becomes `None`. When the last
    /// `PyObject` clone is dropped, Python's reference count reaches zero
    /// and the object becomes eligible for garbage collection.
    pub fn remove(&mut self, id: PythonObjectId) {
        if let Some(slot) = self.objects.get_mut(id.0 as usize) {
            *slot = None;
        }
    }

    /// Return the number of objects currently stored in the registry.
    pub fn get_count(&self) -> usize {
        self.objects.iter().filter(|o| o.is_some()).count()
    }
}

impl Default for PythonRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Global Registry
// ---------------------------------------------------------------------------

/// Global, lazily-initialized Python object registry.
///
/// All `PyObject` references are stored here and accessed via
/// `PythonObjectId` handles. The `Mutex` ensures thread-safe access
/// to the registry itself; individual `PyObject` operations still
/// require the GIL.
static PYTHON_REGISTRY: OnceLock<Mutex<PythonRegistry>> = OnceLock::new();

/// Get a reference to the global Python registry.
///
/// Panics if the mutex is poisoned (another thread panicked while holding
/// the lock).
pub fn global_registry() -> std::sync::MutexGuard<'static, PythonRegistry> {
    PYTHON_REGISTRY
        .get_or_init(|| Mutex::new(PythonRegistry::new()))
        .lock()
        .expect("Python registry mutex poisoned")
}

/// Convenience: insert a `PyObject` into the global registry.
pub fn register_object(obj: PyObject) -> PythonObjectId {
    global_registry().insert(obj)
}

/// Convenience: get a `PyObject` from the global registry.
pub fn get_object(id: PythonObjectId) -> Option<PyObject> {
    global_registry().get(id)
}

/// Convenience: remove a `PyObject` from the global registry.
pub fn unregister_object(id: PythonObjectId) {
    global_registry().remove(id);
}

// ---------------------------------------------------------------------------
// PyBridge — main Python interop interface
// ---------------------------------------------------------------------------

/// The main Python interoperability bridge.
///
/// `PyBridge` provides a high-level API for importing Python modules,
/// accessing attributes, calling functions, and creating Python data
/// structures. All operations acquire the GIL automatically via
/// `Python::with_gil`.
///
/// # Example
/// ```ignore
/// let mut bridge = PyBridge::new();
/// bridge.initialize().expect("Failed to init Python");
/// let math = bridge.import_module("math").unwrap();
/// let pi = bridge.get_attr(math, "pi").unwrap();
/// let sqrt = bridge.get_attr(math, "sqrt").unwrap();
/// let result = bridge.call(sqrt, vec![pi]).unwrap();
/// ```
pub struct PyBridge {
    /// Cache of imported modules: module name → registry ID.
    /// Avoids repeated imports of the same module.
    module_cache: HashMap<String, PythonObjectId>,
}

impl PyBridge {
    /// Create a new `PyBridge` instance.
    ///
    /// Does **not** initialize the Python interpreter — call
    /// [`initialize`](Self::initialize) before use.
    pub fn new() -> Self {
        PyBridge {
            module_cache: HashMap::new(),
        }
    }

    /// Initialize the Python interpreter.
    ///
    /// With PyO3's `auto-initialize` feature, this is usually a no-op
    /// if Python has already been initialized. Returns an error if
    /// initialization fails.
    pub fn initialize(&self) -> Result<(), String> {
        // PyO3 auto-initialize handles the actual init.
        // Just verify we can acquire the GIL.
        Python::with_gil(|_py| Ok(()))
            .map_err(|e| format!("Failed to initialize Python: {}", e))
    }

    /// Check whether the Python interpreter is initialized.
    pub fn is_initialized() -> bool {
        Python::with_gil(|_py| true)
    }

    /// Import a Python module by name.
    ///
    /// Modules are cached — repeated imports of the same module return
    /// the cached `PythonObjectId`.
    pub fn import_module(&mut self, name: &str) -> Result<PythonObjectId, String> {
        // Check cache first
        if let Some(&id) = self.module_cache.get(name) {
            return Ok(id);
        }

        Python::with_gil(|py| {
            let module = py
                .import_bound(name)
                .map_err(|e| format!("Failed to import module '{}': {}", name, e))?;
            let obj: PyObject = module.unbind().into();
            let id = register_object(obj);
            self.module_cache.insert(name.to_string(), id);
            Ok(id)
        })
    }

    /// Get an attribute from a Python object.
    ///
    /// Equivalent to Python's `getattr(obj, attr)`.
    pub fn get_attr(&self, obj_id: PythonObjectId, attr: &str) -> Result<PythonObjectId, String> {
        let obj = get_object(obj_id)
            .ok_or_else(|| format!("Python object ID {:?} not found in registry", obj_id))?;

        Python::with_gil(|py| {
            let bound = obj.bind(py);
            let attr_obj = bound
                .getattr(attr)
                .map_err(|e| format!("Failed to get attribute '{}': {}", attr, e))?;
            let handle: PyObject = attr_obj.unbind().into();
            Ok(register_object(handle))
        })
    }

    /// Set an attribute on a Python object.
    ///
    /// Equivalent to Python's `setattr(obj, attr, value)`.
    pub fn set_attr(
        &self,
        obj_id: PythonObjectId,
        attr: &str,
        val_id: PythonObjectId,
    ) -> Result<(), String> {
        let obj = get_object(obj_id)
            .ok_or_else(|| format!("Python object ID {:?} not found in registry", obj_id))?;
        let val = get_object(val_id)
            .ok_or_else(|| format!("Python object ID {:?} not found in registry", val_id))?;

        Python::with_gil(|py| {
            let bound = obj.bind(py);
            let val_bound = val.bind(py);
            bound
                .setattr(attr, val_bound)
                .map_err(|e| format!("Failed to set attribute '{}': {}", attr, e))
        })
    }

    /// Call a Python callable with positional arguments.
    ///
    /// Equivalent to Python's `callable(*args)`.
    pub fn call(
        &self,
        callable_id: PythonObjectId,
        args: Vec<PythonObjectId>,
    ) -> Result<PythonObjectId, String> {
        let callable = get_object(callable_id)
            .ok_or_else(|| format!("Callable ID {:?} not found in registry", callable_id))?;

        // Collect argument objects from the registry
        let arg_objs: Vec<PyObject> = args
            .iter()
            .map(|&id| {
                get_object(id)
                    .ok_or_else(|| format!("Argument ID {:?} not found in registry", id))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Python::with_gil(|py| {
            let callable_bound = callable.bind(py);

            // Build a PyTuple from the argument objects
            let arg_refs: Vec<&pyo3::Bound<'_, pyo3::PyAny>> =
                arg_objs.iter().map(|o| o.bind(py)).collect();

            let args_tuple = PyTuple::new(py, &arg_refs)
                .map_err(|e| format!("Failed to create argument tuple: {}", e))?;

            let result = callable_bound
                .call1(args_tuple)
                .map_err(|e| format!("Python call failed: {}", e))?;

            let handle: PyObject = result.unbind().into();
            Ok(register_object(handle))
        })
    }

    /// Call a Python callable with positional and keyword arguments.
    ///
    /// Equivalent to Python's `callable(*args, **kwargs)`.
    pub fn call_kw(
        &self,
        callable_id: PythonObjectId,
        args: Vec<PythonObjectId>,
        kwargs: HashMap<String, PythonObjectId>,
    ) -> Result<PythonObjectId, String> {
        let callable = get_object(callable_id)
            .ok_or_else(|| format!("Callable ID {:?} not found in registry", callable_id))?;

        // Collect positional arg objects
        let arg_objs: Vec<PyObject> = args
            .iter()
            .map(|&id| {
                get_object(id)
                    .ok_or_else(|| format!("Argument ID {:?} not found in registry", id))
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Collect keyword arg objects
        let mut kwarg_objs: HashMap<String, PyObject> = HashMap::new();
        for (key, id) in kwargs {
            let obj = get_object(id)
                .ok_or_else(|| format!("Kwarg '{}' ID {:?} not found in registry", key, id))?;
            kwarg_objs.insert(key, obj);
        }

        Python::with_gil(|py| {
            let callable_bound = callable.bind(py);

            // Build positional tuple
            let arg_refs: Vec<&pyo3::Bound<'_, pyo3::PyAny>> =
                arg_objs.iter().map(|o| o.bind(py)).collect();
            let args_tuple = PyTuple::new(py, &arg_refs)
                .map_err(|e| format!("Failed to create argument tuple: {}", e))?;

            // Build keyword dict
            let kwargs_dict = PyDict::new(py);
            for (key, obj) in &kwarg_objs {
                kwargs_dict
                    .set_item(key, obj.bind(py))
                    .map_err(|e| format!("Failed to set kwarg '{}': {}", key, e))?;
            }

            let result = callable_bound
                .call(args_tuple, Some(&kwargs_dict))
                .map_err(|e| format!("Python call (with kwargs) failed: {}", e))?;

            let handle: PyObject = result.unbind().into();
            Ok(register_object(handle))
        })
    }

    /// Create a Python `list` from a sequence of registry object IDs.
    pub fn create_list(&self, items: Vec<PythonObjectId>) -> Result<PythonObjectId, String> {
        let item_objs: Vec<PyObject> = items
            .iter()
            .map(|&id| {
                get_object(id)
                    .ok_or_else(|| format!("List item ID {:?} not found in registry", id))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Python::with_gil(|py| {
            let item_refs: Vec<&pyo3::Bound<'_, pyo3::PyAny>> =
                item_objs.iter().map(|o| o.bind(py)).collect();
            let list = PyList::new(py, &item_refs)
                .map_err(|e| format!("Failed to create list: {}", e))?;
            let handle: PyObject = list.unbind().into();
            Ok(register_object(handle))
        })
    }

    /// Create a Python `dict` from a map of string keys to registry object IDs.
    pub fn create_dict(
        &self,
        items: HashMap<String, PythonObjectId>,
    ) -> Result<PythonObjectId, String> {
        // Pre-resolve all values from the registry
        let mut resolved: HashMap<String, PyObject> = HashMap::new();
        for (key, id) in items {
            let obj = get_object(id)
                .ok_or_else(|| format!("Dict value ID {:?} for key '{}' not found", id, key))?;
            resolved.insert(key, obj);
        }

        Python::with_gil(|py| {
            let dict = PyDict::new(py);
            for (key, obj) in &resolved {
                dict.set_item(key, obj.bind(py))
                    .map_err(|e| format!("Failed to set dict item '{}': {}", key, e))?;
            }
            let handle: PyObject = dict.unbind().into();
            Ok(register_object(handle))
        })
    }

    /// Return the number of entries in the module cache.
    pub fn module_cache_len(&self) -> usize {
        self.module_cache.len()
    }

    /// Clear the module cache, removing all cached module references.
    /// The underlying PyObjects remain in the registry until explicitly removed.
    pub fn clear_module_cache(&mut self) {
        self.module_cache.clear();
    }
}

impl Default for PyBridge {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: ensure Python is initialized for all tests
    fn ensure_python() {
        // With auto-initialize, just acquiring the GIL is enough
        let _ = Python::with_gil(|_py| ());
    }

    // ------------------------------------------------------------------
    // Registry tests
    // ------------------------------------------------------------------

    #[test]
    fn test_registry_insert_get_remove() {
        ensure_python();

        let mut reg = PythonRegistry::new();
        assert_eq!(reg.get_count(), 0);

        // Insert an object
        let obj = Python::with_gil(|py| {
            let val: PyObject = py.None();
            val
        });
        let id = reg.insert(obj);
        assert_eq!(reg.get_count(), 1);

        // Get it back
        let retrieved = reg.get(id);
        assert!(retrieved.is_some());

        // Remove it
        reg.remove(id);
        assert_eq!(reg.get_count(), 0);
        assert!(reg.get(id).is_none());
    }

    // ------------------------------------------------------------------
    // Module import tests
    // ------------------------------------------------------------------

    #[test]
    fn test_import_math() {
        let mut bridge = PyBridge::new();
        bridge.initialize().expect("Python init failed");

        let math = bridge.import_module("math");
        assert!(math.is_ok(), "Failed to import math: {:?}", math.err());
    }

    #[test]
    fn test_module_cache() {
        let mut bridge = PyBridge::new();
        bridge.initialize().expect("Python init failed");

        // First import
        let math1 = bridge.import_module("math").unwrap();
        let cache_len_after_first = bridge.module_cache_len();
        assert_eq!(cache_len_after_first, 1);

        // Second import — should be cached
        let math2 = bridge.import_module("math").unwrap();
        assert_eq!(bridge.module_cache_len(), 1);
        assert_eq!(math1, math2, "Cached module should return same ID");
    }

    // ------------------------------------------------------------------
    // Attribute access tests
    // ------------------------------------------------------------------

    #[test]
    fn test_get_attr() {
        let mut bridge = PyBridge::new();
        bridge.initialize().expect("Python init failed");

        let math = bridge.import_module("math").unwrap();
        let pi = bridge.get_attr(math, "pi");
        assert!(pi.is_ok(), "Failed to get math.pi: {:?}", pi.err());
    }

    // ------------------------------------------------------------------
    // Function call tests
    // ------------------------------------------------------------------

    #[test]
    fn test_call_python_function() {
        let mut bridge = PyBridge::new();
        bridge.initialize().expect("Python init failed");

        let math = bridge.import_module("math").unwrap();
        let sqrt = bridge.get_attr(math, "sqrt").unwrap();

        // Create arg: 16.0
        let arg = Python::with_gil(|py| {
            let val: PyObject = 16.0f64.into_py(py);
            register_object(val)
        });

        let result = bridge.call(sqrt, vec![arg]);
        assert!(result.is_ok(), "sqrt(16.0) failed: {:?}", result.err());

        // Verify result is 4.0
        let result_id = result.unwrap();
        Python::with_gil(|py| {
            let obj = get_object(result_id).unwrap();
            let bound = obj.bind(py);
            let val: f64 = bound.extract().expect("Expected float result");
            assert!((val - 4.0).abs() < f64::EPSILON, "Expected 4.0, got {}", val);
        });
    }

    #[test]
    fn test_call_with_args() {
        let mut bridge = PyBridge::new();
        bridge.initialize().expect("Python init failed");

        let builtins = bridge.import_module("builtins").unwrap();
        let max_fn = bridge.get_attr(builtins, "max").unwrap();

        // Create args: 3, 7, 1
        let args: Vec<PythonObjectId> = Python::with_gil(|py| {
            vec![3i64, 7i64, 1i64]
                .into_iter()
                .map(|n| {
                    let val: PyObject = n.into_py(py);
                    register_object(val)
                })
                .collect()
        });

        let result = bridge.call(max_fn, args);
        assert!(result.is_ok(), "max(3,7,1) failed: {:?}", result.err());

        let result_id = result.unwrap();
        Python::with_gil(|py| {
            let obj = get_object(result_id).unwrap();
            let bound = obj.bind(py);
            let val: i64 = bound.extract().expect("Expected int result");
            assert_eq!(val, 7, "Expected max=7, got {}", val);
        });
    }

    // ------------------------------------------------------------------
    // Data structure creation tests
    // ------------------------------------------------------------------

    #[test]
    fn test_create_list() {
        let mut bridge = PyBridge::new();
        bridge.initialize().expect("Python init failed");

        // Create some Python integers
        let items: Vec<PythonObjectId> = Python::with_gil(|py| {
            vec![10i64, 20i64, 30i64]
                .into_iter()
                .map(|n| {
                    let val: PyObject = n.into_py(py);
                    register_object(val)
                })
                .collect()
        });

        let list_id = bridge.create_list(items);
        assert!(list_id.is_ok(), "create_list failed: {:?}", list_id.err());

        // Verify it's a list with 3 elements
        let id = list_id.unwrap();
        Python::with_gil(|py| {
            let obj = get_object(id).unwrap();
            let bound = obj.bind(py);
            let list = bound.downcast::<PyList>();
            assert!(list.is_ok(), "Expected a PyList");
            assert_eq!(list.unwrap().len(), 3, "Expected list of length 3");
        });
    }

    #[test]
    fn test_create_dict() {
        let mut bridge = PyBridge::new();
        bridge.initialize().expect("Python init failed");

        // Create key-value pairs
        let mut items = HashMap::new();
        Python::with_gil(|py| {
            let x_val: PyObject = 1i64.into_py(py);
            let y_val: PyObject = 2i64.into_py(py);
            items.insert("x".to_string(), register_object(x_val));
            items.insert("y".to_string(), register_object(y_val));
        });

        let dict_id = bridge.create_dict(items);
        assert!(dict_id.is_ok(), "create_dict failed: {:?}", dict_id.err());

        // Verify it's a dict with 2 items
        let id = dict_id.unwrap();
        Python::with_gil(|py| {
            let obj = get_object(id).unwrap();
            let bound = obj.bind(py);
            let dict = bound.downcast::<PyDict>();
            assert!(dict.is_ok(), "Expected a PyDict");
            assert_eq!(dict.unwrap().len(), 2, "Expected dict with 2 items");
        });
    }
}
