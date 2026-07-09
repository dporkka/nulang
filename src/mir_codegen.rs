//! MIR -> Bytecode codegen.
//!
//! Converts the Mid-level IR into the existing `CodeModule` bytecode format,
//! following the same runtime contracts as the stable AST compiler:
//!
//!   - call arguments travel in r0..rN, the callee value in r254;
//!   - closures are `Closure` objects over function-table entries, with
//!     captures stored via `CapStore` and loaded via a `CapLoad` prologue;
//!   - records use module-wide field ids (`RecMk`/`RecS`/`RecL`);
//!   - effect handlers use `Handle`/`Unwind`/`Resume` with handler tables.
//!
//! Register scheme: r0..r15 are a scratch/staging zone (call and effect
//! arguments, transient values); each MIR local gets the fixed register
//! `LOCAL_BASE + local_id`. A function whose locals exceed the register file
//! fails to compile with an honest error.

use crate::bytecode::{
    CodeModule, Constant, ForeignFunctionDef, HandlerBinding, HandlerTable, Instruction, OpCode,
};
use crate::mir;
use crate::types::{NuError, NuResult, Span};
use std::collections::HashMap;

const FUNC_VALUE_REG: u8 = 254;
const LOCAL_BASE: u32 = 16;
const MAX_STAGED_ARGS: usize = 16;
const SCRATCH0: u8 = 0;
const SCRATCH1: u8 = 1;

fn not_yet_implemented(feature: &str) -> NuError {
    NuError::NotYetImplemented {
        feature: feature.to_string(),
        span: Span::default(),
    }
}

fn compile_err(msg: impl Into<String>) -> NuError {
    NuError::VMError(msg.into())
}

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
    /// Module-wide record field ids, mirroring the stable compiler's layout.
    field_map: HashMap<String, u8>,
    next_field_id: u8,
}

impl MirCodegen {
    pub fn new(module_name: impl Into<String>) -> Self {
        MirCodegen {
            module: CodeModule::new(module_name),
            field_map: HashMap::new(),
            next_field_id: 0,
        }
    }

