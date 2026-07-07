//! MIR -> Bytecode codegen.
//!
//! Converts the Mid-level IR into the existing `CodeModule` bytecode format.
//! For v0.2 this is a minimal bridge: it creates a `CodeModule` and populates
//! the function table, then lowers each MIR function to bytecode instructions.

use crate::bytecode::{CodeModule, Constant, Instruction, OpCode};
use crate::mir;
use crate::types::NuResult;

const FUNC_VALUE_REG: u8 = 254;

#[derive(Debug, Clone, Copy)]
enum JumpKind {
    Jmp,
    JmpF,
}

#[derive(Debug, Clone)]
struct JumpPatch {
    instr_idx: usize,
    target_block: mir::BlockId,
    kind: JumpKind,
}

pub struct MirCodegen {
    module: CodeModule,
    func_map: std::collections::HashMap<String, usize>,
    next_reg: u8,
}

impl MirCodegen {
    pub fn new(module_name: impl Into<String>) -> Self {
        MirCodegen {
            module: CodeModule::new(module_name),
            func_map: std::collections::HashMap::new(),
            next_reg: 0,
        }
    }

    pub fn compile_module(&mut self, mir: &mir::Module) -> NuResult<&CodeModule> {
        // Reserve function table entries.
        for (idx, func) in mir.functions.iter().enumerate() {
            if !func.is_behavior {
                self.func_map.insert(func.name.clone(), idx);
                if idx >= self.module.function_table.len() {
                    self.module.function_table.resize(idx + 1, 0);
                }
            }
        }

        // Compile each function body.
        for (idx, func) in mir.functions.iter().enumerate() {
            if !func.is_behavior {
                let offset = self.compile_function(func)?;
                self.module.function_table[idx] = offset;
            }
        }

        // If a synthetic __main function exists, emit a tiny top-level prologue
        // that calls it and halts. Otherwise fall back to offset 0.
        if let Some(&main_idx) = self.func_map.get("__main") {
            let entry = self.module.instructions.len();
            self.load_constant(0, &Constant::Int(main_idx as i64));
            self.emit(Instruction::new3(OpCode::Call, 0, 0, 0));
            self.emit(Instruction::new0(OpCode::Halt));
            self.module.entry_point = Some(entry);
        } else if self.module.instructions.is_empty() {
            self.emit(Instruction::new0(OpCode::Halt));
            self.module.entry_point = Some(0);
        } else {
            self.module.entry_point = Some(0);
        }

        Ok(&self.module)
    }

    fn compile_function(&mut self, func: &mir::Function) -> NuResult<usize> {
        // Isolate this function's bytecode so block offsets are relative to the
        // function start while still allowing forward jump resolution.
        let mut saved_instructions = Vec::new();
        std::mem::swap(&mut saved_instructions, &mut self.module.instructions);
        let function_start_in_module = saved_instructions.len();

        // Map locals to registers.
        let mut local_reg: std::collections::HashMap<mir::LocalId, u8> =
            std::collections::HashMap::new();
        self.next_reg = 0;
        for local in &func.locals {
            let reg = self.alloc_reg();
            local_reg.insert(local.id, reg);
        }

        let mut block_offsets: std::collections::HashMap<mir::BlockId, usize> =
            std::collections::HashMap::new();
        let mut patches: Vec<JumpPatch> = Vec::new();

        for block in &func.blocks {
            block_offsets.insert(block.id, self.module.instructions.len());
            for stmt in &block.stmts {
                self.compile_stmt(stmt, &local_reg)?;
            }
            self.compile_terminator(&block.terminator, &local_reg, &block_offsets, &mut patches)?;
        }

        // Patch forward jumps now that all block offsets are known.
        for patch in &patches {
            let target_offset = block_offsets
                .get(&patch.target_block)
                .copied()
                .expect("patch target block missing");
            let diff = target_offset as i64 - patch.instr_idx as i64;
            let instr = &mut self.module.instructions[patch.instr_idx];
            match patch.kind {
                JumpKind::Jmp => {
                    instr.op1 = ((diff as i16 >> 8) & 0xFF) as u8;
                    instr.op2 = (diff as i16 & 0xFF) as u8;
                }
                JumpKind::JmpF => {
                    instr.op2 = ((diff as i16 >> 8) & 0xFF) as u8;
                    instr.op3 = (diff as i16 & 0xFF) as u8;
                }
            }
        }

        let mut function_code = Vec::new();
        std::mem::swap(&mut function_code, &mut self.module.instructions);
        self.module.instructions = saved_instructions;
        self.module.instructions.extend(function_code);

        Ok(function_start_in_module)
    }

