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
use crate::types::{NuResult, Span};

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
            crate::types::NuError::VMError { msg: format!("host machine not supported: {}", msg), span: Span::default() }
        })?;
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|e| {
                crate::types::NuError::VMError { msg: format!("failed to finalize ISA: {}", e), span: Span::default() }
            })?;

        let mut jit_builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());

        // Register NaN-tag-aware runtime helpers.
        register_runtime_helpers(&mut jit_builder);

        let mut jit_module = JITModule::new(jit_builder);
        let mut builder_context = FunctionBuilderContext::new();

        // Pass 1: declare all functions so forward references resolve.
        let mut func_ids: Vec<cranelift_module::FuncId> =
            Vec::with_capacity(mir_module.functions.len());
        // Unboxed variants for all-Int functions (same indices, empty for non-Int).
        let mut unboxed_ids: Vec<Option<cranelift_module::FuncId>> =
            vec![None; mir_module.functions.len()];

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
                    crate::types::NuError::VMError { msg: format!(
                        "failed to declare '{}': {}",
                        func.name, e
                    ), span: Span::default() }
                })?;
            func_ids.push(fid);

            // If the function is all-Int, also declare an unboxed variant.
            if codegen::is_all_int(func) {
                let ub_name = format!("nulang_fn_{}_unboxed", idx);
                let mut ub_sig = jit_module.make_signature();
                for _ in &func.params {
                    ub_sig.params.push(AbiParam::new(types::I64));
                }
                ub_sig.returns.push(AbiParam::new(types::I64));
                let ub_fid = jit_module
                    .declare_function(&ub_name, cranelift_module::Linkage::Local, &ub_sig)
                    .map_err(|e| {
                        crate::types::NuError::VMError { msg: format!(
                            "failed to declare unboxed '{}': {}",
                            func.name, e
                        ), span: Span::default() }
                    })?;
                unboxed_ids[idx] = Some(ub_fid);
            }
        }

        // Pass 2: compile each function body (boxed + optionally unboxed).
        let mut entry_idx: Option<usize> = None;

        for (idx, func) in mir_module.functions.iter().enumerate() {
            // For all-Int functions: compile unboxed body first, then
            // generate a boxing wrapper as the boxed entry point. The
            // original boxed body is never compiled.
            // For non-all-Int functions: compile boxed body as usual.
            if let Some(ub_fid) = unboxed_ids[idx] {
                // Compile unboxed variant (self-recursive calls resolve to ub_fid).
                let mut ctx2 = codegen::AotContext::new(&mut jit_module, &mut builder_context);
                ctx2.func_ids = func_ids.clone();
                ctx2.func_ids[idx] = ub_fid; // Step 4d: self-calls use unboxed variant
                codegen::compile_mir_function_body(
                    &mut ctx2,
                    func,
                    idx,
                    ub_fid,
                    codegen::CompileMode::Unboxed,
                )
                .map_err(|e| {
                    crate::types::NuError::VMError { msg: format!(
                        "AOT compilation of unboxed '{}' failed: {}",
                        func.name, e
                    ), span: Span::default() }
                })?;

                // Compile boxing wrapper as the boxed function table entry.
                let mut ctx3 = codegen::AotContext::new(&mut jit_module, &mut builder_context);
                codegen::compile_boxing_wrapper(
                    &mut ctx3,
                    func.params.len(),
                    func_ids[idx],
                    ub_fid,
                )
                .map_err(|e| {
                    crate::types::NuError::VMError { msg: format!(
                        "AOT boxing wrapper for '{}' failed: {}",
                        func.name, e
                    ), span: Span::default() }
                })?;
            } else {
                // Normal boxed compilation for non-all-Int functions.
                let mut ctx = codegen::AotContext::new(&mut jit_module, &mut builder_context);
                ctx.func_ids = func_ids.clone();
                codegen::compile_mir_function_body(
                    &mut ctx,
                    func,
                    idx,
                    func_ids[idx],
                    codegen::CompileMode::Boxed,
                )
                .map_err(|e| {
                    crate::types::NuError::VMError { msg: format!(
                        "AOT compilation of '{}' failed: {}",
                        func.name, e
                    ), span: Span::default() }
                })?;
            }

            if func.name == "__main" || func.name == "main" {
                if entry_idx.is_none() || func.name == "__main" {
                    entry_idx = Some(idx);
                }
            }
        }
        jit_module.finalize_definitions().map_err(|e| {
            crate::types::NuError::VMError { msg: format!("failed to finalize JIT definitions: {}", e), span: Span::default() }
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
        let idx = self.entry_idx.unwrap_or(0);
        let ptr = self
            .compiled_funcs
            .get(idx)
            .ok_or_else(|| crate::types::NuError::VMError { msg: "no compiled entry point".into(), span: Span::default() })?;

        // Call the compiled function. Signature: extern "C" fn() -> u64
        // (for the entry point with no params).
        let func: extern "C" fn() -> u64 = unsafe { std::mem::transmute(*ptr) };
        Ok(func())
    }
}

/// Register all runtime helper symbols with the JIT builder.
/// Single source of truth: `src/jit/helpers.rs` `define_helpers!` macro.
fn register_runtime_helpers(builder: &mut JITBuilder) {
    crate::jit::helpers::register_with_builder(builder);
}
