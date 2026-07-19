//! AOT code generation: MIR → Cranelift CLIF.
//!
//! Compiles whole MIR functions to native code with unboxed parameter and
//! return types when type metadata is available. Falls back to NaN-tagged
//! runtime helpers when types are unknown.
//!
//! # Calling convention
//!
//! Compiled functions follow the C ABI:
//! ```c
//! uint64_t nulang_fn_N(uint64_t arg0, uint64_t arg1, ...);
//! ```
//! All arguments and return values are `u64` (NaN-tagged when type is
//! unknown, raw bits when unboxed). The AOT runtime trampoline handles
//! boxing/unboxing at function boundaries.

use cranelift::codegen::ir::{BlockArg, FuncRef};
use cranelift::prelude::*;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::JITModule;
use cranelift_module::{Linkage, Module};

use std::collections::{HashMap, HashSet};

use crate::mir;
use crate::type_metadata::{KnownType, TypeMetadata};

// Reuse NaN-tag constants from the JIT shared helpers.
use crate::value_layout::{PAYLOAD_MASK, SIGN_BIT, TAG_BOOL, TAG_INT, TAG_NIL};

const TAG_INT_I64: i64 = TAG_INT as i64;
const TAG_BOOL_I64: i64 = TAG_BOOL as i64;
const TAG_NIL_I64: i64 = TAG_NIL as i64;
const PAYLOAD_MASK_I64: i64 = PAYLOAD_MASK as i64;
const SIGN_BIT_I64: i64 = SIGN_BIT as i64;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during AOT compilation.
#[derive(Debug)]
pub enum AotCompileError {
    /// A MIR construct that isn't yet supported by the AOT backend.
    Unsupported(String),
    /// Internal compiler error.
    Internal(String),
    /// Cranelift compilation failure.
    Cranelift(String),
}

impl std::fmt::Display for AotCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AotCompileError::Unsupported(msg) => write!(f, "AOT unsupported: {}", msg),
            AotCompileError::Internal(msg) => write!(f, "AOT internal error: {}", msg),
            AotCompileError::Cranelift(msg) => write!(f, "AOT cranelift error: {}", msg),
        }
    }
}

impl std::error::Error for AotCompileError {}

pub type AotResult<T> = Result<T, AotCompileError>;

// ---------------------------------------------------------------------------
// CLIF helpers (adapted from jit/typed_compiler.rs)
// ---------------------------------------------------------------------------

/// Sign-extend a 48-bit payload to a full i64.
fn emit_sext48(builder: &mut FunctionBuilder, raw: Value) -> Value {
    let payload_mask = builder.ins().iconst(types::I64, PAYLOAD_MASK_I64);
    let sign_bit_mask = builder.ins().iconst(types::I64, SIGN_BIT_I64);
    let payload = builder.ins().band(raw, payload_mask);
    let sign_bit = builder.ins().band(raw, sign_bit_mask);
    let zero = builder.ins().iconst(types::I64, 0);
    let is_neg = builder.ins().icmp(IntCC::NotEqual, sign_bit, zero);
    let sign_ext = builder
        .ins()
        .iconst(types::I64, 0xFFFF_0000_0000_0000u64 as i64);
    let extended = builder.ins().bor(payload, sign_ext);
    builder.ins().select(is_neg, extended, payload)
}

/// Re-tag an i64 value into a NaN-tagged integer.
fn emit_tag_int(builder: &mut FunctionBuilder, value: Value) -> Value {
    let tag = builder.ins().iconst(types::I64, TAG_INT_I64);
    let mask = builder.ins().iconst(types::I64, PAYLOAD_MASK_I64);
    let masked = builder.ins().band(value, mask);
    builder.ins().bor(tag, masked)
}

// ---------------------------------------------------------------------------
// Compilation context
// ---------------------------------------------------------------------------

/// State maintained during compilation of one MIR module.
pub struct AotContext<'a> {
    /// The Cranelift JIT module.
    pub module: &'a mut JITModule,
    /// Reusable function builder context.
    pub builder_context: &'a mut FunctionBuilderContext,
    /// Cranelift codegen context (holds the current function being compiled).
    pub codegen_ctx: codegen::Context,
    /// Runtime helpers registered with the JIT module.
    pub helpers: HashMap<&'static str, FuncRef>,
    /// FuncIds of already-compiled functions, indexed by MIR function index.
    pub func_ids: Vec<cranelift_module::FuncId>,
    /// Compilation mode: boxed (NaN-tagged) or unboxed (raw i64 for Int).
    pub mode: CompileMode,
}

impl<'a> AotContext<'a> {
    pub fn new(module: &'a mut JITModule, builder_context: &'a mut FunctionBuilderContext) -> Self {
        let codegen_ctx = module.make_context();
        AotContext {
            module,
            builder_context,
            codegen_ctx,
            helpers: HashMap::new(),
            func_ids: Vec::new(),
            mode: CompileMode::Boxed,
        }
    }
}

// ---------------------------------------------------------------------------
// SSA construction helpers
// ---------------------------------------------------------------------------