    pub fn compile_module(&mut self, mir: &mir::Module) -> NuResult<&CodeModule> {
        // Register foreign functions first so FFICall indices line up.
        for ff in &mir.foreign_functions {
            let params = ff
                .params
                .iter()
                .map(crate::compiler::nulang_type_to_ffi_type)
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    compile_err(format!(
                        "unsupported parameter type in extern function {}",
                        ff.symbol
                    ))
                })?;
            let ret = crate::compiler::nulang_type_to_ffi_type(&ff.ret).ok_or_else(|| {
                compile_err(format!(
                    "unsupported return type in extern function {}",
                    ff.symbol
                ))
            })?;
            self.module.foreign_functions.push(ForeignFunctionDef {
                library: ff.library.clone(),
                symbol: ff.symbol.clone(),
                params,
                ret,
            });
        }

        // Reserve one function-table slot per MIR function; MIR function
        // indices are function-table indices.
        self.module.function_table.resize(mir.functions.len(), 0);

        let mut main_idx = None;
        for (idx, func) in mir.functions.iter().enumerate() {
            let offset = self.compile_function(func)?;
            self.module.function_table[idx] = offset;
            if func.name == "__main" {
                main_idx = Some(idx);
            }
        }

        // Actor behaviors compile through the exact same machinery as
        // ordinary functions, but land in CodeModule.behaviors instead of
        // function_table — Spawn/Send/Ask reference them by index there,
        // and (unlike functions) they are never reachable via Call.
        // mir_lower.rs computed ActorMeta.behavior_indices assuming
        // behaviors compile in this order, so this loop must not be
        // reordered or interleaved with function compilation.
        for func in &mir.behaviors {
            let offset = self.compile_function(func)?;
            self.module.behaviors.push(crate::bytecode::BehaviorTableEntry {
                name: func.name.clone(),
                param_count: func.params.len(),
                code_offset: offset,
                local_count: LOCAL_BASE as usize + func.locals.len(),
                effect_mask: 0,
                compensate_offset: None,
                parallel_branches: None,
            });
        }
        self.module.actor_metadata = mir.actor_metadata.clone();

        // Entry prologue: call __main (if present) and halt.
        if let Some(main_idx) = main_idx {
            let entry = self.module.instructions.len();
            self.load_constant(SCRATCH0, &Constant::Int(main_idx as i64));
            self.emit(Instruction::new3(OpCode::Call, SCRATCH0, 0, 0));
            self.emit(Instruction::new0(OpCode::Halt));
            self.module.entry_point = Some(entry);
        } else {
            let entry = self.module.instructions.len();
            self.emit(Instruction::new0(OpCode::Halt));
            self.module.entry_point = Some(entry);
        }

        Ok(&self.module)
    }

    fn compile_function(&mut self, func: &mir::Function) -> NuResult<usize> {
        // Isolate this function's bytecode so block offsets are relative to
        // the function start while still allowing forward jump resolution.
        let mut saved_instructions = Vec::new();
        std::mem::swap(&mut saved_instructions, &mut self.module.instructions);
        let function_start = saved_instructions.len();

        if LOCAL_BASE as usize + func.locals.len() > FUNC_VALUE_REG as usize - 1 {
            // Restore before erroring so the codegen stays usable.
            self.module.instructions = saved_instructions;
            return Err(compile_err(format!(
                "function '{}' needs {} locals, exceeding the MIR register allocator's capacity",
                func.name,
                func.locals.len()
            )));
        }

        // Prologue: move incoming arguments from r0..rN to their fixed local
        // registers, then load closure captures.
        for (i, param) in func.params.iter().enumerate() {
            let dst = reg_of(*param);
            let src = i as u8;
            if src != dst {
                self.emit(Instruction::new2(OpCode::Move, src, dst));
            }
        }
        for (i, cap) in func.captures.iter().enumerate() {
            self.emit(Instruction::new3(OpCode::CapLoad, i as u8, reg_of(*cap), 0));
        }

        let mut block_offsets: HashMap<mir::BlockId, usize> = HashMap::new();
        let mut patches: Vec<JumpPatch> = Vec::new();
        // Handler-param moves to inject at the start of handler body blocks.
        let mut handler_prologues: HashMap<mir::BlockId, Vec<mir::LocalId>> = HashMap::new();
        for table in &func.handler_tables {
            for binding in &table.bindings {
                handler_prologues.insert(binding.body, binding.params.clone());
            }
        }
        // `Handle` instructions awaiting their table index (fn-relative idx).
        let mut handle_patches: Vec<(usize, usize)> = Vec::new();

        for block in &func.blocks {
            block_offsets.insert(block.id, self.module.instructions.len());
            if let Some(params) = handler_prologues.get(&block.id) {
                // The VM delivers effect arguments in r0..rN.
                for (i, p) in params.clone().iter().enumerate() {
                    let dst = reg_of(*p);
                    if i as u8 != dst {
                        self.emit(Instruction::new2(OpCode::Move, i as u8, dst));
                    }
                }
            }
            for stmt in &block.stmts {
                self.compile_stmt(stmt, func, &mut handle_patches)?;
            }
            self.compile_terminator(&block.terminator, &func.name, &block_offsets, &mut patches)?;
        }

        // Patch forward jumps now that all block offsets are known.
        for patch in &patches {
            let target_offset = block_offsets
                .get(&patch.target_block)
                .copied()
                .ok_or_else(|| compile_err("internal: jump to unknown MIR block"))?;
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

        // Build handler tables: offsets become module-absolute.
        for (instr_idx, table_idx) in handle_patches {
            let def = &func.handler_tables[table_idx];
            let mut bindings = Vec::with_capacity(def.bindings.len());
            for b in &def.bindings {
                let rel = block_offsets
                    .get(&b.body)
                    .copied()
                    .ok_or_else(|| compile_err("internal: handler body block missing"))?;
                let result_reg = func
                    .blocks
                    .get(b.body.0 as usize)
                    .and_then(|blk| match blk.terminator {
                        mir::Terminator::Resume(id) => Some(reg_of(id)),
                        _ => None,
                    })
                    .unwrap_or(0);
                bindings.push(HandlerBinding {
                    effect_name: b.effect_name.clone(),
                    handler_offset: function_start + rel,
                    arg_count: b.params.len() as u8,
                    result_reg,
                });
            }
            let global_idx = self.module.add_handler_table(HandlerTable {
                bindings,
                fallback_offset: None,
            });
            if global_idx > u8::MAX as usize {
                return Err(compile_err("too many effect handler tables in module"));
            }
            self.module.instructions[instr_idx].op1 = global_idx as u8;
        }

        let mut function_code = Vec::new();
        std::mem::swap(&mut function_code, &mut self.module.instructions);
        self.module.instructions = saved_instructions;
        self.module.instructions.extend(function_code);

        Ok(function_start)
    }

    fn compile_stmt(
        &mut self,
        stmt: &mir::Stmt,
        func: &mir::Function,
        handle_patches: &mut Vec<(usize, usize)>,
    ) -> NuResult<()> {
        match stmt {
            mir::Stmt::Assign { dst, op } => {
                self.compile_rvalue(reg_of(*dst), op)?;
            }
            mir::Stmt::StoreFieldNamed { obj, field, src } => {
                let fid = self.field_id(field);
                self.emit(Instruction::new3(OpCode::RecS, reg_of(*obj), fid, reg_of(*src)));
            }
            mir::Stmt::StoreTupleSlot { obj, slot, src } => {
                self.emit(Instruction::new3(OpCode::FieldS, reg_of(*obj), *slot, reg_of(*src)));
            }
            mir::Stmt::ArrayStore { arr, idx, src } => {
                self.emit(Instruction::new3(
                    OpCode::ArrStore,
                    reg_of(*arr),
                    reg_of(*idx),
                    reg_of(*src),
                ));
            }
            mir::Stmt::EnterHandle { table } => {
                if *table >= func.handler_tables.len() {
                    return Err(compile_err("internal: EnterHandle references unknown table"));
                }
                let instr_idx = self.module.instructions.len();
                self.emit(Instruction::new1(OpCode::Handle, 0));
                handle_patches.push((instr_idx, *table));
            }
            mir::Stmt::PopHandler => {
                self.emit(Instruction::new0(OpCode::Unwind));
            }
            mir::Stmt::StateSet { field, src } => {
                let field_idx = self.module.add_constant(Constant::String(field.clone()));
                self.emit(Instruction::new3(
                    OpCode::StateSet,
                    ((field_idx >> 8) & 0xFF) as u8,
                    (field_idx & 0xFF) as u8,
                    reg_of(*src),
                ));
            }
            mir::Stmt::Emit { event, args } => {
                self.stage_args(args)?;
                let event_idx = self.module.add_constant(Constant::String(event.clone()));
                self.emit(Instruction::new3(
                    OpCode::Emit,
                    ((event_idx >> 8) & 0xFF) as u8,
                    (event_idx & 0xFF) as u8,
                    args.len() as u8,
                ));
            }
        }
        Ok(())
    }

    /// Move argument locals into the staging registers r0..rN.
    fn stage_args(&mut self, args: &[mir::LocalId]) -> NuResult<()> {
        if args.len() > MAX_STAGED_ARGS {
            return Err(compile_err(format!(
                "call/effect with {} arguments exceeds the MIR staging limit of {}",
                args.len(),
                MAX_STAGED_ARGS
            )));
        }
        for (i, a) in args.iter().enumerate() {
            let src = reg_of(*a);
            if src != i as u8 {
                self.emit(Instruction::new2(OpCode::Move, src, i as u8));
            }
        }
        Ok(())
    }

    fn compile_rvalue(&mut self, dst: u8, rv: &mir::RValue) -> NuResult<()> {
        match rv {
            mir::RValue::Const(c) => {
                self.load_constant(dst, c);
            }
            mir::RValue::Load(id) => {
                let src = reg_of(*id);
                if src != dst {
                    self.emit(Instruction::new2(OpCode::Move, src, dst));
                }
            }
            mir::RValue::LoadFieldNamed { obj, field } => {
                let fid = self.field_id(field);
                self.emit(Instruction::new3(OpCode::RecL, reg_of(*obj), fid, dst));
            }
            mir::RValue::ArrayLoad { arr, idx } => {
                self.emit(Instruction::new3(
                    OpCode::ArrLoad,
                    reg_of(*arr),
                    reg_of(*idx),
                    dst,
                ));
            }
            mir::RValue::ArrayLen(arr) => {
                self.emit(Instruction::new2(OpCode::ArrLen, reg_of(*arr), dst));
            }
            mir::RValue::ArrayLit(elems) => {
                self.load_constant(SCRATCH0, &Constant::Int(elems.len() as i64));
                self.emit(Instruction::new2(OpCode::ArrAlloc, SCRATCH0, dst));
                for (i, e) in elems.iter().enumerate() {
                    self.load_constant(SCRATCH1, &Constant::Int(i as i64));
                    self.emit(Instruction::new3(OpCode::ArrStore, dst, SCRATCH1, reg_of(*e)));
                }
            }
            mir::RValue::Unary(op, id) => {
                let src = reg_of(*id);
                let opcode = match op {
                    crate::ast::UnOp::Neg => OpCode::INeg,
                    crate::ast::UnOp::Not => OpCode::Not,
                    other => {
                        return Err(not_yet_implemented(&format!(
                            "unary operator {:?} in HIR/MIR pipeline",
                            other
                        )))
                    }
                };
                self.emit(Instruction::new2(opcode, src, dst));
            }
            mir::RValue::Binary(op, l, r) => {
                let lr = reg_of(*l);
                let rr = reg_of(*r);
                if *op == crate::ast::BinOp::Ne {
                    self.emit(Instruction::new3(OpCode::ICmpEq, lr, rr, SCRATCH0));
                    self.emit(Instruction::new2(OpCode::Not, SCRATCH0, dst));
                } else {
                    let opcode = binary_opcode(op)?;
                    self.emit(Instruction::new3(opcode, lr, rr, dst));
                }
            }
            mir::RValue::StringEq(l, r) => {
                self.emit(Instruction::new3(OpCode::SCmpEq, reg_of(*l), reg_of(*r), dst));
            }
            mir::RValue::Call { func, args } => {
                // Load the callee value first (it lives above the staging
                // zone, so staging cannot clobber it).
                match func {
                    mir::FuncRef::Index(idx) => {
                        self.load_constant(FUNC_VALUE_REG, &Constant::Int(*idx as i64));
                    }
                    mir::FuncRef::Local(id) => {
                        self.emit(Instruction::new2(OpCode::Move, reg_of(*id), FUNC_VALUE_REG));
                    }
                }
                self.stage_args(args)?;
                self.emit(Instruction::new3(
                    OpCode::Call,
                    FUNC_VALUE_REG,
                    args.len() as u8,
                    dst,
                ));
            }
            mir::RValue::Closure { func, captures } => {
                self.emit(Instruction::new3(
                    OpCode::Closure,
                    ((*func >> 8) & 0xFF) as u8,
                    (*func & 0xFF) as u8,
                    dst,
                ));
                for (i, cap) in captures.iter().enumerate() {
                    self.emit(Instruction::new3(OpCode::CapStore, dst, i as u8, reg_of(*cap)));
                }
            }
            mir::RValue::Tuple(elems) => {
                self.emit(Instruction::new2(OpCode::TupleMk, elems.len() as u8, dst));
                for (i, e) in elems.iter().enumerate() {
                    self.emit(Instruction::new3(OpCode::FieldS, dst, i as u8, reg_of(*e)));
                }
            }
            mir::RValue::Record(fields) => {
                // Stable-compiler layout: records are flat arrays indexed by
                // module-wide field ids; slot count covers the largest id.
                let mut max_field_id: u8 = 0;
                for (name, _) in fields {
                    let fid = self.field_id(name);
                    max_field_id = max_field_id.max(fid);
                }
                let slot_count = max_field_id.saturating_add(1);
                self.emit(Instruction::new2(OpCode::RecMk, slot_count, dst));
                for (name, e) in fields {
                    let fid = self.field_id(name);
                    self.emit(Instruction::new3(OpCode::RecS, dst, fid, reg_of(*e)));
                }
            }
            mir::RValue::Perform { effect, op, args } => {
                self.stage_args(args)?;
                let eff_idx = self.module.add_constant(Constant::String(effect.clone()));
                // The op name is recorded as a constant for symmetry with the
                // stable compiler; the VM currently dispatches on effect name.
                let _op_idx = self.module.add_constant(Constant::String(op.clone()));
                self.emit(Instruction::new3(
                    OpCode::Perform,
                    ((eff_idx >> 8) & 0xFF) as u8,
                    (eff_idx & 0xFF) as u8,
                    dst,
                ));
            }
            mir::RValue::LlmAsk { prompt } => {
                let src = reg_of(*prompt);
                if src != dst {
                    self.emit(Instruction::new2(OpCode::Move, src, dst));
                }
                let model_idx = self.module.add_constant(Constant::String(String::new()));
                // LlmAsk reads the prompt from and writes the reply to op3.
                self.emit(Instruction::new3(
                    OpCode::LlmAsk,
                    ((model_idx >> 8) & 0xFF) as u8,
                    (model_idx & 0xFF) as u8,
                    dst,
                ));
            }
            mir::RValue::SignalWait { name } => {
                let name_idx = self.module.add_constant(Constant::String(name.clone()));
                self.emit(Instruction::new3(
                    OpCode::SignalWait,
                    ((name_idx >> 8) & 0xFF) as u8,
                    (name_idx & 0xFF) as u8,
                    dst,
                ));
            }
            mir::RValue::FFICall { idx, args } => {
                self.stage_args(args)?;
                self.emit(Instruction::new3(
                    OpCode::FFICall,
                    ((*idx >> 8) & 0xFF) as u8,
                    (*idx & 0xFF) as u8,
                    dst,
                ));
            }
            mir::RValue::Migrate { actor, node } => {
                self.emit(Instruction::new3(
                    OpCode::Migrate,
                    reg_of(*actor),
                    reg_of(*node),
                    dst,
                ));
            }
            mir::RValue::SelfRef => {
                self.emit(Instruction::new1(OpCode::SelfOp, dst));
            }
            mir::RValue::CapabilityCheck { val } => {
                let _ = val;
                self.emit(Instruction::new1(OpCode::Const1, dst)); // true
            }
            mir::RValue::StateGet { field } => {
                let field_idx = self.module.add_constant(Constant::String(field.clone()));
                self.emit(Instruction::new3(
                    OpCode::StateGet,
                    ((field_idx >> 8) & 0xFF) as u8,
                    (field_idx & 0xFF) as u8,
                    dst,
                ));
            }
            mir::RValue::Spawn { behavior_idx } => {
                self.emit(Instruction::new3(
                    OpCode::Spawn,
                    ((*behavior_idx >> 8) & 0xFF) as u8,
                    (*behavior_idx & 0xFF) as u8,
                    dst,
                ));
            }
            mir::RValue::Send { actor, behavior_idx, args } => {
                // Protect the actor value in a register outside the 0..15
                // staging zone before staging args, mirroring the Call/
                // FUNC_VALUE_REG pattern.
                self.emit(Instruction::new2(OpCode::Move, reg_of(*actor), FUNC_VALUE_REG));
                self.stage_args(args)?;
                self.emit(Instruction::new3(
                    OpCode::Send,
                    FUNC_VALUE_REG,
                    ((*behavior_idx >> 8) & 0xFF) as u8,
                    (*behavior_idx & 0xFF) as u8,
                ));
                // Send is fire-and-forget with no VM-level result register;
                // the stable compiler yields 0 for send-as-expression.
                self.load_constant(dst, &Constant::Int(0));
            }
            mir::RValue::Ask { actor, behavior_idx, args } => {
                self.emit(Instruction::new2(OpCode::Move, reg_of(*actor), FUNC_VALUE_REG));
                self.stage_args(args)?;
                self.emit(Instruction::new3(
                    OpCode::Ask,
                    FUNC_VALUE_REG,
                    ((*behavior_idx >> 8) & 0xFF) as u8,
                    (*behavior_idx & 0xFF) as u8,
                ));
                // Ask writes its result back into its own op1 register.
                self.emit(Instruction::new2(OpCode::Move, FUNC_VALUE_REG, dst));
            }
        }
        Ok(())
    }

    fn compile_terminator(
        &mut self,
        term: &mir::Terminator,
        func_name: &str,
        block_offsets: &HashMap<mir::BlockId, usize>,
        patches: &mut Vec<JumpPatch>,
    ) -> NuResult<()> {
        match term {
            mir::Terminator::Return(val) => match val {
                Some(id) => {
                    self.emit(Instruction::new1(OpCode::RetVal, reg_of(*id)));
                }
                None => {
                    self.emit(Instruction::new1(OpCode::Const0, SCRATCH0));
                    self.emit(Instruction::new1(OpCode::RetVal, SCRATCH0));
                }
            },
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
                let cond_reg = reg_of(*cond);

                // JmpF to else_ when the condition is false.
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
            mir::Terminator::Resume(id) => {
                self.emit(Instruction::new1(OpCode::Resume, reg_of(*id)));
            }
            mir::Terminator::Unterminated => {
                return Err(compile_err(format!(
                    "internal: unterminated MIR block in function '{}'",
                    func_name
                )));
            }
        }
        Ok(())
    }

    fn field_id(&mut self, name: &str) -> u8 {
        if let Some(&id) = self.field_map.get(name) {
            return id;
        }
        let id = self.next_field_id;
        self.next_field_id = self.next_field_id.saturating_add(1);
        self.field_map.insert(name.to_string(), id);
        id
    }

    fn load_constant(&mut self, dst: u8, c: &Constant) {
        match c {
            Constant::Int(0) => self.emit(Instruction::new1(OpCode::Const0, dst)),
            Constant::Int(1) => self.emit(Instruction::new1(OpCode::Const1, dst)),
            Constant::Int(2) => self.emit(Instruction::new1(OpCode::Const2, dst)),
            Constant::Int(-1) => self.emit(Instruction::new1(OpCode::ConstM1, dst)),
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

    fn emit(&mut self, instr: Instruction) {
        self.module.instructions.push(instr);
    }

    pub fn finish(self) -> CodeModule {
        self.module
    }
}

fn reg_of(id: mir::LocalId) -> u8 {
    (LOCAL_BASE + id.0) as u8
}

fn binary_opcode(op: &crate::ast::BinOp) -> NuResult<OpCode> {
    match op {
        crate::ast::BinOp::Add => Ok(OpCode::IAdd),
        crate::ast::BinOp::Sub => Ok(OpCode::ISub),
        crate::ast::BinOp::Mul => Ok(OpCode::IMul),
        crate::ast::BinOp::Div => Ok(OpCode::IDiv),
        crate::ast::BinOp::Mod => Ok(OpCode::IMod),
        crate::ast::BinOp::Eq => Ok(OpCode::ICmpEq),
        crate::ast::BinOp::Lt => Ok(OpCode::ICmpLt),
        crate::ast::BinOp::Gt => Ok(OpCode::ICmpGt),
        crate::ast::BinOp::Le => Ok(OpCode::ICmpLe),
        crate::ast::BinOp::Ge => Ok(OpCode::ICmpGe),
        crate::ast::BinOp::And => Ok(OpCode::And),
        crate::ast::BinOp::Or => Ok(OpCode::Or),
        crate::ast::BinOp::BitAnd => Ok(OpCode::BitAnd),
        crate::ast::BinOp::BitOr => Ok(OpCode::BitOr),
        crate::ast::BinOp::BitXor => Ok(OpCode::Xor),
        crate::ast::BinOp::Shl => Ok(OpCode::Shl),
        crate::ast::BinOp::Shr => Ok(OpCode::Shr),
        other => Err(not_yet_implemented(&format!("binary operator {:?}", other))),
    }
}

pub fn compile_mir(mir: &mir::Module, module_name: impl Into<String>) -> NuResult<CodeModule> {
    let mut codegen = MirCodegen::new(module_name);
    codegen.compile_module(mir)?;
    Ok(codegen.finish())
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::typechecker::TypeChecker;
    use crate::vm::VM;

    fn compile_mir_source(source: &str) -> NuResult<CodeModule> {
        let mut lexer = Lexer::new(source);
        let tokens = lexer.lex()?;
        let mut parser = Parser::new(tokens);
        let ast = parser.parse_module()?;

        let mut type_checker = TypeChecker::new();
        type_checker.check_module(&ast)?;

        let hir = crate::hir_lower::lower_module(&ast);
        let mir = crate::mir_lower::lower_module(&hir)?;
        compile_mir(&mir, "test")
    }

    fn run_mir_source(source: &str) -> NuResult<crate::vm::Value> {
        let module = compile_mir_source(source)?;
        let mut vm = VM::new();
        vm.load_module(module);
        vm.run()
    }

    #[test]
    fn test_mir_codegen_simple_arithmetic() {
        let value = run_mir_source("1 + 2 * 3").unwrap();
        assert_eq!(value.as_int(), Some(7));
    }

    #[test]
    fn test_mir_codegen_bitwise_or() {
        let value = run_mir_source("6 ||| 3").unwrap();
        assert_eq!(value.as_int(), Some(7));
    }

    #[test]
    fn test_mir_codegen_if_expression_position() {
        // Statements after an expression-position if must run after it.
        let value = run_mir_source("let x = if true then 1 else 2 in x + 10").unwrap();
        assert_eq!(value.as_int(), Some(11));
    }

    #[test]
    fn test_mir_codegen_recursive_closure() {
        let value = run_mir_source(
            "let fib = fn(n) { if n <= 1 then n else fib(n - 1) + fib(n - 2) } in fib(10)",
        )
        .unwrap();
        assert_eq!(value.as_int(), Some(55));
    }

    #[test]
    fn test_mir_codegen_closure_capture() {
        let value = run_mir_source("let a = 40 in let add = fn(x) { x + a } in add(2)").unwrap();
        assert_eq!(value.as_int(), Some(42));
    }

    /// Only a bare `ident = v` parses as the dedicated Expr::Assign AST node;
    /// `arr[i] = v` and `record.f = v` are ordinary-looking BinOp::Assign
    /// binary expressions instead. Both must route through place-based
    /// lowering (regression: the stable compiler does NOT do this for
    /// non-self targets — see test_legacy_index_assign_is_a_noop_bug below).
    #[test]
    fn test_mir_codegen_index_and_field_assign() {
        let value = run_mir_source("let arr = [1, 2, 3] in { arr[0] = 99 arr[0] }").unwrap();
        assert_eq!(value.as_int(), Some(99), "arr[0] = 99 should actually mutate the array");

        let value =
            run_mir_source("let r = { x: 1, y: 2 } in { r.x = 99 r.x + r.y }").unwrap();
        assert_eq!(value.as_int(), Some(101), "r.x = 99 should actually mutate the record");
    }

    #[test]
    fn test_mir_codegen_effect_handler() {
        let value = run_mir_source(
            "handle perform Math.getAnswer() { | Math.getAnswer() => 42 }",
        )
        .unwrap();
        assert_eq!(value.as_int(), Some(42));
    }

    #[test]
    fn test_mir_codegen_actor_spawn_returns_actor_ref() {
        // Actors are now lowered by the HIR/MIR pipeline. Without a real
        // Runtime attached, spawn_actor's default stub always returns
        // actor_ref(0); real behavior semantics (state, ask) are covered by
        // src/integration_tests.rs's MIR-vs-legacy actor tests, which attach
        // a Runtime.
        let value = run_mir_source(
            "actor A { state x = 0 behavior get() { self.x } }\nspawn A { x = 0 }",
        )
        .unwrap();
        assert!(value.as_actor_id().is_some(), "spawn should yield an actor reference");
    }

    #[test]
    fn test_mir_codegen_workflow_agent_still_honest_nyi() {
        // Workflow/agent desugaring is a separate, larger effort (they
        // synthesize additional actor behaviors at compile time) and stays
        // out of the MIR pipeline's scope for now.
        let result = compile_mir_source("workflow W { step a { 1 } }");
        assert!(
            matches!(result, Err(NuError::NotYetImplemented { .. })),
            "workflow must be an honest NotYetImplemented, got {:?}",
            result
        );
        let result = compile_mir_source(
            r#"agent Ag = { model: "gpt-4o" }"#,
        );
        assert!(
            matches!(result, Err(NuError::NotYetImplemented { .. })),
            "agent must be an honest NotYetImplemented, got {:?}",
            result
        );
    }

    #[test]
    fn test_mir_codegen_unknown_call_is_error_not_zero() {
        // Regression: unknown callees used to silently compile to Const0.
        let hir = crate::hir::Module {
            name: "t".into(),
            decls: vec![crate::hir::Decl::Function(crate::hir::FunctionDef {
                name: "__main".into(),
                type_params: vec![],
                params: vec![],
                ret: crate::types::Type::unit(),
                effect: crate::types::EffectRow::empty(),
                cap: crate::types::Capability::Ref,
                body: {
                    let mut b = crate::hir::Body::new();
                    b.push(crate::hir::Stmt::Let {
                        name: "r".into(),
                        ty: crate::types::Type::unit(),
                        value: crate::hir::RValue::Call {
                            func: crate::hir::Operand::Var("nope".into(), crate::types::Type::unit()),
                            args: vec![],
                            ty: crate::types::Type::unit(),
                        },
                        span: Span::default(),
                    });
                    b
                },
                public: false,
                span: Span::default(),
            })],
        };
        let result = crate::mir_lower::lower_module(&hir);
        assert!(result.is_err(), "unknown callee must be a compile error");
    }
}
