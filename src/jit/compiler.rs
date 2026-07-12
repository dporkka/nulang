//! Bytecode to Cranelift IR compiler.
//!
//! Translates a contiguous region of Nulang bytecode into native machine
//! code via Cranelift. Each opcode is mapped to one or more CLIF
//! instructions, with NaN-tag-aware arithmetic delegated to runtime
//! helper functions (see `runtime.rs`).
//!
//! # Supported Opcodes
//!
//! | Category | Opcodes |
//! |----------|---------|
//! | Special | Nop, Halt, Const0-2, ConstM1 |
//! | Register | Load, Store, Move, Swap, Dup |
//! | Integer Arith | IAdd, ISub, IMul, IDiv, IMod, INeg, IInc, IDec |
//! | Bitwise | Xor, Shl, Shr, BitAnd, BitOr |
//! | Float Arith | FAdd, FSub, FMul, FDiv, FNeg |
//! | Compare | ICmp{Eq,Lt,Gt,Le,Ge}, FCmp{Eq,Lt,Gt} |
//! | Logic | Not, And, Or |
//! | Control | Jmp, JmpT, JmpF |
//! | Convert | IToF, FToI |
//! | Debug | DbgPrint |

use std::collections::HashMap;

use cranelift::codegen::ir::FuncRef;
use cranelift::prelude::*;
use cranelift_frontend::FunctionBuilder;
use cranelift_jit::JITModule;
use cranelift_module::{Linkage, Module};

use crate::bytecode::{Instruction, OpCode};
use crate::value_layout::{PAYLOAD_MASK, TAG_INT};

// ---------------------------------------------------------------------------
// Opcode Support Matrix
// ---------------------------------------------------------------------------

/// Check if an opcode can be compiled by the JIT.
pub fn is_opcode_compilable(op: OpCode) -> bool {
    matches!(
        op,
        OpCode::Nop
            | OpCode::Halt
            | OpCode::Const0
            | OpCode::Const1
            | OpCode::Const2
            | OpCode::ConstM1
            | OpCode::ConstU
            | OpCode::Load
            | OpCode::Store
            | OpCode::Move
            | OpCode::Swap
            | OpCode::Dup
            | OpCode::IAdd
            | OpCode::ISub
            | OpCode::IMul
            | OpCode::IDiv
            | OpCode::IMod
            | OpCode::INeg
            | OpCode::IInc
            | OpCode::IDec
            | OpCode::Xor
            | OpCode::Shl
            | OpCode::Shr
            | OpCode::BitAnd
            | OpCode::BitOr
            | OpCode::FAdd
            | OpCode::FSub
            | OpCode::FMul
            | OpCode::FDiv
            | OpCode::FNeg
            | OpCode::ICmpEq
            | OpCode::ICmpLt
            | OpCode::ICmpGt
            | OpCode::ICmpLe
            | OpCode::ICmpGe
            | OpCode::FCmpEq
            | OpCode::FCmpLt
            | OpCode::FCmpGt
            | OpCode::Not
            | OpCode::And
            | OpCode::Or
            | OpCode::Jmp
            | OpCode::JmpT
            | OpCode::JmpF
            | OpCode::IToF
            | OpCode::FToI
            | OpCode::DbgPrint
            | OpCode::Ret
            | OpCode::RetVal
            | OpCode::ArrLoad
     )
}

// ---------------------------------------------------------------------------
// Signature Helpers
// ---------------------------------------------------------------------------

fn make_bin_sig<M: Module>(module: &M) -> Signature {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    sig
}

fn make_unary_sig<M: Module>(module: &M) -> Signature {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    sig
}

// ---------------------------------------------------------------------------
// Runtime Helper Registration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeHelper {
    IAdd,
    ISub,
    IMul,
    IDiv,
    IMod,
    Xor,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    ICmpEq,
    ICmpLt,
    ICmpGt,
    ICmpLe,
    ICmpGe,
    FAdd,
    FSub,
    FMul,
    FDiv,
    FNeg,
    FCmpEq,
    FCmpLt,
    FCmpGt,
    And,
    Or,
    INeg,
    IInc,
    IDec,
    Not,
    IToF,
    FToI,
}