/// Compute block predecessors from terminators.
fn compute_predecessors(func: &mir::Function) -> HashMap<mir::BlockId, Vec<mir::BlockId>> {
    let mut preds: HashMap<mir::BlockId, Vec<mir::BlockId>> = HashMap::new();
    for block in &func.blocks {
        match &block.terminator {
            mir::Terminator::Jump(target) => {
                preds.entry(*target).or_default().push(block.id);
            }
            mir::Terminator::Branch { then_, else_, .. } => {
                preds.entry(*then_).or_default().push(block.id);
                preds.entry(*else_).or_default().push(block.id);
            }
            _ => {}
        }
    }
    preds
}

/// For each block, collect the set of register indices that are:
/// - Last assigned in at least one predecessor, AND
/// - The block has >1 predecessor.
///
/// These locals need CLIF block parameters for proper SSA merging.
fn compute_liveins(
    func: &mir::Function,
    preds: &HashMap<mir::BlockId, Vec<mir::BlockId>>,
    local_base: u32,
) -> HashMap<mir::BlockId, Vec<u32>> {
    // First, for each block, find which locals are last-assigned in that block.
    let mut block_defs: HashMap<mir::BlockId, HashSet<u32>> = HashMap::new();
    for block in &func.blocks {
        let mut defs = HashSet::new();
        for stmt in &block.stmts {
            if let mir::Stmt::Assign { dst, .. } = stmt {
                defs.insert(local_base + dst.0);
            }
        }
        block_defs.insert(block.id, defs);
    }

    // For each block with >1 predecessor, compute locals defined in ALL predecessors.
    let mut liveins: HashMap<mir::BlockId, Vec<u32>> = HashMap::new();
    for block in &func.blocks {
        let pids = match preds.get(&block.id) {
            Some(p) if p.len() > 1 => p,
            _ => continue,
        };
        // Start with definitions from first predecessor.
        let mut merged: HashSet<u32> = block_defs.get(&pids[0]).cloned().unwrap_or_default();
        for pid in &pids[1..] {
            if let Some(defs) = block_defs.get(pid) {
                merged = merged.intersection(defs).copied().collect();
            } else {
                merged.clear();
                break;
            }
        }
        if !merged.is_empty() {
            let mut sorted: Vec<u32> = merged.into_iter().collect();
            sorted.sort();
            liveins.insert(block.id, sorted);
        }
    }
    liveins
}

/// Like `compile_terminator` but passes block-param values for merged locals.
fn compile_terminator_with_params(
    builder: &mut FunctionBuilder,
    term: &mir::Terminator,
    block_map: &HashMap<mir::BlockId, cranelift::prelude::Block>,
    block_params: &HashMap<mir::BlockId, Vec<u32>>,
    local_vals: &HashMap<u32, Value>,
    _mode: CompileMode,
) -> AotResult<()> {
    match term {
        mir::Terminator::Return(val) => {
            if let Some(id) = val {
                let reg = mir::FunctionBuilder::LOCAL_BASE + id.0;
                let v = *local_vals.get(&reg).ok_or_else(|| {
                    AotCompileError::Internal("return value uninitialized".into())
                })?;
                builder.ins().return_(&[v]);
            } else {
                let nil = builder
                    .ins()
                    .iconst(types::I64, 0x7FF8_0000_0000_0000u64 as i64);
                builder.ins().return_(&[nil]);
            }
            Ok(())
        }
        mir::Terminator::Jump(target) => {
            let clif_block = *block_map
                .get(target)
                .ok_or_else(|| AotCompileError::Internal("jump to unknown block".into()))?;
            let args = block_param_args(block_params, target, local_vals);
            builder.ins().jump(clif_block, &args);
            Ok(())
        }
        mir::Terminator::Branch { cond, then_, else_ } => {
            let cond_reg = mir::FunctionBuilder::LOCAL_BASE + cond.0;
            let cond_val = *local_vals
                .get(&cond_reg)
                .ok_or_else(|| AotCompileError::Internal("branch cond uninitialized".into()))?;
            let then_block = *block_map
                .get(then_)
                .ok_or_else(|| AotCompileError::Internal("branch then unknown".into()))?;
            let else_block = *block_map
                .get(else_)
                .ok_or_else(|| AotCompileError::Internal("branch else unknown".into()))?;

            let false_val = builder.ins().iconst(types::I64, TAG_BOOL_I64);
            let is_true = builder.ins().icmp(IntCC::NotEqual, cond_val, false_val);
            let then_args = block_param_args(block_params, then_, local_vals);
            let else_args = block_param_args(block_params, else_, local_vals);
            builder
                .ins()
                .brif(is_true, then_block, &then_args, else_block, &else_args);
            Ok(())
        }
        _ => Err(AotCompileError::Unsupported(format!(
            "terminator {:?}",
            term
        ))),
    }
}

/// Build the argument list for a jump/branch to `target`: one Value per
/// block parameter, taken from the current `local_vals`.
fn block_param_args(
    block_params: &HashMap<mir::BlockId, Vec<u32>>,
    target: &mir::BlockId,
    local_vals: &HashMap<u32, Value>,
) -> Vec<BlockArg> {
    if let Some(params) = block_params.get(target) {
        params
            .iter()
            .map(|reg| {
                let val = *local_vals.get(reg).expect("block param local missing");
                BlockArg::from(val)
            })
            .collect()
    } else {
        vec![]
    }
}
// ---------------------------------------------------------------------------
// Main entry point: compile a MIR function to a native function pointer
// ---------------------------------------------------------------------------

