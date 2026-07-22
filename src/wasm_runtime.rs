//! Wasmtime-based WASM runtime for Nulang Cloud.
//!
//! Loads `.wasm` modules produced by `mir_wasm::WasmBackend` and executes
//! them with an optimized Wasmtime configuration:
//!
//! - **Memory guard pages**: `memory_reservation(4 GiB)` +
//!   `memory_guard_size(128 MiB)`. Cranelift emits plain `mov` without bounds
//!   checks; the MMU catches OOB as SIGSEGV → Wasmtime trap.
//! - **Cranelift speed**: `cranelift_opt_level(Speed)` enables cross-function
//!   inlining and other optimizations.
//! - **SIMD**: `wasm_simd(true)` enables the WASM SIMD proposal (v128 ops).
//!
//! # Host imports
//!
//! The WASM backend emits modules that import:
//! - `env.memory` — linear memory
//! - `env.nulang_alloc(i32) -> i32` — bump allocator in WASM memory
//! - `env.nulang_dispatch(i32,i32,i32,i32)` — effect dispatch (stub)
//! - `env.log(i32,i32) -> i64` — log to stderr
//! - `env.io_print(i32,i32) -> i64` — print to stdout
//! - `env.io_read() -> i64` — read stdin (stub: returns nil)

use crate::types::{NuError, NuResult};
use crate::value_layout;
use wasmtime::*;

// ── Default configuration ────────────────────────────────────────────

/// Create a Wasmtime `Config` with Nulang Cloud optimizations.
///
/// Enables:
/// - 4 GiB virtual memory reservation + 128 MiB guard region
/// - Cranelift speed optimizations (includes inlining)
/// - WASM SIMD proposal
pub fn default_wasm_config() -> Config {
    let mut config = Config::new();
    // Guard pages: reserve 4 GiB virtual, 128 MiB guard.
    config.memory_reservation(4 << 30);
    config.memory_guard_size(128 << 20);
    // Cranelift speed optimizations (enables cross-function inlining).
    config.cranelift_opt_level(OptLevel::Speed);
    // WASM SIMD proposal.
    config.wasm_simd(true);
    config
}

// ── Host state ───────────────────────────────────────────────────────

/// Minimal host state stored in the `Store`, accessible via `Caller::data_mut()`.
#[derive(Default)]
struct HostState {
    /// Next allocation offset in WASM linear memory (bump allocator).
    alloc_offset: u32,
}

// ── WASM Runtime ─────────────────────────────────────────────────────

/// A compiled and instantiated WASM module ready to run.
pub struct WasmRuntime {
    _engine: Engine,
    store: Store<HostState>,
    /// The `nulang_init` export function.
    init_func: TypedFunc<(), i64>,
}

impl WasmRuntime {
    /// Compile WASM bytecode and instantiate with host imports.
    pub fn new(wasm_bytes: &[u8], config: Option<Config>) -> NuResult<Self> {
        let config = config.unwrap_or_else(default_wasm_config);
        let engine = Engine::new(&config).map_err(map_wasmtime_err)?;

        let module = Module::new(&engine, wasm_bytes).map_err(map_wasmtime_err)?;

        let mut store = Store::new(&engine, HostState::default());

        // Build a Linker and define all host imports.
        let mut linker: Linker<HostState> = Linker::new(&engine);

        linker
            .func_wrap("env", "nulang_alloc", host_alloc)
            .map_err(map_wasmtime_err)?;
        linker
            .func_wrap("env", "nulang_dispatch", host_dispatch)
            .map_err(map_wasmtime_err)?;
        linker
            .func_wrap("env", "log", host_log)
            .map_err(map_wasmtime_err)?;
        linker
            .func_wrap("env", "io_print", host_print)
            .map_err(map_wasmtime_err)?;
        linker
            .func_wrap("env", "io_read", host_read)
            .map_err(map_wasmtime_err)?;

        // Provide memory: 1-page (64KB) linear memory.
        let mem_type = MemoryType::new(1, None);
        let memory = Memory::new(&mut store, mem_type).map_err(map_wasmtime_err)?;
        linker
            .define(&mut store, "env", "memory", memory)
            .map_err(map_wasmtime_err)?;

        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(map_wasmtime_err)?;

        // Initialize bump allocator offset to after the data segment.
        if let Some(exported_mem) = instance.get_memory(&mut store, "memory") {
            let data_end = exported_mem.data_size(&store);
            store.data_mut().alloc_offset = data_end as u32;
        }

        let init_func = instance
            .get_typed_func::<(), i64>(&mut store, "nulang_init")
            .map_err(map_wasmtime_err)?;

        Ok(WasmRuntime {
            _engine: engine,
            store,
            init_func,
        })
    }

    /// Execute the module's `nulang_init` function, returning the tagged result.
    pub fn run(&mut self) -> NuResult<crate::vm::Value> {
        self.init_func
            .call(&mut self.store, ())
            .map(|raw| crate::vm::Value::from_raw(raw as u64))
            .map_err(map_wasmtime_err)
    }
}

// ── Host import functions ────────────────────────────────────────────

/// `env.io_print(offset: i32, len: i32) -> i64`
fn host_print(mut caller: Caller<'_, HostState>, offset: i32, len: i32) -> Result<i64, Error> {
    let mem = get_memory(&mut caller)?;
    let data = mem.data(&caller);
    let off = offset as usize;
    let end = std::cmp::min(off + len as usize, data.len());
    let text = String::from_utf8_lossy(&data[off..end]);
    print!("{}", text);
    Ok(value_layout::TAG_UNIT as i64)
}