fn register_runtime_helpers<M: Module>(
    module: &mut M,
    builder: &mut FunctionBuilder,
) -> Result<HashMap<RuntimeHelper, FuncRef>, CompileError> {
    let mut helpers = HashMap::new();

    let bin_helpers: &[(RuntimeHelper, &str)] = &[
        (RuntimeHelper::IAdd, "nulang_iadd"),
        (RuntimeHelper::ISub, "nulang_isub"),
        (RuntimeHelper::IMul, "nulang_imul"),
        (RuntimeHelper::IDiv, "nulang_idiv"),
        (RuntimeHelper::IMod, "nulang_imod"),
        (RuntimeHelper::Xor, "nulang_xor"),
        (RuntimeHelper::Shl, "nulang_shl"),
        (RuntimeHelper::Shr, "nulang_shr"),
        (RuntimeHelper::BitAnd, "nulang_bitand"),
        (RuntimeHelper::BitOr, "nulang_bitor"),
        (RuntimeHelper::ICmpEq, "nulang_icmp_eq"),
        (RuntimeHelper::ICmpLt, "nulang_icmp_lt"),
        (RuntimeHelper::ICmpGt, "nulang_icmp_gt"),
        (RuntimeHelper::ICmpLe, "nulang_icmp_le"),
        (RuntimeHelper::ICmpGe, "nulang_icmp_ge"),
        (RuntimeHelper::FAdd, "nulang_fadd"),
        (RuntimeHelper::FSub, "nulang_fsub"),
        (RuntimeHelper::FMul, "nulang_fmul"),
        (RuntimeHelper::FDiv, "nulang_fdiv"),
        (RuntimeHelper::FCmpEq, "nulang_fcmp_eq"),
        (RuntimeHelper::FCmpLt, "nulang_fcmp_lt"),
        (RuntimeHelper::FCmpGt, "nulang_fcmp_gt"),
        (RuntimeHelper::And, "nulang_and"),
        (RuntimeHelper::Or, "nulang_or"),
    ];

    for (helper, name) in bin_helpers {
        let func_id = module
            .declare_function(name, Linkage::Import, &make_bin_sig(module))
            .map_err(|e| CompileError::Internal(format!("declare {}: {}", name, e)))?;
        let func_ref = module.declare_func_in_func(func_id, builder.func);
        helpers.insert(*helper, func_ref);
    }

    let unary_helpers: &[(RuntimeHelper, &str)] = &[
        (RuntimeHelper::INeg, "nulang_ineg"),
        (RuntimeHelper::IInc, "nulang_iinc"),
        (RuntimeHelper::IDec, "nulang_idec"),
        (RuntimeHelper::Not, "nulang_not"),
        (RuntimeHelper::IToF, "nulang_itof"),
        (RuntimeHelper::FToI, "nulang_ftoi"),
        (RuntimeHelper::FNeg, "nulang_fneg"),
    ];

    for (helper, name) in unary_helpers {
        let func_id = module
            .declare_function(name, Linkage::Import, &make_unary_sig(module))
            .map_err(|e| CompileError::Internal(format!("declare {}: {}", name, e)))?;
        let func_ref = module.declare_func_in_func(func_id, builder.func);
        helpers.insert(*helper, func_ref);
    }

    Ok(helpers)
}

// ---------------------------------------------------------------------------
// Compilation
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum CompileError {
    DeclareFailed(String),
    CompileFailed(String),
    /// The region contains an opcode this compiler does not support;
    /// callers should fall back to another compiler.
    UnsupportedOpcode(String),
    /// An internal invariant was violated (missing block, missing helper).
    Internal(String),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::DeclareFailed(msg) => write!(f, "function declaration failed: {}", msg),
            CompileError::CompileFailed(msg) => write!(f, "compilation failed: {}", msg),
            CompileError::UnsupportedOpcode(msg) => write!(f, "unsupported opcode: {}", msg),
            CompileError::Internal(msg) => write!(f, "internal compiler error: {}", msg),
        }
    }
}

impl std::error::Error for CompileError {}

