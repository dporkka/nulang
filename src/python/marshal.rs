//! Bidirectional marshalling between Nulang VM Values and Python objects.
//!
//! Converts data between the Nulang runtime's NaN-tagged `Value` type and
//! Python objects managed by the CPython interpreter via PyO3.
//!
//! # Conversion Rules
//!
//! | Nulang Value | Python Object | Notes |
//! |-------------|---------------|-------|
//! | `TAG_INT` | `int` | 48-bit signed range |
//! | Float (non-NaN) | `float` | IEEE 754 double |
//! | `TAG_SPECIAL` (true/false) | `bool` | Direct mapping |
//! | `TAG_SPECIAL` (unit) | `None` | Nulang `()` → Python `None` |
//! | `TAG_STRING` | `str` | Interned string (ID → string lookup) |
//! | `TAG_PYTHON` | opaque PyObject | Direct registry lookup |
//! | `TAG_PTR` | `None` | Heap pointers are opaque |
//! | Python `str` | `TAG_PYTHON` | Stored as opaque Python object |
//! | Python `list` | `TAG_PYTHON` | Stored as opaque Python object |
//! | Python `dict` | `TAG_PYTHON` | Stored as opaque Python object |
//! | Other Python | `TAG_PYTHON` | Stored as opaque Python object |

use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple, PyDictMethods, PyListMethods};

use crate::python::bridge::{register_object, get_object, PythonObjectId, PyBridge};
use crate::vm::Value;

// ---------------------------------------------------------------------------
// Constants (mirrored from vm.rs for local use)
// ---------------------------------------------------------------------------

const TAG_MASK: u64 = 0xFFFF000000000000;
const TAG_INT: u64 = 0x7FF9000000000000;
const TAG_PTR: u64 = 0x7FFA000000000000;
const TAG_ACTOR: u64 = 0x7FFB000000000000;
const TAG_SPECIAL: u64 = 0x7FFC000000000000;
const TAG_STRING: u64 = 0x7FFD000000000000;
const TAG_PYTHON: u64 = 0x7FFE000000000000;

const SPECIAL_UNIT: u64 = 0;
const SPECIAL_TRUE: u64 = 1;
const SPECIAL_FALSE: u64 = 2;
const SPECIAL_NIL: u64 = 3;

// ---------------------------------------------------------------------------
// Nulang → Python
// ---------------------------------------------------------------------------

