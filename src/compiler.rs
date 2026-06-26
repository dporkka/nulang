//! AST -> Bytecode compiler.
//!
//! Compiles Nulang AST into bytecode modules for the VM.

use crate::ast::*;
use crate::bytecode::*;
use crate::types::NuResult;

/// Workaround for the `Self` opcode (0x83) which conflicts with the Rust keyword.
/// Uses transmute from the known discriminant value.
fn op_self() -> OpCode {
    // Safety: 0x83 is the guaranteed discriminant for the `Self` variant.
    unsafe { std::mem::transmute::<u8, OpCode>(0x83) }
}

/// A mapping from local variable name to register index within a scope.
#[derive(Debug, Clone)]
struct ScopeFrame {
    bindings: Vec<(String, u8)>,
}

impl ScopeFrame {
    fn new() -> Self {
        ScopeFrame { bindings: Vec::new() }
    }
    fn get(&self, name: &str) -> Option<u8> {
        for (n, r) in self.bindings.iter().rev() {
            if n == name { return Some(*r); }
        }
        None
    }
    fn insert(&mut self, name: String, reg: u8) {
        self.bindings.push((name, reg));
    }
}

/// Compiler state during AST -> bytecode translation.
pub struct Compiler {
    module: CodeModule,
    locals: Vec<ScopeFrame>,
    next_reg: u8,
    constants: Vec<Constant>,
    label_counter: usize,
    /// Map field names to numeric indices for FieldL.
    field_map: std::collections::HashMap<String, u8>,
    next_field_id: u8,
    /// Map function names to function_table indices.
    func_map: std::collections::HashMap<String, usize>,
    /// Track the function_table index of the __main entry function.
    main_func_idx: Option<usize>,
}

impl Compiler {
    pub fn new(module_name: impl Into<String>) -> Self {
        Compiler {
            module: CodeModule::new(module_name),
            locals: vec![ScopeFrame::new()],
            next_reg: 0,
            constants: Vec::new(),
            label_counter: 0,
            field_map: std::collections::HashMap::new(),
            next_field_id: 0,
            func_map: std::collections::HashMap::new(),
            main_func_idx: None,
        }
    }

    /// Compile all declarations in the AST module.
    /// __main functions are compiled inline (not via function table).
    /// Uses two-pass compilation to support forward references and mutual recursion.
    pub fn compile_module(&mut self, ast: &AstModule) -> NuResult<&CodeModule> {
        // Pass 1: Collect all function names and reserve function table slots.
        // This enables forward references and mutual recursion.
        self.collect_functions(&ast.decls)?;

        // Pass 2: Compile all function bodies and __main inline code.
        for decl in &ast.decls {
            self.compile_decl(decl)?;
        }
        // If no __main inline body was emitted, add a halt
        if self.module.instructions.is_empty() {
            self.emit(Instruction::new0(OpCode::Halt));
        }
        Ok(&self.module)
    }