    fn compile_stmt(
        &mut self,
        stmt: &mir::Stmt,
        local_reg: &std::collections::HashMap<mir::LocalId, u8>,
    ) -> NuResult<()> {
        match stmt {
            mir::Stmt::Assign { dst, op } => {
                let dst_reg = local_reg[dst];
                self.compile_rvalue(dst_reg, op, local_reg)?;
            }
            mir::Stmt::StoreField { obj, field, src } => {
                let obj_reg = local_reg[obj];
                let src_reg = local_reg[src];
                self.emit(Instruction::new3(OpCode::FieldS, obj_reg, *field, src_reg));
            }
            mir::Stmt::ArrayStore { arr, idx, src } => {
                let arr_reg = local_reg[arr];
                let idx_reg = local_reg[idx];
                let src_reg = local_reg[src];
                self.emit(Instruction::new3(OpCode::ArrStore, arr_reg, idx_reg, src_reg));
            }
            mir::Stmt::StateSet { field_idx, src } => {
                let src_reg = local_reg[src];
                let const_idx = self.module.add_constant(Constant::Int(*field_idx as i64));
                self.emit(Instruction::new3(
                    OpCode::ConstU,
                    ((const_idx >> 8) & 0xFF) as u8,
                    (const_idx & 0xFF) as u8,
                    src_reg,
                ));
            }
            mir::Stmt::Emit { event_idx, args } => {
                let _ = event_idx;
                let _ = args;
                // TODO: emit Emit opcode.
            }
        }
        Ok(())
    }