/// Convert a Nulang `Value` to a Python object.
///
/// This function acquires the GIL internally and returns a `PyObject`
/// (a refcounted handle). The caller is responsible for managing the
/// returned object's lifecycle — typically by inserting it into the
/// [`PYTHON_REGISTRY`](crate::python::bridge::PYTHON_REGISTRY).
///
/// # Tag Mapping
///
/// | Nulang Tag | Python Type |
/// |------------|-------------|
/// | `TAG_INT` | `int` |
/// | Float | `float` |
/// | `TAG_SPECIAL` (true/false) | `bool` |
/// | `TAG_SPECIAL` (unit/nil) | `None` |
/// | `TAG_STRING` | `str` (via interned string ID) |
/// | `TAG_PYTHON` | opaque `PyObject` (registry lookup) |
/// | `TAG_PTR` | `None` (opaque heap pointer) |
/// | `TAG_ACTOR` | `None` (opaque actor reference) |
pub fn nulang_to_python(value: Value) -> Result<PyObject, String> {
    let tag = value.0 & TAG_MASK;

    if tag == TAG_INT {
        // TAG_INT → Python int
        let bits = value.0 & 0x0000FFFFFFFFFFFF;
        let n = if bits & 0x0000800000000000 != 0 {
            (bits | 0xFFFF000000000000) as i64
        } else {
            bits as i64
        };
        Ok(Python::with_gil(|py| n.into_py(py)))
    } else if tag == TAG_SPECIAL {
        // TAG_SPECIAL → Python bool or None
        let s = value.0 & 0xFFFF;
        match s {
            SPECIAL_TRUE => Ok(Python::with_gil(|py| true.into_py(py))),
            SPECIAL_FALSE => Ok(Python::with_gil(|py| false.into_py(py))),
            SPECIAL_UNIT | SPECIAL_NIL => {
                Ok(Python::with_gil(|py| py.None()))
            }
            _ => {
                // Unknown special value — return None as fallback
                Ok(Python::with_gil(|py| py.None()))
            }
        }
    } else if tag == TAG_STRING {
        // TAG_STRING → Python str
        // Nulang strings are interned; the payload is a u32 string pool ID.
        // Since we don't have the string pool here, we represent it as a
        // descriptive string. A future optimization will resolve the ID
        // through the string interner.
        let string_id = (value.0 & 0x0000FFFFFFFFFFFF) as u32;
        let repr = format!("<nulang_string:{}>", string_id);
        Ok(Python::with_gil(|py| repr.into_py(py)))
    } else if tag == TAG_PYTHON {
        // TAG_PYTHON → look up in registry and return the PyObject
        let obj_id = (value.0 & 0x0000FFFFFFFFFFFF) as u64;
        let py_id = PythonObjectId(obj_id);
        get_object(py_id)
            .ok_or_else(|| format!("TAG_PYTHON object with ID {} not found in registry", obj_id))
    } else if tag == TAG_PTR {
        // TAG_PTR → None (heap pointers are opaque to Python)
        Ok(Python::with_gil(|py| py.None()))
    } else if tag == TAG_ACTOR {
        // TAG_ACTOR → None (actor references are opaque to Python)
        Ok(Python::with_gil(|py| py.None()))
    } else {
        // Check if it's a float (non-NaN IEEE 754 value)
        let f = f64::from_bits(value.0);
        if !f.is_nan() {
            Ok(Python::with_gil(|py| f.into_py(py)))
        } else {
            // Unrecognized NaN-tagged value — return None as fallback
            Ok(Python::with_gil(|py| py.None()))
        }
    }
}

// ---------------------------------------------------------------------------
// Python → Nulang
// ---------------------------------------------------------------------------

/// Convert a Python object (as a `Bound` reference) to a Nulang `Value`.
///
/// The caller must hold the GIL — this function takes a `&Bound<'_, PyAny>`
/// which can only be obtained within a `Python::with_gil` scope.
///
/// # Type Mapping
///
/// | Python Type | Nulang Value | Notes |
/// |-------------|-------------|-------|
/// | `None` | `Value::unit()` | |
/// | `bool` | `Value::bool()` | |
/// | `int` | `Value::int()` | Clamped to i64 range |
/// | `float` | `Value::float()` | |
/// | `str` | `TAG_PYTHON` | Stored as opaque Python object |
/// | `list` | `TAG_PYTHON` | Stored as opaque Python object |
/// | `tuple` | `TAG_PYTHON` | Stored as opaque Python object |
/// | `dict` | `TAG_PYTHON` | Stored as opaque Python object |
/// | Other | `TAG_PYTHON` | Stored as opaque Python object |
pub fn python_to_nulang(obj: &pyo3::Bound<'_, pyo3::PyAny>) -> Result<Value, String> {
    // Check for None first
    if obj.is_none() {
        return Ok(Value::unit());
    }

    // Check for bool (must come before int, since bool subclasses int in Python)
    if let Ok(b) = obj.downcast::<PyBool>() {
        let val: bool = b.extract().map_err(|e| format!("Failed to extract bool: {}", e))?;
        return Ok(Value::bool(val));
    }

    // Check for int
    if let Ok(i) = obj.downcast::<PyInt>() {
        let val: i64 = i
            .extract()
            .unwrap_or_else(|_| {
                // Big int — try to extract and clamp
                Python::with_gil(|_py| {
                    let big_int_str = i.str().map(|s| s.to_string()).unwrap_or_default();
                    big_int_str.parse::<i64>().unwrap_or(i64::MAX)
                })
            });
        return Ok(Value::int(val));
    }

    // Check for float
    if let Ok(f) = obj.downcast::<PyFloat>() {
        let val: f64 = f.extract().map_err(|e| format!("Failed to extract float: {}", e))?;
        return Ok(Value::float(val));
    }

    // Check for str
    if let Ok(_s) = obj.downcast::<PyString>() {
        // Store as opaque Python object (TAG_PYTHON).
        // Future optimization: intern the string and use TAG_STRING.
        let py_obj: PyObject = obj.clone().unbind().into();
        let id = register_object(py_obj);
        return Ok(Value(TAG_PYTHON | id.0));
    }

    // Check for list
    if let Ok(_lst) = obj.downcast::<PyList>() {
        let py_obj: PyObject = obj.clone().unbind().into();
        let id = register_object(py_obj);
        return Ok(Value(TAG_PYTHON | id.0));
    }

    // Check for tuple
    if let Ok(_tup) = obj.downcast::<PyTuple>() {
        let py_obj: PyObject = obj.clone().unbind().into();
        let id = register_object(py_obj);
        return Ok(Value(TAG_PYTHON | id.0));
    }

    // Check for dict
    if let Ok(_d) = obj.downcast::<PyDict>() {
        let py_obj: PyObject = obj.clone().unbind().into();
        let id = register_object(py_obj);
        return Ok(Value(TAG_PYTHON | id.0));
    }

    // Any other Python type — store as opaque Python object
    let py_obj: PyObject = obj.clone().unbind().into();
    let id = register_object(py_obj);
    Ok(Value(TAG_PYTHON | id.0))
}

