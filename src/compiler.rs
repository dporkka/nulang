//! AST -> Bytecode compiler.
//!
//! Compiles Nulang AST into bytecode modules for the VM.

use crate::ast::*;
use crate::bytecode::*;
use crate::types::{NuResult, Span};

/// Workaround for the `Self` opcode (0x83) which conflicts with the Rust keyword.
/// Uses transmute from the known discriminant value.
fn op_self() -> OpCode {
    // Safety: 0x83 is the guaranteed discriminant for the `Self` variant.
    unsafe { std::mem::transmute::<u8, OpCode>(0x83) }
}

/// Collect all variable names bound by a pattern.
fn pattern_bindings(pat: &Pattern, out: &mut std::collections::HashSet<String>) {
    match pat {
        Pattern::Wild | Pattern::Lit(_) => {}
        Pattern::Var(name) | Pattern::Alias(name, _) => { out.insert(name.clone()); }
        Pattern::Tuple(pats) => {
            for p in pats { pattern_bindings(p, out); }
        }
        Pattern::Record(fields) => {
            for (_, p) in fields { pattern_bindings(p, out); }
        }
        Pattern::Variant(_, Some(inner)) => pattern_bindings(inner, out),
        Pattern::Variant(_, None) => {}
    }
}

/// Accumulate free variable names in `expr` that are not in `bound`.
fn free_vars(expr: &Expr, bound: &std::collections::HashSet<String>, acc: &mut std::collections::HashSet<String>) {
    match expr {
        Expr::Var(name, _) => {
            if !bound.contains(name) {
                acc.insert(name.clone());
            }
        }
        Expr::Lambda { params, body, .. } => {
            let mut new_bound = bound.clone();
            for (p, _) in params { new_bound.insert(p.clone()); }
            free_vars(body, &new_bound, acc);
        }
        Expr::App { func, args, .. } => {
            free_vars(func, bound, acc);
            for a in args { free_vars(a, bound, acc); }
        }
        Expr::Let { name, value, body, .. } => {
            free_vars(value, bound, acc);
            let mut new_bound = bound.clone();
            new_bound.insert(name.clone());
            free_vars(body, &new_bound, acc);
        }
        Expr::LetRec { name, params, value, body, .. } => {
            let mut value_bound = bound.clone();
            value_bound.insert(name.clone());
            for (p, _) in params { value_bound.insert(p.clone()); }
            free_vars(value, &value_bound, acc);
            let mut body_bound = bound.clone();
            body_bound.insert(name.clone());
            free_vars(body, &body_bound, acc);
        }
        Expr::If { cond, then_branch, else_branch, .. } => {
            free_vars(cond, bound, acc);
            free_vars(then_branch, bound, acc);
            if let Some(e) = else_branch { free_vars(e, bound, acc); }
        }
        Expr::Match { scrutinee, arms, .. } => {
            free_vars(scrutinee, bound, acc);
            for (pat, arm_expr) in arms {
                let mut arm_bound = bound.clone();
                pattern_bindings(pat, &mut arm_bound);
                free_vars(arm_expr, &arm_bound, acc);
            }
        }
        Expr::Block { exprs, .. } | Expr::Tuple(exprs, _) | Expr::Array(exprs, _) => {
            for e in exprs { free_vars(e, bound, acc); }
        }
        Expr::Record(fields, _) => {
            for (_, e) in fields { free_vars(e, bound, acc); }
        }
        Expr::FieldAccess { expr, .. } => free_vars(expr, bound, acc),
        Expr::Index { arr, idx, .. } => {
            free_vars(arr, bound, acc);
            free_vars(idx, bound, acc);
        }
        Expr::Binary { left, right, .. } => {
            free_vars(left, bound, acc);
            free_vars(right, bound, acc);
        }
        Expr::Unary { expr, .. } => free_vars(expr, bound, acc),
        Expr::Pipe { left, right, .. } => {
            free_vars(left, bound, acc);
            free_vars(right, bound, acc);
        }
        Expr::Assign { target, value, .. } => {
            free_vars(target, bound, acc);
            free_vars(value, bound, acc);
        }
        Expr::For { var, iterable, body, .. } => {
            free_vars(iterable, bound, acc);
            let mut new_bound = bound.clone();
            new_bound.insert(var.clone());
            free_vars(body, &new_bound, acc);
        }
        Expr::Return(e, _) => {
            if let Some(e) = e { free_vars(e, bound, acc); }
        }
        Expr::TypeAnnotate { expr, .. } | Expr::CapAnnotate { expr, .. } => free_vars(expr, bound, acc),
        _ => {}
    }
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
    /// Dedicated high-register allocator for let-bound values, so closures
    /// and other multi-use bindings survive argument/dst register churn.
    binding_reg: u8,
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
            binding_reg: 240,
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
        // Compute free variables before changing scope state.
        let param_set: std::collections::HashSet<String> = params.iter().map(|(n, _)| n.clone()).collect();
        let mut free = std::collections::HashSet::new();
        free_vars(body, &param_set, &mut free);
        // Only capture variables that are live in the enclosing scope. Top-level
        // function references are resolved via func_map and do not need captures.
        let mut captures: Vec<String> = free.into_iter()
            .filter(|name| self.lookup_local(name).is_some())
            .collect();
        captures.sort(); // deterministic ordering

        // Save current state
        let saved_locals = std::mem::replace(&mut self.locals, vec![ScopeFrame::new()]);
        let saved_next_reg = self.next_reg;
        self.next_reg = 0;

        // Allocate registers for parameters
        for (name, _) in params {
            let reg = self.alloc_reg();
            self.define_local(name, reg);
        }

        // Emit a jump over the lambda body so it is not executed as part of
        // the enclosing __main flow. The closure instruction still references
        // the body start for Call/ClosureCall.
        let skip_jmp_idx = self.emit(Instruction::new2(OpCode::Jmp, 0, 0));

        // Record start offset before compiling body
        let start_offset = self.module.current_offset();

        // Prologue: load captured free variables into registers and bind them
        // as locals so the body can reference them normally.
        for (idx, name) in captures.iter().enumerate() {
            let reg = self.alloc_reg();
            self.define_local(name, reg);
            self.emit(Instruction::new3(OpCode::CapLoad, idx as u8, reg, 0));
        }

        // Compile body
        let body_reg = self.compile_expr(body)?;
        self.emit(Instruction::new1(OpCode::RetVal, body_reg));

        // Patch the skip jump to land after the body
        let after_body = self.module.current_offset() as i16;
        let skip_offset = after_body - skip_jmp_idx as i16;
        if let Some(instr) = self.module.instructions.get_mut(skip_jmp_idx) {
            instr.op1 = ((skip_offset as u16) >> 8) as u8;
            instr.op2 = ((skip_offset as u16) & 0xFF) as u8;
        }

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

        // Capture live values from the enclosing scope into the closure.
        for (idx, name) in captures.iter().enumerate() {
            let src = if let Some(reg) = self.lookup_local(name) {
                reg
            } else {
                // Fallback: materialise the value into a register. This should
                // not be reached for local captures, but keeps the compiler
                // self-contained for global references.
                self.compile_var(name)?
            };
            self.emit(Instruction::new3(OpCode::CapStore, dst, idx as u8, src));
        }

        Ok(dst)
    }
    fn compile_app(&mut self, func: &Expr, args: &[Expr]) -> NuResult<u8> {
        const FUNC_VALUE_REG: u8 = 254;
        let saved_next_reg = self.next_reg;

        // Compile the function reference first using normal allocation, then
        // stash the value in a high fixed register. This guarantees argument
        // compilation (which resets allocation to r0) cannot overwrite the
        // function value — essential for recursive calls and local closures.
        let func_src = if let Expr::Var(name, _) = func {
            if let Some(&func_idx) = self.func_map.get(name) {
                // Named top-level function: load its table index as a constant.
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
        self.emit(Instruction::new2(OpCode::Move, func_src, FUNC_VALUE_REG));

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

        let dst = self.alloc_reg();
        let argc = args.len().min(255) as u8;
        self.emit(Instruction::new3(OpCode::Call, FUNC_VALUE_REG, argc, dst));
        // Restore next_reg, but ensure dst is reserved for the caller
        self.next_reg = saved_next_reg.max(dst + 1);
        Ok(dst)
    }

    fn compile_let(&mut self, name: &str, value: &Expr, body: &Expr) -> NuResult<u8> {
        let val_reg = self.compile_expr(value)?;
        // Bind the name to a dedicated high register and move the value there.
        // This prevents later argument/sub-expression compilation from
        // overwriting the binding (critical when the value is a closure used
        // multiple times, e.g. `let id = fn(x) x in (id(1), id(true))`).
        let bound_reg = self.binding_reg;
        self.binding_reg = self.binding_reg.saturating_add(1);
        self.emit(Instruction::new2(OpCode::Move, val_reg, bound_reg));
        self.define_local(name, bound_reg);
        self.compile_expr(body)
    }

    fn compile_let_rec(&mut self, name: &str, params: &[(String, Option<crate::types::Type>)], value: &Expr, body: &Expr) -> NuResult<u8> {
        // Reserve a function-table slot for the recursive function and make it
        // resolvable by name (like a top-level function declaration).
        let func_idx = self.module.function_table.len();
        self.module.function_table.push(0); // placeholder
        self.func_map.insert(name.to_string(), func_idx);

        // Emit a jump over the function body so it is not executed as part of
        // the enclosing __main flow. The closure instruction still references
        // the body start for Call/ClosureCall.
        let skip_jmp_idx = self.emit(Instruction::new2(OpCode::Jmp, 0, 0));

        // Save outer compilation state.
        let saved_locals = std::mem::replace(&mut self.locals, vec![ScopeFrame::new()]);
        let saved_next_reg = self.next_reg;
        self.next_reg = 0;

        // Bind parameters to low registers initially, then save them to a high
        // register safe zone so recursive calls (which use r0, r1, ... for args
        // and temporaries) do not overwrite them.
        const PARAM_SAVE_BASE: u8 = 16;
        for (param_name, _) in params {
            let reg = self.alloc_reg();
            self.define_local(param_name, reg);
        }

        // Record start offset BEFORE param saves — they are part of the body.
        let start_offset = self.module.current_offset();

        for (i, (param_name, _)) in params.iter().enumerate() {
            let old_reg = i as u8;
            let new_reg = PARAM_SAVE_BASE + i as u8;
            self.emit(Instruction::new2(OpCode::Move, old_reg, new_reg));
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

        // Compile the recursive body.
        let body_reg = self.compile_expr(value)?;
        self.emit(Instruction::new1(OpCode::RetVal, body_reg));

        // Restore outer state.
        self.locals = saved_locals;
        self.next_reg = saved_next_reg;

        // Patch the skip jump to land after the body.
        let after_body = self.module.current_offset() as i16;
        let skip_offset = after_body - skip_jmp_idx as i16;
        if let Some(instr) = self.module.instructions.get_mut(skip_jmp_idx) {
            instr.op1 = ((skip_offset as u16) >> 8) as u8;
            instr.op2 = ((skip_offset as u16) & 0xFF) as u8;
        }

        // Patch the function table with the actual code offset.
        self.module.function_table[func_idx] = start_offset;

        // Bind the name in the outer scope to a register holding the function
        // index, so references in the let body work.
        let closure_reg = self.alloc_reg();
        let idx_const = self.add_const(Constant::Int(func_idx as i64));
        self.emit(make_constu(idx_const as u16, closure_reg));
        self.define_local(name, closure_reg);

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

    fn compile_handle(&mut self, body: &Expr, handlers: &[EffectHandler]) -> NuResult<u8> {
        // Compile handler bodies FIRST (they go after the main body + Unwind).
        // Each handler body is a sequence of: [compiled body] [Resume result_reg]
        // We remember the offset of each handler body to build the HandlerTable.

        // Save the offset where handler bodies will start (after body + Unwind).
        let body_start = self.current_offset();

        // We need two passes:
        //   1. Emit the Handle + body + Unwind (jumping past handler bodies)
        //   2. Emit handler bodies and build the HandlerTable
        //
        // But the body might contain Perform that references the handler table
        // index. So we:
        //   - Reserve the handler table index
        //   - Emit Handle with a placeholder index
        //   - Compile body
        //   - Emit Unwind
        //   - Emit Jmp past handler bodies
        //   - Emit handler bodies, collecting offsets
        //   - Build and add the HandlerTable
        //   - Patch the Handle instruction with the real index

        // Step 1: Emit Handle with placeholder index (will be patched).
        let handle_instr_idx = self.emit(Instruction::new1(OpCode::Handle, 0));

        // Step 2: Compile the body (may contain Perform that matches our handlers).
        self.push_scope();
        let body_reg = self.compile_expr(body)?;
        self.pop_scope();

        // Step 3: Unwind to pop the handler frame.
        self.emit(Instruction::new0(OpCode::Unwind));

        // Step 4: Jump past the handler bodies (they're only entered via Perform).
        let jmp_past_idx = self.emit(Instruction::new2(OpCode::Jmp, 0, 0));

        // Step 5: Compile each handler body and collect offsets.
        let mut bindings: Vec<HandlerBinding> = Vec::with_capacity(handlers.len());

        for eh in handlers {
            let handler_offset = self.current_offset();

            // Handler params go into consecutive registers starting at r0.
            self.push_scope();
            for (i, param) in eh.params.iter().enumerate() {
                let param_reg = i as u8; // r0, r1, r2, ...
                self.define_local(param, param_reg);
            }

            // Compile the handler body.
            let result_reg = self.compile_expr(&eh.body)?;
            self.pop_scope();

            // Resume with the result.
            self.emit(Instruction::new1(OpCode::Resume, result_reg));

            // Build the binding.
            // The VM matches Perform's effect name against this.
            // compile_perform uses just the effect part (not effect.op),
            // so we use effect_name here for consistency.
            bindings.push(HandlerBinding {
                effect_name: eh.effect_name.clone(),
                handler_offset,
                arg_count: eh.params.len() as u8,
                result_reg,
            });
        }

        // Step 6: Build and add the HandlerTable.
        let handler_table = HandlerTable {
            bindings,
            fallback_offset: None,
        };
        let handler_table_idx = self.module.add_handler_table(handler_table);

        // Step 7: Patch the Handle instruction with the real index.
        if let Some(instr) = self.module.instructions.get_mut(handle_instr_idx) {
            instr.op1 = handler_table_idx as u8;
        }

        // Step 8: Patch the Jmp past handler bodies.
        let after_handlers = self.current_offset() as i16;
        let jmp_offset = after_handlers - jmp_past_idx as i16;
        if jmp_offset >= 0 {
            if let Some(instr) = self.module.instructions.get_mut(jmp_past_idx) {
                instr.op1 = ((jmp_offset as u16 >> 8) & 0xFF) as u8;
                instr.op2 = (jmp_offset as u16 & 0xFF) as u8;
            }
        }

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
        // Lower `x |> f(a, b)` to `f(x, a, b)` and reuse the standard
        // application compiler, which correctly places arguments in R0.. and
        // protects the function value from register churn.
        let app = match right {
            Expr::App { func, args, span } => {
                let mut new_args = vec![left.clone()];
                new_args.extend(args.iter().cloned());
                Expr::App {
                    func: func.clone(),
                    args: new_args,
                    span: *span,
                }
            }
            Expr::Var(name, span) => Expr::App {
                func: Box::new(Expr::Var(name.clone(), *span)),
                args: vec![left.clone()],
                span: *span,
            },
            _ => Expr::App {
                func: Box::new(right.clone()),
                args: vec![left.clone()],
                span: Span::default(),
            }
        };
        match app {
            Expr::App { func, args, .. } => self.compile_app(func.as_ref(), args.as_slice()),
            _ => unreachable!(),
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

    // -- Python Interop — RESERVED (see audit, native_actor.rs) --
    //
    // These emit methods are retained for API compatibility but emit
    // reserved opcodes that trap at runtime with a clear error message.
    // Python interop goes through NativeActor (src/python/native_actor.rs),
    // never through direct bytecode.
    //
    // To call Python from Nulang, use:
    //   perform Python.call("module.function", args)
    // which is dispatched by the effect handler to
    // a future bytecode-level native-actor call instruction.
    //
    // Per the architectural audit: Python objects MUST NOT enter
    // the VM value representation. All Python code runs in
    // dedicated OS threads with marshal-only data crossing.
    ///
    /// RESERVED: Python module import. Use `perform Python.import(...)`
    /// via the native actor runtime instead.
    pub fn emit_py_import(&mut self, module_name_const_idx: u16, dst_reg: u8) {
        self.emit(Instruction::new2(OpCode::PyImport, (module_name_const_idx & 0xFF) as u8, dst_reg));
    }
    /// RESERVED: Python getattr. Use native actor runtime.
    pub fn emit_py_getattr(&mut self, obj_reg: u8, attr_name_const_idx: u16, dst_reg: u8) {
        self.emit(Instruction::new3(OpCode::PyGetAttr, obj_reg, (attr_name_const_idx & 0xFF) as u8, dst_reg));
    }
    /// RESERVED: Python call. Use native actor runtime.
    pub fn emit_py_call(&mut self, callable_reg: u8, arg_count: u8, dst_reg: u8) {
        self.emit(Instruction::new3(OpCode::PyCall, callable_reg, arg_count, dst_reg));
    }
    /// RESERVED: Python call with kwargs. Use native actor runtime.
    pub fn emit_py_call_kw(&mut self, callable_reg: u8, args_tuple_reg: u8, kwargs_dict_reg: u8) {
        self.emit(Instruction::new3(OpCode::PyCallKw, callable_reg, args_tuple_reg, kwargs_dict_reg));
    }
    /// RESERVED: Python setattr. Use native actor runtime.
    pub fn emit_py_setattr(&mut self, obj_reg: u8, attr_name_const_idx: u16, val_reg: u8) {
        self.emit(Instruction::new3(OpCode::PySetAttr, obj_reg, (attr_name_const_idx & 0xFF) as u8, val_reg));
    }
    /// RESERVED: Python-to-Nulang marshal. Use native actor runtime.
    pub fn emit_py_to_nu(&mut self, py_val_reg: u8, dst_reg: u8) {
        self.emit(Instruction::new2(OpCode::PyToNu, py_val_reg, dst_reg));
    }
    /// RESERVED: Nulang-to-Python marshal. Use native actor runtime.
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