/// Whether to emit NaN-tagged (boxed) or raw (unboxed) integer values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompileMode {
    /// NaN-tagged i64 — the default, interoperable representation.
    Boxed,
    /// Raw i64 for Int types — faster, but requires type knowledge at call sites.
    Unboxed,
}

/// Check whether a function is eligible for unboxed compilation:
/// all params are `KnownType::Int` and the return type is Int or void.
pub fn is_all_int(func: &mir::Function) -> bool {
    let local_base = mir::FunctionBuilder::LOCAL_BASE as usize;
    for param in &func.params {
        let reg = local_base + param.0 as usize;
        if func.type_metadata.get_type(reg) != KnownType::Int {
            return false;
        }
    }
    // Return type: None (unit) is fine, Some(Int) is fine, anything else disqualifies.
    if let Some(ref ret_ty) = func.ret {
        match ret_ty {
            crate::types::Type::Primitive(crate::types::PrimitiveType::Int) => {}
            _ => return false,
        }
    }
    true
}

/// Compile the body of a MIR function that was already declared.
///
pub fn compile_mir_function_body(
    aot: &mut AotContext,
    mir_func: &mir::Function,
    _func_index: usize,
    func_id: cranelift_module::FuncId,
    mode: CompileMode,
) -> AotResult<()> {
    aot.mode = mode;
    // Reconstruct the signature for the codegen context.
    let mut sig = aot.module.make_signature();
    for _ in &mir_func.params {
        sig.params.push(AbiParam::new(types::I64));
    }
    sig.returns.push(AbiParam::new(types::I64));
    aot.codegen_ctx.func.signature = sig;

    // Split module and codegen_ctx for independent borrows.
    let module: &mut JITModule = aot.module;
    let codegen_ctx: &mut codegen::Context = &mut aot.codegen_ctx;
    let builder_ctx: &mut FunctionBuilderContext = aot.builder_context;
    let local_base = mir::FunctionBuilder::LOCAL_BASE;
    let type_meta = mir_func.type_metadata.clone();

    // Analyze block predecessors.
    let preds = compute_predecessors(mir_func);

    // For each block, collect locals assigned in any predecessor that are
    // used in this block — these need block params when multiple preds exist.
    let block_liveins = compute_liveins(mir_func, &preds, local_base);

    // Pre-resolve cross-function call targets (will fill inside builder scope).
    let mut call_targets: HashMap<usize, FuncRef> = HashMap::new();

    let _helpers = {
        let mut builder = FunctionBuilder::new(&mut codegen_ctx.func, builder_ctx);
        let entry_block = builder.create_block();
        builder.switch_to_block(entry_block);
        builder.append_block_params_for_function_params(entry_block);

        // Register runtime helpers with proper signatures.
        let mut h: HashMap<&str, FuncRef> = HashMap::new();

        // Binary helpers: (i64, i64) -> i64
        let bin_helpers: &[&str] = &[
            "nulang_iadd",
            "nulang_isub",
            "nulang_imul",
            "nulang_idiv",
            "nulang_imod",
            "nulang_icmp_eq",
            "nulang_icmp_lt",
            "nulang_icmp_gt",
            "nulang_icmp_le",
            "nulang_icmp_ge",
            "nulang_fadd",
            "nulang_fsub",
            "nulang_fmul",
            "nulang_fdiv",
            "nulang_fcmp_eq",
            "nulang_fcmp_lt",
            "nulang_fcmp_gt",
            "nulang_and",
            "nulang_or",
            "nulang_xor",
            "nulang_shl",
            "nulang_shr",
            "nulang_bitand",
            "nulang_bitor",
        ];
        for name in bin_helpers {
            let mut h_sig = module.make_signature();
            h_sig.params.push(AbiParam::new(types::I64));
            h_sig.params.push(AbiParam::new(types::I64));
            h_sig.returns.push(AbiParam::new(types::I64));
            let h_id = module
                .declare_function(name, Linkage::Import, &h_sig)
                .map_err(|e| AotCompileError::Cranelift(e.to_string()))?;
            let func_ref = module.declare_func_in_func(h_id, builder.func);
            h.insert(*name, func_ref);
        }

        // Unary helpers: (i64) -> i64
        let unary_helpers: &[&str] = &[
            "nulang_ineg",
            "nulang_iinc",
            "nulang_idec",
            "nulang_not",
            "nulang_itof",
            "nulang_ftoi",
            "nulang_fneg",
        ];
        for name in unary_helpers {
            let mut h_sig = module.make_signature();
            h_sig.params.push(AbiParam::new(types::I64));
            h_sig.returns.push(AbiParam::new(types::I64));
            let h_id = module
                .declare_function(name, Linkage::Import, &h_sig)
                .map_err(|e| AotCompileError::Cranelift(e.to_string()))?;
            let func_ref = module.declare_func_in_func(h_id, builder.func);
            h.insert(*name, func_ref);
        }

        // Helper to register a call target FuncRef.
        let mut register_call_target = |n: usize| {
            if !call_targets.contains_key(&n) {
                if let Some(&callee_fid) = aot.func_ids.get(n) {
                    let local_ref = module.declare_func_in_func(callee_fid, builder.func);
                    call_targets.insert(n, local_ref);
                }
            }
        };

        // Pre-scan: register all call targets from Call and Closure rvalues.
        for block in &mir_func.blocks {
            for stmt in &block.stmts {
                match stmt {
                    mir::Stmt::Assign {
                        op:
                            mir::RValue::Call {
                                func: mir::FuncRef::Index(n),
                                ..
                            },
                        ..
                    } => {
                        register_call_target(*n);
                    }
                    mir::Stmt::Assign {
                        op: mir::RValue::Closure { func, captures },
                        ..
                    } if captures.is_empty() => {
                        register_call_target(*func);
                    }
                    _ => {}
                }
            }
        }

        // Track which locals hold zero-capture closures for call resolution.
        let mut closure_targets: HashMap<u32, usize> = HashMap::new();
        let mut local_vals: HashMap<u32, Value> = HashMap::new();

        for (i, param_id) in mir_func.params.iter().enumerate() {
            let reg = local_base + param_id.0;
            let val = builder.block_params(entry_block)[i];
            local_vals.insert(reg, val);
        }

        // Create CLIF blocks — allocate block params for merge blocks.
        let mut block_map: HashMap<mir::BlockId, cranelift::prelude::Block> = HashMap::new();
        // Track which locals have block params in each block.
        let mut block_params: HashMap<mir::BlockId, Vec<u32>> = HashMap::new();
        for block in &mir_func.blocks {
            let clif_block = if block.id == mir_func.entry {
                entry_block
            } else {
                let blk = builder.create_block();
                // Add block params for locals that need merging.
                if let Some(liveins) = block_liveins.get(&block.id) {
                    let mut params = Vec::new();
                    for &reg in liveins {
                        builder.append_block_param(blk, types::I64);
                        params.push(reg);
                    }
                    block_params.insert(block.id, params);
                }
                blk
            };
            block_map.insert(block.id, clif_block);
        }

        // Compile blocks in order.
        for block in &mir_func.blocks {
            let clif_block = block_map[&block.id];
            builder.switch_to_block(clif_block);

            // Read block parameters into local_vals for non-entry blocks.
            if block.id != mir_func.entry {
                if let Some(params) = block_params.get(&block.id) {
                    for (i, &reg) in params.iter().enumerate() {
                        let val = builder.block_params(clif_block)[i];
                        local_vals.insert(reg, val);
                    }
                }
            }

            for stmt in &block.stmts {
                compile_stmt(
                    &mut builder,
                    stmt,
                    &type_meta,
                    &h,
                    &call_targets,
                    &mut closure_targets,
                    &mut local_vals,
                    mode,
                )?;
            }
            compile_terminator_with_params(
                &mut builder,
                &block.terminator,
                &block_map,
                &block_params,
                &local_vals,
                mode,
            )?;
        }

        builder.seal_all_blocks();
        builder.finalize();
        h
    };
    // Debug: dump CLIF when verbose.
    if std::env::var("NULANG_DUMP_CLIF").is_ok() {
        eprintln!("=== CLIF for fn_{} ===", _func_index);
        eprintln!("{}", codegen_ctx.func.display());
    }

    module
        .define_function(func_id, codegen_ctx)
        .map_err(|e| AotCompileError::Cranelift(e.to_string()))?;

    module.clear_context(codegen_ctx);

    Ok(())
}

