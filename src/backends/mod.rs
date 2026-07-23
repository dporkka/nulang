//! Backend trait boundaries — the longevity layer that decouples the
//! language from its transient dependencies.
//!
//! Every backend (JIT, WASM, storage, transport) is accessed through a trait
//! defined here. The core language (`src/vm.rs`, `src/runtime/`, `src/main.rs`)
//! talks only to these traits. Concrete implementations live in their existing
//! modules (`src/jit/`, `src/mir_wasm.rs`, `src/wasm_runtime.rs`,
//! `src/runtime/persistence.rs`, `src/runtime/network.rs`) and are selected at
//! link time via feature flags.
//!
//! This means a 2125 Nulang runtime can swap Cranelift for whatever codegen
//! exists then, Wasmtime for whatever WASM runtime exists then, and
//! quinn/rustls for whatever transport exists then — without touching
//! `src/vm.rs`, `src/bytecode.rs`, or any user program.
//!
//! # Current status
//!
//! - [`StorageBackend`] aliases the existing [`crate::runtime::PersistenceStore`]
//!   trait — storage was already behind a trait. This module makes the
//!   boundary explicit and discoverable.
//! - [`JitBackend`], [`WasmBackend`], [`Transport`] are defined here as the
//!   target interfaces. The existing concrete impls (`src/jit/`,
//!   `src/mir_wasm.rs` + `src/wasm_runtime.rs`, `src/runtime/network.rs`)
//!   will be wired behind these traits incrementally (RFC 0003, item 6).

use crate::bytecode::CodeModule;
use crate::mir::Module as MirModule;
use crate::types::NuResult;
use crate::vm::Value;

// ---------------------------------------------------------------------------
// Storage backend — already exists as PersistenceStore, re-exported here
// ---------------------------------------------------------------------------

/// The storage backend trait. This is the single point through which the
/// runtime accesses durable storage. Concrete impls: `MemoryStore`,
/// `JsonFileStore`, `SqliteStore` (feature `sqlite`).
///
/// This is a re-export of [`crate::runtime::PersistenceStore`] — storage was
/// already behind a trait. This alias makes the boundary discoverable from
/// one place.
pub trait StorageBackend: crate::runtime::PersistenceStore {}
impl<T: crate::runtime::PersistenceStore> StorageBackend for T {}

// ---------------------------------------------------------------------------
// JIT backend — the interface for register-VM JIT compilers
// ---------------------------------------------------------------------------

/// Action taken by the tiered execution system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TieredAction {
    /// The JIT could not run; fall back to the interpreter.
    Interpret,
    /// JIT-compiled code was executed; advance the PC.
    RanJit,
    /// The region was SIMD-vectorized, compiled, and executed.
    /// (Unused while the SIMD path is gated off; kept for API stability.)
    CompiledSimdAndRan,
}

/// A JIT backend compiles hot bytecode regions into native code for faster
/// execution. The default implementation uses Cranelift (`src/jit/`). A future
/// runtime could implement this trait with LLVM, GCC JIT, or whatever
/// codegen exists in 2125.
pub trait JitBackend {
    /// Whether a region at `(module_idx, pc)` has already been compiled.
    fn is_compiled(&self, module_idx: usize, pc: usize) -> bool;

    /// Record one interpretation and return `true` when the region is hot.
    fn record_and_check_hot(&mut self, module_idx: usize, pc: usize) -> bool;

    /// Number of bytecode instructions in the compiled region at `(module_idx, pc)`.
    fn compiled_region_len(&self, module_idx: usize, pc: usize) -> Option<usize>;

    /// Number of regions compiled (scalar path).
    fn compiled_count(&self) -> usize;

    /// Number of regions compiled through the type-directed path.
    fn typed_compiled_count(&self) -> usize;

    /// Reset hot counters.
    fn reset_hot_counters(&mut self);

    /// Execute one tiered step: if the region at `pc` is compiled, run it;
    /// if hot, compile then run; otherwise record and return `Interpret`.
    fn tiered_execute_step_typed(
        &mut self,
        module_idx: usize,
        pc: usize,
        module: &CodeModule,
        regs: &mut [u64; 256],
        constants: &[u64],
    ) -> TieredAction;
}

// ---------------------------------------------------------------------------
// WASM backend — the interface for MIR→WASM compilers + host runtimes
// ---------------------------------------------------------------------------

/// A WASM backend compiles MIR to a `.wasm` module and provides a host
/// runtime to execute it. The default implementation uses `wasm-encoder` +
/// `wasmtime` (`src/mir_wasm.rs` + `src/wasm_runtime.rs`, feature
/// `wasm-backend`). A future runtime could implement this trait with a
/// different WASM compiler or host.
pub trait WasmBackend: Send {
    /// Compile a MIR module to WASM bytes.
    fn compile(&mut self, module: &MirModule, name: &str) -> NuResult<Vec<u8>>;

