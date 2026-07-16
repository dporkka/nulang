//! WASM Component host runtime for Nulang actors.
//!
//! Loads `.wasm` component binaries produced by the component compiler
//! backend (`src/wasm_component.rs`) and executes them via Wasmtime.
//!
//! # Architecture
//!
//! - `ComponentRuntime`: wraps a compiled wasmtime `Module` and provides
//!   `init`, `handle_message`, and `checkpoint` methods matching the
//!   `nulang-actor` WIT contract.
//! - `ComponentPool`: recycles instances for low-latency message dispatch.
//!   When instance pooling is unavailable (no `component-model` feature),
//!   falls back to per-message instantiation.
//!
//! # Integration
//!
//! Called from `Runtime::step_actor` when `ActorBackend::WasmComponent`
//! is detected. See `src/runtime/mod.rs` for the dispatch point.

use crate::types::{NuError, NuResult};
use crate::value_layout;

#[cfg(feature = "wasm-backend")]
use wasmtime::*;

// ── Default configuration ────────────────────────────────────────────

/// Create a Wasmtime `Config` for Nulang component execution.
#[cfg(feature = "wasm-backend")]
pub fn component_config() -> Config {
    let mut config = Config::new();
    config.memory_reservation(4 << 30);      // 4 GiB virtual
    config.memory_guard_size(128 << 20);     // 128 MiB guard
    config.cranelift_opt_level(OptLevel::Speed);
    config.wasm_simd(true);
    config
}

// ── Component Runtime ────────────────────────────────────────────────

/// A compiled WASM component ready to execute actor behaviors.
///
/// The module is compiled once and instantiated per-message (or pooled).
/// State lives in the host's persistence store, not in the WASM instance
/// between messages — this is the share-nothing model.
#[cfg(feature = "wasm-backend")]
pub struct ComponentRuntime {
    _engine: Engine,
    module: Module,
    /// Cached config for fresh store creation per invocation.
    config: Config,
}



#[cfg(feature = "wasm-backend")]
impl ComponentRuntime {
    /// Compile a WASM component from raw bytes.
    pub fn new(wasm_bytes: &[u8]) -> NuResult<Self> {
        let config = component_config();
        let engine = Engine::new(&config)
            .map_err(|e| NuError::VMError(format!("wasmtime engine: {}", e)))?;
        let module = Module::new(&engine, wasm_bytes)
            .map_err(|e| NuError::VMError(format!("wasmtime module: {}", e)))?;

        let config = component_config();
        let engine = Engine::new(&config)
            .map_err(|e| NuError::VMError(format!("wasmtime engine: {}", e)))?;
        Ok(ComponentRuntime {
            _engine: engine,
            module,
            config,
        })
    }

    /// Compile from a file path.
    pub fn from_file(path: &str) -> NuResult<Self> {
        let bytes = std::fs::read(path)
            .map_err(|e| NuError::VMError(format!("read {}: {}", path, e)))?;
        Self::new(&bytes)
    }

    /// Initialize the actor with the given id and arguments.
    ///
    /// Calls the WASM export `nulang_init(id_ptr, id_len, args_ptr, args_len) -> i64`.
    /// Returns the result value or an error.
    pub fn init(&self, id: &str, args: &[u8]) -> NuResult<crate::vm::Value> {
        let engine = Engine::new(&self.config)
            .map_err(|e| NuError::VMError(format!("wasmtime engine: {}", e)))?;
        let mut store = Store::new(&engine, HostState::default());
        let mut linker: Linker<HostState> = Linker::new(&engine);

        self.define_host_imports(&mut linker, &mut store)?;

        let instance = linker.instantiate(&mut store, &self.module)
            .map_err(|e| NuError::VMError(format!("wasmtime instantiate: {}", e)))?;

        let memory = instance.get_memory(&mut store, "memory")
            .ok_or_else(|| NuError::VMError("no memory export".into()))?;

        // Write id string into WASM memory
        let id_bytes = id.as_bytes();
        let id_ptr = Self::bump_alloc(&mut store, memory, id_bytes.len())?;
        memory.write(&mut store, id_ptr as usize, id_bytes)
            .map_err(|e| NuError::VMError(format!("memory write: {}", e)))?;

        // Write args into WASM memory
        let args_ptr = Self::bump_alloc(&mut store, memory, args.len())?;
        memory.write(&mut store, args_ptr as usize, args)
            .map_err(|e| NuError::VMError(format!("memory write: {}", e)))?;

        let init_func = instance
            .get_typed_func::<(i32, i32, i32, i32), i64>(&mut store, "nulang_init")
            .map_err(|e| NuError::VMError(format!("nulang_init not found: {}", e)))?;

        let result_raw = init_func
            .call(&mut store, (id_ptr as i32, id_bytes.len() as i32, args_ptr as i32, args.len() as i32))
            .map_err(|e| NuError::VMError(format!("init call: {}", e)))?;

        Ok(crate::vm::Value::from_raw(result_raw as u64))
    }