/// Compile a bytecode region to a native function.
pub fn compile_bytecode_region(
    module: &mut JITModule,
    builder_context: &mut FunctionBuilderContext,
    ctx: &mut codegen::Context,
    func_name: &str,
    start_offset: usize,
    num_instrs: usize,
    instructions: &[Instruction],
) -> Result<*const u8, CompileError> {
    ctx.clear();

    let pointer_type = module.isa().pointer_type();
    ctx.func.signature.params.push(AbiParam::new(pointer_type));
    ctx.func.signature.params.push(AbiParam::new(pointer_type));

    let mut builder = FunctionBuilder::new(&mut ctx.func, builder_context);

    let entry_block = builder.create_block();
    builder.append_block_params_for_function_params(entry_block);
    builder.switch_to_block(entry_block);
    builder.seal_block(entry_block);

    let regs_ptr = builder.block_params(entry_block)[0];
    let consts_ptr = builder.block_params(entry_block)[1];

    let helpers = register_runtime_helpers(module, &mut builder)?;

    let end_offset = (start_offset + num_instrs).min(instructions.len());
    let mut blocks: HashMap<usize, Block> = HashMap::new();
    for i in start_offset..end_offset {
        blocks.insert(i, builder.create_block());
    }
    let return_block = builder.create_block();

    if let Some(&first_block) = blocks.get(&start_offset) {
        builder.ins().jump(first_block, &[]);
    } else {
        builder.ins().return_(&[]);
    }

    for pc in start_offset..end_offset {
        let instr = instructions[pc];
        let block = *blocks
            .get(&pc)
            .ok_or_else(|| CompileError::Internal("missing block in compiled region".into()))?;
        builder.switch_to_block(block);

        match instr.opcode {
            OpCode::Nop => {}
            OpCode::Halt => {
                builder.ins().jump(return_block, &[]);
            }
            OpCode::Const0 => {
                emit_const(&mut builder, regs_ptr, instr.op1 as usize, 0);
            }
            OpCode::Const1 => {
                emit_const(&mut builder, regs_ptr, instr.op1 as usize, 1);
            }
            OpCode::Const2 => {
                emit_const(&mut builder, regs_ptr, instr.op1 as usize, 2);
            }
            OpCode::ConstM1 => {
                emit_const(&mut builder, regs_ptr, instr.op1 as usize, -1);
            }
            OpCode::ConstU => {
                let idx = instr.imm16() as usize;
                let offset = (idx * 8) as i32;
                let addr = if offset == 0 {
                    consts_ptr
                } else {
                    let off = builder.ins().iconst(types::I64, offset as i64);
                    builder.ins().iadd(consts_ptr, off)
                };
                let val = builder.ins().load(types::I64, MemFlags::new(), addr, 0);
                store_reg(&mut builder, regs_ptr, instr.op3 as usize, val);
            }

            OpCode::Load | OpCode::Store | OpCode::Move | OpCode::Dup => {
                let v = load_reg(&mut builder, regs_ptr, instr.op1 as usize);
                store_reg(&mut builder, regs_ptr, instr.op2 as usize, v);
            }
            OpCode::Swap => {
                let v1 = load_reg(&mut builder, regs_ptr, instr.op1 as usize);
                let v2 = load_reg(&mut builder, regs_ptr, instr.op2 as usize);
                store_reg(&mut builder, regs_ptr, instr.op1 as usize, v2);
                store_reg(&mut builder, regs_ptr, instr.op2 as usize, v1);
            }

            OpCode::IAdd => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::IAdd,
            ),
            OpCode::ISub => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::ISub,
            ),
            OpCode::IMul => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::IMul,
            ),
            OpCode::IDiv => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::IDiv,
            ),
            OpCode::IMod => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::IMod,
            ),
            OpCode::INeg => emit_unary(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                RuntimeHelper::INeg,
            ),
            OpCode::IInc => emit_self_unary(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                RuntimeHelper::IInc,
            ),
            OpCode::IDec => emit_self_unary(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                RuntimeHelper::IDec,
            ),
            OpCode::Xor => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::Xor,
            ),
            OpCode::Shl => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::Shl,
            ),
            OpCode::Shr => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::Shr,
            ),
            OpCode::BitAnd => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::BitAnd,
            ),
            OpCode::BitOr => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::BitOr,
            ),

            OpCode::FAdd => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::FAdd,
            ),
            OpCode::FSub => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::FSub,
            ),
            OpCode::FMul => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::FMul,
            ),
            OpCode::FDiv => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::FDiv,
            ),
            // Interpreter reads src from op1 and writes dst to op3 for FNeg.
            OpCode::FNeg => emit_unary(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op3 as usize,
                RuntimeHelper::FNeg,
            ),

            OpCode::ICmpEq => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::ICmpEq,
            ),
            OpCode::ICmpLt => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::ICmpLt,
            ),
            OpCode::ICmpGt => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::ICmpGt,
            ),
            OpCode::ICmpLe => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::ICmpLe,
            ),
            OpCode::ICmpGe => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::ICmpGe,
            ),
            OpCode::FCmpEq => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::FCmpEq,
            ),
            OpCode::FCmpLt => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::FCmpLt,
            ),
            OpCode::FCmpGt => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::FCmpGt,
            ),

            OpCode::Not => emit_unary(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                RuntimeHelper::Not,
            ),
            OpCode::And => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::And,
            ),
            OpCode::Or => emit_binop(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                instr.op3 as usize,
                RuntimeHelper::Or,
            ),

            OpCode::Jmp => {
                let target = (pc as i64 + instr.simm16() as i64) as usize;
                if let Some(&target_block) = blocks.get(&target) {
                    builder.ins().jump(target_block, &[]);
                } else {
                    builder.ins().jump(return_block, &[]);
                }
            }
            OpCode::JmpT => {
                let target = (pc as i64 + instr.offset16() as i64) as usize;
                let cond_val = load_reg(&mut builder, regs_ptr, instr.op1 as usize);
                let zero = builder.ins().iconst(types::I64, 0);
                let is_nonzero = builder.ins().icmp(IntCC::NotEqual, cond_val, zero);
                let fallthrough = *blocks.get(&(pc + 1)).unwrap_or(&return_block);
                if let Some(&target_block) = blocks.get(&target) {
                    builder
                        .ins()
                        .brif(is_nonzero, target_block, &[], fallthrough, &[]);
                } else {
                    builder.ins().jump(fallthrough, &[]);
                }
            }
            OpCode::JmpF => {
                let target = (pc as i64 + instr.offset16() as i64) as usize;
                let cond_val = load_reg(&mut builder, regs_ptr, instr.op1 as usize);
                let zero = builder.ins().iconst(types::I64, 0);
                let is_zero = builder.ins().icmp(IntCC::Equal, cond_val, zero);
                let fallthrough = *blocks.get(&(pc + 1)).unwrap_or(&return_block);
                if let Some(&target_block) = blocks.get(&target) {
                    builder
                        .ins()
                        .brif(is_zero, target_block, &[], fallthrough, &[]);
                } else {
                    builder.ins().jump(fallthrough, &[]);
                }
            }

            OpCode::IToF => emit_unary(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                RuntimeHelper::IToF,
            ),
            OpCode::FToI => emit_unary(
                &mut builder,
                &helpers,
                regs_ptr,
                instr.op1 as usize,
                instr.op2 as usize,
                RuntimeHelper::FToI,
            ),

            OpCode::Ret | OpCode::RetVal => {
                builder.ins().jump(return_block, &[]);
            }
            OpCode::DbgPrint => {}


            OpCode::ArrLoad => {
                emit_arr_load(
                    &mut builder, regs_ptr,
                    instr.op1 as usize, instr.op2 as usize, instr.op3 as usize,
                );
            }
            _ => {
                builder.ins().jump(return_block, &[]);
            }
        }

        let is_terminator = matches!(
            instr.opcode,
            OpCode::Jmp | OpCode::JmpT | OpCode::JmpF | OpCode::Halt | OpCode::Ret | OpCode::RetVal
        );

        if !is_terminator {
            if let Some(&next_block) = blocks.get(&(pc + 1)) {
                builder.ins().jump(next_block, &[]);
            } else {
                builder.ins().jump(return_block, &[]);
            }
        }
    }

    for block in blocks.values() {
        builder.seal_block(*block);
    }

    builder.switch_to_block(return_block);
    builder.seal_block(return_block);
    builder.ins().return_(&[]);

    builder.finalize();

    let func_id = module
        .declare_function(func_name, Linkage::Local, &ctx.func.signature.clone())
        .map_err(|e| CompileError::DeclareFailed(format!("{}", e)))?;
    module
        .define_function(func_id, ctx)
        .map_err(|e| CompileError::CompileFailed(format!("{}", e)))?;
    module
        .finalize_definitions()
        .map_err(|e| CompileError::CompileFailed(format!("finalize: {}", e)))?;

    let code = module.get_finalized_function(func_id);
    Ok(code as *const u8)
}

