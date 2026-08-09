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
pub struct AotModule {
    /// The Cranelift JIT module that owns compiled code memory.
    #[allow(dead_code)]
    jit_module: JITModule,
    /// Reusable function builder context.
    #[allow(dead_code)]
    builder_context: FunctionBuilderContext,
    /// Compiled function pointers indexed by MIR function index.
    compiled_funcs: Vec<*const u8>,
    /// Entry point index (the `__main` or `main` function).
    entry_idx: Option<usize>,
    /// Module-wide field name → slot index mapping for records.
    #[allow(dead_code)]
    field_map: std::collections::HashMap<String, u8>,
    /// Constant pool (String literals), for runtime string resolution.
    constants: Vec<crate::bytecode::Constant>,
}

impl AotModule {
    /// Compile a MIR module to native code for the specified target.
    pub fn compile(mir_module: &mir::Module) -> NuResult<Self> {
        Self::compile_for_target(mir_module, "native")
    }

    /// Compile a MIR module to native code for a specific target ISA.
    pub fn compile_for_target(mir_module: &mir::Module, target: &str) -> NuResult<Self> {
        // Set up Cranelift with the target ISA.
        let mut flag_builder = settings::builder();
        let _ = flag_builder.set("enable_simd", "true");
        let isa_builder = create_isa_builder(target)?;
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|e| crate::types::NuError::VMError {
                msg: format!("failed to finalize ISA for target '{}': {}", target, e),
                span: Span::default(),
            })?;

        let mut jit_builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());

        // Register NaN-tag-aware runtime helpers.
        register_runtime_helpers(&mut jit_builder);

        let mut jit_module = JITModule::new(jit_builder);
        let mut builder_context = FunctionBuilderContext::new();

        // Pre-scan: build module-wide field name → slot index map and
        // constant pool for string literals.
        let mut field_map: std::collections::HashMap<String, u8> = std::collections::HashMap::new();
        let mut next_field_id: u8 = 0;
        let mut constants: Vec<crate::bytecode::Constant> = Vec::new();

        for func in &mir_module.functions {
            for block in &func.blocks {
                for stmt in &block.stmts {
                    collect_field_and_consts(
                        stmt,
                        &mut field_map,
                        &mut next_field_id,
                        &mut constants,
                    );
                }
            }
        }
        for func in &mir_module.behaviors {
            for block in &func.blocks {
                for stmt in &block.stmts {
                    collect_field_and_consts(
                        stmt,
                        &mut field_map,
                        &mut next_field_id,
                        &mut constants,
                    );
                }
            }
        }

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
                .map_err(|e| crate::types::NuError::VMError {
                    msg: format!("failed to declare '{}': {}", func.name, e),
                    span: Span::default(),
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
                    .map_err(|e| crate::types::NuError::VMError {
                        msg: format!("failed to declare unboxed '{}': {}", func.name, e),
                        span: Span::default(),
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
                ctx2.func_ids[idx] = ub_fid;
                ctx2.field_map = field_map.clone();
                ctx2.constants = constants.clone();
                codegen::compile_mir_function_body(
                    &mut ctx2,
                    func,
                    idx,
                    ub_fid,
                    codegen::CompileMode::Unboxed,
                )
                .map_err(|e| crate::types::NuError::VMError {
                    msg: format!("AOT compilation of unboxed '{}' failed: {}", func.name, e),
                    span: Span::default(),
                })?;

                // Compile boxing wrapper as the boxed function table entry.
                let mut ctx3 = codegen::AotContext::new(&mut jit_module, &mut builder_context);
                codegen::compile_boxing_wrapper(
                    &mut ctx3,
                    func.params.len(),
                    func_ids[idx],
                    ub_fid,
                )
                .map_err(|e| crate::types::NuError::VMError {
                    msg: format!("AOT boxing wrapper for '{}' failed: {}", func.name, e),
                    span: Span::default(),
                })?;
            } else {
                // Normal boxed compilation for non-all-Int functions.
                let mut ctx = codegen::AotContext::new(&mut jit_module, &mut builder_context);
                ctx.func_ids = func_ids.clone();
                ctx.field_map = field_map.clone();
                ctx.constants = constants.clone();
                codegen::compile_mir_function_body(
                    &mut ctx,
                    func,
                    idx,
                    func_ids[idx],
                    codegen::CompileMode::Boxed,
                )
                .map_err(|e| crate::types::NuError::VMError {
                    msg: format!("AOT compilation of '{}' failed: {}", func.name, e),
                    span: Span::default(),
                })?;
            }

            if func.name == "__main" || func.name == "main" {
                if entry_idx.is_none() || func.name == "__main" {
                    entry_idx = Some(idx);
                }
            }
        }
        jit_module
            .finalize_definitions()
            .map_err(|e| crate::types::NuError::VMError {
                msg: format!("failed to finalize JIT definitions: {}", e),
                span: Span::default(),
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
            field_map,
            constants,
        })
    }

    /// Execute the module entry point and return the result as a u64 value.
    ///
    pub fn run(&self) -> NuResult<u64> {
        let idx = self.entry_idx.unwrap_or(0);
        let ptr = self
            .compiled_funcs
            .get(idx)
            .ok_or_else(|| crate::types::NuError::VMError {
                msg: "no compiled entry point".into(),
                span: Span::default(),
            })?;

        // Set up standalone heap for AOT runtime helpers.
        let mut heap = crate::runtime::heap::ActorHeap::new(1024 * 1024);
        heap.set_actor_id(0);
        crate::jit::runtime::aot_set_heap(heap);

        // Set up constant pool for string resolution.
        if !self.constants.is_empty() {
            unsafe {
                crate::jit::runtime::aot_set_constants(&self.constants);
            }
        }

        // Call the compiled function. Signature: extern "C" fn() -> u64
        // (for the entry point with no params).
        let func: extern "C" fn() -> u64 = unsafe { std::mem::transmute(*ptr) };
        let result = func();

        // Clean up.
        crate::jit::runtime::aot_clear_constants();
        let _ = crate::jit::runtime::aot_take_heap();

        Ok(result)
    }

    /// Emit assembly text for the compiled module.
    pub fn emit_assembly(&self) -> String {
        // For now, we'll just show the function names and basic info
        // Full assembly emission would require using cranelift_object or TextSectionBuilder
        let mut output = String::new();
        output.push_str(&format!("; AOT Module for target\n"));
        output.push_str(&format!("; Functions: {}\n", self.compiled_funcs.len()));
        for (idx, _) in self.compiled_funcs.iter().enumerate() {
            output.push_str(&format!("nulang_fn_{}:\n", idx));
            output.push_str("  ; [assembly would be emitted here]\n");
        }
        output
    }
}

