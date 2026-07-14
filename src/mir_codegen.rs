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
//!
//! Intra-actor reclamation: `compile_function` runs a conservative
//! liveness-based analysis (`plan_drops`) that emits `OpCode::Drop` when a
//! local provably holding the sole counted reference to a heap object dies —
//! overwritten by a new definition, dead after its last use, or dead at the
//! entry of a block its value flows into unused. The VM clears the register
//! on `Drop`, so duplicate drops are harmless no-ops.

use crate::bytecode::{
    CodeModule, Constant, ForeignFunctionDef, HandlerBinding, HandlerTable, Instruction, OpCode,
};
use crate::mir;
use crate::types::{NuError, NuResult, PrimitiveType, Span, Type};
use std::collections::{HashMap, HashSet};

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
    /// Constant-pool index of each `self.field` name already emitted for
    /// `StateGet`/`StateSet`, so repeated access to the same field reuses
    /// one constant instead of growing the pool with a fresh duplicate
    /// string every time (unlike record fields, `state` is string-keyed at
    /// runtime, not a positional slot `field_id` could cover).
    state_field_constants: HashMap<String, usize>,
    /// Per-function float-ness of MIR locals (see `float_locals`), used to
    /// pick float opcode variants for arithmetic and comparisons. Rebuilt
    /// at the start of every `compile_function`.
    float_locals: Vec<bool>,
}

impl MirCodegen {
    pub fn new(module_name: impl Into<String>) -> Self {
        MirCodegen {
            module: CodeModule::new(module_name),
            field_map: HashMap::new(),
            next_field_id: 0,
            state_field_constants: HashMap::new(),
            float_locals: Vec::new(),
        }
    }

    /// Whether the given local of the function currently being compiled is
    /// known to hold a Float at runtime.
    fn is_float_local(&self, id: mir::LocalId) -> bool {
        self.float_locals.get(id.0 as usize).copied().unwrap_or(false)
    }

    /// Constant-pool index for a `self.field` name, reusing an existing
    /// entry if this field was already referenced elsewhere in the module.
    fn state_field_constant(&mut self, field: &str) -> usize {
        if let Some(&idx) = self.state_field_constants.get(field) {
            return idx;
        }
        let idx = self
            .module
            .add_constant(Constant::String(field.to_string()));
        self.state_field_constants.insert(field.to_string(), idx);
        idx
    }