// ---------------------------------------------------------------------------
// CLIF Generation Helpers
// ---------------------------------------------------------------------------

fn load_reg(builder: &mut FunctionBuilder, regs_ptr: Value, idx: usize) -> Value {
    let offset = (idx * 8) as i32;
    let addr = if offset == 0 {
        regs_ptr
    } else {
        let off = builder.ins().iconst(types::I64, offset as i64);
        builder.ins().iadd(regs_ptr, off)
    };
    builder.ins().load(types::I64, MemFlags::new(), addr, 0)
}

fn store_reg(builder: &mut FunctionBuilder, regs_ptr: Value, idx: usize, val: Value) {
    let offset = (idx * 8) as i32;
    let addr = if offset == 0 {
        regs_ptr
    } else {
        let off = builder.ins().iconst(types::I64, offset as i64);
        builder.ins().iadd(regs_ptr, off)
    };
    builder.ins().store(MemFlags::new(), val, addr, 0);
}

fn emit_const(builder: &mut FunctionBuilder, regs_ptr: Value, dst: usize, value: i64) {
    let tag = builder.ins().iconst(types::I64, TAG_INT as i64);
    let masked = value & (PAYLOAD_MASK as i64);
    let val_part = builder.ins().iconst(types::I64, masked);
    let tagged = builder.ins().bor(tag, val_part);
    store_reg(builder, regs_ptr, dst, tagged);
}