/// Create an ISA builder for the specified target.
fn create_isa_builder(target: &str) -> NuResult<isa::Builder> {
    use target_lexicon::Triple;
    
    match target {
        "native" => {
            cranelift_native::builder().map_err(|msg| crate::types::NuError::VMError {
                msg: format!("host machine not supported: {}", msg),
                span: Span::default(),
            })
        }
        "ptx" | "nvptx64" => {
            // PTX (NVIDIA GPU) target
            let triple: Triple = "nvptx64-nvidia-cuda".parse().map_err(|e| crate::types::NuError::VMError {
                msg: format!("invalid PTX triple: {}", e),
                span: Span::default(),
            })?;
            isa::lookup(triple).map_err(|e| crate::types::NuError::VMError {
                msg: format!("PTX target not supported: {}", e),
                span: Span::default(),
            })
        }
        "riscv64" | "riscv" => {
            // RISC-V 64-bit target
            let triple: Triple = "riscv64gc-unknown-none-elf".parse().map_err(|e| crate::types::NuError::VMError {
                msg: format!("invalid RISC-V triple: {}", e),
                span: Span::default(),
            })?;
            isa::lookup(triple).map_err(|e| crate::types::NuError::VMError {
                msg: format!("RISC-V target not supported: {}", e),
                span: Span::default(),
            })
        }
        _ => Err(crate::types::NuError::VMError {
            msg: format!("unknown target '{}' (expected native | ptx | riscv64)", target),
            span: Span::default(),
        }),
    }
}

/// Register all runtime helper symbols with the JIT builder.
/// Single source of truth: `src/jit/helpers.rs` `define_helpers!` macro.
fn register_runtime_helpers(builder: &mut JITBuilder) {
    crate::jit::helpers::register_with_builder(builder);
}

/// Scan MIR statements to collect field names and string constants.
fn collect_field_and_consts(
    stmt: &mir::Stmt,
    field_map: &mut std::collections::HashMap<String, u8>,
    next_field_id: &mut u8,
    constants: &mut Vec<crate::bytecode::Constant>,
) {
    match stmt {
        mir::Stmt::Assign { op, .. } => {
            collect_rvalue_field_and_consts(op, field_map, next_field_id, constants);
        }
        mir::Stmt::StoreFieldNamed { field, .. } => {
            field_map.entry(field.clone()).or_insert_with(|| {
                let id = *next_field_id;
                *next_field_id = next_field_id.saturating_add(1);
                id
            });
        }
        _ => {}
    }
}

fn collect_rvalue_field_and_consts(
    rv: &mir::RValue,
    field_map: &mut std::collections::HashMap<String, u8>,
    next_field_id: &mut u8,
    constants: &mut Vec<crate::bytecode::Constant>,
) {
    match rv {
        mir::RValue::Const(c) => {
            if let crate::bytecode::Constant::String(_) = c {
                // Add string constant to pool, returning index
                constants.push(c.clone());
            }
        }
        mir::RValue::Record(fields)
        | mir::RValue::RecordUpdate {
            overrides: fields, ..
        } => {
            for (name, _) in fields {
                field_map.entry(name.clone()).or_insert_with(|| {
                    let id = *next_field_id;
                    *next_field_id = next_field_id.saturating_add(1);
                    id
                });
            }
        }
        mir::RValue::LoadFieldNamed { field, .. } => {
            field_map.entry(field.clone()).or_insert_with(|| {
                let id = *next_field_id;
                *next_field_id = next_field_id.saturating_add(1);
                id
            });
        }
        mir::RValue::Spawn { init, .. } => {
            for (name, rv) in init {
                field_map.entry(name.clone()).or_insert_with(|| {
                    let id = *next_field_id;
                    *next_field_id = next_field_id.saturating_add(1);
                    id
                });
                collect_rvalue_field_and_consts(rv, field_map, next_field_id, constants);
            }
        }
        _ => {}
    }
}