/// `env.io_read() -> i64`
fn host_read(_caller: Caller<'_, HostState>) -> Result<i64, Error> {
    // Stub: read is not yet wired to the actor mailbox.
    Ok(value_layout::TAG_NIL as i64)
}

/// `env.log(offset: i32, len: i32) -> i64`
fn host_log(mut caller: Caller<'_, HostState>, offset: i32, len: i32) -> Result<i64, Error> {
    let mem = get_memory(&mut caller)?;
    let data = mem.data(&caller);
    let off = offset as usize;
    let end = std::cmp::min(off + len as usize, data.len());
    let text = String::from_utf8_lossy(&data[off..end]);
    eprintln!("[wasm] {}", text);
    Ok(value_layout::TAG_UNIT as i64)
}

/// `env.nulang_alloc(size: i32) -> i32`
///
/// Simple bump allocator in WASM linear memory. Single-threaded.
fn host_alloc(mut caller: Caller<'_, HostState>, size: i32) -> Result<i32, Error> {
    let size = (size as u32 + 7) & !7u32; // align to 8
    let offset = caller.data().alloc_offset;
    let required = offset
        .checked_add(size)
        .ok_or_else(|| Error::msg("alloc overflow"))?;
    let mem = get_memory(&mut caller)?;
    let current_size = mem.data_size(&caller) as u32;
    if required > current_size {
        let pages_needed = ((required - current_size) + 65535) / 65536;
        mem.grow(&mut caller, pages_needed as u64)
            .map_err(|e| Error::msg(format!("memory grow: {}", e)))?;
    }
    caller.data_mut().alloc_offset = required;
    Ok(offset as i32)
}

/// `env.nulang_dispatch(a: i32, b: i32, c: i32, d: i32)`
///
/// Stub: effect dispatch through the actor runtime is not yet wired.
fn host_dispatch(_caller: Caller<'_, HostState>, _a: i32, _b: i32, _c: i32, _d: i32) {
    // No-op for now.
}

/// Helper: retrieve `env.memory` from a caller context.
fn get_memory(caller: &mut Caller<'_, HostState>) -> Result<Memory, Error> {
    caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| Error::msg("env.memory not found in store"))
}

// ── Error mapping ────────────────────────────────────────────────────

fn map_wasmtime_err(e: impl std::fmt::Display) -> NuError {
    NuError::VMError(format!("wasmtime: {}", e))
}

// ── AOT compilation ──────────────────────────────────────────────────

/// Compile a WASM module ahead-of-time to a `.cwasm` file via `wasmtime compile`.
pub fn aot_compile(wasm_path: &str, cwasm_path: &str) -> NuResult<()> {
    let output = std::process::Command::new("wasmtime")
        .args(["compile", wasm_path, "-o", cwasm_path])
        .output()
        .map_err(|e| NuError::VMError(format!("wasmtime compile not found: {}", e)))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(NuError::VMError(format!(
            "wasmtime compile failed: {}",
            stderr.trim()
        )));
    }
    Ok(())
}

/// Load a precompiled `.cwasm` module and instantiate it.
pub fn load_precompiled(cwasm_bytes: &[u8]) -> NuResult<WasmRuntime> {
    let config = default_wasm_config();
    let engine = Engine::new(&config).map_err(map_wasmtime_err)?;

    let module = unsafe { Module::deserialize(&engine, cwasm_bytes) }.map_err(map_wasmtime_err)?;

    let mut store = Store::new(&engine, HostState::default());
    let mut linker: Linker<HostState> = Linker::new(&engine);

    linker
        .func_wrap("env", "nulang_alloc", host_alloc)
        .map_err(map_wasmtime_err)?;
    linker
        .func_wrap("env", "nulang_dispatch", host_dispatch)
        .map_err(map_wasmtime_err)?;
    linker
        .func_wrap("env", "log", host_log)
        .map_err(map_wasmtime_err)?;
    linker
        .func_wrap("env", "io_print", host_print)
        .map_err(map_wasmtime_err)?;
    linker
        .func_wrap("env", "io_read", host_read)
        .map_err(map_wasmtime_err)?;

    let mem_type = MemoryType::new(1, None);
    let memory = Memory::new(&mut store, mem_type).map_err(map_wasmtime_err)?;
    linker
        .define(&mut store, "env", "memory", memory)
        .map_err(map_wasmtime_err)?;

    let instance = linker
        .instantiate(&mut store, &module)
        .map_err(map_wasmtime_err)?;

    if let Some(exported_mem) = instance.get_memory(&mut store, "memory") {
        let data_end = exported_mem.data_size(&store);
        store.data_mut().alloc_offset = data_end as u32;
    }

    let init_func = instance
        .get_typed_func::<(), i64>(&mut store, "nulang_init")
        .map_err(map_wasmtime_err)?;

    Ok(WasmRuntime {
        _engine: engine,
        store,
        init_func,
    })
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_creates() {
        let config = default_wasm_config();
        let engine = Engine::new(&config);
        assert!(engine.is_ok(), "engine should create: {:?}", engine.err());
    }

    #[test]
    fn test_wasm_runtime_empty_module() {
        // Minimal valid WASM module: magic + version.
        let wasm = vec![
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version
        ];
        let config = default_wasm_config();
        let engine = Engine::new(&config).unwrap();
        assert!(Module::new(&engine, &wasm).is_ok());
    }
}