    pub fn compile_module(&mut self, mir: &mir::Module) -> NuResult<&CodeModule> {
        // Register foreign functions first so FFICall indices line up.
        for ff in &mir.foreign_functions {
            let params = ff
                .params
                .iter()
                .map(crate::ffi::marshal::nulang_type_to_ffi_type)
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    compile_err(format!(
                        "unsupported parameter type in extern function {}",
                        ff.symbol
                    ))
                })?;
            let ret = crate::ffi::marshal::nulang_type_to_ffi_type(&ff.ret).ok_or_else(|| {
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
        let mut user_main_idx = None;
        for (idx, func) in mir.functions.iter().enumerate() {
            let offset = self.compile_function(func)?;
            self.module.function_table[idx] = offset;
            if func.name == "__main" {
                main_idx = Some(idx);
            }
            if func.name == "main" {
                user_main_idx = Some(idx);
            }
        }
        // If no synthetic __main wrapper exists but user declared fn main(),
        // treat main as the entry point (matching the legacy compiler).
        let effective_main = main_idx.or(user_main_idx);

        // Actor behaviors compile through the exact same machinery as
        // ordinary functions, but land in CodeModule.behaviors instead of
        // function_table — Spawn/Send/Ask reference them by index there,
        // and (unlike functions) they are never reachable via Call.
        // mir_lower.rs computed ActorMeta.behavior_indices assuming
        // behaviors compile in this order, so this loop must not be
        // reordered or interleaved with function compilation.
        for func in &mir.behaviors {
            let offset = self.compile_function(func)?;
            self.module
                .behaviors
                .push(crate::bytecode::BehaviorTableEntry {
                    name: func.name.clone(),
                    param_count: func.params.len(),
                    code_offset: offset,
                    local_count: LOCAL_BASE as usize + func.locals.len(),
                    effect_mask: 0,
                    compensate_offset: None,
                    parallel_branches: None,
                });
        }
        // Saga compensation: patch each step's compensate_offset from its
        // already-compiled compensation function's code offset. Both
        // indices are into module.behaviors (see mir::Module::compensation_of).
        for (behavior_idx, comp_idx) in &mir.compensation_of {
            let comp_offset = self
                .module
                .behaviors
                .get(*comp_idx)
                .map(|b| b.code_offset)
                .ok_or_else(|| compile_err("internal: compensation behavior index out of range"))?;
            let entry = self
                .module
                .behaviors
                .get_mut(*behavior_idx)
                .ok_or_else(|| compile_err("internal: compensated behavior index out of range"))?;
            entry.compensate_offset = Some(comp_offset);
        }
        // Parallel-branch metadata: copy branch names onto the matching
        // synthesized step's BehaviorTableEntry (see mir::Module::parallel_branches_of).
        for (behavior_idx, branches) in &mir.parallel_branches_of {
            let entry = self
                .module
                .behaviors
                .get_mut(*behavior_idx)
                .ok_or_else(|| {
                    compile_err("internal: parallel-branch behavior index out of range")
                })?;
            entry.parallel_branches = Some(branches.clone());
        }
        self.module.actor_metadata = mir.actor_metadata.clone();

        // Collect tools from agent actors into module.tools so the runtime
        // can resolve @tool-annotated functions for agent LLM requests.
        for meta in &self.module.actor_metadata {
            if meta.is_agent {
                for tool in &meta.tools {
                    if !self.module.tools.iter().any(|t| t.name == tool.name) {
                        self.module.tools.push(tool.clone());
                    }
                }
            }
        }

        // Entry prologue: call the effective main function and halt.
        if let Some(idx) = effective_main {
            let entry = self.module.instructions.len();
            self.load_constant(SCRATCH0, &Constant::Int(idx as i64));
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

        if func.params.len() > MAX_STAGED_ARGS {
            // Mirrors stage_args's call-site limit: the prologue below reads
            // incoming arguments from r0..r15 (the same staging zone callers
            // stage into), so a param count above that would alias into
            // LOCAL_BASE-mapped registers instead of erroring cleanly.
            self.module.instructions = saved_instructions;
            return Err(compile_err(format!(
                "function '{}' has {} parameters, exceeding the MIR calling convention's limit of {}",
                func.name,
                func.params.len(),
                MAX_STAGED_ARGS
            )));
        }

        // Type-directed opcode selection: the VM's integer handlers coerce
        // float operands to 0, so float arithmetic/comparisons must be
        // emitted as their F* variants.
        self.float_locals = float_locals(func);

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
                if binding.params.len() > MAX_STAGED_ARGS {
                    // The VM delivers effect arguments in r0..r15; beyond
                    // that the prologue moves below would alias into
                    // LOCAL_BASE-mapped locals — the same corruption the
                    // function-parameter check above rejects.
                    self.module.instructions = saved_instructions;
                    return Err(compile_err(format!(
                        "handler for effect '{}' in function '{}' has {} parameters, exceeding the MIR staging limit of {}",
                        binding.effect_name,
                        func.name,
                        binding.params.len(),
                        MAX_STAGED_ARGS
                    )));
                }
                handler_prologues.insert(binding.body, binding.params.clone());
            }
        }
        // `Handle` instructions awaiting their table index (fn-relative idx).
        let mut handle_patches: Vec<(usize, usize)> = Vec::new();

        // Conservative liveness-based placement of `Drop` instructions (see
        // the module docs and `plan_drops`).
        let drop_plan = plan_drops(func);

        for (bi, block) in func.blocks.iter().enumerate() {
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
            if let Some(ids) = drop_plan.block_entry.get(&bi) {
                for id in ids {
                    self.emit(Instruction::new1(OpCode::Drop, reg_of(*id)));
                }
            }
            for (si, stmt) in block.stmts.iter().enumerate() {
                if let Some(ids) = drop_plan.before_stmt.get(&(bi, si)) {
                    for id in ids {
                        self.emit(Instruction::new1(OpCode::Drop, reg_of(*id)));
                    }
                }
                self.compile_stmt(stmt, func, &mut handle_patches)?;
                if let Some(ids) = drop_plan.after_stmt.get(&(bi, si)) {
                    for id in ids {
                        self.emit(Instruction::new1(OpCode::Drop, reg_of(*id)));
                    }
                }
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
                let fid = self.field_id(field)?;
                self.emit(Instruction::new3(
                    OpCode::RecS,
                    reg_of(*obj),
                    fid,
                    reg_of(*src),
                ));
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
                    return Err(compile_err(
                        "internal: EnterHandle references unknown table",
                    ));
                }
                let instr_idx = self.module.instructions.len();
                self.emit(Instruction::new1(OpCode::Handle, 0));
                handle_patches.push((instr_idx, *table));
            }
            mir::Stmt::PopHandler => {
                self.emit(Instruction::new0(OpCode::Unwind));
            }
            mir::Stmt::StateSet { field, src } => {
                let field_idx = self.state_field_constant(field);
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
                let fid = self.field_id(field)?;
                self.emit(Instruction::new3(OpCode::RecL, reg_of(*obj), fid, dst));
            }
            mir::RValue::LoadFieldPos { obj, index } => {
                self.emit(Instruction::new3(OpCode::FieldL, reg_of(*obj), *index, dst));
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
                    self.emit(Instruction::new3(
                        OpCode::ArrStore,
                        dst,
                        SCRATCH1,
                        reg_of(*e),
                    ));
                }
            }
            mir::RValue::Unary(op, id) => {
                let src = reg_of(*id);
                // `Deref`/`Ref` are register copies, same as the stable
                // compiler's compile_unary: Nulang's ref cells are locals
                // reassigned in place (see lower_place's Var arm), not a
                // distinct heap allocation, so `&`/`*` are no-ops at the
                // bytecode level — the type checker is what restricts
                // reassignment to Ref-typed locals.
                let opcode = match op {
                    crate::ast::UnOp::Neg => {
                        if self.is_float_local(*id) {
                            OpCode::FNeg
                        } else {
                            OpCode::INeg
                        }
                    }
                    crate::ast::UnOp::Not => OpCode::Not,
                    crate::ast::UnOp::Deref => OpCode::Load,
                    crate::ast::UnOp::Ref(_) => OpCode::Move,
                };
                if opcode == OpCode::FNeg {
                    // The interpreter reads the source from op1 and writes
                    // the destination to op3 for FNeg (unlike INeg's op2).
                    self.emit(Instruction::new3(OpCode::FNeg, src, 0, dst));
                } else {
                    self.emit(Instruction::new2(opcode, src, dst));
                }
            }
            mir::RValue::Binary(op, l, r) => {
                let lr = reg_of(*l);
                let rr = reg_of(*r);
                // The type checker rejects mixed int/float arithmetic, so
                // operands are homogeneous: one float operand means both
                // are floats and the F* opcode variants are required (the
                // integer handlers coerce float operands to 0).
                let is_float = self.is_float_local(*l) || self.is_float_local(*r);
                use crate::ast::BinOp;
                match (op, is_float) {
                    (BinOp::Ne, f) => {
                        let eq = if f { OpCode::FCmpEq } else { OpCode::ICmpEq };
                        self.emit(Instruction::new3(eq, lr, rr, SCRATCH0));
                        self.emit(Instruction::new2(OpCode::Not, SCRATCH0, dst));
                    }
                    // Float Le/Ge have no dedicated opcodes: expand to the
                    // negated inverse comparison (a <= b == !(a > b)).
                    (BinOp::Le, true) => {
                        self.emit(Instruction::new3(OpCode::FCmpGt, lr, rr, SCRATCH0));
                        self.emit(Instruction::new2(OpCode::Not, SCRATCH0, dst));
                    }
                    (BinOp::Ge, true) => {
                        self.emit(Instruction::new3(OpCode::FCmpLt, lr, rr, SCRATCH0));
                        self.emit(Instruction::new2(OpCode::Not, SCRATCH0, dst));
                    }
                    _ => {
                        let opcode = binary_opcode(op, is_float)?;
                        self.emit(Instruction::new3(opcode, lr, rr, dst));
                    }
                }
            }
            mir::RValue::StringEq(l, r) => {
                self.emit(Instruction::new3(
                    OpCode::SCmpEq,
                    reg_of(*l),
                    reg_of(*r),
                    dst,
                ));
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
                    self.emit(Instruction::new3(
                        OpCode::CapStore,
                        dst,
                        i as u8,
                        reg_of(*cap),
                    ));
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
                let mut field_ids = Vec::with_capacity(fields.len());
                for (name, _) in fields {
                    let fid = self.field_id(name)?;
                    max_field_id = max_field_id.max(fid);
                    field_ids.push(fid);
                }
                let slot_count = max_field_id.saturating_add(1);
                self.emit(Instruction::new2(OpCode::RecMk, slot_count, dst));
                for ((_, e), fid) in fields.iter().zip(field_ids) {
                    self.emit(Instruction::new3(OpCode::RecS, dst, fid, reg_of(*e)));
                }
            }
            mir::RValue::Perform { effect, op, args } => {
                self.stage_args(args)?;
                // The dispatch name carries the operation as "Effect.op"
                // (e.g. "IO.print") so the VM matches handlers on the
                // (effect, op) pair instead of the effect name alone.
                let eff_idx = self
                    .module
                    .add_constant(Constant::String(format!("{}.{}", effect, op)));
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
            mir::RValue::Receive => {
                // Pops the next mailbox message via ActorVmCallbacks::try_receive;
                // writes its first payload value (or nil) to dst.
                self.emit(Instruction::new1(OpCode::Receive, dst));
            }
            mir::RValue::ReceiveMatch {
                behavior_ids,
                max_params,
            } => {
                // Selective receive: the spec constant encodes the reserved
                // payload-register count and the candidate arm behavior ids
                // as "max_params:id1,id2,...". The VM writes the matched arm
                // index (or the arm count when nothing matched) to dst and
                // payload values into the registers following dst.
                let ids = behavior_ids
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                let spec = format!("{}:{}", max_params, ids);
                let spec_idx = self.module.add_constant(Constant::String(spec));
                self.emit(Instruction::new3(
                    OpCode::ReceiveMatch,
                    ((spec_idx >> 8) & 0xFF) as u8,
                    (spec_idx & 0xFF) as u8,
                    dst,
                ));
            }
            mir::RValue::ReceiveWait {
                behavior_ids,
                max_params,
                timeout,
            } => {
                // Timed selective receive (receive-after): same spec constant
                // and dst contract as ReceiveMatch, plus the timeout in
                // milliseconds staged into r0 (fixed-register staging, like
                // the pipeline opcodes). See OpCode::ReceiveWait (0xA0) in
                // bytecode.rs for the full VM-side contract.
                let ids = behavior_ids
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                let spec = format!("{}:{}", max_params, ids);
                let spec_idx = self.module.add_constant(Constant::String(spec));
                self.emit(Instruction::new2(OpCode::Move, reg_of(*timeout), SCRATCH0));
                self.emit(Instruction::new3(
                    OpCode::ReceiveWait,
                    ((spec_idx >> 8) & 0xFF) as u8,
                    (spec_idx & 0xFF) as u8,
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
                let field_idx = self.state_field_constant(field);
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
            mir::RValue::Send {
                actor,
                behavior_idx,
                args,
            } => {
                // Protect the actor value in a register outside the 0..15
                // staging zone before staging args, mirroring the Call/
                // FUNC_VALUE_REG pattern.
                self.emit(Instruction::new2(
                    OpCode::Move,
                    reg_of(*actor),
                    FUNC_VALUE_REG,
                ));
                self.stage_args(args)?;
                self.emit(Instruction::new3(
                    OpCode::Send,
                    FUNC_VALUE_REG,
                    ((*behavior_idx >> 8) & 0xFF) as u8,
                    (*behavior_idx & 0xFF) as u8,
                ));
                // Send is fire-and-forget with no VM-level result register;
                // the stable compiler yields 0 for send-as-expression.
            }
            mir::RValue::Ask {
                actor,
                behavior_idx,
                args,
            } => {
                self.emit(Instruction::new2(
                    OpCode::Move,
                    reg_of(*actor),
                    FUNC_VALUE_REG,
                ));
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
            mir::RValue::PipelineNew => {
                self.emit(Instruction::new1(OpCode::PipelineNew, dst));
            }
            mir::RValue::PipelineStage {
                id,
                name,
                actor,
                template,
            } => {
                self.emit(Instruction::new2(OpCode::Move, reg_of(*id), SCRATCH0));
                self.emit(Instruction::new2(OpCode::Move, reg_of(*name), SCRATCH0 + 1));
                self.emit(Instruction::new2(
                    OpCode::Move,
                    reg_of(*actor),
                    SCRATCH0 + 2,
                ));
                self.emit(Instruction::new2(
                    OpCode::Move,
                    reg_of(*template),
                    SCRATCH0 + 3,
                ));
                self.emit(Instruction::new1(OpCode::PipelineStage, dst));
            }
            mir::RValue::PipelineRun { id, input } => {
                self.emit(Instruction::new2(OpCode::Move, reg_of(*id), SCRATCH0));
                self.emit(Instruction::new2(
                    OpCode::Move,
                    reg_of(*input),
                    SCRATCH0 + 1,
                ));
                self.emit(Instruction::new1(OpCode::PipelineRun, dst));
            }
            mir::RValue::SupervisorNew => {
                self.emit(Instruction::new1(OpCode::SupervisorNew, dst));
            }
            mir::RValue::SupervisorWorker {
                id,
                name,
                actor,
                description,
            } => {
                self.emit(Instruction::new2(OpCode::Move, reg_of(*id), SCRATCH0));
                self.emit(Instruction::new2(OpCode::Move, reg_of(*name), SCRATCH0 + 1));
                self.emit(Instruction::new2(
                    OpCode::Move,
                    reg_of(*actor),
                    SCRATCH0 + 2,
                ));
                self.emit(Instruction::new2(
                    OpCode::Move,
                    reg_of(*description),
                    SCRATCH0 + 3,
                ));
                self.emit(Instruction::new1(OpCode::SupervisorWorker, dst));
            }
            mir::RValue::SupervisorRun { id, task } => {
                self.emit(Instruction::new2(OpCode::Move, reg_of(*id), SCRATCH0));
                self.emit(Instruction::new2(OpCode::Move, reg_of(*task), SCRATCH0 + 1));
                self.emit(Instruction::new1(OpCode::SupervisorRun, dst));
            }
            mir::RValue::DebateNew {
                topic,
                rounds,
                threshold,
            } => {
                self.emit(Instruction::new2(OpCode::Move, reg_of(*topic), SCRATCH0));
                self.emit(Instruction::new2(
                    OpCode::Move,
                    reg_of(*rounds),
                    SCRATCH0 + 1,
                ));
                self.emit(Instruction::new2(
                    OpCode::Move,
                    reg_of(*threshold),
                    SCRATCH0 + 2,
                ));
                self.emit(Instruction::new1(OpCode::DebateNew, dst));
            }
            mir::RValue::DebateParticipant {
                id,
                name,
                stance,
                actor,
            } => {
                self.emit(Instruction::new2(OpCode::Move, reg_of(*id), SCRATCH0));
                self.emit(Instruction::new2(OpCode::Move, reg_of(*name), SCRATCH0 + 1));
                self.emit(Instruction::new2(
                    OpCode::Move,
                    reg_of(*stance),
                    SCRATCH0 + 2,
                ));
                self.emit(Instruction::new2(
                    OpCode::Move,
                    reg_of(*actor),
                    SCRATCH0 + 3,
                ));
                self.emit(Instruction::new1(OpCode::DebateParticipant, dst));
            }
            mir::RValue::DebateRun { id } => {
                self.emit(Instruction::new2(OpCode::Move, reg_of(*id), SCRATCH0));
                self.emit(Instruction::new1(OpCode::DebateRun, dst));
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

    fn field_id(&mut self, name: &str) -> NuResult<u8> {
        if let Some(&id) = self.field_map.get(name) {
            return Ok(id);
        }
        if self.field_map.len() >= u8::MAX as usize + 1 {
            // Mirrors the stable compiler's field_id: the 256th distinct
            // field name has no free id left (a single byte encodes it), so
            // this is an honest error instead of silently aliasing two
            // unrelated fields onto the same slot.
            return Err(compile_err(format!(
                "module has more than {} distinct record/tuple field names (limit for the current u8 field-id encoding); '{}' has no id left to assign",
                u8::MAX as usize + 1,
                name
            )));
        }
        let id = self.next_field_id;
        self.next_field_id = self.next_field_id.saturating_add(1);
        self.field_map.insert(name.to_string(), id);
        Ok(id)
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

fn binary_opcode(op: &crate::ast::BinOp, is_float: bool) -> NuResult<OpCode> {
    use crate::ast::BinOp;
    match (op, is_float) {
        (BinOp::Add, false) => Ok(OpCode::IAdd),
        (BinOp::Add, true) => Ok(OpCode::FAdd),
        (BinOp::Sub, false) => Ok(OpCode::ISub),
        (BinOp::Sub, true) => Ok(OpCode::FSub),
        (BinOp::Mul, false) => Ok(OpCode::IMul),
        (BinOp::Mul, true) => Ok(OpCode::FMul),
        (BinOp::Div, false) => Ok(OpCode::IDiv),
        (BinOp::Div, true) => Ok(OpCode::FDiv),
        (BinOp::Mod, false) => Ok(OpCode::IMod),
        (BinOp::Mod, true) => Ok(OpCode::FMod),
        (BinOp::Eq, false) => Ok(OpCode::ICmpEq),
        (BinOp::Eq, true) => Ok(OpCode::FCmpEq),
        (BinOp::Lt, false) => Ok(OpCode::ICmpLt),
        (BinOp::Lt, true) => Ok(OpCode::FCmpLt),
        (BinOp::Gt, false) => Ok(OpCode::ICmpGt),
        (BinOp::Gt, true) => Ok(OpCode::FCmpGt),
        (BinOp::Le, false) => Ok(OpCode::ICmpLe),
        (BinOp::Ge, false) => Ok(OpCode::ICmpGe),
        // Float Le/Ge are expanded to negated inverse comparisons by the
        // caller (there are no FCmpLe/FCmpGe opcodes).
        (BinOp::Le, true) | (BinOp::Ge, true) => Err(compile_err(
            "internal: float Le/Ge must be expanded by the caller",
        )),
        (BinOp::And, _) => Ok(OpCode::And),
        (BinOp::Or, _) => Ok(OpCode::Or),
        (BinOp::BitAnd, _) => Ok(OpCode::BitAnd),
        (BinOp::BitOr, _) => Ok(OpCode::BitOr),
        (BinOp::BitXor, _) => Ok(OpCode::Xor),
        (BinOp::Shl, _) => Ok(OpCode::Shl),
        (BinOp::Shr, _) => Ok(OpCode::Shr),
        (other, _) => Err(not_yet_implemented(&format!("binary operator {:?}", other))),
    }
}

/// Compute which locals of a function may hold a Float at runtime, so
/// binary/unary opcode emission can pick the float opcode variants.
///
/// Seeds: locals declared with a Float type and locals assigned a float
/// constant. Propagates to a fixpoint through register copies, float
/// arithmetic, and unary negation. Best-effort: MIR temp types are
/// unreliable (see hir_lower's fallbacks), so float values arriving via
/// paths with no Float-typed origin — unannotated function parameters,
/// call results, array/record loads — are not tracked, and operations on
/// them keep the legacy integer opcodes. That is a limitation, not a
/// regression: pre-fix behavior for those cases was the same.
fn float_locals(func: &mir::Function) -> Vec<bool> {
    let mut is_float = vec![false; func.locals.len()];
    for local in &func.locals {
        if local.ty == Type::Primitive(PrimitiveType::Float) {
            is_float[local.id.0 as usize] = true;
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for block in &func.blocks {
            for stmt in &block.stmts {
                let mir::Stmt::Assign { dst, op } = stmt else {
                    continue;
                };
                let result = match op {
                    mir::RValue::Const(Constant::Float(_)) => true,
                    mir::RValue::Load(src) => is_float[src.0 as usize],
                    mir::RValue::Unary(crate::ast::UnOp::Neg, src) => is_float[src.0 as usize],
                    mir::RValue::Binary(op, l, r)
                        if matches!(
                            op,
                            crate::ast::BinOp::Add
                                | crate::ast::BinOp::Sub
                                | crate::ast::BinOp::Mul
                                | crate::ast::BinOp::Div
                                | crate::ast::BinOp::Mod
                        ) =>
                    {
                        is_float[l.0 as usize] || is_float[r.0 as usize]
                    }
                    _ => false,
                };
                if result && !is_float[dst.0 as usize] {
                    is_float[dst.0 as usize] = true;
                    changed = true;
                }
            }
        }
    }
    is_float
}

pub fn compile_mir(mir: &mir::Module, module_name: impl Into<String>) -> NuResult<CodeModule> {
    let mut codegen = MirCodegen::new(module_name);
    codegen.compile_module(mir)?;
    Ok(codegen.finish())
}

// ---------------------------------------------------------------------------
// Liveness-based Drop planning
// ---------------------------------------------------------------------------
//
// The VM's `Drop` opcode releases a register's local reference to a heap
// object and clears the register to nil (so duplicate drops are no-ops).
// This pass decides where to emit it. The goal is conservative correctness:
// when in doubt, no drop is emitted and the value lives until actor exit.
//
// A local is a *candidate* when the analysis can prove its register always
// holds the value's only counted reference (besides references taken by the
// retaining store barriers, which keep the object alive independently):
//
//   - its type may hold a NaN-boxed heap pointer (MIR temp types are
//     unreliable, so only definitely-scalar types are excluded);
//   - it is not a parameter, closure capture, or effect-handler parameter
//     (those arrive through plain, uncounted register copies);
//   - it has at least one definition (never-assigned registers may hold
//     VM-written values such as ReceiveMatch payloads, which follow the
//     foreign-ref protocol and must not be dropped locally);
//   - every definition is an owning rvalue — Tuple/Record/ArrayLit (fresh
//     allocation) or Const (never a heap pointer) — that does not read the
//     local itself;
//   - no use copies the value through an uncounted channel: Move/Load,
//     `&`/`*`, call or effect arguments, closure captures, sends/asks,
//     returns/resumes, `StateSet`, or the AI builtins' staging moves.
//
// Uses through the retaining barriers (container element stores) and
// read-only uses (container base/length, operands, branch conditions) do
// not disqualify: after a retaining store the slot owns its own reference,
// so releasing the register's duplicate is sound.
//
// Escapees: a local defined by a field/element load (`RecL`/`FieldL`/
// `ArrLoad`) from a candidate aliases that container's slots *without* a
// counted reference, so a candidate is never dropped at a point where one
// of its (transitive) escapees is still live.
//
// Drop points per candidate: before every redefinition (release the old
// value; always sound — the register is nil after any earlier drop), after
// a definition whose value is immediately dead, after a last read-only or
// retaining use, and at the entry of blocks the value flows into but never
// uses (branch-edge death). A value whose last use is a branch condition
// cannot be dropped there (the terminator must run first) and simply lives
// until actor exit.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UseKind {
    /// Read without copying the value bits (container base, length, operand).
    ReadOnly,
    /// Copied into a heap container through a retaining barrier — the
    /// register's own reference may be released afterwards.
    Retaining,
    /// Copied through a channel that takes no counted reference (Move/Load,
    /// call staging, send, capture, return, actor state).
    Copy,
}

/// Locals of these types can hold NaN-boxed heap pointers at runtime. MIR
/// temps are often typed `Type::unit()` while carrying pointers, so only
/// definitely-scalar types are excluded.
fn may_hold_heap_ptr(ty: &Type) -> bool {
    match ty {
        Type::Primitive(p) => matches!(p, PrimitiveType::String | PrimitiveType::Unit),
        Type::Tuple(_)
        | Type::Record(_)
        | Type::Array(_)
        | Type::App { .. }
        | Type::Var(_)
        | Type::Scheme { .. }
        | Type::Reference { .. } => true,
        Type::Variant(_) | Type::Function { .. } | Type::Actor { .. } => false,
    }
}

/// The rvalue forms whose result is a freshly allocated heap object (or a
/// non-pointer constant) owned solely by the destination register.
fn rvalue_is_owning(op: &mir::RValue) -> bool {
    matches!(
        op,
        mir::RValue::Tuple(_)
            | mir::RValue::Record(_)
            | mir::RValue::ArrayLit(_)
            | mir::RValue::Const(_)
    )
}

/// Every occurrence of a local inside an rvalue, with how the value is used.
fn rvalue_uses(op: &mir::RValue) -> Vec<(usize, UseKind)> {
    use mir::RValue::*;
    let mut out = Vec::new();
    let ro = |out: &mut Vec<(usize, UseKind)>, id: mir::LocalId| {
        out.push((id.0 as usize, UseKind::ReadOnly))
    };
    let ret = |out: &mut Vec<(usize, UseKind)>, id: mir::LocalId| {
        out.push((id.0 as usize, UseKind::Retaining))
    };
    let cp = |out: &mut Vec<(usize, UseKind)>, id: mir::LocalId| {
        out.push((id.0 as usize, UseKind::Copy))
    };
    match op {
        Const(_)
        | SignalWait { .. }
        | Receive
        | ReceiveMatch { .. }
        | PipelineNew
        | SupervisorNew
        | Spawn { .. }
        | SelfRef
        | StateGet { .. } => {}
        // The timeout value is staged into r0 with a plain Move — an
        // uncounted copy channel like call/effect argument staging.
        ReceiveWait { timeout, .. } => cp(&mut out, *timeout),
        Load(x) => cp(&mut out, *x),
        LoadFieldNamed { obj, .. } | LoadFieldPos { obj, .. } => ro(&mut out, *obj),
        ArrayLoad { arr, idx } => {
            ro(&mut out, *arr);
            ro(&mut out, *idx);
        }
        ArrayLen(x) => ro(&mut out, *x),
        ArrayLit(elems) => {
            for e in elems {
                ret(&mut out, *e);
            }
        }
        Unary(_, x) => cp(&mut out, *x),
        Binary(_, l, r) => {
            ro(&mut out, *l);
            ro(&mut out, *r);
        }
        StringEq(l, r) => {
            ro(&mut out, *l);
            ro(&mut out, *r);
        }
        Call { func, args } => {
            if let mir::FuncRef::Local(f) = func {
                cp(&mut out, *f);
            }
            for a in args {
                cp(&mut out, *a);
            }
        }
        Closure { captures, .. } => {
            for c in captures {
                cp(&mut out, *c);
            }
        }
        Tuple(elems) => {
            for e in elems {
                ret(&mut out, *e);
            }
        }
        Record(fields) => {
            for (_, e) in fields {
                ret(&mut out, *e);
            }
        }
        Perform { args, .. } | FFICall { args, .. } => {
            for a in args {
                cp(&mut out, *a);
            }
        }
        LlmAsk { prompt } => cp(&mut out, *prompt),
        Migrate { actor, node } => {
            cp(&mut out, *actor);
            cp(&mut out, *node);
        }
        CapabilityCheck { val } => cp(&mut out, *val),
        Send { actor, args, .. } | Ask { actor, args, .. } => {
            cp(&mut out, *actor);
            for a in args {
                cp(&mut out, *a);
            }
        }
        PipelineStage { id, name, actor, template } => {
            for x in [id, name, actor, template] {
                cp(&mut out, *x);
            }
        }
        PipelineRun { id, input } => {
            cp(&mut out, *id);
            cp(&mut out, *input);
        }
        SupervisorWorker { id, name, actor, description } => {
            for x in [id, name, actor, description] {
                cp(&mut out, *x);
            }
        }
        SupervisorRun { id, task } => {
            cp(&mut out, *id);
            cp(&mut out, *task);
        }
        DebateNew { topic, rounds, threshold } => {
            cp(&mut out, *topic);
            cp(&mut out, *rounds);
            cp(&mut out, *threshold);
        }
        DebateParticipant { id, name, stance, actor } => {
            for x in [id, name, stance, actor] {
                cp(&mut out, *x);
            }
        }
        DebateRun { id } => cp(&mut out, *id),
    }
    out
}

/// Every occurrence of a local inside a statement (an assignment's
/// destination is a definition, not a use).
fn stmt_uses(stmt: &mir::Stmt) -> Vec<(usize, UseKind)> {
    match stmt {
        mir::Stmt::Assign { op, .. } => rvalue_uses(op),
        mir::Stmt::StoreFieldNamed { obj, src, .. } => vec![
            (obj.0 as usize, UseKind::ReadOnly),
            (src.0 as usize, UseKind::Retaining),
        ],
        mir::Stmt::ArrayStore { arr, idx, src } => vec![
            (arr.0 as usize, UseKind::ReadOnly),
            (idx.0 as usize, UseKind::ReadOnly),
            (src.0 as usize, UseKind::Retaining),
        ],
        mir::Stmt::EnterHandle { .. } | mir::Stmt::PopHandler => Vec::new(),
        mir::Stmt::Emit { args, .. } => args
            .iter()
            .map(|a| (a.0 as usize, UseKind::Copy))
            .collect(),
        // StateSet stores into actor state without retaining, so the stored
        // value must keep its register reference: treat it as a copy.
        mir::Stmt::StateSet { src, .. } => vec![(src.0 as usize, UseKind::Copy)],
    }
}

fn terminator_uses(term: &mir::Terminator) -> Vec<(usize, UseKind)> {
    match term {
        mir::Terminator::Return(Some(v)) | mir::Terminator::Resume(v) => {
            vec![(v.0 as usize, UseKind::Copy)]
        }
        mir::Terminator::Branch { cond, .. } => vec![(cond.0 as usize, UseKind::ReadOnly)],
        _ => Vec::new(),
    }
}

/// Successor block indices of a terminator (block ids are dense indices
/// into `Function::blocks`).
fn terminator_successors(term: &mir::Terminator) -> Vec<usize> {
    match term {
        mir::Terminator::Jump(t) => vec![t.0 as usize],
        mir::Terminator::Branch { then_, else_, .. } => {
            vec![then_.0 as usize, else_.0 as usize]
        }
        _ => Vec::new(),
    }
}

/// Where to emit `Drop` instructions for one function.
#[derive(Default)]
struct DropPlan {
    /// Before the block's first statement (after any handler prologue).
    block_entry: HashMap<usize, Vec<mir::LocalId>>,
    /// Before the first instruction of statement (block, stmt).
    before_stmt: HashMap<(usize, usize), Vec<mir::LocalId>>,
    /// After the last instruction of statement (block, stmt).
    after_stmt: HashMap<(usize, usize), Vec<mir::LocalId>>,
}

/// Compute conservative `Drop` placements for one function; see the section
/// docs above for the soundness argument.
fn plan_drops(func: &mir::Function) -> DropPlan {
    let mut plan = DropPlan::default();
    let nlocals = func.locals.len();
    let nblocks = func.blocks.len();
    if nlocals == 0 || nblocks == 0 {
        return plan;
    }

    let ptr_ty: Vec<bool> = func.locals.iter().map(|l| may_hold_heap_ptr(&l.ty)).collect();

    // Locals that receive their value outside MIR assignments can never be
    // proven solely owned.
    let mut excluded = vec![false; nlocals];
    for id in func.params.iter().chain(&func.captures) {
        excluded[id.0 as usize] = true;
    }
    for table in &func.handler_tables {
        for binding in &table.bindings {
            for id in &binding.params {
                excluded[id.0 as usize] = true;
            }
        }
    }

    // Scan defs and uses for the whole function.
    let mut has_def = vec![false; nlocals];
    let mut defs_owning = vec![true; nlocals];
    let mut no_copy_use = vec![true; nlocals];
    let mut block_defs: Vec<HashSet<usize>> = (0..nblocks).map(|_| HashSet::new()).collect();
    let mut block_uses: Vec<HashSet<usize>> = (0..nblocks).map(|_| HashSet::new()).collect();
    // (dst, base) pairs of field/element loads, for escapee tracking.
    let mut loads: Vec<(usize, usize)> = Vec::new();

    for (bi, block) in func.blocks.iter().enumerate() {
        for stmt in &block.stmts {
            for (u, kind) in stmt_uses(stmt) {
                block_uses[bi].insert(u);
                if kind == UseKind::Copy {
                    no_copy_use[u] = false;
                }
            }
            if let mir::Stmt::Assign { dst, op } = stmt {
                let d = dst.0 as usize;
                has_def[d] = true;
                block_defs[bi].insert(d);
                if !rvalue_is_owning(op) || rvalue_uses(op).iter().any(|(u, _)| *u == d) {
                    defs_owning[d] = false;
                }
                match op {
                    mir::RValue::LoadFieldNamed { obj, .. } | mir::RValue::LoadFieldPos { obj, .. } => {
                        loads.push((d, obj.0 as usize))
                    }
                    mir::RValue::ArrayLoad { arr, .. } => loads.push((d, arr.0 as usize)),
                    _ => {}
                }
            }
        }
        for (u, kind) in terminator_uses(&block.terminator) {
            block_uses[bi].insert(u);
            if kind == UseKind::Copy {
                no_copy_use[u] = false;
            }
        }
    }

    let candidate: Vec<bool> = (0..nlocals)
        .map(|i| ptr_ty[i] && !excluded[i] && has_def[i] && defs_owning[i] && no_copy_use[i])
        .collect();

    // Escapees: locals defined by field/element loads from a candidate or
    // another escapee (transitively).
    let mut escapees: Vec<Vec<usize>> = (0..nlocals).map(|_| Vec::new()).collect();
    for c in 0..nlocals {
        if !candidate[c] {
            continue;
        }
        let mut seen = HashSet::new();
        let mut frontier = vec![c];
        while let Some(x) = frontier.pop() {
            for &(dst, base) in &loads {
                if base == x && ptr_ty[dst] && seen.insert(dst) {
                    escapees[c].push(dst);
                    frontier.push(dst);
                }
            }
        }
    }
    let esc_clear = |c: usize, live: &HashSet<usize>| escapees[c].iter().all(|e| !live.contains(e));

    // Backward may-liveness over all locals.
    let mut live_in: Vec<HashSet<usize>> = (0..nblocks).map(|_| HashSet::new()).collect();
    let mut live_out: Vec<HashSet<usize>> = (0..nblocks).map(|_| HashSet::new()).collect();
    loop {
        let mut changed = false;
        for bi in (0..nblocks).rev() {
            let mut out: HashSet<usize> = HashSet::new();
            for succ in terminator_successors(&func.blocks[bi].terminator) {
                for l in &live_in[succ] {
                    out.insert(*l);
                }
            }
            let mut inset = out.clone();
            for d in &block_defs[bi] {
                inset.remove(d);
            }
            for u in &block_uses[bi] {
                inset.insert(*u);
            }
            if inset != live_in[bi] || out != live_out[bi] {
                live_in[bi] = inset;
                live_out[bi] = out;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Walk each block backward, emitting drops where a candidate's value
    // dies.
    for (bi, block) in func.blocks.iter().enumerate() {
        let mut live: HashSet<usize> = live_out[bi].clone();
        for (u, _) in terminator_uses(&block.terminator) {
            live.insert(u);
        }
        for (si, stmt) in block.stmts.iter().enumerate().rev() {
            let uses = stmt_uses(stmt);
            // Last-use drops for candidates this statement reads.
            for (u, _) in &uses {
                if candidate[*u] && !live.contains(u) && esc_clear(*u, &live) {
                    plan.after_stmt
                        .entry((bi, si))
                        .or_default()
                        .push(func.locals[*u].id);
                }
            }
            for (u, _) in &uses {
                live.insert(*u);
            }
            if let mir::Stmt::Assign { dst, .. } = stmt {
                let d = dst.0 as usize;
                if candidate[d] {
                    // The new value is dead on arrival: release it right
                    // after the statement.
                    if !live.contains(&d) && esc_clear(d, &live) {
                        plan.after_stmt.entry((bi, si)).or_default().push(*dst);
                    }
                    // Release the overwritten old value before the
                    // statement. Always sound for a candidate: the register
                    // holds the previous definition's product (or nil after
                    // an earlier drop), never an alias.
                    if esc_clear(d, &live) {
                        plan.before_stmt.entry((bi, si)).or_default().push(*dst);
                    }
                }
                live.remove(&d);
            }
        }
        // Entry drops: candidates held on some incoming edge but dead at
        // this block's entry (their value died at the branch that led here).
        let mut held_in: HashSet<usize> = HashSet::new();
        for (pj, pred) in func.blocks.iter().enumerate() {
            if terminator_successors(&pred.terminator).contains(&bi) {
                for l in &live_out[pj] {
                    held_in.insert(*l);
                }
            }
        }
        for c in 0..nlocals {
            if candidate[c] && held_in.contains(&c) && !live_in[bi].contains(&c) && esc_clear(c, &live_in[bi]) {
                plan.block_entry.entry(bi).or_default().push(func.locals[c].id);
            }
        }
    }

    for ids in plan
        .block_entry
        .values_mut()
        .chain(plan.before_stmt.values_mut())
        .chain(plan.after_stmt.values_mut())
    {
        ids.sort();
        ids.dedup();
    }
    plan
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
        assert_eq!(
            value.as_int(),
            Some(99),
            "arr[0] = 99 should actually mutate the array"
        );

        let value = run_mir_source("let r = { x: 1, y: 2 } in { r.x = 99 r.x + r.y }").unwrap();
        assert_eq!(
            value.as_int(),
            Some(101),
            "r.x = 99 should actually mutate the record"
        );
    }

    #[test]
    fn test_mir_codegen_assign_expression_yields_assigned_value() {
        // Mirrors the stable compiler's compile_assign, which returns the
        // assigned value rather than unit — an assignment used as a block's
        // trailing expression must yield that value, not unit.
        let value = run_mir_source("let x = &1 in { x = 2 }").unwrap();
        assert_eq!(
            value.as_int(),
            Some(2),
            "`x = 2` as an expression should yield 2"
        );

        let value = run_mir_source("let r = { x: 1 } in { r.x = 5 }").unwrap();
        assert_eq!(
            value.as_int(),
            Some(5),
            "`r.x = 5` as an expression should yield 5"
        );

        let value = run_mir_source("let arr = [1, 2] in { arr[0] = 7 }").unwrap();
        assert_eq!(
            value.as_int(),
            Some(7),
            "`arr[0] = 7` as an expression should yield 7"
        );
    }

    #[test]
    fn test_mir_codegen_ref_cell_deref_and_assign() {
        // Mirrors src/integration_tests.rs's test_local_assignment (legacy
        // pipeline): `&` creates a ref cell, `*` dereferences it, and
        // assignment mutates it in place.
        let value = run_mir_source("let x = &10 in { x = 3; *x }").unwrap();
        assert_eq!(value.as_int(), Some(3));
    }

    #[test]
    fn test_mir_codegen_over_limit_params_is_honest_error_not_corruption() {
        // A function with more than MAX_STAGED_ARGS (16) parameters used to
        // compile "successfully" with a prologue that reads incoming
        // arguments from registers overlapping LOCAL_BASE-mapped locals —
        // corrupt bytecode nothing could ever validly call (every call site
        // is bounded by the same 16-arg staging limit), but a compile error
        // is the honest outcome, matching this pipeline's "no silent
        // misbehavior" guarantee.
        let params = (0..17)
            .map(|i| format!("a{}: Int", i))
            .collect::<Vec<_>>()
            .join(", ");
        let source = format!("fn f({}) -> Int {{ a0 }}\n0", params);
        let result = compile_mir_source(&source);
        assert!(
            matches!(result, Err(NuError::VMError(_))),
            "a 17-parameter function should be an honest compile error, got {:?}",
            result
        );
    }

    #[test]
    fn test_mir_codegen_field_id_errors_past_256_distinct_field_names() {
        // Mirrors the same regression in the stable compiler: the 257th
        // distinct record field name has no free u8 id, and must be an
        // honest error rather than silently aliasing onto an existing id.
        //
        // Each field name lives in its own top-level function's own tiny
        // record literal, not a single 257-field record — a single record
        // (or a chain of 257 `let`s) hits MIR's unrelated per-function local
        // count cap first, which would mask the field_id check this test is
        // actually targeting.
        let fns: Vec<String> = (0..257)
            .map(|i| format!("fn g{i}() -> Int {{ {{ f{i}: {i} }}.f{i} }}"))
            .collect();
        let source = format!("{}\ng0()", fns.join("\n"));
        let result = compile_mir_source(&source);
        assert!(
            result.is_err(),
            "the 257th distinct field name should be an honest error, not silent aliasing"
        );
    }

    #[test]
    fn test_mir_codegen_effect_handler() {
        let value =
            run_mir_source("handle perform Math.getAnswer() { | Math.getAnswer() => 42 }").unwrap();
        assert_eq!(value.as_int(), Some(42));
    }

    #[test]
    fn test_mir_codegen_float_arithmetic() {
        // Binary/unary opcode emission is type-directed: float operands
        // must compile to FAdd/FSub/FMul/FDiv/FNeg — the integer handlers
        // coerce float operands to 0.
        let value = run_mir_source("1.5 + 2.5").unwrap();
        assert_eq!(value.as_float(), Some(4.0));
        let value = run_mir_source("5.5 - 2.0").unwrap();
        assert_eq!(value.as_float(), Some(3.5));
        let value = run_mir_source("1.5 * 2.0").unwrap();
        assert_eq!(value.as_float(), Some(3.0));
        let value = run_mir_source("7.0 / 2.0").unwrap();
        assert_eq!(value.as_float(), Some(3.5));
        let value = run_mir_source("-1.5").unwrap();
        assert_eq!(value.as_float(), Some(-1.5));
    }

    #[test]
    fn test_mir_codegen_float_arithmetic_through_locals() {
        // Float-ness propagates through let bindings and intermediate
        // temps: `y` holds a float even though hir_lower types binary
        // results as Int.
        let value = run_mir_source("let x = 1.5 in let y = x + 2.5 in y * 2.0").unwrap();
        assert_eq!(value.as_float(), Some(8.0));
        let value = run_mir_source("let a = 6.0 in let b = a / 4.0 in b").unwrap();
        assert_eq!(value.as_float(), Some(1.5));
    }

    #[test]
    fn test_mir_codegen_float_comparisons() {
        // Integer comparisons on float operands coerce both sides to 0,
        // making `2.0 == 3.0` true and every ordering comparison false
        // (or always-true for Le/Ge/Ne); floats need FCmp*.
        let value = run_mir_source("1.5 < 2.5").unwrap();
        assert_eq!(value.as_bool(), Some(true));
        let value = run_mir_source("2.5 > 1.5").unwrap();
        assert_eq!(value.as_bool(), Some(true));
        let value = run_mir_source("2.5 <= 1.5").unwrap();
        assert_eq!(value.as_bool(), Some(false));
        let value = run_mir_source("1.5 >= 2.5").unwrap();
        assert_eq!(value.as_bool(), Some(false));
        let value = run_mir_source("2.0 == 3.0").unwrap();
        assert_eq!(value.as_bool(), Some(false));
        let value = run_mir_source("2.0 != 3.0").unwrap();
        assert_eq!(value.as_bool(), Some(true));
        let value = run_mir_source("2.0 == 2.0").unwrap();
        assert_eq!(value.as_bool(), Some(true));
    }

    #[test]
    fn test_mir_codegen_float_div_by_zero_yields_nil() {
        // Matches the interpreter's FDiv semantics: a zero float divisor
        // yields nil, not a trap or inf.
        let value = run_mir_source("7.0 / 0.0").unwrap();
        assert_eq!(value.as_raw(), crate::vm::Value::nil().as_raw());
    }

    #[test]
    fn test_mir_codegen_float_modulo() {
        // Float `%` compiles to the FMod opcode (0x35), which the
        // interpreter implements with f64 % f64 semantics and a nil
        // result on a zero divisor, mirroring FDiv.
        let value = run_mir_source("7.5 % 2.0").unwrap();
        assert_eq!(value.as_float(), Some(1.5));
        let value = run_mir_source("7.0 % 0.0").unwrap();
        assert_eq!(value.as_raw(), crate::vm::Value::nil().as_raw());
        // Integer modulo is unaffected.
        let value = run_mir_source("7 % 2").unwrap();
        assert_eq!(value.as_int(), Some(1));
    }

    #[test]
    fn test_mir_codegen_over_limit_handler_params_is_honest_error() {
        // A handler binding with more than MAX_STAGED_ARGS (16) parameters
        // used to compile a prologue moving r16.. into LOCAL_BASE-mapped
        // registers, silently aliasing the enclosing function's locals —
        // the VM only ever stages effect arguments in r0..r15. Like the
        // 17-parameter function check, this must be a compile error.
        let params = (0..17)
            .map(|i| format!("p{}", i))
            .collect::<Vec<_>>()
            .join(", ");
        let source = format!("handle 0 {{ | E.op({}) => p0 }}", params);
        let result = compile_mir_source(&source);
        assert!(
            matches!(result, Err(NuError::VMError(_))),
            "a 17-parameter handler binding should be an honest compile error, got {:?}",
            result
        );
        // A 16-parameter binding stays legal.
        let params = (0..16)
            .map(|i| format!("p{}", i))
            .collect::<Vec<_>>()
            .join(", ");
        let source = format!("handle 0 {{ | E.op({}) => p0 }}", params);
        assert!(
            compile_mir_source(&source).is_ok(),
            "a 16-parameter handler binding should compile"
        );
    }

    #[test]
    fn test_mir_codegen_actor_spawn_returns_actor_ref() {
        // Actors are now lowered by the HIR/MIR pipeline. Without a real
        // Runtime attached, spawn_actor's default stub always returns
        // actor_ref(0); real behavior semantics (state, ask) are covered by
        // src/integration_tests.rs's MIR-vs-legacy actor tests, which attach
        // a Runtime.
        let value =
            run_mir_source("actor A { state x = 0 behavior get() { self.x } }\nspawn A { x = 0 }")
                .unwrap();
        assert!(
            value.as_actor_id().is_some(),
            "spawn should yield an actor reference"
        );
    }

    #[test]
    fn test_mir_codegen_state_field_access_reuses_one_constant() {
        // Every `self.x` read/write used to add a fresh, duplicate string
        // constant to the module's constant pool. A behavior referencing the
        // same field several times should only cost one "x" constant.
        let module = compile_mir_source(
            "actor A { state x = 0 behavior bump() { (self.x = self.x + 1, self.x = self.x + 1, self.x) } }\nspawn A { x = 0 }",
        )
        .unwrap();
        let x_constants = module
            .constants
            .iter()
            .filter(|c| matches!(c, crate::bytecode::Constant::String(s) if s == "x"))
            .count();
        assert_eq!(
            x_constants, 1,
            "repeated self.x access should reuse one constant-pool entry, found {}",
            x_constants
        );
    }

    #[test]
    fn test_mir_codegen_plain_workflow_and_agent_compile() {
        // Sequential workflows and tool-less agents desugar to actors and
        // compile like any other actor declaration.
        let result = compile_mir_source("workflow W { step a { 1 } }");
        assert!(
            result.is_ok(),
            "plain sequential workflow should compile: {:?}",
            result
        );

        let result = compile_mir_source(r#"agent Ag = { model: "gpt-4o" }"#);
        assert!(
            result.is_ok(),
            "tool-less agent should compile: {:?}",
            result
        );
    }

    #[test]
    fn test_mir_codegen_parallel_workflow_compiles() {
        let result = compile_mir_source("workflow W { parallel { step a { 1 } step b { 2 } } }");
        assert!(
            result.is_ok(),
            "parallel workflow should compile: {:?}",
            result
        );
        let module = result.unwrap();
        assert_eq!(module.behaviors.len(), 1);
        assert_eq!(
            module.behaviors[0].parallel_branches,
            Some(vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn test_mir_codegen_agent_with_resolved_tool_compiles() {
        let result = compile_mir_source(
            r#"
            @tool(description: "Search the web.")
            fn search(query: String) -> String { query }

            agent Ag = { model: "gpt-4o", tools: [search] }
            "#,
        );
        assert!(
            result.is_ok(),
            "agent with a resolvable tool should compile: {:?}",
            result
        );
        let module = result.unwrap();
        assert_eq!(module.actor_metadata.len(), 1);
        assert_eq!(module.actor_metadata[0].tools.len(), 1);
        assert_eq!(module.actor_metadata[0].tools[0].name, "search");
    }

    #[test]
    fn test_mir_codegen_agent_with_unknown_tool_falls_back_honestly() {
        // No @tool-annotated `search` function exists; this must not
        // silently compile with a dropped/garbage tool reference.
        let result = compile_mir_source(r#"agent Ag = { model: "gpt-4o", tools: [search] }"#);
        assert!(
            matches!(result, Err(NuError::NotYetImplemented { .. })),
            "agent with an unresolvable tool must be an honest NotYetImplemented, got {:?}",
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
                            func: crate::hir::Operand::Var(
                                "nope".into(),
                                crate::types::Type::unit(),
                            ),
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

    #[test]
    fn test_mir_nested_module_declarations_are_flattened() {
        // Nested `module Name { ... }` blocks are a pure namespacing
        // construct: the stable compiler's compile_decl flattens them by
        // recursing over their inner decls in place, and mir_lower.rs now
        // does the same instead of erroring. Constructed directly against
        // HIR (rather than via source + the type checker) because nested
        // modules don't yet export bindings into the enclosing scope at the
        // type-checker level — a separate, pre-existing gap in both
        // pipelines, unrelated to this mir_lower.rs fix.
        let square_fn = crate::hir::FunctionDef {
            name: "square".into(),
            type_params: vec![],
            params: vec![("x".into(), crate::types::Type::int())],
            ret: crate::types::Type::int(),
            effect: crate::types::EffectRow::empty(),
            cap: crate::types::Capability::Ref,
            body: {
                let mut b = crate::hir::Body::new();
                b.set_terminator(crate::hir::Terminator::Yield(crate::hir::Operand::Var(
                    "__result".into(),
                    crate::types::Type::int(),
                )));
                b.push(crate::hir::Stmt::Let {
                    name: "__result".into(),
                    ty: crate::types::Type::int(),
                    value: crate::hir::RValue::Binary(
                        crate::ast::BinOp::Mul,
                        crate::hir::Operand::Var("x".into(), crate::types::Type::int()),
                        crate::hir::Operand::Var("x".into(), crate::types::Type::int()),
                        crate::types::Type::int(),
                    ),
                    span: Span::default(),
                });
                b
            },
            public: false,
            span: Span::default(),
        };
        let main_fn = crate::hir::FunctionDef {
            name: "__main".into(),
            type_params: vec![],
            params: vec![],
            ret: crate::types::Type::int(),
            effect: crate::types::EffectRow::empty(),
            cap: crate::types::Capability::Ref,
            body: {
                let mut b = crate::hir::Body::new();
                b.set_terminator(crate::hir::Terminator::Yield(crate::hir::Operand::Var(
                    "r".into(),
                    crate::types::Type::int(),
                )));
                b.push(crate::hir::Stmt::Let {
                    name: "r".into(),
                    ty: crate::types::Type::int(),
                    value: crate::hir::RValue::Call {
                        func: crate::hir::Operand::Var("square".into(), crate::types::Type::unit()),
                        args: vec![crate::hir::Operand::Literal(
                            crate::ast::Literal::Int(6),
                            crate::types::Type::int(),
                        )],
                        ty: crate::types::Type::int(),
                    },
                    span: Span::default(),
                });
                b
            },
            public: false,
            span: Span::default(),
        };
        let hir = crate::hir::Module {
            name: "t".into(),
            decls: vec![
                crate::hir::Decl::Module {
                    name: "Math".into(),
                    exports: vec![],
                    decls: vec![crate::hir::Decl::Function(square_fn)],
                    span: Span::default(),
                },
                crate::hir::Decl::Function(main_fn),
            ],
        };
        let mir = crate::mir_lower::lower_module(&hir).unwrap();
        let module = crate::mir_codegen::compile_mir(&mir, "t").unwrap();
        let mut vm = VM::new();
        vm.load_module(module);
        let value = vm.run().unwrap();
        assert_eq!(
            value.as_int(),
            Some(36),
            "nested module's function should be reachable unqualified"
        );
    }

    #[test]
    fn test_receive_after_emits_receivewait_with_staged_timeout() {
        // receive-after codegen: a Move stages the timeout (ms) into r0,
        // then ReceiveWait (0xA0) carries the candidate-ids spec constant in
        // op1+op2 and the arm-index/payload base register in op3, exactly
        // like ReceiveMatch. (Compiling only — the VM handler is wave 2.)
        let module =
            compile_mir_source("receive { | Msg(x) => x } after 100 => 0").unwrap();
        let pos = module
            .instructions
            .iter()
            .position(|i| i.opcode == OpCode::ReceiveWait)
            .unwrap_or_else(|| {
                panic!(
                    "receive-after must emit ReceiveWait: {:?}",
                    module.instructions
                )
            });
        let instr = module.instructions[pos];
        // The spec constant is "max_params:id1,id2,..." like ReceiveMatch.
        let spec_idx = instr.imm16() as usize;
        match &module.constants[spec_idx] {
            Constant::String(s) => {
                assert_eq!(
                    s.split(':').next(),
                    Some("1"),
                    "one arm with one param reserves one payload register: {}",
                    s
                );
            }
            other => panic!("spec constant must be a string, got {:?}", other),
        }
        // Immediately before: Move timeout_reg -> r0.
        let prev = module.instructions[pos - 1];
        assert_eq!(prev.opcode, OpCode::Move, "timeout staging move");
        assert_eq!(prev.op2, 0, "timeout must be staged into r0");
        // No ReceiveMatch and no legacy pop-any Receive in the timed form.
        assert!(
            !module
                .instructions
                .iter()
                .any(|i| i.opcode == OpCode::ReceiveMatch),
            "timed receive must not emit ReceiveMatch"
        );
        assert!(
            !module
                .instructions
                .iter()
                .any(|i| i.opcode == OpCode::Receive),
            "receive-after must not emit the legacy pop-any Receive"
        );
    }

    #[test]
    fn test_receive_without_after_emits_receivematch_not_receivewait() {
        let module = compile_mir_source("receive { | Msg(x) => x }").unwrap();
        assert!(
            module
                .instructions
                .iter()
                .any(|i| i.opcode == OpCode::ReceiveMatch),
            "plain receive must emit ReceiveMatch"
        );
        assert!(
            module
                .instructions
                .iter()
                .any(|i| i.opcode == OpCode::Receive),
            "plain receive must keep the legacy fallback"
        );
        assert!(
            !module
                .instructions
                .iter()
                .any(|i| i.opcode == OpCode::ReceiveWait),
            "plain receive must not emit ReceiveWait"
        );
    }
}
