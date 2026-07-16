//! AOT (Ahead-of-Time) native code compilation backend.
//!
//! Compiles Nulang MIR modules to native code via Cranelift, leveraging
//! compile-time type information to emit unboxed operations.
//!
//! # Architecture
//!
//! - `codegen`: MIR → Cranelift CLIF compilation (per-function)
//! - This module: orchestrates module-level compilation, registers runtime
//!   helpers, and provides the execution entry point.
//!
//! # Current status
//!
//! Uses `cranelift_jit::JITModule` (same as the tiered JIT) rather than
//! true AOT object-file emission. This gives us native code without needing
//! a linker — the trampoline calls into the JIT module at startup.

pub mod codegen;

use cranelift::prelude::*;
use cranelift_frontend::FunctionBuilderContext;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::Module;

use crate::mir;
use crate::types::NuResult;

/// Compiled AOT module ready for execution.
#[allow(dead_code)]
pub struct AotModule {
    /// The Cranelift JIT module that owns compiled code memory.
    jit_module: JITModule,
    /// Reusable function builder context.
    builder_context: FunctionBuilderContext,
    /// Compiled function pointers indexed by MIR function index.
    compiled_funcs: Vec<*const u8>,
    /// Entry point index (the `__main` or `main` function).
    entry_idx: Option<usize>,
}

impl AotModule {
    /// Compile a MIR module to native code.
    pub fn compile(mir_module: &mir::Module) -> NuResult<Self> {
        // Set up Cranelift with the native target ISA.
        let mut flag_builder = settings::builder();
        let _ = flag_builder.set("enable_simd", "true");
        let isa_builder = cranelift_native::builder().map_err(|msg| {
            crate::types::NuError::VMError(format!("host machine not supported: {}", msg))
        })?;
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|e| crate::types::NuError::VMError(format!("failed to finalize ISA: {}", e)))?;

        let mut jit_builder =
            JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());

        // Register NaN-tag-aware runtime helpers.
        register_runtime_helpers(&mut jit_builder);

        let mut jit_module = JITModule::new(jit_builder);
        let mut builder_context = FunctionBuilderContext::new();

        // Pass 1: declare all functions so forward references resolve.
        let mut func_ids: Vec<cranelift_module::FuncId> =
            Vec::with_capacity(mir_module.functions.len());
        for (idx, func) in mir_module.functions.iter().enumerate() {
            let func_name = format!("nulang_fn_{}", idx);
            let mut sig = jit_module.make_signature();
            for _ in &func.params {
                sig.params.push(AbiParam::new(types::I64));
            }
            sig.returns.push(AbiParam::new(types::I64));
            let fid = jit_module
                .declare_function(&func_name, cranelift_module::Linkage::Local, &sig)
                .map_err(|e| {
                    crate::types::NuError::VMError(format!("failed to declare '{}': {}", func.name, e))
                })?;
            func_ids.push(fid);
        }

        // Pass 2: compile each function body with all FuncIds available.
        let mut entry_idx: Option<usize> = None;

        for (idx, func) in mir_module.functions.iter().enumerate() {
            let mut ctx = codegen::AotContext::new(&mut jit_module, &mut builder_context);
            ctx.func_ids = func_ids.clone();
            codegen::compile_mir_function_body(&mut ctx, func, idx, func_ids[idx]).map_err(|e| {
                crate::types::NuError::VMError(format!(
                    "AOT compilation of '{}' failed: {}",
                    func.name, e
                ))
            })?;

            if func.name == "__main" || func.name == "main" {
                if entry_idx.is_none() || func.name == "__main" {
                    entry_idx = Some(idx);
                }
            }
        }
        // Finalize all definitions so function pointers become available.
        jit_module.finalize_definitions().map_err(|e| {
            crate::types::NuError::VMError(format!("failed to finalize JIT definitions: {}", e))
        })?;

        let compiled_funcs: Vec<*const u8> = func_ids
            .iter()
            .map(|fid| jit_module.get_finalized_function(*fid))
            .collect();

        Ok(AotModule {
            jit_module,
            builder_context,
            compiled_funcs,
            entry_idx,
        })
    }

    /// Execute the module entry point and return the result as a u64 value.
    ///
    /// The entry point is `__main` if it exists, otherwise `main`, otherwise
    /// the first function. Returns the NaN-tagged result value.
    pub fn run(&self) -> NuResult<u64> {
        let idx = self
            .entry_idx
            .unwrap_or(0);
        let ptr = self
            .compiled_funcs
            .get(idx)
            .ok_or_else(|| {
                crate::types::NuError::VMError("no compiled entry point".into())
            })?;

        // Call the compiled function. Signature: extern "C" fn() -> u64
        // (for the entry point with no params).
        let func: extern "C" fn() -> u64 = unsafe { std::mem::transmute(*ptr) };
        Ok(func())
    }
}