/// Generate a thin boxing wrapper for an all-Int function.
///
/// The wrapper takes tagged i64 arguments, untags them, calls the unboxed
/// variant, tags the result, and returns. This replaces the boxed body so
/// that callers always go through the wrapper — the original boxed body
/// is never compiled.
pub fn compile_boxing_wrapper(
    aot: &mut AotContext,
    param_count: usize,
    boxed_fid: cranelift_module::FuncId,
    unboxed_fid: cranelift_module::FuncId,
) -> AotResult<()> {
    // Split module and codegen_ctx for independent borrows.
    let module: &mut JITModule = aot.module;
    let codegen_ctx: &mut codegen::Context = &mut aot.codegen_ctx;
    let builder_ctx: &mut FunctionBuilderContext = aot.builder_context;
    // Set up function signature: tagged i64 params, tagged i64 return.
    let mut sig = module.make_signature();
    for _ in 0..param_count {
        sig.params.push(AbiParam::new(types::I64));
    }
    sig.returns.push(AbiParam::new(types::I64));
    codegen_ctx.func.signature = sig;

    let mut builder = FunctionBuilder::new(&mut codegen_ctx.func, builder_ctx);
    let entry_block = builder.create_block();
    builder.switch_to_block(entry_block);
    builder.append_block_params_for_function_params(entry_block);

    // Get unboxed function reference.
    let callee_ref = module.declare_func_in_func(unboxed_fid, builder.func);

    // Untag each parameter.
    let params: Vec<Value> = builder.block_params(entry_block).to_vec();
    let unboxed_args: Vec<Value> = params
        .iter()
        .map(|&p| emit_sext48(&mut builder, p))
        .collect();

    // Call unboxed variant.
    let call = builder.ins().call(callee_ref, &unboxed_args);
    let raw_result = builder.inst_results(call)[0];

    // Tag result and return.
    let tagged = emit_tag_int(&mut builder, raw_result);
    builder.ins().return_(&[tagged]);

    builder.seal_all_blocks();
    builder.finalize();

    // Debug: dump CLIF when verbose.
    if std::env::var("NULANG_DUMP_CLIF").is_ok() {
        eprintln!("=== CLIF for boxing wrapper ({}) ===", param_count);
        eprintln!("{}", codegen_ctx.func.display());
    }

    module
        .define_function(boxed_fid, codegen_ctx)
        .map_err(|e| AotCompileError::Cranelift(e.to_string()))?;

    module.clear_context(codegen_ctx);

    Ok(())
}