// ---------------------------------------------------------------------------
// Higher-level conversion helpers
// ---------------------------------------------------------------------------

/// Convert a Nulang `Value` to a `PythonObjectId` (inserts into registry).
///
/// This is a convenience function that first converts the `Value` to a
/// Python object via [`nulang_to_python`], then inserts the result into
/// the global registry and returns its handle.
///
/// # Errors
///
/// Returns an error if the Nulang value cannot be converted, or if the
/// registry lookup for `TAG_PYTHON` values fails.
pub fn value_to_python_object_id(val: Value) -> Result<PythonObjectId, String> {
    let py_obj = nulang_to_python(val)?;
    Ok(register_object(py_obj))
}

/// Convert a `PythonObjectId` to a Nulang `Value`.
///
/// Looks up the object in the global registry, then converts it via
/// [`python_to_nulang`]. The resulting `Value` will have `TAG_PYTHON`
/// for complex Python objects, or a more specific tag for primitive
/// values (int, float, bool, unit).
///
/// # Errors
///
/// Returns an error if the object ID is not found in the registry.
pub fn python_object_id_to_value(obj_id: PythonObjectId) -> Result<Value, String> {
    let py_obj = get_object(obj_id)
        .ok_or_else(|| format!("Python object ID {:?} not found in registry", obj_id))?;

    Python::with_gil(|py| {
        let bound = py_obj.bind(py);
        python_to_nulang(bound)
    })
}

// ---------------------------------------------------------------------------
// Compatibility wrappers (bridge parameter ignored — uses global registry)
// ---------------------------------------------------------------------------

/// Convert a Nulang `Value` to a `PythonObjectId`.
///
/// Wrapper with the interface expected by the VM. The `bridge` parameter
/// is accepted for API consistency but the global registry is used.
pub fn value_to_python_object(val: Value, _bridge: &PyBridge) -> Result<PythonObjectId, String> {
    value_to_python_object_id(val)
}