    fn collect_functions(&mut self, decls: &[Decl]) -> NuResult<()> {
        for decl in decls {
            match decl {
                Decl::Function { name, .. } if name != "__main" => {
                    let func_idx = self.module.function_table.len();
                    self.module.function_table.push(0); // placeholder
                    self.func_map.insert(name.clone(), func_idx);
                }
                Decl::Module { decls: subdecls, .. } => {
                    self.collect_functions(subdecls)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn finish(self) -> CodeModule {
        self.module
    }

    // ========================================================================
    // Expression compilation
    // ========================================================================

    fn compile_expr(&mut self, expr: &Expr) -> NuResult<u8> {
        match expr {
            Expr::Literal(lit, _) => Ok(self.compile_literal(lit)),
            Expr::Var(name, _) => self.compile_var(name),
            Expr::Lambda { params, body, .. } => self.compile_lambda(params, body),
            Expr::App { func, args, .. } => self.compile_app(func, args),
            Expr::Let { name, value, body, .. } => self.compile_let(name, value, body),
            Expr::LetRec { name, params, value, body, .. } => {
                self.compile_let_rec(name, params, value, body)
            }
            Expr::If { cond, then_branch, else_branch, .. } => {
                self.compile_if(cond, then_branch, else_branch)
            }
            Expr::Match { scrutinee, arms, .. } => self.compile_match(scrutinee, arms),
            Expr::Block { exprs, .. } => self.compile_block(exprs),
            Expr::Tuple(elems, _) => self.compile_tuple(elems),
            Expr::Record(fields, _) => self.compile_record(fields),
            Expr::FieldAccess { expr, field, .. } => self.compile_field_access(expr, field),
            Expr::Array(elems, _) => self.compile_array(elems),
            Expr::Index { arr, idx, .. } => self.compile_index(arr, idx),
            Expr::Binary { op, left, right, .. } => self.compile_binary(*op, left, right),
            Expr::Unary { op, expr, .. } => self.compile_unary(*op, expr),
            Expr::Spawn { actor_type, init, .. } => self.compile_spawn(actor_type, init),
            Expr::Send { actor, behavior, args, .. } => self.compile_send(actor, behavior, args),
            Expr::Ask { actor, behavior, args, .. } => self.compile_ask(actor, behavior, args),
            Expr::SelfRef(_) => Ok(self.compile_self()),
            Expr::Perform { effect, op, args, .. } => self.compile_perform(effect, op, args),
            Expr::Handle { body, handlers, .. } => self.compile_handle(body, handlers),
            Expr::Assign { target, value, .. } => self.compile_assign(target, value),
            Expr::For { var, iterable, body, .. } => self.compile_for(var, iterable, body),
            Expr::Pipe { left, right, .. } => self.compile_pipe(left, right),
            Expr::Migrate { actor, node, .. } => self.compile_migrate(actor, node),
            Expr::Return(val, _) => self.compile_return(val),
            Expr::Break(_) => {
                let r = self.alloc_reg();
                self.emit(Instruction::new1(OpCode::Const0, r));
                Ok(r)
            }
            Expr::CapAnnotate { expr, .. } => self.compile_expr(expr),
            Expr::TypeAnnotate { expr, .. } => self.compile_expr(expr),
            Expr::Receive { .. } => {
                let r = self.alloc_reg();
                self.emit(Instruction::new1(OpCode::Receive, r));
                Ok(r)
            }
        }
    }

    fn compile_literal(&mut self, lit: &Literal) -> u8 {
        let dst = self.alloc_reg();
        match lit {
            Literal::Int(0) => { self.emit(Instruction::new1(OpCode::Const0, dst)); }
            Literal::Int(1) => { self.emit(Instruction::new1(OpCode::Const1, dst)); }
            Literal::Int(2) => { self.emit(Instruction::new1(OpCode::Const2, dst)); }
            Literal::Int(-1) => { self.emit(Instruction::new1(OpCode::ConstM1, dst)); }
            Literal::Int(n) => {
                let idx = self.add_const(Constant::Int(*n));
                self.emit(make_constu(idx as u16, dst));
            }
            Literal::Float(f) => {
                let idx = self.add_const(Constant::Float(*f));
                self.emit(make_constu(idx as u16, dst));
            }
            Literal::String(s) => {
                let idx = self.add_const(Constant::String(s.clone()));
                self.emit(make_constu(idx as u16, dst));
            }
            Literal::Bool(true) => {
                let idx = self.add_const(Constant::Bool(true));
                self.emit(make_constu(idx as u16, dst));
            }
            Literal::Bool(false) => {
                let idx = self.add_const(Constant::Bool(false));
                self.emit(make_constu(idx as u16, dst));
            }
            Literal::Unit => {
                let idx = self.add_const(Constant::Unit);
                self.emit(make_constu(idx as u16, dst));
            }
        };
        dst
    }

    fn compile_var(&mut self, name: &str) -> NuResult<u8> {
        if let Some(reg) = self.lookup_local(name) {
            let dst = self.alloc_reg();
            self.emit(Instruction::new2(OpCode::Load, reg, dst));
            Ok(dst)
        } else {
            // Free variable / global: emit as constant reference
            let dst = self.alloc_reg();
            let idx = self.add_const(Constant::String(name.to_string()));
            self.emit(make_constu(idx as u16, dst));
            Ok(dst)
        }
    }

    fn compile_lambda(&mut self, params: &[(String, Option<crate::types::Type>)], body: &Expr) -> NuResult<u8> {
        // Save current state
        let saved_locals = std::mem::replace(&mut self.locals, vec![ScopeFrame::new()]);
        let saved_next_reg = self.next_reg;
        self.next_reg = 0;

        // Allocate registers for parameters
        for (name, _) in params {
            let reg = self.alloc_reg();
            self.define_local(name, reg);
        }

        // Record start offset before compiling body
        let start_offset = self.module.current_offset();

        // Compile body
        let body_reg = self.compile_expr(body)?;
        self.emit(Instruction::new1(OpCode::RetVal, body_reg));

        // Restore state
        self.locals = saved_locals;
        self.next_reg = saved_next_reg;

        // Create closure instruction referencing the function start
        let dst = self.alloc_reg();
        let func_idx = self.module.function_table.len();
        self.module.function_table.push(start_offset);
        self.emit(Instruction::new3(OpCode::Closure,
            ((func_idx >> 8) & 0xFF) as u8,
            (func_idx & 0xFF) as u8,
            dst));
        Ok(dst)
    }

    fn compile_app(&mut self, func: &Expr, args: &[Expr]) -> NuResult<u8> {
        let saved_next_reg = self.next_reg;

        // Compile arguments, tracking which register each result lands in
        self.next_reg = 0;
        let mut arg_regs = Vec::new();
        for arg in args {
            let arg_reg = self.compile_expr(arg)?;
            arg_regs.push(arg_reg);
        }

        // Move args to consecutive registers r0, r1, r2, ...
        // (Call copies regs[0..argc] to the new frame)
        for (i, &arg_reg) in arg_regs.iter().enumerate() {
            if arg_reg != i as u8 {
                self.emit(Instruction::new2(OpCode::Move, arg_reg, i as u8));
            }
        }

        // Compile function reference past all args
        self.next_reg = args.len() as u8;
        let func_reg = if let Expr::Var(name, _) = func {
            if let Some(&func_idx) = self.func_map.get(name) {
                let fr = self.alloc_reg();
                let idx_const = self.add_const(Constant::Int(func_idx as i64));
                self.emit(make_constu(idx_const as u16, fr));
                fr
            } else {
                self.compile_expr(func)?
            }
        } else {
            self.compile_expr(func)?
        };

        let dst = self.alloc_reg();
        let argc = args.len().min(255) as u8;
        self.emit(Instruction::new3(OpCode::Call, func_reg, argc, dst));
        // Restore next_reg, but ensure dst is reserved for the caller
        self.next_reg = saved_next_reg.max(dst + 1);
        Ok(dst)
    }

    fn compile_let(&mut self, name: &str, value: &Expr, body: &Expr) -> NuResult<u8> {
        let val_reg = self.compile_expr(value)?;
        self.define_local(name, val_reg);
        self.compile_expr(body)
    }

    fn compile_let_rec(&mut self, name: &str, _params: &[(String, Option<crate::types::Type>)], value: &Expr, body: &Expr) -> NuResult<u8> {
        let rec_reg = self.alloc_reg();
        self.define_local(name, rec_reg);
        let val_reg = self.compile_expr(value)?;
        // Update the binding
        if let Some(frame) = self.locals.last_mut() {
            for (n, r) in frame.bindings.iter_mut().rev() {
                if n == name {
                    *r = val_reg;
                    break;
                }
            }
        }
        self.compile_expr(body)
    }

    fn compile_if(&mut self, cond: &Expr, then_b: &Expr, else_b: &Option<Box<Expr>>) -> NuResult<u8> {
        let cond_reg = self.compile_expr(cond)?;

        // Emit JmpF with condition register in op1, placeholder offset in op2/op3.
        // VM reads condition from instr.op1 and offset from instr.op2+op3 via offset16().
        let jmpf_idx = self.emit(Instruction::new3(OpCode::JmpF, cond_reg, 0, 0));

        // Compile then branch
        let then_reg = self.compile_expr(then_b)?;
        let result_reg = self.alloc_reg();
        if then_reg != result_reg {
            self.emit(Instruction::new2(OpCode::Move, then_reg, result_reg));
        }

        // Emit Jmp to skip else branch (placeholder offset)
        let jmp_idx = self.emit(Instruction::new2(OpCode::Jmp, 0, 0));

        // --- Start of else branch ---
        let else_start = self.current_offset();

        // Patch JmpF: set op2/op3 to the full 16-bit forward offset.
        // VM reads offset as (op2 << 8) | op3 (signed 16-bit)
        // offset = else_start - jmpf_idx (VM computes: pc = (pc+1) + offset - 1 = pc + offset)
        let jmpf_offset = else_start as i16 - jmpf_idx as i16;
        if let Some(instr) = self.module.instructions.get_mut(jmpf_idx) {
            instr.op2 = ((jmpf_offset >> 8) & 0xFF) as u8;
            instr.op3 = (jmpf_offset & 0xFF) as u8;
        }

        if let Some(else_expr) = else_b {
            let else_reg = self.compile_expr(else_expr)?;
            if else_reg != result_reg {
                self.emit(Instruction::new2(OpCode::Move, else_reg, result_reg));
            }
        } else {
            self.emit(Instruction::new1(OpCode::Const0, result_reg));
        }

        // --- End of if-else ---
        let end = self.current_offset();

        // Patch Jmp (unconditional): full 16-bit offset in op1+op2
        let jmp_offset = end as i16 - jmp_idx as i16;
        if let Some(instr) = self.module.instructions.get_mut(jmp_idx) {
            instr.op1 = ((jmp_offset as u16) >> 8) as u8;
            instr.op2 = ((jmp_offset as u16) & 0xFF) as u8;
        }

        Ok(result_reg)
    }

    fn compile_match(&mut self, scrut: &Expr, arms: &[(Pattern, Expr)]) -> NuResult<u8> {
        let scrut_reg = self.compile_expr(scrut)?;
        let result_reg = self.alloc_reg();
        let mut jump_fixups: Vec<usize> = Vec::new();

        for (i, (pattern, arm_expr)) in arms.iter().enumerate() {
            let is_last = i == arms.len() - 1;

            if is_last {
                self.compile_pattern_bindings(pattern, scrut_reg)?;
                let arm_reg = self.compile_expr(arm_expr)?;
                if arm_reg != result_reg {
                    self.emit(Instruction::new2(OpCode::Move, arm_reg, result_reg));
                }
            } else {
                let test_reg = self.compile_pattern_test(pattern, scrut_reg)?;

                // JmpF reads condition from op1 and offset from op2+op3.
                let jmpf_idx = self.emit(Instruction::new3(OpCode::JmpF, test_reg, 0, 0));

                self.compile_pattern_bindings(pattern, scrut_reg)?;
                let arm_reg = self.compile_expr(arm_expr)?;
                if arm_reg != result_reg {
                    self.emit(Instruction::new2(OpCode::Move, arm_reg, result_reg));
                }

                let jmp_idx = self.emit(Instruction::new2(OpCode::Jmp, 0, 0));
                jump_fixups.push(jmp_idx);

                // Patch JmpF: offset = next_arm_start - jmpf_idx
                let next_start = self.current_offset() as i16;
                let jmpf_off = next_start - jmpf_idx as i16;
                if let Some(instr) = self.module.instructions.get_mut(jmpf_idx) {
                    instr.op2 = ((jmpf_off >> 8) & 0xFF) as u8;
                    instr.op3 = (jmpf_off & 0xFF) as u8;
                }
            }
        }

        // Patch all end jumps
        let end = self.current_offset() as i16;
        for jmp_idx in jump_fixups {
            let jmp_off = end - jmp_idx as i16;
            if let Some(instr) = self.module.instructions.get_mut(jmp_idx) {
                instr.op1 = ((jmp_off as u16) >> 8) as u8;
                instr.op2 = ((jmp_off as u16) & 0xFF) as u8;
            }
        }

        Ok(result_reg)
    }

    fn compile_pattern_test(&mut self, pattern: &Pattern, scrut_reg: u8) -> NuResult<u8> {
        let dst = self.alloc_reg();
        match pattern {
            Pattern::Wild | Pattern::Var(_) => {
                let idx = self.add_const(Constant::Bool(true));
                self.emit(make_constu(idx as u16, dst));
            }
            Pattern::Lit(lit) => {
                let lit_reg = self.compile_literal(lit);
                self.emit(Instruction::new3(OpCode::ICmpEq, scrut_reg, lit_reg, dst));
            }
            Pattern::Tuple(pats) => {
                let idx = self.add_const(Constant::Bool(!pats.is_empty()));
                self.emit(make_constu(idx as u16, dst));
            }
            Pattern::Record(fields) => {
                let idx = self.add_const(Constant::Bool(!fields.is_empty()));
                self.emit(make_constu(idx as u16, dst));
            }
            Pattern::Variant(tag, _) => {
                let tag_idx = self.add_const(Constant::String(tag.clone()));
                let tag_reg = self.alloc_reg();
                self.emit(make_constu(tag_idx as u16, tag_reg));
                self.emit(Instruction::new3(OpCode::SCmpEq, scrut_reg, tag_reg, dst));
            }
            Pattern::Alias(_, inner) => {
                return self.compile_pattern_test(inner, scrut_reg);
            }
        }
        Ok(dst)
    }

    fn compile_pattern_bindings(&mut self, pattern: &Pattern, scrut_reg: u8) -> NuResult<()> {
        match pattern {
            Pattern::Wild => {}
            Pattern::Var(name) => { self.define_local(name, scrut_reg); }
            Pattern::Lit(_) => {}
            Pattern::Tuple(_) => {}
            Pattern::Record(_) => {}
            Pattern::Variant(_tag, Some(inner)) => {
                self.compile_pattern_bindings(inner, scrut_reg)?;
            }
            Pattern::Variant(_tag, None) => {}
            Pattern::Alias(name, inner) => {
                self.define_local(name, scrut_reg);
                self.compile_pattern_bindings(inner, scrut_reg)?;
            }
        }
        Ok(())
    }

    fn compile_block(&mut self, exprs: &[Expr]) -> NuResult<u8> {
        if exprs.is_empty() {
            let r = self.alloc_reg();
            self.emit(Instruction::new1(OpCode::Const0, r));
            return Ok(r);
        }
        let mut last_reg = 0;
        for (i, expr) in exprs.iter().enumerate() {
            let r = self.compile_expr(expr)?;
            if i < exprs.len() - 1 {
                self.free_reg(r);
            }
            last_reg = r;
        }
        Ok(last_reg)
    }

    fn compile_binary(&mut self, op: BinOp, left: &Expr, right: &Expr) -> NuResult<u8> {
        let r1 = self.compile_expr(left)?;
        // Save left result to a fresh register so the right expr can't overwrite it.
        // This is essential when both sub-expressions are function calls that may
        // use the same internal destination register (e.g., both use r2 for Call dst).
        let r1_save = self.alloc_reg();
        if r1 != r1_save {
            self.emit(Instruction::new2(OpCode::Move, r1, r1_save));
        }
        let r2 = self.compile_expr(right)?;
        let dst = self.alloc_reg();

        match op {
            BinOp::Add => { self.emit(Instruction::new3(OpCode::IAdd, r1_save, r2, dst)); }
            BinOp::Sub => { self.emit(Instruction::new3(OpCode::ISub, r1_save, r2, dst)); }
            BinOp::Mul => { self.emit(Instruction::new3(OpCode::IMul, r1_save, r2, dst)); }
            BinOp::Div => { self.emit(Instruction::new3(OpCode::IDiv, r1_save, r2, dst)); }
            BinOp::Mod => { self.emit(Instruction::new3(OpCode::IMod, r1_save, r2, dst)); }
            BinOp::Eq => { self.emit(Instruction::new3(OpCode::ICmpEq, r1_save, r2, dst)); }
            BinOp::Ne => {
                let tmp = self.alloc_reg();
                self.emit(Instruction::new3(OpCode::ICmpEq, r1_save, r2, tmp));
                self.emit(Instruction::new2(OpCode::Not, tmp, dst));
            }
            BinOp::Lt => { self.emit(Instruction::new3(OpCode::ICmpLt, r1_save, r2, dst)); }
            BinOp::Le => { self.emit(Instruction::new3(OpCode::ICmpLe, r1_save, r2, dst)); }
            BinOp::Gt => { self.emit(Instruction::new3(OpCode::ICmpGt, r1_save, r2, dst)); }
            BinOp::Ge => { self.emit(Instruction::new3(OpCode::ICmpGe, r1_save, r2, dst)); }
            BinOp::And => { self.emit(Instruction::new3(OpCode::And, r1_save, r2, dst)); }
            BinOp::Or => { self.emit(Instruction::new3(OpCode::Or, r1_save, r2, dst)); }
            BinOp::BitAnd => { self.emit(Instruction::new3(OpCode::And, r1_save, r2, dst)); }
            BinOp::BitOr => { self.emit(Instruction::new3(OpCode::Or, r1_save, r2, dst)); }
            BinOp::BitXor => { self.emit(Instruction::new3(OpCode::And, r1_save, r2, dst)); }
            BinOp::Shl => { self.emit(Instruction::new3(OpCode::And, r1_save, r2, dst)); }
            BinOp::Shr => { self.emit(Instruction::new3(OpCode::And, r1_save, r2, dst)); }
            BinOp::Assign => {
                self.emit(Instruction::new2(OpCode::Store, r2, r1_save));
                self.emit(Instruction::new2(OpCode::Move, r2, dst));
            }
            BinOp::Pipe => {
                self.emit(Instruction::new2(OpCode::Move, r1_save, dst));
            }
        }
        Ok(dst)
    }

    fn compile_unary(&mut self, op: UnOp, expr: &Expr) -> NuResult<u8> {
        let src = self.compile_expr(expr)?;
        let dst = self.alloc_reg();
        match op {
            UnOp::Neg => { self.emit(Instruction::new2(OpCode::INeg, src, dst)); }
            UnOp::Not => { self.emit(Instruction::new2(OpCode::Not, src, dst)); }
            UnOp::Deref => {
                self.emit(Instruction::new2(OpCode::Load, src, dst));
            }
            UnOp::Ref(_) => {
                self.emit(Instruction::new2(OpCode::Move, src, dst));
            }
        }
        Ok(dst)
    }

    fn compile_tuple(&mut self, elems: &[Expr]) -> NuResult<u8> {
        let mut elem_regs = Vec::with_capacity(elems.len());
        for elem in elems {
            let r = self.compile_expr(elem)?;
            elem_regs.push(r);
        }
        let dst = self.alloc_reg();
        self.emit(Instruction::new2(OpCode::TupleMk, elems.len() as u8, dst));
        for (i, reg) in elem_regs.iter().enumerate() {
            self.emit(Instruction::new3(OpCode::FieldS, dst, i as u8, *reg));
        }
        Ok(dst)
    }

    fn compile_record(&mut self, fields: &[(String, Expr)]) -> NuResult<u8> {
        let mut field_regs = Vec::with_capacity(fields.len());
        for (_name, expr) in fields {
            let r = self.compile_expr(expr)?;
            field_regs.push(r);
        }
        let dst = self.alloc_reg();
        self.emit(Instruction::new2(OpCode::RecMk, fields.len() as u8, dst));
        // Register field names for later lookup
        for (i, (name, _)) in fields.iter().enumerate() {
            if !self.field_map.contains_key(name) {
                let id = self.next_field_id;
                self.next_field_id = self.next_field_id.saturating_add(1);
                self.field_map.insert(name.clone(), id);
            }
            self.emit(Instruction::new3(OpCode::RecS, dst, i as u8, field_regs[i]));
        }
        Ok(dst)
    }

    fn compile_field_access(&mut self, expr: &Expr, field: &str) -> NuResult<u8> {
        let obj_reg = self.compile_expr(expr)?;
        let dst = self.alloc_reg();
        // Use RecL with field name constant pool index
        let field_idx = self.add_const(Constant::String(field.to_string()));
        self.emit(Instruction::new3(OpCode::RecL, obj_reg,
            ((field_idx >> 8) & 0xFF) as u8,
            (field_idx & 0xFF) as u8));
        // The VM is expected to place the result in a designated register;
        // for now we assume dst will contain the result via convention.
        // Emit a move from obj_reg to dst as placeholder since RecL's
        // exact encoding is VM-dependent.
        self.emit(Instruction::new2(OpCode::Move, obj_reg, dst));
        Ok(dst)
    }

    fn compile_spawn(&mut self, _actor_type: &Expr, init: &[(String, Expr)]) -> NuResult<u8> {
        for (_name, expr) in init {
            let _r = self.compile_expr(expr)?;
        }
        let behavior_idx = self.module.behaviors.len();
        let dst = self.alloc_reg();
        self.emit(Instruction::new3(OpCode::Spawn,
            ((behavior_idx >> 8) & 0xFF) as u8,
            (behavior_idx & 0xFF) as u8,
            dst));
        Ok(dst)
    }

    fn compile_send(&mut self, actor: &Expr, behavior: &str, args: &[Expr]) -> NuResult<u8> {
        let addr_reg = self.compile_expr(actor)?;
        for arg in args {
            let _r = self.compile_expr(arg)?;
        }
        let behavior_idx = self.add_const(Constant::String(behavior.to_string()));
        let dst = self.alloc_reg();
        self.emit(Instruction::new3(OpCode::Send, addr_reg,
            ((behavior_idx >> 8) & 0xFF) as u8,
            (behavior_idx & 0xFF) as u8));
        self.emit(Instruction::new1(OpCode::Const0, dst));
        Ok(dst)
    }

    fn compile_ask(&mut self, actor: &Expr, behavior: &str, args: &[Expr]) -> NuResult<u8> {
        let addr_reg = self.compile_expr(actor)?;
        for arg in args {
            let _r = self.compile_expr(arg)?;
        }
        let behavior_idx = self.add_const(Constant::String(behavior.to_string()));
        let dst = self.alloc_reg();
        self.emit(Instruction::new3(OpCode::Ask, addr_reg,
            ((behavior_idx >> 8) & 0xFF) as u8,
            (behavior_idx & 0xFF) as u8));
        self.emit(Instruction::new2(OpCode::Move, addr_reg, dst));
        Ok(dst)
    }

    fn compile_self(&mut self) -> u8 {
        let dst = self.alloc_reg();
        self.emit(Instruction::new1(op_self(), dst));
        dst
    }

    fn compile_perform(&mut self, effect: &str, op: &str, args: &[Expr]) -> NuResult<u8> {
        for arg in args {
            let _r = self.compile_expr(arg)?;
        }
        let eff_idx = self.add_const(Constant::String(effect.to_string()));
        let _op_idx = self.add_const(Constant::String(op.to_string()));
        let dst = self.alloc_reg();
        self.emit(Instruction::new3(OpCode::Perform,
            ((eff_idx >> 8) & 0xFF) as u8,
            (eff_idx & 0xFF) as u8,
            dst));
        let _ = _op_idx;
        Ok(dst)
    }

    fn compile_handle(&mut self, body: &Expr, _handlers: &[EffectHandler]) -> NuResult<u8> {
        let handler_idx = self.module.behaviors.len();
        self.emit(Instruction::new1(OpCode::Handle, handler_idx as u8));
        let body_reg = self.compile_expr(body)?;
        self.emit(Instruction::new0(OpCode::Unwind));
        Ok(body_reg)
    }

    fn compile_assign(&mut self, target: &Expr, value: &Expr) -> NuResult<u8> {
        let val_reg = self.compile_expr(value)?;
        match target {
            Expr::Var(name, _) => {
                if let Some(reg) = self.lookup_local(name) {
                    self.emit(Instruction::new2(OpCode::Move, val_reg, reg));
                    Ok(reg)
                } else {
                    let name_idx = self.add_const(Constant::String(name.to_string()));
                    let name_reg = self.alloc_reg();
                    self.emit(make_constu(name_idx as u16, name_reg));
                    self.emit(Instruction::new2(OpCode::Store, val_reg, name_reg));
                    Ok(val_reg)
                }
            }
            _ => {
                let target_reg = self.compile_expr(target)?;
                self.emit(Instruction::new2(OpCode::Store, val_reg, target_reg));
                Ok(val_reg)
            }
        }
    }

    fn compile_for(&mut self, var: &str, iterable: &Expr, body: &Expr) -> NuResult<u8> {
        let iter_reg = self.compile_expr(iterable)?;
        let len_reg = self.alloc_reg();
        self.emit(Instruction::new2(OpCode::ArrLen, iter_reg, len_reg));
        let idx_reg = self.alloc_reg();
        self.emit(Instruction::new1(OpCode::Const0, idx_reg));
        let loop_start = self.current_offset();
        let cond_reg = self.alloc_reg();
        self.emit(Instruction::new3(OpCode::ICmpLt, idx_reg, len_reg, cond_reg));

        // JmpF reads condition from op1 and offset from op2+op3.
        let jmpf_idx = self.emit(Instruction::new3(OpCode::JmpF, cond_reg, 0, 0));

        let elem_reg = self.alloc_reg();
        self.emit(Instruction::new3(OpCode::ArrLoad, iter_reg, idx_reg, elem_reg));
        self.push_scope();
        self.define_local(var, elem_reg);
        let _body_reg = self.compile_expr(body)?;
        self.pop_scope();
        self.emit(Instruction::new1(OpCode::IInc, idx_reg));
        let jmp_idx = self.current_offset();
        let loop_back_offset = loop_start as i16 - jmp_idx as i16;
        self.emit(Instruction::new2(OpCode::Jmp,
            ((loop_back_offset as u16 >> 8) & 0xFF) as u8,
            (loop_back_offset as u16 & 0xFF) as u8));

        // Patch JmpF: offset = loop_end - jmpf_idx
        let loop_end_offset = self.current_offset() as i16 - jmpf_idx as i16;
        if let Some(instr) = self.module.instructions.get_mut(jmpf_idx) {
            instr.op2 = ((loop_end_offset >> 8) & 0xFF) as u8;
            instr.op3 = (loop_end_offset & 0xFF) as u8;
        }
        let result_reg = self.alloc_reg();
        self.emit(Instruction::new1(OpCode::Const0, result_reg));
        Ok(result_reg)
    }

    fn compile_array(&mut self, elems: &[Expr]) -> NuResult<u8> {
        let len_reg = self.alloc_reg();
        let len_idx = self.add_const(Constant::Int(elems.len() as i64));
        self.emit(make_constu(len_idx as u16, len_reg));
        let arr_reg = self.alloc_reg();
        self.emit(Instruction::new2(OpCode::ArrAlloc, len_reg, arr_reg));
        for (i, elem) in elems.iter().enumerate() {
            let elem_reg = self.compile_expr(elem)?;
            let idx_reg = self.alloc_reg();
            let idx_idx = self.add_const(Constant::Int(i as i64));
            self.emit(make_constu(idx_idx as u16, idx_reg));
            self.emit(Instruction::new3(OpCode::ArrStore, arr_reg, idx_reg, elem_reg));
        }
        Ok(arr_reg)
    }

    fn compile_index(&mut self, arr: &Expr, idx: &Expr) -> NuResult<u8> {
        let arr_reg = self.compile_expr(arr)?;
        let idx_reg = self.compile_expr(idx)?;
        let dst = self.alloc_reg();
        self.emit(Instruction::new3(OpCode::ArrLoad, arr_reg, idx_reg, dst));
        Ok(dst)
    }

    fn compile_pipe(&mut self, left: &Expr, right: &Expr) -> NuResult<u8> {
        let left_reg = self.compile_expr(left)?;
        match right {
            Expr::App { func, args, .. } => {
                let func_reg = self.compile_expr(func)?;
                for arg in args {
                    let _r = self.compile_expr(arg)?;
                }
                let dst = self.alloc_reg();
                self.emit(Instruction::new3(OpCode::Call, func_reg, (args.len() + 1) as u8, dst));
                Ok(dst)
            }
            Expr::Var(name, _) => {
                let func_reg = self.compile_var(name)?;
                let dst = self.alloc_reg();
                self.emit(Instruction::new3(OpCode::Call, func_reg, 1, dst));
                Ok(dst)
            }
            _ => {
                let func_reg = self.compile_expr(right)?;
                let dst = self.alloc_reg();
                self.emit(Instruction::new3(OpCode::Call, func_reg, 1, dst));
                Ok(dst)
            }
        }
    }

    fn compile_migrate(&mut self, actor: &Expr, node: &Expr) -> NuResult<u8> {
        let actor_reg = self.compile_expr(actor)?;
        let node_reg = self.compile_expr(node)?;
        let dst = self.alloc_reg();
        self.emit(Instruction::new3(OpCode::Migrate, actor_reg, node_reg, dst));
        Ok(dst)
    }

    fn compile_return(&mut self, val: &Option<Box<Expr>>) -> NuResult<u8> {
        match val {
            Some(expr) => {
                let r = self.compile_expr(expr)?;
                self.emit(Instruction::new1(OpCode::RetVal, r));
                Ok(r)
            }
            None => {
                let r = self.alloc_reg();
                self.emit(Instruction::new1(OpCode::Const0, r));
                self.emit(Instruction::new1(OpCode::RetVal, r));
                Ok(r)
            }
        }
    }

    // ========================================================================
    // Declaration compilation
    // ========================================================================

    fn compile_decl(&mut self, decl: &Decl) -> NuResult<()> {
        match decl {
            Decl::Function { name, body, .. } if name == "__main" => {
                // __main is compiled INLINE (not as a function) to avoid
                // function table index issues. Body executes at module level.
                // Record entry point so VM knows where to start.
                self.module.entry_point = Some(self.module.current_offset());
                let body_reg = self.compile_expr(body)?;
                // VM::run() returns regs[0], so move result there
                if body_reg != 0 {
                    self.emit(Instruction::new2(OpCode::Move, body_reg, 0));
                }
                self.emit(Instruction::new1(OpCode::Print, 0));
                self.emit(Instruction::new0(OpCode::Halt));
            }
            Decl::Function { name, params, body, .. } => {
                self.compile_function(name, params, body)?;
            }
            Decl::Actor { name, behaviors, init, .. } => {
                self.compile_actor(name, behaviors, init)?;
            }
            Decl::Agent { name, state_fields, behaviors, observe, .. } => {
                self.compile_agent(name, state_fields, behaviors, observe)?;
            }
            Decl::TypeAlias { .. }
            | Decl::RecordType { .. }
            | Decl::VariantType { .. }
            | Decl::EffectDecl { .. }
            | Decl::Import { .. } => {}
            Decl::Module { decls, .. } => {
                for subdecl in decls {
                    self.compile_decl(subdecl)?;
                }
            }
        }
        Ok(())
    }

    fn compile_function(&mut self, name: &str, params: &[(String, Option<crate::types::Type>)], body: &Expr) -> NuResult<()> {
        let saved_locals = std::mem::replace(&mut self.locals, vec![ScopeFrame::new()]);
        let saved_next_reg = self.next_reg;
        self.next_reg = 0;

        // Function table slot and func_map entry were already created
        // during collect_functions() for forward reference support.
        let func_idx = *self.func_map.get(name)
            .expect("Function not in func_map (collect_functions missed it)");

        for (param_name, _) in params {
            let reg = self.alloc_reg();
            self.define_local(param_name, reg);
        }

        // Record start offset BEFORE param saves — they are part of the function body.
        let start_offset = self.module.current_offset();

        // Save parameters to a high register safe zone so recursive calls
        // (which use r0, r1, ... for args and temporaries) don't overwrite them.
        const PARAM_SAVE_BASE: u8 = 16;
        for (i, (param_name, _)) in params.iter().enumerate() {
            let old_reg = i as u8;
            let new_reg = PARAM_SAVE_BASE + i as u8;
            self.emit(Instruction::new2(OpCode::Move, old_reg, new_reg));
            // Update binding to point to the saved register
            if let Some(frame) = self.locals.last_mut() {
                for (n, r) in frame.bindings.iter_mut() {
                    if n == param_name {
                        *r = new_reg;
                        break;
                    }
                }
            }
        }
        self.next_reg = PARAM_SAVE_BASE + params.len() as u8;
        let body_reg = self.compile_expr(body)?;
        self.emit(Instruction::new1(OpCode::RetVal, body_reg));

        // Patch the function table with the actual code offset
        self.module.function_table[func_idx] = start_offset;

        let const_idx = self.add_const(Constant::FunctionRef(func_idx));
        self.module.exports.push((name.to_string(), const_idx));

        self.locals = saved_locals;
        self.next_reg = saved_next_reg;
        Ok(())
    }

    fn compile_actor(&mut self, name: &str, behaviors: &[Behavior], init: &[(String, Expr)]) -> NuResult<()> {
        for (_field_name, expr) in init {
            let _ = self.compile_expr(expr)?;
        }
        for behavior in behaviors {
            self.compile_behavior(behavior, name)?;
        }
        Ok(())
    }

    fn compile_agent(&mut self, name: &str, state_fields: &[(String, crate::types::Type, Expr)], behaviors: &[Behavior], observe: &Expr) -> NuResult<()> {
        for (_field_name, _ty, expr) in state_fields {
            let _ = self.compile_expr(expr)?;
        }
        for behavior in behaviors {
            self.compile_behavior(behavior, name)?;
        }
        let _ = self.compile_expr(observe)?;
        Ok(())
    }

    fn compile_behavior(&mut self, b: &Behavior, actor_name: &str) -> NuResult<()> {
        let saved_locals = std::mem::replace(&mut self.locals, vec![ScopeFrame::new()]);
        let saved_next_reg = self.next_reg;
        self.next_reg = 0;

        for (param_name, _) in &b.params {
            let reg = self.alloc_reg();
            self.define_local(param_name, reg);
        }

        let self_reg = self.alloc_reg();
        self.emit(Instruction::new1(op_self(), self_reg));
        self.define_local("self", self_reg);

        let start_offset = self.module.current_offset();
        let body_reg = self.compile_expr(&b.body)?;
        self.emit(Instruction::new1(OpCode::RetVal, body_reg));

        let entry = BehaviorTableEntry {
            name: format!("{}.{}", actor_name, b.name),
            param_count: b.params.len(),
            code_offset: start_offset,
            local_count: self.next_reg as usize,
            effect_mask: 0,
        };
        self.module.add_behavior(entry);

        self.locals = saved_locals;
        self.next_reg = saved_next_reg;
        Ok(())
    }

    // ========================================================================
    // Helpers
    // ========================================================================

    fn alloc_reg(&mut self) -> u8 {
        let reg = self.next_reg;
        self.next_reg = self.next_reg.saturating_add(1);
        reg
    }

    fn free_reg(&mut self, _reg: u8) {}

    fn lookup_local(&self, name: &str) -> Option<u8> {
        for frame in self.locals.iter().rev() {
            if let Some(reg) = frame.get(name) {
                return Some(reg);
            }
        }
        None
    }

    fn define_local(&mut self, name: &str, reg: u8) {
        if let Some(frame) = self.locals.last_mut() {
            frame.insert(name.to_string(), reg);
        }
    }

    fn push_scope(&mut self) { self.locals.push(ScopeFrame::new()); }
    fn pop_scope(&mut self) { self.locals.pop(); }
    fn emit(&mut self, instr: Instruction) -> usize { self.module.emit(instr) }
    fn add_const(&mut self, c: Constant) -> usize { self.module.add_constant(c) }
    fn fresh_label(&mut self) -> usize { self.label_counter += 1; self.label_counter }
    fn current_offset(&self) -> usize { self.module.current_offset() }
    fn patch_jump(&mut self, instr_idx: usize, target: i16) { self.module.patch_jump(instr_idx, target); }

    // ===================================================================
    // Python Interop Bytecode Emission
    // ===================================================================

    /// Emit a Python import instruction.
    ///
    /// Loads a Python module by name (from the constant pool) and stores
    /// the resulting Python object reference in `dst_reg`.
    ///
    /// Encoding: op1=module_name_const_idx (u8), op2=dst_reg
    pub fn emit_py_import(&mut self, module_name_const_idx: u16, dst_reg: u8) {
        self.emit(Instruction::new2(OpCode::PyImport, (module_name_const_idx & 0xFF) as u8, dst_reg));
    }

    /// Emit a Python getattr instruction.
    ///
    /// Retrieves an attribute from a Python object and stores the result.
    ///
    /// Encoding: op1=obj_reg, op2=attr_name_const_idx (u8), op3=dst_reg
    pub fn emit_py_getattr(&mut self, obj_reg: u8, attr_name_const_idx: u16, dst_reg: u8) {
        self.emit(Instruction::new3(OpCode::PyGetAttr, obj_reg, (attr_name_const_idx & 0xFF) as u8, dst_reg));
    }

    /// Emit a Python call instruction.
    ///
    /// Calls a Python callable with `arg_count` positional arguments.
    /// Arguments are read from consecutive registers starting at callable_reg + 1.
    ///
    /// Encoding: op1=callable_reg, op2=arg_count, op3=dst_reg
    pub fn emit_py_call(&mut self, callable_reg: u8, arg_count: u8, dst_reg: u8) {
        self.emit(Instruction::new3(OpCode::PyCall, callable_reg, arg_count, dst_reg));
    }

    /// Emit a Python call-with-kwargs instruction.
    ///
    /// MVP: Delegates to a regular call (kwargs are ignored).
    ///
    /// Encoding: op1=callable_reg, op2=args_tuple_reg, op3=kwargs_dict_reg
    pub fn emit_py_call_kw(&mut self, callable_reg: u8, args_tuple_reg: u8, kwargs_dict_reg: u8) {
        self.emit(Instruction::new3(OpCode::PyCallKw, callable_reg, args_tuple_reg, kwargs_dict_reg));
    }

    /// Emit a Python setattr instruction.
    ///
    /// Sets an attribute on a Python object.
    ///
    /// Encoding: op1=obj_reg, op2=attr_name_const_idx (u8), op3=val_reg
    pub fn emit_py_setattr(&mut self, obj_reg: u8, attr_name_const_idx: u16, val_reg: u8) {
        self.emit(Instruction::new3(OpCode::PySetAttr, obj_reg, (attr_name_const_idx & 0xFF) as u8, val_reg));
    }

    /// Emit a Python-to-Nulang conversion instruction.
    ///
    /// Converts a Python object to a native Nulang Value.
    ///
    /// Encoding: op1=py_val_reg, op2=dst_reg
    pub fn emit_py_to_nu(&mut self, py_val_reg: u8, dst_reg: u8) {
        self.emit(Instruction::new2(OpCode::PyToNu, py_val_reg, dst_reg));
    }

    /// Emit a Nulang-to-Python conversion instruction.
    ///
    /// Converts a native Nulang Value to a Python object reference.
    ///
    /// Encoding: op1=nu_val_reg, op2=dst_reg
    pub fn emit_py_from_nu(&mut self, nu_val_reg: u8, dst_reg: u8) {
        self.emit(Instruction::new2(OpCode::PyFromNu, nu_val_reg, dst_reg));
    }

    /// Emit a Python release instruction.
    ///
    /// Releases a reference to a Python object (MVP: no-op).
    ///
    /// Encoding: op1=py_val_reg
    pub fn emit_py_release(&mut self, py_val_reg: u8) {
        self.emit(Instruction::new1(OpCode::PyRelease, py_val_reg));
    }
}

// Helper: create a ConstU instruction with constant pool index and destination register.
fn make_constu(idx: u16, dst: u8) -> Instruction {
    Instruction::new3(OpCode::ConstU, ((idx >> 8) & 0xFF) as u8, (idx & 0xFF) as u8, dst)
}