// ---------------------------------------------------------------------------
// Statement compilation
// ---------------------------------------------------------------------------

fn compile_stmt(
    builder: &mut FunctionBuilder,
    stmt: &mir::Stmt,
    type_meta: &TypeMetadata,
    helpers: &HashMap<&str, FuncRef>,
    call_targets: &HashMap<usize, FuncRef>,
    closure_targets: &mut HashMap<u32, usize>,
    local_vals: &mut HashMap<u32, Value>,
    mode: CompileMode,
) -> AotResult<()> {
    match stmt {
        mir::Stmt::Assign { dst, op } => {
            if let mir::RValue::Closure { func, captures } = op {
                if captures.is_empty() {
                    let reg = mir::FunctionBuilder::LOCAL_BASE + dst.0;
                    closure_targets.insert(reg, *func);
                }
            }
            let val = compile_rvalue(
                builder,
                op,
                type_meta,
                helpers,
                call_targets,
                closure_targets,
                local_vals,
                mode,
            )?;
            let reg = mir::FunctionBuilder::LOCAL_BASE + dst.0;
            local_vals.insert(reg, val);
            Ok(())
        }
        _ => Err(AotCompileError::Unsupported(format!(
            "statement {:?}",
            stmt
        ))),
    }
}

// ---------------------------------------------------------------------------
// RValue compilation
// ---------------------------------------------------------------------------

fn compile_rvalue(
    builder: &mut FunctionBuilder,
    rv: &mir::RValue,
    type_meta: &TypeMetadata,
    helpers: &HashMap<&str, FuncRef>,
    call_targets: &HashMap<usize, FuncRef>,
    closure_targets: &mut HashMap<u32, usize>,
    local_vals: &HashMap<u32, Value>,
    mode: CompileMode,
) -> AotResult<Value> {
    match rv {
        mir::RValue::Const(c) => compile_const(builder, c, mode),

        mir::RValue::Load(id) => {
            let reg = mir::FunctionBuilder::LOCAL_BASE + id.0;
            local_vals
                .get(&reg)
                .copied()
                .ok_or_else(|| AotCompileError::Internal(format!("uninitialized local {}", id.0)))
        }
        mir::RValue::Binary(op, lhs, rhs) => compile_binary(
            builder, *op, *lhs, *rhs, type_meta, helpers, local_vals, mode,
        ),

        mir::RValue::Unary(op, operand) => {
            compile_unary(builder, *op, *operand, type_meta, helpers, local_vals, mode)
        }

        mir::RValue::Call { func, args } => {
            let callee_ref = match func {
                mir::FuncRef::Index(n) => call_targets.get(n).copied().ok_or_else(|| {
                    AotCompileError::Internal(format!("call target fn {} not compiled yet", n))
                })?,
                mir::FuncRef::Local(closure_id) => {
                    let reg = mir::FunctionBuilder::LOCAL_BASE + closure_id.0;
                    let target_idx = closure_targets.get(&reg).copied().ok_or_else(|| {
                        AotCompileError::Unsupported(
                            "indirect call: closure target unknown at compile time".into(),
                        )
                    })?;
                    call_targets.get(&target_idx).copied().ok_or_else(|| {
                        AotCompileError::Internal(format!(
                            "call target fn {} not compiled yet",
                            target_idx
                        ))
                    })?
                }
            };
            let arg_vals: Vec<Value> =
                args.iter()
                    .map(|id| {
                        let reg = mir::FunctionBuilder::LOCAL_BASE + id.0;
                        local_vals.get(&reg).copied().ok_or_else(|| {
                            AotCompileError::Internal("call arg uninitialized".into())
                        })
                    })
                    .collect::<AotResult<Vec<_>>>()?;
            let call = builder.ins().call(callee_ref, &arg_vals);
            Ok(builder.inst_results(call)[0])
        }

        mir::RValue::Closure { func, captures } => {
            if captures.is_empty() {
                // Return tagged function index — also register for call resolution.
                let idx = builder.ins().iconst(types::I64, *func as i64);
                Ok(emit_tag_int(builder, idx))
            } else {
                Err(AotCompileError::Unsupported(
                    "closures with captures".into(),
                ))
            }
        }

        _ => Err(AotCompileError::Unsupported(format!("rvalue {:?}", rv))),
    }
}

// ---------------------------------------------------------------------------
// Constant emission
// ---------------------------------------------------------------------------

