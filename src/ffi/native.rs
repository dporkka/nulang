//! Native function and library registry.
//!
//! Provides a thread-safe global registry of dynamically loaded libraries and
//! resolved symbols. Symbols are keyed by `(library_name, symbol_name)` so the
//! same name can be provided by different libraries.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use super::marshal::Signature;

/// A loaded dynamic library.
pub struct NativeLibrary {
    inner: libloading::Library,
    name: String,
}

impl NativeLibrary {
    /// Open a dynamic library by path.
    ///
    /// # Safety
    /// The caller must ensure the path points to a valid shared library.
    pub unsafe fn open(path: &str) -> Result<Self, String> {
        let inner = unsafe { libloading::Library::new(path) }.map_err(|e| e.to_string())?;
        Ok(Self {
            inner,
            name: path.to_string(),
        })
    }

    /// Return the path/name used to open this library.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Resolve a symbol from this library.
    ///
    /// # Safety
    /// The caller must ensure the symbol actually has the requested type.
    pub unsafe fn resolve<T>(&self, symbol: &[u8]) -> Result<libloading::Symbol<'_, T>, String> {
        self.inner
            .get(symbol)
            .map_err(|e| format!("failed to resolve {}: {}", String::from_utf8_lossy(symbol), e))
    }
}

impl std::fmt::Debug for NativeLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeLibrary")
            .field("name", &self.name)
            .finish()
    }
}

/// A native function callable through the FFI layer.
///
/// The function pointer is stored as an opaque `*const c_void` so it can be
/// transmuted to the correct `extern "C"` signature at call time.
#[derive(Debug, Clone)]
pub struct NativeFunction {
    pub ptr: *const c_void,
    pub signature: Signature,
    pub library: Option<String>,
    pub symbol: String,
}

// SAFETY: `*const c_void` is used as an opaque function pointer. The registry
// guarantees that the pointed-to function outlives the registry entry, and all
// access is serialized by the enclosing `Mutex`.
unsafe impl Send for NativeFunction {}
// SAFETY: function pointers are immutable once registered; shared access is
// safe because `call_native` only reads from the pointer.
unsafe impl Sync for NativeFunction {}

impl NativeFunction {
    /// Create a native function entry from a raw C function pointer.
    ///
    /// # Safety
    /// `ptr` must point to a function whose ABI matches `signature`.
    pub unsafe fn new(
        ptr: *const c_void,
        signature: Signature,
        library: Option<String>,
        symbol: String,
    ) -> Self {
        Self {
            ptr,
            signature,
            library,
            symbol,
        }
    }
}

/// Internal registry backing the global `FFI_REGISTRY`.
#[derive(Debug, Default)]
pub struct FfiRegistry {
    functions: HashMap<(Option<String>, String), NativeFunction>,
    libraries: HashMap<String, NativeLibrary>,
}

impl FfiRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load a dynamic library and keep it open for symbol resolution.
    ///
    /// # Safety
    /// The caller must ensure `path` points to a valid shared library.
    pub unsafe fn load_library(&mut self, path: &str) -> Result<NativeLibrary, String> {
        if let Some(_lib) = self.libraries.get(path) {
            // Library is already open; return a reference-equivalent description.
            // Note: `libloading::Library` is not `Clone`, so we return a fresh
            // open handle. On most platforms this is a cheap re-open.
            return unsafe { NativeLibrary::open(path) };
        }
        let lib = unsafe { NativeLibrary::open(path) }?;
        let stored = NativeLibrary {
            inner: unsafe { libloading::Library::new(path).map_err(|e| e.to_string())? },
            name: path.to_string(),
        };
        self.libraries.insert(path.to_string(), stored);
        Ok(lib)
    }

    /// Resolve a registered native function.
    pub fn resolve(&self, library: Option<&str>, symbol: &str) -> Option<NativeFunction> {
        self.functions
            .get(&(library.map(String::from), symbol.to_string()))
            .cloned()
    }

    /// Register a native function under its symbol (and optional library).
    pub fn register(&mut self, function: NativeFunction) {
        let key = (function.library.clone(), function.symbol.clone());
        self.functions.insert(key, function);
    }

    /// Resolve a native function, loading its library on demand if necessary.
    ///
    /// First tries a pre-registered function under `(Some(library), symbol)`,
    /// then `(None, symbol)`. If neither is found, the library is opened and
    /// the symbol is resolved as an opaque function pointer.
    ///
    /// # Safety
    /// `library` must name a valid shared library when the function is not
    /// pre-registered.
    pub unsafe fn resolve_or_load(
        &mut self,
        library: &str,
        symbol: &str,
        signature: Signature,
    ) -> Result<NativeFunction, String> {
        if let Some(func) = self.resolve(Some(library), symbol) {
            return Ok(func);
        }
        if let Some(func) = self.resolve(None, symbol) {
            return Ok(func);
        }
        let lib = self.load_library(library)?;
        let symbol_name = symbol.to_string();
        // SAFETY: caller guarantees the symbol exists and has the requested type.
        let sym = unsafe { lib.resolve::<unsafe extern "C" fn()>(symbol.as_bytes())? };
        let ptr: *const c_void = *sym as *const c_void;
        let func = NativeFunction::new(
            ptr,
            signature,
            Some(library.to_string()),
            symbol_name,
        );
        self.register(func.clone());
        Ok(func)
    }
}

/// Global thread-safe FFI registry.
pub static FFI_REGISTRY: OnceLock<Mutex<FfiRegistry>> = OnceLock::new();

fn global_registry() -> &'static Mutex<FfiRegistry> {
    FFI_REGISTRY.get_or_init(|| Mutex::new(FfiRegistry::new()))
}

/// Register a native function in the global registry.
///
/// # Safety
/// `ptr` must point to a function whose C ABI matches `signature`. The
/// function must remain valid for the lifetime of the registry entry.
pub unsafe fn register_native_function(
    name: &str,
    ptr: *const c_void,
    signature: Signature,
) -> Result<(), String> {
    let func = NativeFunction::new(ptr, signature, None, name.to_string());
    let mut reg = global_registry().lock().map_err(|e| e.to_string())?;
    reg.register(func);
    Ok(())
}