pub(crate) fn emit_arr_load(builder: &mut FunctionBuilder, regs_ptr: Value, arr_reg: usize, idx_reg: usize, dst: usize) {
    // Load NaN-boxed array pointer and index.
    let arr_val = load_reg(builder, regs_ptr, arr_reg);
    let idx_val = load_reg(builder, regs_ptr, idx_reg);

    // Extract raw pointer (mask off tag bits from NaN-boxed pointer).
    let mask = builder.ins().iconst(types::I64, PAYLOAD_MASK as i64);
    let arr_ptr = builder.ins().band(arr_val, mask);

    // Extract int payload from NaN-boxed int (sext48 via shift-left-16 then sshr-16).
    let shifted = builder.ins().ishl_imm(idx_val, 16);
    let idx_raw = builder.ins().sshr_imm(shifted, 16);

    // Scale: idx * sizeof(Value) = idx * 8.
    let eight = builder.ins().iconst(types::I64, 8);
    let offset = builder.ins().imul(idx_raw, eight);

    // Compute effective address and load the tagged Value.
    let addr = builder.ins().iadd(arr_ptr, offset);
    let result = builder.ins().load(types::I64, MemFlags::new(), addr, 0);

    store_reg(builder, regs_ptr, dst, result);
}



fn emit_binop(
    builder: &mut FunctionBuilder,
    helpers: &HashMap<RuntimeHelper, FuncRef>,
    regs_ptr: Value,
    op1: usize,
    op2: usize,
    dst: usize,
    helper: RuntimeHelper,
) {
    let a = load_reg(builder, regs_ptr, op1);
    let b = load_reg(builder, regs_ptr, op2);
    let func_ref = *helpers
        .get(&helper)
        .expect("runtime helper not registered in helpers map");
    let call = builder.ins().call(func_ref, &[a, b]);
    let result = builder.inst_results(call)[0];
    store_reg(builder, regs_ptr, dst, result);
}

fn emit_unary(
    builder: &mut FunctionBuilder,
    helpers: &HashMap<RuntimeHelper, FuncRef>,
    regs_ptr: Value,
    src: usize,
    dst: usize,
    helper: RuntimeHelper,
) {
    let a = load_reg(builder, regs_ptr, src);
    let func_ref = *helpers
        .get(&helper)
        .expect("runtime helper not registered in helpers map");
    let call = builder.ins().call(func_ref, &[a]);
    let result = builder.inst_results(call)[0];
    store_reg(builder, regs_ptr, dst, result);
}
fn emit_self_unary(
    builder: &mut FunctionBuilder,
    helpers: &HashMap<RuntimeHelper, FuncRef>,
    regs_ptr: Value,
    reg: usize,
    helper: RuntimeHelper,
) {
    emit_unary(builder, helpers, regs_ptr, reg, reg, helper);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::OpCode;

    #[test]
    fn test_is_opcode_compilable_mvp() {
        assert!(is_opcode_compilable(OpCode::IAdd));
        assert!(is_opcode_compilable(OpCode::ISub));
        assert!(is_opcode_compilable(OpCode::Move));
        assert!(is_opcode_compilable(OpCode::Jmp));
        assert!(is_opcode_compilable(OpCode::Ret));
    }

    #[test]
    fn test_is_opcode_compilable_extended() {
        // Register copies.
        assert!(is_opcode_compilable(OpCode::Load));
        assert!(is_opcode_compilable(OpCode::Store));
        // Bitwise integer ops.
        assert!(is_opcode_compilable(OpCode::Xor));
        assert!(is_opcode_compilable(OpCode::Shl));
        assert!(is_opcode_compilable(OpCode::Shr));
        assert!(is_opcode_compilable(OpCode::BitAnd));
        assert!(is_opcode_compilable(OpCode::BitOr));
        // Float negate.
        assert!(is_opcode_compilable(OpCode::FNeg));
        // Opcodes the interpreter itself does not implement stay unsupported.
        assert!(!is_opcode_compilable(OpCode::IPow));
        assert!(!is_opcode_compilable(OpCode::FMod));
        assert!(!is_opcode_compilable(OpCode::ConstL));
    }

    #[test]
    fn test_is_opcode_compilable_not_mvp() {
        assert!(!is_opcode_compilable(OpCode::Spawn));
        assert!(!is_opcode_compilable(OpCode::Send));
        assert!(!is_opcode_compilable(OpCode::FFICall));
    }
}
