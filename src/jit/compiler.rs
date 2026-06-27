//! Bytecode to Cranelift IR compiler.
//!
//! Translates a contiguous region of Nulang bytecode into native machine
//! code via Cranelift. Each opcode is mapped to one or more CLIF
//! instructions, with NaN-tag-aware arithmetic delegated to runtime
//! helper functions (see `runtime.rs`).
//!
//! # Supported Opcodes (MVP)
//!
//! | Category | Opcodes |
//! |----------|---------|
//! | Special | Nop, Halt, Const0-2, ConstM1 |
//! | Register | Move, Swap, Dup |
//! | Integer Arith | IAdd, ISub, IMul, IDiv, IMod, INeg, IInc, IDec |
//! | Float Arith | FAdd, FSub, FMul, FDiv |
//! | Compare | ICmp{Eq,Lt,Gt,Le,Ge}, FCmp{Eq,Lt,Gt} |
//! | Logic | Not, And, Or |
//! | Control | Jmp, JmpT, JmpF |
//! | Convert | IToF, FToI |
//! | Debug | DbgPrint |

use std::collections::HashMap;

use cranelift::prelude::*;
use cranelift::codegen::ir::FuncRef;
use cranelift_frontend::FunctionBuilder;
use cranelift_module::{Linkage, Module};
use cranelift_jit::JITModule;

use crate::bytecode::{Instruction, OpCode};

// ---------------------------------------------------------------------------
// Opcode Support Matrix
// ---------------------------------------------------------------------------