    fn compile_rvalue(
        &mut self,
        dst: u8,
        rv: &mir::RValue,
        local_reg: &std::collections::HashMap<mir::LocalId, u8>,
    ) -> NuResult<()> {
        match rv {
            mir::RValue::Const(c) => {
                self.load_constant(dst, c);
            }
            mir::RValue::Load(id) => {
                let src = local_reg[id];
                if src != dst {
                    self.emit(Instruction::new2(OpCode::Move, src, dst));
                }
            }
            mir::RValue::LoadField { obj, field } => {
                let obj_reg = local_reg[obj];
                self.emit(Instruction::new3(OpCode::FieldL, obj_reg, *field, dst));
            }
            mir::RValue::ArrayLoad { arr, idx } => {
                let arr_reg = local_reg[arr];
                let idx_reg = local_reg[idx];
                self.emit(Instruction::new3(OpCode::ArrLoad, arr_reg, idx_reg, dst));
            }
            mir::RValue::Unary(op, id) => {
                let src = local_reg[id];
                let opcode = match op {
                    crate::ast::UnOp::Neg => OpCode::INeg,
                    crate::ast::UnOp::Not => OpCode::Not,
                    _ => OpCode::Nop,
                };
                self.emit(Instruction::new2(opcode, src, dst));
            }
            mir::RValue::Binary(op, l, r) => {
                let lr = local_reg[l];
                let rr = local_reg[r];
                let opcode = binary_opcode(op);
                self.emit(Instruction::new3(opcode, lr, rr, dst));
            }
            mir::RValue::Call { func, args } => {
                // Move arguments into the low registers expected by the callee.
                for (i, a) in args.iter().enumerate() {
                    let reg = local_reg[a];
                    self.emit(Instruction::new2(OpCode::Move, reg, i as u8));
                }

                // Resolve the callee to a function-table index in a protected register.
                let func_idx = match func {
                    mir::FuncRef::Named(name) => self.func_map.get(name).copied(),
                    mir::FuncRef::Index(idx) => Some(*idx),
                };
                if let Some(idx) = func_idx {
                    self.load_constant(FUNC_VALUE_REG, &Constant::Int(idx as i64));
                    self.emit(Instruction::new3(
                        OpCode::Call,
                        FUNC_VALUE_REG,
                        args.len() as u8,
                        dst,
                    ));
                } else {
                    self.emit(Instruction::new1(OpCode::Const0, dst));
                }
            }
            mir::RValue::Tuple(elems) => {
                for (i, e) in elems.iter().enumerate() {
                    let reg = local_reg[e];
                    self.emit(Instruction::new2(OpCode::Move, reg, i as u8));
                }
                self.emit(Instruction::new2(OpCode::TupleMk, elems.len() as u8, dst));
            }
            mir::RValue::Record(fields) => {
                for (i, (_, e)) in fields.iter().enumerate() {
                    let reg = local_reg[e];
                    self.emit(Instruction::new2(OpCode::Move, reg, i as u8));
                }
                self.emit(Instruction::new2(OpCode::RecMk, fields.len() as u8, dst));
            }
            mir::RValue::Array { len } => {
                let len_reg = local_reg[len];
                self.emit(Instruction::new2(OpCode::ArrAlloc, len_reg, dst));
            }
            mir::RValue::Spawn { behavior_idx, init } => {
                let _ = behavior_idx;
                let _ = init;
                self.emit(Instruction::new1(OpCode::Const0, dst));
            }
            mir::RValue::Send { actor, behavior_id, args } => {
                let _ = actor;
                let _ = behavior_id;
                let _ = args;
                self.emit(Instruction::new1(OpCode::Const0, dst));
            }
            mir::RValue::Ask { actor, behavior_id, args } => {
                let _ = actor;
                let _ = behavior_id;
                let _ = args;
                self.emit(Instruction::new1(OpCode::Const0, dst));
            }
            mir::RValue::Perform { effect_id, op_id, args } => {
                let _ = effect_id;
                let _ = op_id;
                let _ = args;
                self.emit(Instruction::new1(OpCode::Const0, dst));
            }
            mir::RValue::FFICall { idx, args } => {
                let _ = idx;
                let _ = args;
                self.emit(Instruction::new1(OpCode::Const0, dst));
            }
            mir::RValue::SelfRef => {
                self.emit(Instruction::new1(OpCode::SelfOp, dst));
            }
            mir::RValue::NodeId => {
                self.emit(Instruction::new1(OpCode::NodeId, dst));
            }
            mir::RValue::CapabilityCheck { val, .. } => {
                let _ = local_reg[val];
                self.emit(Instruction::new1(OpCode::Const1, dst)); // true
            }
            mir::RValue::Closure { .. } => {
                self.emit(Instruction::new1(OpCode::Const0, dst));
            }
        }
        Ok(())
    }