/// Convert a `PythonObjectId` to a Nulang `Value`.
///
/// Wrapper with the interface expected by the VM. The `bridge` parameter
/// is accepted for API consistency but the global registry is used.
pub fn python_object_to_value(obj_id: PythonObjectId, _bridge: &PyBridge) -> Result<Value, String> {
    python_object_id_to_value(obj_id)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python::bridge::{PyBridge, register_object, unregister_object};

    // Helper: ensure Python is initialized
    fn ensure_python() {
        let _ = Python::with_gil(|_py| ());
    }

    // ------------------------------------------------------------------
    // Integer roundtrip
    // ------------------------------------------------------------------

    #[test]
    fn test_marshal_int_roundtrip() {
        ensure_python();

        let original = Value::int(42);
        let py_obj = nulang_to_python(original).expect("nulang_to_python failed");

        // Convert back
        let restored = Python::with_gil(|py| {
            let bound = py_obj.bind(py);
            python_to_nulang(bound).expect("python_to_nulang failed")
        });

        assert_eq!(restored.as_int(), Some(42), "Expected 42, got {:?}", restored.to_string_repr());
    }

    #[test]
    fn test_marshal_int_negative_roundtrip() {
        ensure_python();

        let original = Value::int(-1000);
        let py_obj = nulang_to_python(original).expect("nulang_to_python failed");

        let restored = Python::with_gil(|py| {
            let bound = py_obj.bind(py);
            python_to_nulang(bound).expect("python_to_nulang failed")
        });

        assert_eq!(restored.as_int(), Some(-1000));
    }

    // ------------------------------------------------------------------
    // Float roundtrip
    // ------------------------------------------------------------------

    #[test]
    fn test_marshal_float_roundtrip() {
        ensure_python();

        let original = Value::float(3.14159);
        let py_obj = nulang_to_python(original).expect("nulang_to_python failed");

        let restored = Python::with_gil(|py| {
            let bound = py_obj.bind(py);
            python_to_nulang(bound).expect("python_to_nulang failed")
        });

        let val = restored.as_float().expect("Expected float");
        assert!((val - 3.14159).abs() < 1e-5, "Expected ~3.14159, got {}", val);
    }

    // ------------------------------------------------------------------
    // Bool roundtrip
    // ------------------------------------------------------------------

    #[test]
    fn test_marshal_bool_roundtrip() {
        ensure_python();

        for &original_bool in &[true, false] {
            let original = Value::bool(original_bool);
            let py_obj = nulang_to_python(original).expect("nulang_to_python failed");

            let restored = Python::with_gil(|py| {
                let bound = py_obj.bind(py);
                python_to_nulang(bound).expect("python_to_nulang failed")
            });

            assert_eq!(
                restored.as_bool(),
                Some(original_bool),
                "Bool roundtrip failed for {}",
                original_bool
            );
        }
    }

    // ------------------------------------------------------------------
    // Unit → None → unit roundtrip
    // ------------------------------------------------------------------

    #[test]
    fn test_marshal_unit_to_none() {
        ensure_python();

        let original = Value::unit();
        let py_obj = nulang_to_python(original).expect("nulang_to_python failed");

        // Verify it's None
        let is_none = Python::with_gil(|py| {
            let bound = py_obj.bind(py);
            bound.is_none()
        });
        assert!(is_none, "Value::unit() should convert to Python None");

        // Convert back
        let restored = Python::with_gil(|py| {
            let bound = py_obj.bind(py);
            python_to_nulang(bound).expect("python_to_nulang failed")
        });
        assert!(restored.is_unit(), "None should convert back to unit");
    }

    // ------------------------------------------------------------------
    // Python object roundtrip (TAG_PYTHON)
    // ------------------------------------------------------------------

    #[test]
    fn test_marshal_python_object() {
        ensure_python();

        // Create a Python object and register it
        let py_id = Python::with_gil(|py| {
            let obj: PyObject = 99i64.into_py(py);
            register_object(obj)
        });

        // Create a Nulang Value referencing it
        let val = Value(TAG_PYTHON | py_id.0);

        // Convert to Python
        let py_obj = nulang_to_python(val).expect("nulang_to_python failed");

        // Verify it's the same value
        Python::with_gil(|py| {
            let bound = py_obj.bind(py);
            let extracted: i64 = bound.extract().expect("Expected int");
            assert_eq!(extracted, 99, "TAG_PYTHON object should resolve to 99");
        });
    }

    // ------------------------------------------------------------------
    // Opaque Python types (list, dict) stay as TAG_PYTHON
    // ------------------------------------------------------------------

    #[test]
    fn test_marshal_list_stays_opaque() {
        ensure_python();

        // Create a Python list
        let list_id = Python::with_gil(|py| {
            let list = PyList::new_bound(py, &[1i64, 2i64, 3i64]);
            let obj: PyObject = list.unbind().into();
            register_object(obj)
        });

        // Get the list from registry and convert to Nulang
        let py_obj = get_object(list_id).unwrap();
        let val = Python::with_gil(|py| {
            let bound = py_obj.bind(py);
            python_to_nulang(bound).expect("python_to_nulang failed")
        });

        // Verify it has TAG_PYTHON tag
        assert_eq!(
            val.0 & TAG_MASK,
            TAG_PYTHON,
            "Python list should be stored as TAG_PYTHON, got tag 0x{:016X}",
            val.0 & TAG_MASK
        );

        // Cleanup
        unregister_object(list_id);
    }

    #[test]
    fn test_marshal_dict_stays_opaque() {
        ensure_python();

        // Create a Python dict
        let dict_id = Python::with_gil(|py| {
            let dict = PyDict::new_bound(py);
            dict.set_item("key", 42i64).unwrap();
            let obj: PyObject = dict.unbind().into();
            register_object(obj)
        });

        // Convert to Nulang
        let py_obj = get_object(dict_id).unwrap();
        let val = Python::with_gil(|py| {
            let bound = py_obj.bind(py);
            python_to_nulang(bound).expect("python_to_nulang failed")
        });

        // Verify it has TAG_PYTHON tag
        assert_eq!(
            val.0 & TAG_MASK,
            TAG_PYTHON,
            "Python dict should be stored as TAG_PYTHON"
        );

        // Cleanup
        unregister_object(dict_id);
    }

    #[test]
    fn test_marshal_str_stays_opaque() {
        ensure_python();

        // Create a Python string
        let str_id = Python::with_gil(|py| {
            let s: PyObject = "hello nulang".into_py(py);
            register_object(s)
        });

        // Convert to Nulang
        let py_obj = get_object(str_id).unwrap();
        let val = Python::with_gil(|py| {
            let bound = py_obj.bind(py);
            python_to_nulang(bound).expect("python_to_nulang failed")
        });

        // Verify it has TAG_PYTHON tag (strings from Python stay opaque)
        assert_eq!(
            val.0 & TAG_MASK,
            TAG_PYTHON,
            "Python str should be stored as TAG_PYTHON"
        );

        // Cleanup
        unregister_object(str_id);
    }

    // ------------------------------------------------------------------
    // Higher-level helper tests
    // ------------------------------------------------------------------

    #[test]
    fn test_value_to_python_object_id_and_back() {
        ensure_python();

        let original = Value::float(2.71828);

        // Value → PythonObjectId
        let py_id = value_to_python_object_id(original).expect("value_to_python_object_id failed");

        // PythonObjectId → Value
        let restored = python_object_id_to_value(py_id).expect("python_object_id_to_value failed");

        let val = restored.as_float().expect("Expected float");
        assert!((val - 2.71828).abs() < 1e-5, "Expected ~2.71828, got {}", val);
    }

    #[test]
    fn test_marshal_nil_roundtrip() {
        ensure_python();

        let original = Value::nil();
        let py_obj = nulang_to_python(original).expect("nulang_to_python failed");

        let is_none = Python::with_gil(|py| {
            let bound = py_obj.bind(py);
            bound.is_none()
        });
        assert!(is_none, "Value::nil() should convert to Python None");
    }

    #[test]
    fn test_marshal_float_special_values() {
        ensure_python();

        for &special in &[0.0, -0.0, f64::INFINITY, f64::NEG_INFINITY] {
            let original = Value::float(special);
            let py_obj = nulang_to_python(original).expect("nulang_to_python failed");

            let restored = Python::with_gil(|py| {
                let bound = py_obj.bind(py);
                python_to_nulang(bound).expect("python_to_nulang failed")
            });

            let val = restored.as_float().expect("Expected float");
            if special.is_infinite() {
                assert_eq!(
                    val.is_infinite(),
                    true,
                    "Expected infinite, got {}",
                    val
                );
                assert_eq!(
                    val.is_sign_positive(),
                    special.is_sign_positive(),
                    "Sign mismatch for infinity"
                );
            } else {
                assert!(
                    (val - special).abs() < f64::EPSILON,
                    "Expected {}, got {}",
                    special,
                    val
                );
            }
        }
    }
}