/// Check if an opcode can be compiled by the JIT (MVP subset).
pub fn is_opcode_compilable(op: OpCode) -> bool {
    matches!(
        op,
        OpCode::Nop | OpCode::Halt
        | OpCode::Const0 | OpCode::Const1 | OpCode::Const2 | OpCode::ConstM1 | OpCode::ConstU
        | OpCode::Move | OpCode::Swap | OpCode::Dup
        | OpCode::IAdd | OpCode::ISub | OpCode::IMul | OpCode::IDiv | OpCode::IMod
        | OpCode::INeg | OpCode::IInc | OpCode::IDec
        | OpCode::FAdd | OpCode::FSub | OpCode::FMul | OpCode::FDiv
        | OpCode::ICmpEq | OpCode::ICmpLt | OpCode::ICmpGt | OpCode::ICmpLe | OpCode::ICmpGe
        | OpCode::FCmpEq | OpCode::FCmpLt | OpCode::FCmpGt
        | OpCode::Not | OpCode::And | OpCode::Or
        | OpCode::Jmp | OpCode::JmpT | OpCode::JmpF
        | OpCode::IToF | OpCode::FToI
        | OpCode::DbgPrint
        | OpCode::Ret | OpCode::RetVal
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

fn register_runtime_helpers<M: Module>(
    module: &mut M,
    builder: &mut FunctionBuilder,
) -> HashMap<&'static str, FuncRef> {
    let mut helpers = HashMap::new();

    let bin_helpers: &[&str] = &[
        "nulang_iadd", "nulang_isub", "nulang_imul", "nulang_idiv", "nulang_imod",
        "nulang_icmp_eq", "nulang_icmp_lt", "nulang_icmp_gt", "nulang_icmp_le", "nulang_icmp_ge",
        "nulang_fadd", "nulang_fsub", "nulang_fmul", "nulang_fdiv",
        "nulang_fcmp_eq", "nulang_fcmp_lt", "nulang_fcmp_gt",
        "nulang_and", "nulang_or",
    ];

    for name in bin_helpers {
        let func_id = module.declare_function(name, Linkage::Import, &make_bin_sig(module))
            .expect("failed to declare runtime helper");
        let func_ref = module.declare_func_in_func(func_id, builder.func);
        helpers.insert(*name, func_ref);
    }

    let unary_helpers: &[&str] = &[
        "nulang_ineg", "nulang_iinc", "nulang_idec",
        "nulang_not", "nulang_itof", "nulang_ftoi",
    ];

    for name in unary_helpers {
        let func_id = module.declare_function(name, Linkage::Import, &make_unary_sig(module))
            .expect("failed to declare runtime helper");
        let func_ref = module.declare_func_in_func(func_id, builder.func);
        helpers.insert(*name, func_ref);
    }

    helpers
}

// ---------------------------------------------------------------------------
// Compilation
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum CompileError {
    DeclareFailed(String),
    CompileFailed(String),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::DeclareFailed(msg) => write!(f, "function declaration failed: {}", msg),
            CompileError::CompileFailed(msg) => write!(f, "compilation failed: {}", msg),
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
    let _consts_ptr = builder.block_params(entry_block)[1];

    let helpers = register_runtime_helpers(module, &mut builder);

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
        let block = *blocks.get(&pc).unwrap();
        builder.switch_to_block(block);

        match instr.opcode {
            OpCode::Nop => {}
            OpCode::Halt => { builder.ins().jump(return_block, &[]); }
            OpCode::Const0 => { emit_const(&mut builder, regs_ptr, instr.op1 as usize, 0); }
            OpCode::Const1 => { emit_const(&mut builder, regs_ptr, instr.op1 as usize, 1); }
            OpCode::Const2 => { emit_const(&mut builder, regs_ptr, instr.op1 as usize, 2); }
            OpCode::ConstM1 => { emit_const(&mut builder, regs_ptr, instr.op1 as usize, -1); }
            OpCode::ConstU => { emit_const(&mut builder, regs_ptr, instr.op1 as usize, instr.imm16() as i64); }

            OpCode::Move => { let v = load_reg(&mut builder, regs_ptr, instr.op1 as usize); store_reg(&mut builder, regs_ptr, instr.op2 as usize, v); }
            OpCode::Dup => { let v = load_reg(&mut builder, regs_ptr, instr.op1 as usize); store_reg(&mut builder, regs_ptr, instr.op2 as usize, v); }
            OpCode::Swap => {
                let v1 = load_reg(&mut builder, regs_ptr, instr.op1 as usize);
                let v2 = load_reg(&mut builder, regs_ptr, instr.op2 as usize);
                store_reg(&mut builder, regs_ptr, instr.op1 as usize, v2);
                store_reg(&mut builder, regs_ptr, instr.op2 as usize, v1);
            }

            OpCode::IAdd => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_iadd"),
            OpCode::ISub => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_isub"),
            OpCode::IMul => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_imul"),
            OpCode::IDiv => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_idiv"),
            OpCode::IMod => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_imod"),
            OpCode::INeg => emit_unary(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, "nulang_ineg"),
            OpCode::IInc => emit_self_unary(&mut builder, &helpers, regs_ptr, instr.op1 as usize, "nulang_iinc"),
            OpCode::IDec => emit_self_unary(&mut builder, &helpers, regs_ptr, instr.op1 as usize, "nulang_idec"),

            OpCode::FAdd => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_fadd"),
            OpCode::FSub => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_fsub"),
            OpCode::FMul => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_fmul"),
            OpCode::FDiv => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_fdiv"),

            OpCode::ICmpEq => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_icmp_eq"),
            OpCode::ICmpLt => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_icmp_lt"),
            OpCode::ICmpGt => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_icmp_gt"),
            OpCode::ICmpLe => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_icmp_le"),
            OpCode::ICmpGe => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_icmp_ge"),
            OpCode::FCmpEq => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_fcmp_eq"),
            OpCode::FCmpLt => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_fcmp_lt"),
            OpCode::FCmpGt => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_fcmp_gt"),

            OpCode::Not => emit_unary(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, "nulang_not"),
            OpCode::And => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_and"),
            OpCode::Or => emit_binop(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, instr.op3 as usize, "nulang_or"),

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
                    builder.ins().brif(is_nonzero, target_block, &[], fallthrough, &[]);
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
                    builder.ins().brif(is_zero, target_block, &[], fallthrough, &[]);
                } else {
                    builder.ins().jump(fallthrough, &[]);
                }
            }

            OpCode::IToF => emit_unary(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, "nulang_itof"),
            OpCode::FToI => emit_unary(&mut builder, &helpers, regs_ptr, instr.op1 as usize, instr.op2 as usize, "nulang_ftoi"),

            OpCode::Ret | OpCode::RetVal => { builder.ins().jump(return_block, &[]); }
            OpCode::DbgPrint => {}

            _ => { builder.ins().jump(return_block, &[]); }
        }

        let is_terminator = matches!(
            instr.opcode,
            OpCode::Jmp | OpCode::JmpT | OpCode::JmpF | OpCode::Halt
            | OpCode::Ret | OpCode::RetVal
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

    let func_id = module.declare_function(func_name, Linkage::Local, &ctx.func.signature.clone())
        .map_err(|e| CompileError::DeclareFailed(format!("{}", e)))?;
    module.define_function(func_id, ctx)
        .map_err(|e| CompileError::CompileFailed(format!("{}", e)))?;
    module.finalize_definitions().unwrap();

    let code = module.get_finalized_function(func_id);
    Ok(code as *const u8)
}