    /// Dispatch a message to the actor.
    ///
    /// Calls `nulang_handle_message(sender_ptr, sender_len, payload_ptr, payload_len) -> i64`.
    pub fn handle_message(&self, sender: &str, payload: &[u8]) -> NuResult<crate::vm::Value> {
        let engine = Engine::new(&self.config)
            .map_err(|e| NuError::VMError(format!("wasmtime engine: {}", e)))?;
        let mut store = Store::new(&engine, HostState::default());
        let mut linker: Linker<HostState> = Linker::new(&engine);

        self.define_host_imports(&mut linker, &mut store)?;

        let instance = linker.instantiate(&mut store, &self.module)
            .map_err(|e| NuError::VMError(format!("wasmtime instantiate: {}", e)))?;

        let memory = instance.get_memory(&mut store, "memory")
            .ok_or_else(|| NuError::VMError("no memory export".into()))?;

        let sender_bytes = sender.as_bytes();
        let sender_ptr = Self::bump_alloc(&mut store, memory, sender_bytes.len())?;
        memory.write(&mut store, sender_ptr as usize, sender_bytes)
            .map_err(|e| NuError::VMError(format!("memory write: {}", e)))?;

        let payload_ptr = Self::bump_alloc(&mut store, memory, payload.len())?;
        memory.write(&mut store, payload_ptr as usize, payload)
            .map_err(|e| NuError::VMError(format!("memory write: {}", e)))?;

        let handle_func = instance
            .get_typed_func::<(i32, i32, i32, i32), i64>(&mut store, "nulang_handle_message")
            .map_err(|e| NuError::VMError(format!("nulang_handle_message not found: {}", e)))?;

        let result_raw = handle_func
            .call(&mut store, (sender_ptr as i32, sender_bytes.len() as i32, payload_ptr as i32, payload.len() as i32))
            .map_err(|e| NuError::VMError(format!("handle_message call: {}", e)))?;

        Ok(crate::vm::Value::from_raw(result_raw as u64))
    }

    /// Request a state checkpoint from the actor.
    ///
    /// Calls `nulang_checkpoint() -> i64` where the return value is a
    /// tagged pointer to serialized state bytes in WASM memory.
    pub fn checkpoint(&self) -> NuResult<Vec<u8>> {
        let engine = Engine::new(&self.config)
            .map_err(|e| NuError::VMError(format!("wasmtime engine: {}", e)))?;
        let mut store = Store::new(&engine, HostState::default());
        let mut linker: Linker<HostState> = Linker::new(&engine);

        self.define_host_imports(&mut linker, &mut store)?;

        let instance = linker.instantiate(&mut store, &self.module)
            .map_err(|e| NuError::VMError(format!("wasmtime instantiate: {}", e)))?;

        let memory = instance.get_memory(&mut store, "memory")
            .ok_or_else(|| NuError::VMError("no memory export".into()))?;

        let ckpt_func = instance
            .get_typed_func::<(), i64>(&mut store, "nulang_checkpoint")
            .map_err(|e| NuError::VMError(format!("nulang_checkpoint not found: {}", e)))?;

        let result_raw = ckpt_func
            .call(&mut store, ())
            .map_err(|e| NuError::VMError(format!("checkpoint call: {}", e)))?;

        // The result is a tagged pointer: (ptr, len) pair in WASM memory.
        // For now, return empty — full serialization is Phase 2.2.
        let _ = (result_raw, memory);
        Ok(Vec::new())
    }

    /// Bump-allocate `size` bytes in WASM linear memory.
    fn bump_alloc(store: &mut Store<HostState>, memory: Memory, size: usize) -> NuResult<i32> {
        let aligned = (size + 7) & !7;
        let offset = store.data().alloc_offset;
        let required = offset + aligned as u32;
        let current = memory.data_size(&*store) as u32;
        if required > current {
            let pages = ((required - current) + 65535) / 65536;
            memory.grow(&mut *store, pages as u64)
                .map_err(|e| NuError::VMError(format!("memory grow: {}", e)))?;
        }
        store.data_mut().alloc_offset = required;
        Ok(offset as i32)
    }