/// Register all runtime helper symbols with the JIT builder.
fn register_runtime_helpers(builder: &mut JITBuilder) {
    let helpers: &[(&str, *const u8)] = &[
        ("nulang_iadd", crate::jit::runtime::nulang_iadd as *const u8),
        ("nulang_isub", crate::jit::runtime::nulang_isub as *const u8),
        ("nulang_imul", crate::jit::runtime::nulang_imul as *const u8),
        ("nulang_idiv", crate::jit::runtime::nulang_idiv as *const u8),
        ("nulang_imod", crate::jit::runtime::nulang_imod as *const u8),
        ("nulang_icmp_eq", crate::jit::runtime::nulang_icmp_eq as *const u8),
        ("nulang_icmp_lt", crate::jit::runtime::nulang_icmp_lt as *const u8),
        ("nulang_icmp_gt", crate::jit::runtime::nulang_icmp_gt as *const u8),
        ("nulang_icmp_le", crate::jit::runtime::nulang_icmp_le as *const u8),
        ("nulang_icmp_ge", crate::jit::runtime::nulang_icmp_ge as *const u8),
        ("nulang_fadd", crate::jit::runtime::nulang_fadd as *const u8),
        ("nulang_fsub", crate::jit::runtime::nulang_fsub as *const u8),
        ("nulang_fmul", crate::jit::runtime::nulang_fmul as *const u8),
        ("nulang_fdiv", crate::jit::runtime::nulang_fdiv as *const u8),
        ("nulang_fcmp_eq", crate::jit::runtime::nulang_fcmp_eq as *const u8),
        ("nulang_fcmp_lt", crate::jit::runtime::nulang_fcmp_lt as *const u8),
        ("nulang_fcmp_gt", crate::jit::runtime::nulang_fcmp_gt as *const u8),
        ("nulang_ineg", crate::jit::runtime::nulang_ineg as *const u8),
        ("nulang_iinc", crate::jit::runtime::nulang_iinc as *const u8),
        ("nulang_idec", crate::jit::runtime::nulang_idec as *const u8),
        ("nulang_not", crate::jit::runtime::nulang_not as *const u8),
        ("nulang_and", crate::jit::runtime::nulang_and as *const u8),
        ("nulang_or", crate::jit::runtime::nulang_or as *const u8),
        ("nulang_itof", crate::jit::runtime::nulang_itof as *const u8),
        ("nulang_ftoi", crate::jit::runtime::nulang_ftoi as *const u8),
        ("nulang_xor", crate::jit::runtime::nulang_xor as *const u8),
        ("nulang_shl", crate::jit::runtime::nulang_shl as *const u8),
        ("nulang_shr", crate::jit::runtime::nulang_shr as *const u8),
        ("nulang_bitand", crate::jit::runtime::nulang_bitand as *const u8),
        ("nulang_bitor", crate::jit::runtime::nulang_bitor as *const u8),
        ("nulang_fneg", crate::jit::runtime::nulang_fneg as *const u8),
    ];
    for (name, ptr) in helpers {
        builder.symbol(*name, *ptr);
    }
}