fn compile_const(
    builder: &mut FunctionBuilder,
    c: &crate::bytecode::Constant,
    mode: CompileMode,
) -> AotResult<Value> {
    match c {
        crate::bytecode::Constant::Int(v) => {
            if mode == CompileMode::Unboxed {
                Ok(builder.ins().iconst(types::I64, *v))
            } else {
                let iconst_val = builder.ins().iconst(types::I64, *v);
                Ok(emit_tag_int(builder, iconst_val))
            }
        }
        crate::bytecode::Constant::Float(f) => {
            Ok(builder.ins().iconst(types::I64, f.to_bits() as i64))
        }
        crate::bytecode::Constant::Bool(b) => Ok(builder
            .ins()
            .iconst(types::I64, TAG_BOOL_I64 | if *b { 1 } else { 0 })),
        crate::bytecode::Constant::Unit => Ok(builder
            .ins()
            .iconst(types::I64, 0x7FF9_0000_0000_0000u64 as i64)),
        _ => Err(AotCompileError::Unsupported(format!("constant {:?}", c))),
    }
}

// ---------------------------------------------------------------------------
// Binary operation emission
// ---------------------------------------------------------------------------

fn compile_binary(
    builder: &mut FunctionBuilder,
    op: crate::ast::BinOp,
    lhs: mir::LocalId,
    rhs: mir::LocalId,
    type_meta: &TypeMetadata,
    helpers: &HashMap<&str, FuncRef>,
    local_vals: &HashMap<u32, Value>,
    mode: CompileMode,
) -> AotResult<Value> {
    let lhs_reg = mir::FunctionBuilder::LOCAL_BASE + lhs.0;
    let rhs_reg = mir::FunctionBuilder::LOCAL_BASE + rhs.0;
    let lhs_val = *local_vals
        .get(&lhs_reg)
        .ok_or_else(|| AotCompileError::Internal("uninitialized lhs".into()))?;
    let rhs_val = *local_vals
        .get(&rhs_reg)
        .ok_or_else(|| AotCompileError::Internal("uninitialized rhs".into()))?;

    use crate::ast::BinOp;
    let lhs_reg_usize = lhs_reg as usize;
    let rhs_reg_usize = rhs_reg as usize;

    match op {
        BinOp::Add => {
            if type_meta.both_known(lhs_reg_usize, rhs_reg_usize, KnownType::Int) {
                if mode == CompileMode::Unboxed {
                    Ok(builder.ins().iadd(lhs_val, rhs_val))
                } else {
                    let l = emit_sext48(builder, lhs_val);
                    let r = emit_sext48(builder, rhs_val);
                    let sum = builder.ins().iadd(l, r);
                    Ok(emit_tag_int(builder, sum))
                }
            } else if type_meta.both_known(lhs_reg_usize, rhs_reg_usize, KnownType::Float) {
                let l = builder.ins().bitcast(types::F64, MemFlags::new(), lhs_val);
                let r = builder.ins().bitcast(types::F64, MemFlags::new(), rhs_val);
                let sum = builder.ins().fadd(l, r);
                Ok(builder.ins().bitcast(types::I64, MemFlags::new(), sum))
            } else {
                // Fall back to runtime helper.
                call_helper(builder, helpers, "nulang_iadd", &[lhs_val, rhs_val])
            }
        }
        BinOp::Sub => {
            if type_meta.both_known(lhs_reg_usize, rhs_reg_usize, KnownType::Int) {
                if mode == CompileMode::Unboxed {
                    Ok(builder.ins().isub(lhs_val, rhs_val))
                } else {
                    let l = emit_sext48(builder, lhs_val);
                    let r = emit_sext48(builder, rhs_val);
                    let diff = builder.ins().isub(l, r);
                    Ok(emit_tag_int(builder, diff))
                }
            } else if type_meta.both_known(lhs_reg_usize, rhs_reg_usize, KnownType::Float) {
                let l = builder.ins().bitcast(types::F64, MemFlags::new(), lhs_val);
                let r = builder.ins().bitcast(types::F64, MemFlags::new(), rhs_val);
                let diff = builder.ins().fsub(l, r);
                Ok(builder.ins().bitcast(types::I64, MemFlags::new(), diff))
            } else {
                call_helper(builder, helpers, "nulang_isub", &[lhs_val, rhs_val])
            }
        }
        BinOp::Mul => {
            if type_meta.both_known(lhs_reg_usize, rhs_reg_usize, KnownType::Int) {
                if mode == CompileMode::Unboxed {
                    Ok(builder.ins().imul(lhs_val, rhs_val))
                } else {
                    let l = emit_sext48(builder, lhs_val);
                    let r = emit_sext48(builder, rhs_val);
                    let prod = builder.ins().imul(l, r);
                    Ok(emit_tag_int(builder, prod))
                }
            } else if type_meta.both_known(lhs_reg_usize, rhs_reg_usize, KnownType::Float) {
                let l = builder.ins().bitcast(types::F64, MemFlags::new(), lhs_val);
                let r = builder.ins().bitcast(types::F64, MemFlags::new(), rhs_val);
                let prod = builder.ins().fmul(l, r);
                Ok(builder.ins().bitcast(types::I64, MemFlags::new(), prod))
            } else {
                call_helper(builder, helpers, "nulang_imul", &[lhs_val, rhs_val])
            }
        }
        BinOp::Div => {
            if type_meta.both_known(lhs_reg_usize, rhs_reg_usize, KnownType::Int)
                && mode == CompileMode::Boxed
            {
                // Inline div-by-zero → nil check for boxed Int operands.
                let l = emit_sext48(builder, lhs_val);
                let r = emit_sext48(builder, rhs_val);
                let zero = builder.ins().iconst(types::I64, 0);
                let is_zero = builder.ins().icmp(IntCC::Equal, r, zero);
                let nil = builder.ins().iconst(types::I64, TAG_NIL_I64);
                let div_result = builder.ins().sdiv(l, r);
                let tagged = emit_tag_int(builder, div_result);
                Ok(builder.ins().select(is_zero, nil, tagged))
            } else {
                call_helper(builder, helpers, "nulang_idiv", &[lhs_val, rhs_val])
            }
        }
        BinOp::Mod => {
            if type_meta.both_known(lhs_reg_usize, rhs_reg_usize, KnownType::Int)
                && mode == CompileMode::Boxed
            {
                let l = emit_sext48(builder, lhs_val);
                let r = emit_sext48(builder, rhs_val);
                let zero = builder.ins().iconst(types::I64, 0);
                let is_zero = builder.ins().icmp(IntCC::Equal, r, zero);
                let nil = builder.ins().iconst(types::I64, TAG_NIL_I64);
                let rem_result = builder.ins().srem(l, r);
                let tagged = emit_tag_int(builder, rem_result);
                Ok(builder.ins().select(is_zero, nil, tagged))
            } else {
                call_helper(builder, helpers, "nulang_imod", &[lhs_val, rhs_val])
            }
        }
        BinOp::Eq => {
            if type_meta.both_known(lhs_reg_usize, rhs_reg_usize, KnownType::Int) {
                if mode == CompileMode::Unboxed {
                    let cmp = builder.ins().icmp(IntCC::Equal, lhs_val, rhs_val);
                    Ok(emit_tag_bool(builder, cmp))
                } else {
                    let l = emit_sext48(builder, lhs_val);
                    let r = emit_sext48(builder, rhs_val);
                    let cmp = builder.ins().icmp(IntCC::Equal, l, r);
                    Ok(emit_tag_bool(builder, cmp))
                }
            } else {
                call_helper(builder, helpers, "nulang_icmp_eq", &[lhs_val, rhs_val])
            }
        }
        BinOp::Lt => {
            if type_meta.both_known(lhs_reg_usize, rhs_reg_usize, KnownType::Int) {
                if mode == CompileMode::Unboxed {
                    let cmp = builder.ins().icmp(IntCC::SignedLessThan, lhs_val, rhs_val);
                    Ok(emit_tag_bool(builder, cmp))
                } else {
                    let l = emit_sext48(builder, lhs_val);
                    let r = emit_sext48(builder, rhs_val);
                    let cmp = builder.ins().icmp(IntCC::SignedLessThan, l, r);
                    Ok(emit_tag_bool(builder, cmp))
                }
            } else {
                call_helper(builder, helpers, "nulang_icmp_lt", &[lhs_val, rhs_val])
            }
        }
        BinOp::Gt => {
            if type_meta.both_known(lhs_reg_usize, rhs_reg_usize, KnownType::Int) {
                if mode == CompileMode::Unboxed {
                    let cmp = builder
                        .ins()
                        .icmp(IntCC::SignedGreaterThan, lhs_val, rhs_val);
                    Ok(emit_tag_bool(builder, cmp))
                } else {
                    let l = emit_sext48(builder, lhs_val);
                    let r = emit_sext48(builder, rhs_val);
                    let cmp = builder.ins().icmp(IntCC::SignedGreaterThan, l, r);
                    Ok(emit_tag_bool(builder, cmp))
                }
            } else {
                call_helper(builder, helpers, "nulang_icmp_gt", &[lhs_val, rhs_val])
            }
        }
        BinOp::Le => {
            if type_meta.both_known(lhs_reg_usize, rhs_reg_usize, KnownType::Int) {
                if mode == CompileMode::Unboxed {
                    let cmp = builder
                        .ins()
                        .icmp(IntCC::SignedLessThanOrEqual, lhs_val, rhs_val);
                    Ok(emit_tag_bool(builder, cmp))
                } else {
                    let l = emit_sext48(builder, lhs_val);
                    let r = emit_sext48(builder, rhs_val);
                    let cmp = builder.ins().icmp(IntCC::SignedLessThanOrEqual, l, r);
                    Ok(emit_tag_bool(builder, cmp))
                }
            } else {
                call_helper(builder, helpers, "nulang_icmp_le", &[lhs_val, rhs_val])
            }
        }
        BinOp::Ge => {
            if type_meta.both_known(lhs_reg_usize, rhs_reg_usize, KnownType::Int) {
                if mode == CompileMode::Unboxed {
                    let cmp = builder
                        .ins()
                        .icmp(IntCC::SignedGreaterThanOrEqual, lhs_val, rhs_val);
                    Ok(emit_tag_bool(builder, cmp))
                } else {
                    let l = emit_sext48(builder, lhs_val);
                    let r = emit_sext48(builder, rhs_val);
                    let cmp = builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, l, r);
                    Ok(emit_tag_bool(builder, cmp))
                }
            } else {
                call_helper(builder, helpers, "nulang_icmp_ge", &[lhs_val, rhs_val])
            }
        }
        BinOp::Ne => {
            if type_meta.both_known(lhs_reg_usize, rhs_reg_usize, KnownType::Int) {
                if mode == CompileMode::Unboxed {
                    let cmp = builder.ins().icmp(IntCC::NotEqual, lhs_val, rhs_val);
                    Ok(emit_tag_bool(builder, cmp))
                } else {
                    let l = emit_sext48(builder, lhs_val);
                    let r = emit_sext48(builder, rhs_val);
                    let cmp = builder.ins().icmp(IntCC::NotEqual, l, r);
                    Ok(emit_tag_bool(builder, cmp))
                }
            } else {
                call_helper(builder, helpers, "nulang_icmp_eq", &[lhs_val, rhs_val])
                    .and_then(|eq| call_helper(builder, helpers, "nulang_not", &[eq]))
            }
        }
        _ => Err(AotCompileError::Unsupported(format!("binary op {:?}", op))),
    }
}