    fn compile_terminator(
        &mut self,
        term: &mir::Terminator,
        local_reg: &std::collections::HashMap<mir::LocalId, u8>,
        block_offsets: &std::collections::HashMap<mir::BlockId, usize>,
        patches: &mut Vec<JumpPatch>,
    ) -> NuResult<()> {
        match term {
            mir::Terminator::Return(val) => {
                if let Some(id) = val {
                    let reg = *local_reg.get(id).expect("return local not mapped");
                    self.emit(Instruction::new2(OpCode::RetVal, reg, 0));
                }
                self.emit(Instruction::new0(OpCode::Ret));
            }
            mir::Terminator::Jump(target) => {
                let idx = self.module.instructions.len();
                if let Some(&offset) = block_offsets.get(target) {
                    let diff = offset as i64 - idx as i64;
                    self.emit(Instruction::new2(
                        OpCode::Jmp,
                        ((diff as i16 >> 8) & 0xFF) as u8,
                        (diff as i16 & 0xFF) as u8,
                    ));
                } else {
                    self.emit(Instruction::new2(OpCode::Jmp, 0, 0));
                    patches.push(JumpPatch {
                        instr_idx: idx,
                        target_block: *target,
                        kind: JumpKind::Jmp,
                    });
                }
            }
            mir::Terminator::Branch { cond, then_, else_ } => {
                let cond_reg = *local_reg.get(cond).expect("cond local not mapped");

                // JmpF to else_ when condition is false.
                let jmpf_idx = self.module.instructions.len();
                if let Some(&else_offset) = block_offsets.get(else_) {
                    let diff = else_offset as i64 - jmpf_idx as i64;
                    self.emit(Instruction::new3(
                        OpCode::JmpF,
                        cond_reg,
                        ((diff as i16 >> 8) & 0xFF) as u8,
                        (diff as i16 & 0xFF) as u8,
                    ));
                } else {
                    self.emit(Instruction::new3(OpCode::JmpF, cond_reg, 0, 0));
                    patches.push(JumpPatch {
                        instr_idx: jmpf_idx,
                        target_block: *else_,
                        kind: JumpKind::JmpF,
                    });
                }

                // Unconditional jump to then_.
                let jmp_idx = self.module.instructions.len();
                if let Some(&then_offset) = block_offsets.get(then_) {
                    let diff = then_offset as i64 - jmp_idx as i64;
                    self.emit(Instruction::new2(
                        OpCode::Jmp,
                        ((diff as i16 >> 8) & 0xFF) as u8,
                        (diff as i16 & 0xFF) as u8,
                    ));
                } else {
                    self.emit(Instruction::new2(OpCode::Jmp, 0, 0));
                    patches.push(JumpPatch {
                        instr_idx: jmp_idx,
                        target_block: *then_,
                        kind: JumpKind::Jmp,
                    });
                }
            }
            mir::Terminator::Switch { .. } => {
                // TODO: implement Switch opcode.
                self.emit(Instruction::new0(OpCode::Ret));
            }
            mir::Terminator::Handle { .. } => {
                self.emit(Instruction::new0(OpCode::Ret));
            }
            mir::Terminator::Unwind => {
                self.emit(Instruction::new0(OpCode::Ret));
            }
        }
        Ok(())
    }

    fn load_constant(&mut self, dst: u8, c: &Constant) {
        match c {
            Constant::Int(0) => self.emit(Instruction::new1(OpCode::Const0, dst)),
            Constant::Int(1) => self.emit(Instruction::new1(OpCode::Const1, dst)),
            Constant::Int(2) => self.emit(Instruction::new1(OpCode::Const2, dst)),
            Constant::Int(-1) => self.emit(Instruction::new1(OpCode::ConstM1, dst)),
            Constant::Int(n) if *n >= 0 && *n <= 0xFFFF => {
                let idx = self.module.add_constant(c.clone());
                self.emit(Instruction::new3(
                    OpCode::ConstU,
                    ((idx >> 8) & 0xFF) as u8,
                    (idx & 0xFF) as u8,
                    dst,
                ));
            }
            _ => {
                let idx = self.module.add_constant(c.clone());
                self.emit(Instruction::new3(
                    OpCode::ConstU,
                    ((idx >> 8) & 0xFF) as u8,
                    (idx & 0xFF) as u8,
                    dst,
                ));
            }
        }
    }

    fn alloc_reg(&mut self) -> u8 {
        let r = self.next_reg;
        self.next_reg += 1;
        r
    }

    fn emit(&mut self, instr: Instruction) {
        self.module.instructions.push(instr);
    }

    pub fn finish(self) -> CodeModule {
        self.module
    }
}

fn binary_opcode(op: &crate::ast::BinOp) -> OpCode {
    match op {
        crate::ast::BinOp::Add => OpCode::IAdd,
        crate::ast::BinOp::Sub => OpCode::ISub,
        crate::ast::BinOp::Mul => OpCode::IMul,
        crate::ast::BinOp::Div => OpCode::IDiv,
        crate::ast::BinOp::Mod => OpCode::IMod,
        crate::ast::BinOp::Eq => OpCode::ICmpEq,
        crate::ast::BinOp::Lt => OpCode::ICmpLt,
        crate::ast::BinOp::Gt => OpCode::ICmpGt,
        crate::ast::BinOp::Le => OpCode::ICmpLe,
        crate::ast::BinOp::Ge => OpCode::ICmpGe,
        _ => OpCode::IAdd,
    }
}

pub fn compile_mir(mir: &mir::Module, module_name: impl Into<String>) -> NuResult<CodeModule> {
    let mut codegen = MirCodegen::new(module_name);
    codegen.compile_module(mir)?;
    Ok(codegen.finish())
}