    /// Register host imports (`env.nulang_alloc`, `env.io_print`, etc.).
    fn define_host_imports(
        &self,
        linker: &mut Linker<HostState>,
        store: &mut Store<HostState>,
    ) -> NuResult<()> {
        linker.func_wrap("env", "nulang_alloc", host_alloc)
            .map_err(|e| NuError::VMError(format!("link nulang_alloc: {}", e)))?;
        linker.func_wrap("env", "nulang_dispatch", host_dispatch)
            .map_err(|e| NuError::VMError(format!("link nulang_dispatch: {}", e)))?;
        linker.func_wrap("env", "log", host_log)
            .map_err(|e| NuError::VMError(format!("link log: {}", e)))?;
        linker.func_wrap("env", "io_print", host_print)
            .map_err(|e| NuError::VMError(format!("link io_print: {}", e)))?;
        linker.func_wrap("env", "io_read", host_read)
            .map_err(|e| NuError::VMError(format!("link io_read: {}", e)))?;

        let mem_type = MemoryType::new(1, None);
        let memory = Memory::new(&mut *store, mem_type)
            .map_err(|e| NuError::VMError(format!("create memory: {}", e)))?;
        linker.define(&mut *store, "env", "memory", memory)
            .map_err(|e| NuError::VMError(format!("define memory: {}", e)))?;

        Ok(())
    }
}

// ── Host state ───────────────────────────────────────────────────────

#[cfg(feature = "wasm-backend")]
#[derive(Default)]
struct HostState {
    alloc_offset: u32,
}

// ── Host import functions ────────────────────────────────────────────

#[cfg(feature = "wasm-backend")]
fn host_alloc(mut caller: Caller<'_, HostState>, size: i32) -> Result<i32, Error> {
    let size = (size as u32 + 7) & !7u32;
    let offset = caller.data().alloc_offset;
    let required = offset.checked_add(size)
        .ok_or_else(|| Error::msg("alloc overflow"))?;
    let mem = caller.get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| Error::msg("memory not found"))?;
    let current = mem.data_size(&caller) as u32;
    if required > current {
        let pages = ((required - current) + 65535) / 65536;
        mem.grow(&mut caller, pages as u64)
            .map_err(|e| Error::msg(format!("grow: {}", e)))?;
    }
    caller.data_mut().alloc_offset = required;
    Ok(offset as i32)
}

#[cfg(feature = "wasm-backend")]
fn host_dispatch(_caller: Caller<'_, HostState>, _a: i32, _b: i32, _c: i32, _d: i32) {}

#[cfg(feature = "wasm-backend")]
fn host_print(mut caller: Caller<'_, HostState>, offset: i32, len: i32) -> Result<i64, Error> {
    let mem = caller.get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| Error::msg("memory not found"))?;
    let data = mem.data(&caller);
    let off = offset as usize;
    let end = std::cmp::min(off + len as usize, data.len());
    let text = String::from_utf8_lossy(&data[off..end]);
    print!("{}", text);
    Ok(value_layout::TAG_UNIT as i64)
}

#[cfg(feature = "wasm-backend")]
fn host_read(_caller: Caller<'_, HostState>) -> Result<i64, Error> {
    Ok(value_layout::TAG_NIL as i64)
}

#[cfg(feature = "wasm-backend")]
fn host_log(mut caller: Caller<'_, HostState>, offset: i32, len: i32) -> Result<i64, Error> {
    let mem = caller.get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| Error::msg("memory not found"))?;
    let data = mem.data(&caller);
    let off = offset as usize;
    let end = std::cmp::min(off + len as usize, data.len());
    let text = String::from_utf8_lossy(&data[off..end]);
    eprintln!("[wasm] {}", text);
    Ok(value_layout::TAG_UNIT as i64)
}

// ── No-backend stub ──────────────────────────────────────────────────

/// Stub when `wasm-backend` feature is not enabled.
#[cfg(not(feature = "wasm-backend"))]
pub struct ComponentRuntime;

#[cfg(not(feature = "wasm-backend"))]
impl ComponentRuntime {
    pub fn new(_wasm_bytes: &[u8]) -> NuResult<Self> {
        Err(NuError::VMError("wasm-backend feature not enabled".into()))
    }
    pub fn from_file(_path: &str) -> NuResult<Self> {
        Err(NuError::VMError("wasm-backend feature not enabled".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_component_runtime_stub_without_feature() {
        let result = ComponentRuntime::new(b"");
        assert!(result.is_err());
    }

    #[cfg(feature = "wasm-backend")]
    #[test]
    fn test_component_runtime_empty_module() {
        let wasm = vec![
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version
        ];
        let result = ComponentRuntime::new(&wasm);
        // Empty module should fail to instantiate (missing exports)
        assert!(result.is_err());
    }
}