    /// Run a compiled WASM module. Returns the tagged program result.
    fn run(&mut self, wasm: &[u8]) -> NuResult<Value>;
}

// ---------------------------------------------------------------------------
// Default WASM backend impl — delegates to mir_wasm + wasm_runtime
// ---------------------------------------------------------------------------

/// The default WASM backend: compiles via `mir_wasm::WasmBackend`,
/// runs via `wasm_runtime::WasmRuntime`.
#[cfg(feature = "wasm-backend")]
pub struct DefaultWasmBackend;

#[cfg(feature = "wasm-backend")]
impl WasmBackend for DefaultWasmBackend {
    fn compile(&mut self, module: &MirModule, name: &str) -> NuResult<Vec<u8>> {
        crate::mir_wasm::WasmBackend::new().compile(module, name)
    }

    fn run(&mut self, wasm: &[u8]) -> NuResult<Value> {
        crate::wasm_runtime::WasmRuntime::new(wasm, None)?.run()
    }
}

// ---------------------------------------------------------------------------
// HTTP provider — the interface for outbound HTTP requests
// ---------------------------------------------------------------------------

/// An HTTP provider makes outbound HTTP requests. The default implementation
/// uses `reqwest`. A future runtime could implement this with hyper, curl,
/// or whatever HTTP client exists in 2125.
pub trait HttpProvider: Send + Sync {
    /// Perform a synchronous POST with a JSON body and return the response body.
    fn post_json(&self, url: &str, body: &str) -> Result<String, String>;

    /// Perform a synchronous GET and return the response body.
    fn get(&self, url: &str) -> Result<String, String>;
}

/// Default HTTP provider backed by `reqwest` (requires `ai-runtime` feature).
#[cfg(feature = "ai-runtime")]
#[derive(Debug, Clone)]
pub struct ReqwestHttpProvider {
    client: reqwest::Client,
}

#[cfg(feature = "ai-runtime")]
impl ReqwestHttpProvider {
    /// Create a new reqwest-backed HTTP provider with a default timeout.
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        ReqwestHttpProvider { client }
    }
}

#[cfg(feature = "ai-runtime")]
impl Default for ReqwestHttpProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "ai-runtime")]
impl HttpProvider for ReqwestHttpProvider {
    fn post_json(&self, url: &str, body: &str) -> Result<String, String> {
        let client = self.client.clone();
        tokio::runtime::Handle::try_current()
            .map_err(|_| "no Tokio runtime available".to_string())?
            .block_on(async {
                client
                    .post(url)
                    .header("Content-Type", "application/json")
                    .body(body.to_string())
                    .send()
                    .await
                    .map_err(|e| e.to_string())?
                    .text()
                    .await
                    .map_err(|e| e.to_string())
            })
    }

    fn get(&self, url: &str) -> Result<String, String> {
        let client = self.client.clone();
        tokio::runtime::Handle::try_current()
            .map_err(|_| "no Tokio runtime available".to_string())?
            .block_on(async {
                client
                    .get(url)
                    .send()
                    .await
                    .map_err(|e| e.to_string())?
                    .text()
                    .await
                    .map_err(|e| e.to_string())
            })
    }
}

// ---------------------------------------------------------------------------
// Transport backend — the interface for network transports
// ---------------------------------------------------------------------------

/// A transport backend provides point-to-point packet delivery between
/// cluster nodes. The default implementation uses TCP
/// (`src/runtime/network.rs`). A future runtime could implement this with
/// QUIC, UDP, or whatever transport exists in 2125.
///
/// This trait mirrors the existing [`crate::runtime::NetworkTransport`] trait
/// — network transport was already behind a trait. This re-export makes the
/// boundary discoverable from one place.
pub trait Transport: crate::runtime::NetworkTransport {}
impl<T: crate::runtime::NetworkTransport> Transport for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_backend_is_persistence_store() {
        // StorageBackend is auto-implemented for any PersistenceStore.
        fn accepts_storage<S: StorageBackend>(_s: &S) {}
        fn accepts_persistence<P: crate::runtime::PersistenceStore>(p: &P) {
            accepts_storage(p);
        }
        let store = crate::runtime::MemoryStore::new();
        accepts_persistence(&store);
    }

    #[test]
    fn test_transport_is_network_transport() {
        fn check_blanket<T: crate::runtime::NetworkTransport>() {
            fn _assert_trait_object(_: &dyn Transport) {}
        }
        check_blanket::<crate::runtime::TcpTransport>();
    }

    #[cfg(feature = "ai-runtime")]
    #[test]
    fn test_http_provider_is_object_safe() {
        fn accepts_http(h: &dyn HttpProvider) {
            // Verify trait object usage compiles (no runtime needed for type-check).
        }
        let provider = ReqwestHttpProvider::new();
        accepts_http(&provider);
    }
}
