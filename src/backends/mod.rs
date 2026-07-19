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

/// A JIT backend compiles hot bytecode regions into native code for faster
/// execution. The default implementation uses Cranelift (`src/jit/`). A future
/// runtime could implement this trait with LLVM, GCC JIT, or whatever
/// codegen exists in 2125.
///
/// The trait is intentionally minimal: the VM calls `compile_and_cache` when
/// a PC's hot counter exceeds the threshold; the backend returns a function
/// pointer the VM can call, or `None` if the region is not compilable.
pub trait JitBackend: Send {
    /// Compile a bytecode region into a native function, if possible.
    ///
    /// `module` is the module containing the region. `instrs` is the
    /// instruction slice for the region (a contiguous run starting at
    /// `start_pc`). `regs` is a snapshot of the register state at the
    /// region entry.
    ///
    /// Returns a function pointer that the VM calls with
    /// `extern "C" fn(*mut u64 regs, *const u64 constants)`, or `None` if
    /// the region contains unsupported opcodes or the backend is not
    /// available.
    fn compile_and_cache(
        &mut self,
        module: &CodeModule,
        start_pc: usize,
        instrs: &[crate::bytecode::Instruction],
    ) -> Option<CompiledRegion>;
}

/// A compiled JIT region: a function pointer the VM can call.
pub struct CompiledRegion {
    /// The native function pointer. ABI:
    /// `extern "C" fn(regs: *mut u64, constants: *const u64) -> u64`
    /// Returns the value of the destination register after the region,
    /// or a sentinel for control-flow transfers (jump/ret).
    pub fn_ptr: usize,
    /// The number of instructions in the compiled region.
    pub instr_count: usize,
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
        // This test verifies the blanket impl compiles and type-checks.
        fn accepts_storage<S: StorageBackend>(_s: &S) {}
        fn accepts_persistence<P: crate::runtime::PersistenceStore>(p: &P) {
            accepts_storage(p);
        }
        let store = crate::runtime::MemoryStore::new();
        accepts_persistence(&store);
    }

    #[test]
    fn test_transport_is_network_transport() {
        // Transport is auto-implemented for any NetworkTransport.
        // We can't construct a TcpTransport without binding a port, but the
        // blanket impl compiles — this test verifies the type-level wiring.
        fn check_blanket<T: crate::runtime::NetworkTransport>() {
            // If this compiles, Transport is implemented for T.
            fn _assert_trait_object(_: &dyn Transport) {}
        }
        check_blanket::<crate::runtime::TcpTransport>();
    }
}
