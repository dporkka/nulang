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

use crate::types::Span;
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

#[derive(Default)]
struct HostState {
    /// Next allocation offset in WASM linear memory (bump allocator).
    alloc_offset: u32,
    /// Reference to the linear memory, stored for access from host functions.
    memory: Option<Memory>,
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

        let res = Module::new(&engine, wasm_bytes);
        if let Err(_) = &res {
            std::fs::write("/tmp/failed_module.wasm", wasm_bytes).unwrap();
        }
        let module = res.map_err(map_wasmtime_err)?;

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
        linker
            .func_wrap("env", "str_concat", host_str_concat)
            .map_err(map_wasmtime_err)?;
        linker
            .func_wrap("env", "str_eq", host_str_eq)
            .map_err(map_wasmtime_err)?;

        // Provide memory: 1-page (64KB) linear memory.
        let mem_type = MemoryType::new(1, None);
        let memory = Memory::new(&mut store, mem_type).map_err(map_wasmtime_err)?;
        store.data_mut().memory = Some(memory.clone());
        linker
            .define(&mut store, "env", "memory", memory)
            .map_err(map_wasmtime_err)?;

        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(map_wasmtime_err)?;

        // Initialize bump allocator offset to after data segments.
        if let Some(ref exported_mem) = store.data().memory {
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

    /// Resolve a tagged string `Value` (`TAG_STRING | offset`) to its text by
    /// reading the null-terminated bytes at that offset from linear memory.
    /// Returns `None` when the value is not a string or the offset is out of
    /// bounds. Used by tests and consumers that need concat/string content
    /// back out of a WASM execution.
    pub fn string_value(&self, val: &crate::vm::Value) -> Option<String> {
        use crate::value_layout::{PAYLOAD_MASK, TAG_MASK, TAG_STRING};
        let raw = val.as_raw();
        if (raw & TAG_MASK) != TAG_STRING {
            return None;
        }
        let offset = (raw & PAYLOAD_MASK) as usize;
        let mem = self.store.data().memory.as_ref()?;
        let data = mem.data(&self.store);
        let bytes: Vec<u8> = data
            .get(offset..)?
            .iter()
            .take_while(|&&b| b != 0)
            .copied()
            .collect();
        Some(String::from_utf8_lossy(&bytes).into_owned())
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

/// `env.str_concat(a: i64, b: i64) -> i64`
///
/// Concatenate two tagged string values. Each value is `TAG_STRING | offset`
/// into linear memory pointing at a null-terminated byte string (the data
/// segment is emitted with a trailing NUL per string, and prior concat
/// results are null-terminated here too). Reads both, writes `a ++ b\0` into
/// a fresh bump-allocated buffer, and returns the new tagged string value.
fn host_str_concat(mut caller: Caller<'_, HostState>, a: i64, b: i64) -> Result<i64, Error> {
    // Resolve each operand to its text, mirroring the interpreter's IAdd
    // string fallback (src/vm.rs): a tagged string reads its null-terminated
    // bytes from memory; anything else coerces through `to_string_repr()`, so
    // `"n=" + 42` concatenates the text "42".
    let (text_a, text_b) = {
        let mem = get_memory(&mut caller)?;
        let data = mem.data(&caller);
        let read = |v: i64| -> String {
            if (v as u64 & value_layout::TAG_MASK) == value_layout::TAG_STRING {
                let off = (v as u64 & value_layout::PAYLOAD_MASK) as usize;
                let bytes: Vec<u8> = data
                    .get(off..)
                    .map(|s| s.iter().take_while(|&&c| c != 0).copied().collect())
                    .unwrap_or_default();
                String::from_utf8_lossy(&bytes).into_owned()
            } else {
                crate::vm::Value::from_raw(v as u64).to_string_repr()
            }
        };
        (read(a), read(b))
    };
    let total = text_a.len() + text_b.len() + 1;
    // Bump-allocate (mirrors host_alloc) so the copy below can use `caller`.
    let size = (total as u32 + 7) & !7u32; // align to 8
    let new_off = caller.data().alloc_offset;
    let required = new_off
        .checked_add(size)
        .ok_or_else(|| Error::msg("alloc overflow"))?;
    let mem = get_memory(&mut caller)?;
    if required > mem.data_size(&caller) as u32 {
        let pages_needed = ((required - mem.data_size(&caller) as u32) + 65535) / 65536;
        mem.grow(&mut caller, pages_needed as u64)
            .map_err(|e| Error::msg(format!("memory grow: {}", e)))?;
    }
    caller.data_mut().alloc_offset = required;

    // Copy both texts into the freshly-allocated region, then null-terminate.
    let mem = get_memory(&mut caller)?;
    {
        let data = mem.data_mut(&mut caller);
        let dst = new_off as usize;
        data[dst..dst + text_a.len()].copy_from_slice(text_a.as_bytes());
        data[dst + text_a.len()..dst + text_a.len() + text_b.len()]
            .copy_from_slice(text_b.as_bytes());
        data[dst + text_a.len() + text_b.len()] = 0;
    }
    Ok(value_layout::TAG_STRING as i64 | new_off as i64)
}

/// `env.str_eq(a: i64, b: i64) -> i64`
///
/// String content equality: both operands must be tagged strings (read their
/// null-terminated bytes from memory); returns a tagged bool of whether they
/// hold the same text. Compares by content, not by data offset, so an
/// interned constant and a runtime `str_concat` result with identical text
/// compare equal. Returns `false` when either operand is not a string —
/// mirroring the interpreter's SCmpEq.
fn host_str_eq(mut caller: Caller<'_, HostState>, a: i64, b: i64) -> Result<i64, Error> {
    let eq = {
        let mem = get_memory(&mut caller)?;
        let data = mem.data(&caller);
        let read = |v: i64| -> Option<String> {
            if (v as u64 & value_layout::TAG_MASK) != value_layout::TAG_STRING {
                return None;
            }
            let off = (v as u64 & value_layout::PAYLOAD_MASK) as usize;
            let bytes: Vec<u8> = data
                .get(off..)
                .map(|s| s.iter().take_while(|&&c| c != 0).copied().collect())
                .unwrap_or_default();
            Some(String::from_utf8_lossy(&bytes).into_owned())
        };
        match (read(a), read(b)) {
            (Some(sa), Some(sb)) => sa == sb,
            _ => false,
        }
    };
    Ok(value_layout::tag_bool(eq) as i64)
}

/// `env.nulang_dispatch(a: i32, b: i32, c: i32, d: i32)`
///
/// Stub: effect dispatch through the actor runtime is not yet wired.
fn host_dispatch(_caller: Caller<'_, HostState>, _a: i32, _b: i32, _c: i32, _d: i32) {
    // No-op for now.
}

/// Helper: retrieve linear memory from the HostState.
fn get_memory(caller: &mut Caller<'_, HostState>) -> Result<Memory, Error> {
    caller
        .data()
        .memory
        .clone()
        .ok_or_else(|| Error::msg("env.memory not initialized"))
}

// ── Error mapping ────────────────────────────────────────────────────

fn map_wasmtime_err(e: impl std::fmt::Display) -> NuError {
    NuError::VMError {
        msg: format!("wasmtime: {}", e),
        span: Span::default(),
    }
}

// ── AOT compilation ──────────────────────────────────────────────────

/// Compile a WASM module ahead-of-time to a `.cwasm` file via `wasmtime compile`.
/// Compile a WebAssembly module to a machine-specific `.cwasm` artifact.
/// Note: No cross-version portability is promised for `.cwasm`. It must be
/// loaded by an `Engine` matching this version of wasmtime and its config.
pub fn aot_compile(wasm_path: &str, cwasm_path: &str) -> NuResult<()> {
    let bytes = std::fs::read(wasm_path).map_err(|e| NuError::VMError {
        msg: format!("failed to read wasm: {}", e),
        span: Span::default(),
    })?;

    let config = default_wasm_config();
    let engine = Engine::new(&config).map_err(|e| NuError::VMError {
        msg: format!("failed to create engine: {}", e),
        span: Span::default(),
    })?;

    let cwasm_bytes = engine
        .precompile_module(&bytes)
        .map_err(|e| NuError::VMError {
            msg: format!("failed to precompile module: {}", e),
            span: Span::default(),
        })?;

    std::fs::write(cwasm_path, cwasm_bytes).map_err(|e| NuError::VMError {
        msg: format!("failed to write cwasm: {}", e),
        span: Span::default(),
    })?;

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

    #[test]
    fn test_wasm_config_reservation_sizes() {
        let config = default_wasm_config();
        let engine = Engine::new(&config).unwrap();
        // Verify default config settings don't conflict.
        let module = Module::new(&engine, &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]);
        assert!(module.is_ok());
    }

    #[test]
    fn test_aot_compile_rejects_missing_file() {
        let result = aot_compile("/nonexistent/path.wasm", "/tmp/out.cwasm");
        assert!(result.is_err(), "compiling a missing file should fail");
    }

    #[test]
    fn test_error_mapping() {
        let err = map_wasmtime_err("test error");
        assert!(err.to_string().contains("wasmtime"));
        assert!(err.to_string().contains("test error"));
    }

    #[test]
    fn test_wasm_runtime_rejects_invalid_module() {
        let config = default_wasm_config();
        let engine = Engine::new(&config).unwrap();
        let invalid_wasm = vec![0x00, 0x00, 0x00, 0x00];
        let result = Module::new(&engine, &invalid_wasm);
        assert!(result.is_err(), "invalid WASM should fail to parse");
    }

    #[test]
    fn test_wasm_runtime_rejects_empty_bytes() {
        let config = default_wasm_config();
        let engine = Engine::new(&config).unwrap();
        let result = Module::new(&engine, &[] as &[u8]);
        assert!(result.is_err(), "empty bytes should fail to parse");
    }

    #[test]
    fn test_host_read_returns_nil() {
        let wasm = br#"(module
            (import "env" "memory" (memory 1))
            (import "env" "nulang_alloc" (func $alloc (param i32) (result i32)))
            (import "env" "nulang_dispatch" (func $dispatch (param i32 i32 i32 i32)))
            (import "env" "log" (func $log (param i32 i32) (result i64)))
            (import "env" "io_print" (func $print (param i32 i32) (result i64)))
            (import "env" "io_read" (func $read (result i64)))
            (func $start (result i64)
                call $read
            )
            (export "nulang_init" (func $start))
        )"#;
        let mut runtime = WasmRuntime::new(wasm, None).unwrap();
        let result = runtime.run().unwrap();
        assert!(result.is_nil(), "io_read stub should return nil");
    }
}