// ---------------------------------------------------------------------------
// CLIF Generation Helpers
// ---------------------------------------------------------------------------

fn load_reg(builder: &mut FunctionBuilder, regs_ptr: Value, idx: usize) -> Value {
    let offset = (idx * 8) as i32;
    let addr = if offset == 0 { regs_ptr } else {
        let off = builder.ins().iconst(types::I64, offset as i64);
        builder.ins().iadd(regs_ptr, off)
    };
    builder.ins().load(types::I64, MemFlags::new(), addr, 0)
}

fn store_reg(builder: &mut FunctionBuilder, regs_ptr: Value, idx: usize, val: Value) {
    let offset = (idx * 8) as i32;
    let addr = if offset == 0 { regs_ptr } else {
        let off = builder.ins().iconst(types::I64, offset as i64);
        builder.ins().iadd(regs_ptr, off)
    };
    builder.ins().store(MemFlags::new(), val, addr, 0);
}

fn emit_const(builder: &mut FunctionBuilder, regs_ptr: Value, dst: usize, value: i64) {
    let tag = builder.ins().iconst(types::I64, 0x7FF9000000000000i64);
    let masked = value & 0x0000FFFFFFFFFFFFi64;
    let val_part = builder.ins().iconst(types::I64, masked);
    let tagged = builder.ins().bor(tag, val_part);
    store_reg(builder, regs_ptr, dst, tagged);
}

fn emit_binop(builder: &mut FunctionBuilder, helpers: &HashMap<&str, FuncRef>, regs_ptr: Value, op1: usize, op2: usize, dst: usize, helper_name: &str) {
    let a = load_reg(builder, regs_ptr, op1);
    let b = load_reg(builder, regs_ptr, op2);
    let func_ref = *helpers.get(helper_name).unwrap();
    let call = builder.ins().call(func_ref, &[a, b]);
    let result = builder.inst_results(call)[0];
    store_reg(builder, regs_ptr, dst, result);
}

fn emit_unary(builder: &mut FunctionBuilder, helpers: &HashMap<&str, FuncRef>, regs_ptr: Value, src: usize, dst: usize, helper_name: &str) {
    let a = load_reg(builder, regs_ptr, src);
    let func_ref = *helpers.get(helper_name).unwrap();
    let call = builder.ins().call(func_ref, &[a]);
    let result = builder.inst_results(call)[0];
    store_reg(builder, regs_ptr, dst, result);
}

fn emit_self_unary(builder: &mut FunctionBuilder, helpers: &HashMap<&str, FuncRef>, regs_ptr: Value, reg: usize, helper_name: &str) {
    emit_unary(builder, helpers, regs_ptr, reg, reg, helper_name);
}