// ---------------------------------------------------------------------------
// Unary operation emission
// ---------------------------------------------------------------------------

fn compile_unary(
    builder: &mut FunctionBuilder,
    op: crate::ast::UnOp,
    operand: mir::LocalId,
    type_meta: &TypeMetadata,
    helpers: &HashMap<&str, FuncRef>,
    local_vals: &HashMap<u32, Value>,
    mode: CompileMode,
) -> AotResult<Value> {
    let reg = mir::FunctionBuilder::LOCAL_BASE + operand.0;
    let val = *local_vals
        .get(&reg)
        .ok_or_else(|| AotCompileError::Internal("uninitialized operand".into()))?;

    use crate::ast::UnOp;
    match op {
        UnOp::Neg => {
            if type_meta.is_known(reg as usize, KnownType::Int) {
                if mode == CompileMode::Unboxed {
                    Ok(builder.ins().ineg(val))
                } else {
                    let payload = emit_sext48(builder, val);
                    let neg = builder.ins().ineg(payload);
                    Ok(emit_tag_int(builder, neg))
                }
            } else if type_meta.is_known(reg as usize, KnownType::Float) {
                let f = builder.ins().bitcast(types::F64, MemFlags::new(), val);
                let neg = builder.ins().fneg(f);
                Ok(builder.ins().bitcast(types::I64, MemFlags::new(), neg))
            } else {
                call_helper(builder, helpers, "nulang_ineg", &[val])
            }
        }
        UnOp::Not => call_helper(builder, helpers, "nulang_not", &[val]),
        _ => Err(AotCompileError::Unsupported(format!("unary op {:?}", op))),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Emit a tagged boolean value from an i8 comparison result.
fn emit_tag_bool(builder: &mut FunctionBuilder, cond: Value) -> Value {
    let tag = builder.ins().iconst(types::I64, TAG_BOOL_I64);
    let true_val = builder.ins().iconst(types::I64, TAG_BOOL_I64 | 1);
    let val = builder.ins().select(cond, true_val, tag);
    val
}

/// Call a runtime helper function by name.
fn call_helper(
    builder: &mut FunctionBuilder,
    helpers: &HashMap<&str, FuncRef>,
    name: &str,
    args: &[Value],
) -> AotResult<Value> {
    let func_ref = helpers
        .get(name)
        .copied()
        .ok_or_else(|| AotCompileError::Internal(format!("helper {} not registered", name)))?;
    let call = builder.ins().call(func_ref, args);
    Ok(builder.inst_results(call)[0])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aot_compile_empty_function() {
        // A function with no params that returns nil.
        let mut builder = mir::FunctionBuilder::new("empty", None);
        builder.terminate(mir::Terminator::Return(None));
        let func = builder.build();

        // Verify type metadata is populated (should be empty for empty function).
        assert!(func.type_metadata.is_empty());
        assert_eq!(func.name, "empty");
    }

    #[test]
    fn test_aot_compile_int_return() {
        // A function that returns a constant int.
        let mut builder = mir::FunctionBuilder::new("answer", Some(crate::types::Type::int()));
        let tmp = builder.add_temp(crate::types::Type::int());
        builder.assign(tmp, mir::RValue::Const(crate::bytecode::Constant::Int(42)));
        builder.terminate(mir::Terminator::Return(Some(tmp)));
        let func = builder.build();

        // Verify type metadata captured the int type.
        let reg = mir::FunctionBuilder::LOCAL_BASE as usize + tmp.0 as usize;
        assert_eq!(func.type_metadata.get_type(reg), KnownType::Int);
    }

    #[test]
    fn test_aot_compile_add() {
        // A function that adds two int params.
        let mut builder = mir::FunctionBuilder::new("add", Some(crate::types::Type::int()));
        let a = builder.add_param("a", crate::types::Type::int());
        let b = builder.add_param("b", crate::types::Type::int());
        let sum = builder.add_temp(crate::types::Type::int());
        builder.assign(sum, mir::RValue::Binary(crate::ast::BinOp::Add, a, b));
        builder.terminate(mir::Terminator::Return(Some(sum)));
        let func = builder.build();

        // Verify params and result have Int type metadata.
        let reg_a = mir::FunctionBuilder::LOCAL_BASE as usize + a.0 as usize;
        let reg_b = mir::FunctionBuilder::LOCAL_BASE as usize + b.0 as usize;
        let reg_sum = mir::FunctionBuilder::LOCAL_BASE as usize + sum.0 as usize;
        assert_eq!(func.type_metadata.get_type(reg_a), KnownType::Int);
        assert_eq!(func.type_metadata.get_type(reg_b), KnownType::Int);
        assert_eq!(func.type_metadata.get_type(reg_sum), KnownType::Int);
    }
}
