//! AST -> Bytecode compiler.
//!
//! Compiles Nulang AST into bytecode modules for the VM.

use crate::ai::memory::EpisodicMemory;
use crate::ai::request::ToolSchema;
use crate::ai::schema::function_to_tool_schema;
use crate::ast::*;
use crate::bytecode::*;
use crate::types::{Capability, NuError, NuResult, PrimitiveType, Span, Type};

/// Workaround for the `Self` opcode (0x83) which conflicts with the Rust keyword.
fn op_self() -> OpCode {
    OpCode::SelfOp
}

fn actor_name_from_expr(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Var(name, _) => Some(name.clone()),
        _ => None,
    }
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

/// Map a Nulang primitive type to its FFI representation.
/// Shared with the MIR codegen, which registers the same foreign functions.
pub(crate) fn nulang_type_to_ffi_type(ty: &Type) -> Option<FfiType> {
    match ty {
        Type::Primitive(PrimitiveType::Int) => Some(FfiType::Int),
        Type::Primitive(PrimitiveType::Float) => Some(FfiType::Float),
        Type::Primitive(PrimitiveType::Bool) => Some(FfiType::Bool),
        Type::Primitive(PrimitiveType::String) => Some(FfiType::String),
        Type::Primitive(PrimitiveType::Unit) => Some(FfiType::Unit),
        Type::Primitive(PrimitiveType::Address) => Some(FfiType::Pointer),
        _ => None,
    }
}

/// Accumulate free variable names in `expr` that are not in `bound`.
/// Shared with the HIR lowering pass, which needs the same capture analysis.
pub(crate) fn free_vars(expr: &Expr, bound: &std::collections::HashSet<String>, acc: &mut std::collections::HashSet<String>) {
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
    /// Map field names to numeric indices for FieldL.
    field_map: std::collections::HashMap<String, u8>,
    next_field_id: u8,
    /// Map function names to function_table indices.
    func_map: std::collections::HashMap<String, usize>,
    /// Map foreign function names to foreign_functions indices.
    extern_func_map: std::collections::HashMap<String, usize>,
    /// Dedicated high-register allocator for let-bound values, so closures
    /// and other multi-use bindings survive argument/dst register churn.
    binding_reg: u8,
    /// Set to true if register allocation exceeds the available register file.
    reg_overflow: bool,
}

impl Compiler {
    pub fn new(module_name: impl Into<String>) -> Self {
        Compiler {
            module: CodeModule::new(module_name),
            locals: vec![ScopeFrame::new()],
            next_reg: 0,
            field_map: std::collections::HashMap::new(),
            next_field_id: 0,
            func_map: std::collections::HashMap::new(),
            extern_func_map: std::collections::HashMap::new(),
            binding_reg: 240,
            reg_overflow: false,
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
        if self.reg_overflow {
            return Err(NuError::VMError(
                "register allocation overflow: expression requires more than 240 temporary registers or more than 15 active let bindings".to_string()
            ));
        }
        Ok(&self.module)
    }

    fn collect_functions(&mut self, decls: &[Decl]) -> NuResult<()> {
        for decl in decls {
            match decl {
                Decl::Function { name, params, ret_type, annotations, .. } if name != "__main" => {
                    let func_idx = self.module.function_table.len();
                    self.module.function_table.push(0); // placeholder
                    self.func_map.insert(name.clone(), func_idx);

                    // Collect tool schemas early so agent declarations can
                    // resolve tool names regardless of source order.
                    if let Some(FunctionAnnotation::Tool { description }) = annotations.iter().find(|a| matches!(a, FunctionAnnotation::Tool { .. })) {
                        let mut typed_params = Vec::with_capacity(params.len());
                        let mut all_typed = true;
                        for (param_name, param_ty) in params {
                            if let Some(ty) = param_ty {
                                typed_params.push((param_name.clone(), ty.clone()));
                            } else {
                                all_typed = false;
                                break;
                            }
                        }
                        if all_typed {
                            let ret = ret_type.clone().unwrap_or_else(Type::unit);
                            let schema = function_to_tool_schema(name, description, &typed_params, &ret);
                            self.module.tools.push(schema);
                        }
                    }
                }
                Decl::Extern { library, funcs, .. } => {
                    for ef in funcs {
                        let params = ef
                            .params
                            .iter()
                            .map(|(_, ty)| nulang_type_to_ffi_type(ty))
                            .collect::<Option<Vec<_>>>()
                            .ok_or_else(|| {
                                NuError::FFIError {
                                    msg: format!(
                                        "unsupported parameter type in extern function {}",
                                        ef.name
                                    ),
                                    span: ef.span,
                                }
                            })?;
                        let ret = nulang_type_to_ffi_type(&ef.ret).ok_or_else(|| {
                            NuError::FFIError {
                                msg: format!(
                                    "unsupported return type in extern function {}",
                                    ef.name
                                ),
                                span: ef.span,
                            }
                        })?;
                        let def = ForeignFunctionDef {
                            library: library.clone(),
                            symbol: ef.name.clone(),
                            params,
                            ret,
                        };
                        let idx = self.module.foreign_functions.len();
                        self.module.foreign_functions.push(def);
                        self.extern_func_map.insert(ef.name.clone(), idx);
                    }
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
            // Only a bare `ident = v` is parsed as Expr::Assign; every other
            // assignment target (`self.f = v`, `arr[i] = v`, `record.f = v`)
            // is an ordinary-looking BinOp::Assign — route both through the
            // same place-based compile_assign instead of compile_binary's
            // generic value-reading prologue, which would read the target's
            // CURRENT value instead of computing a place to write to.
            // Only a bare `ident = v` is parsed as Expr::Assign; every other
            // assignment target (`self.f = v`, `arr[i] = v`, `record.f = v`)
            // is an ordinary-looking BinOp::Assign — route both through the
            // same place-based compile_assign instead of compile_binary's
            // generic value-reading prologue, which would read the target's
            // CURRENT value instead of computing a place to write to.
            Expr::Binary { op: BinOp::Assign, left, right, .. } => {
                self.compile_assign(left, right)
            }
            Expr::Binary { op, left, right, .. } => self.compile_binary(*op, left, right),
            Expr::Unary { op, expr, .. } => self.compile_unary(*op, expr),
            Expr::Spawn { actor_type, init, .. } => self.compile_spawn(actor_type, init),
            Expr::Send { actor, behavior, args, .. } => self.compile_send(actor, behavior, args),
            Expr::Ask { actor, behavior, args, .. } => self.compile_ask(actor, behavior, args),
            Expr::SelfRef(_) => Ok(self.compile_self()),
            Expr::Perform { effect, op, args, .. } => self.compile_perform(effect, op, args),
            Expr::Emit { event, args, .. } => self.compile_emit(event, args),
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
            Literal::Nil => {
                let idx = self.add_const(Constant::Nil);
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

        // Pipeline built-in method calls. These are wired directly to runtime
        // opcodes rather than going through the function table or FFI layer.
        if let Expr::FieldAccess { expr, field, .. } = func {
            if let Expr::Var(base, _) = expr.as_ref() {
                if base == "Pipeline" {
                    if field == "new" && args.is_empty() {
                        let dst = self.alloc_reg();
                        self.emit(Instruction::new1(OpCode::PipelineNew, dst));
                        return Ok(dst);
                    }
                    if field == "stage" && args.len() == 4 {
                        self.next_reg = 0;
                        let mut arg_regs = Vec::new();
                        for arg in args {
                            let arg_reg = self.compile_expr(arg)?;
                            arg_regs.push(arg_reg);
                        }
                        for (i, &arg_reg) in arg_regs.iter().enumerate() {
                            if arg_reg != i as u8 {
                                self.emit(Instruction::new2(OpCode::Move, arg_reg, i as u8));
                            }
                        }
                        let dst = self.alloc_reg();
                        self.emit(Instruction::new1(OpCode::PipelineStage, dst));
                        self.next_reg = saved_next_reg.max(dst + 1);
                        return Ok(dst);
                    }
                }
                if base == "Supervisor" {
                    if field == "new" && args.is_empty() {
                        let dst = self.alloc_reg();
                        self.emit(Instruction::new1(OpCode::SupervisorNew, dst));
                        return Ok(dst);
                    }
                    if field == "worker" && args.len() == 4 {
                        self.next_reg = 0;
                        let mut arg_regs = Vec::new();
                        for arg in args {
                            let arg_reg = self.compile_expr(arg)?;
                            arg_regs.push(arg_reg);
                        }
                        for (i, &arg_reg) in arg_regs.iter().enumerate() {
                            if arg_reg != i as u8 {
                                self.emit(Instruction::new2(OpCode::Move, arg_reg, i as u8));
                            }
                        }
                        let dst = self.alloc_reg();
                        self.emit(Instruction::new1(OpCode::SupervisorWorker, dst));
                        self.next_reg = saved_next_reg.max(dst + 1);
                        return Ok(dst);
                    }
                }
                if base == "Debate" {
                    if field == "new" && args.len() == 3 {
                        self.next_reg = 0;
                        let mut arg_regs = Vec::new();
                        for arg in args {
                            let arg_reg = self.compile_expr(arg)?;
                            arg_regs.push(arg_reg);
                        }
                        for (i, &arg_reg) in arg_regs.iter().enumerate() {
                            if arg_reg != i as u8 {
                                self.emit(Instruction::new2(OpCode::Move, arg_reg, i as u8));
                            }
                        }
                        let dst = self.alloc_reg();
                        self.emit(Instruction::new1(OpCode::DebateNew, dst));
                        self.next_reg = saved_next_reg.max(dst + 1);
                        return Ok(dst);
                    }
                    if field == "participant" && args.len() == 4 {
                        self.next_reg = 0;
                        let mut arg_regs = Vec::new();
                        for arg in args {
                            let arg_reg = self.compile_expr(arg)?;
                            arg_regs.push(arg_reg);
                        }
                        for (i, &arg_reg) in arg_regs.iter().enumerate() {
                            if arg_reg != i as u8 {
                                self.emit(Instruction::new2(OpCode::Move, arg_reg, i as u8));
                            }
                        }
                        let dst = self.alloc_reg();
                        self.emit(Instruction::new1(OpCode::DebateParticipant, dst));
                        self.next_reg = saved_next_reg.max(dst + 1);
                        return Ok(dst);
                    }
                }
                if field == "run" && (args.len() == 1 || args.is_empty()) {
                    // Instance method: receiver is the pipeline/supervisor/debate id variable.
                    self.next_reg = 0;
                    let receiver_reg = self.compile_expr(expr)?;
                    if receiver_reg != 0 {
                        self.emit(Instruction::new2(OpCode::Move, receiver_reg, 0));
                    }
                    if !args.is_empty() {
                        let arg_reg = self.compile_expr(&args[0])?;
                        if arg_reg != 1 {
                            self.emit(Instruction::new2(OpCode::Move, arg_reg, 1));
                        }
                    }
                    let dst = self.alloc_reg();
                    // Choose the opcode based on the receiver type.  The parser
                    // makes `|>` left-associative, so the receiver here is the
                    // variable bound by the chained construction expression.
                    // We conservatively emit SupervisorRun when the base name
                    // is "Supervisor"; otherwise PipelineRun.
                    // Heuristic disambiguation: instance `.run()` is a pipeline
                    // run unless the receiver variable name clearly refers to a
                    // supervisor team or debate.  In the v0.9 MVP, name your
                    // supervisor variable `team`/`supervisor` and your debate
                    // variable `debate`.
                    let opcode = if let Expr::Var(receiver_name, _) = expr.as_ref() {
                        let lowered = receiver_name.to_lowercase();
                        if lowered == "team"
                            || lowered == "supervisor"
                            || lowered.contains("supervisor")
                        {
                            OpCode::SupervisorRun
                        } else if lowered == "debate" || lowered.contains("debate") {
                            OpCode::DebateRun
                        } else {
                            OpCode::PipelineRun
                        }
                    } else {
                        OpCode::PipelineRun
                    };
                    self.emit(Instruction::new1(opcode, dst));
                    self.next_reg = saved_next_reg.max(dst + 1);
                    return Ok(dst);
                }
            }
        }

        // Direct extern function call: no function value needed.
        if let Expr::Var(name, span) = func {
            if let Some(&extern_idx) = self.extern_func_map.get(name) {
                let def = &self.module.foreign_functions[extern_idx];
                if def.params.len() != args.len() {
                    return Err(NuError::FFIError {
                        msg: format!(
                            "extern function {} expects {} arguments, got {}",
                            name,
                            def.params.len(),
                            args.len()
                        ),
                        span: *span,
                    });
                }

                // Compile arguments into consecutive registers r0..rN.
                self.next_reg = 0;
                let mut arg_regs = Vec::new();
                for arg in args {
                    let arg_reg = self.compile_expr(arg)?;
                    arg_regs.push(arg_reg);
                }
                for (i, &arg_reg) in arg_regs.iter().enumerate() {
                    if arg_reg != i as u8 {
                        self.emit(Instruction::new2(OpCode::Move, arg_reg, i as u8));
                    }
                }

                let dst = self.alloc_reg();
                self.emit(Instruction::new3(
                    OpCode::FFICall,
                    ((extern_idx >> 8) & 0xFF) as u8,
                    (extern_idx & 0xFF) as u8,
                    dst,
                ));
                self.next_reg = saved_next_reg.max(dst + 1);
                return Ok(dst);
            }
        }

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
        // Let-bound lambdas that reference themselves (e.g. `let fac = fn(n) ...
        // fac(n-1) ... in ...`) are compiled as recursive functions so the
        // self-reference resolves without capture support. Lambdas that do
        // NOT reference their own name compile as ordinary closures so they
        // can capture the enclosing scope.
        if let Expr::Lambda { params, body: lam_body, .. } = value {
            let param_set: std::collections::HashSet<String> =
                params.iter().map(|(n, _)| n.clone()).collect();
            let mut free = std::collections::HashSet::new();
            free_vars(lam_body, &param_set, &mut free);
            if free.contains(name) {
                return self.compile_let_rec(name, params, lam_body, body);
            }
        }

        let val_reg = self.compile_expr(value)?;
        // Bind the name to a dedicated high register and move the value there.
        // This prevents later argument/sub-expression compilation from
        // overwriting the binding (critical when the value is a closure used
        // multiple times, e.g. `let id = fn(x) x in (id(1), id(true))`).
        const MAX_BINDING_REG: u8 = 254;
        if self.binding_reg > MAX_BINDING_REG {
            self.reg_overflow = true;
        }
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
            let idx = self.add_const(Constant::Unit);
            self.emit(make_constu(idx as u16, r));
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
            BinOp::BitAnd => { self.emit(Instruction::new3(OpCode::BitAnd, r1_save, r2, dst)); }
            BinOp::BitOr => { self.emit(Instruction::new3(OpCode::BitOr, r1_save, r2, dst)); }
            BinOp::BitXor => { self.emit(Instruction::new3(OpCode::Xor, r1_save, r2, dst)); }
            BinOp::Shl => { self.emit(Instruction::new3(OpCode::Shl, r1_save, r2, dst)); }
            BinOp::Shr => { self.emit(Instruction::new3(OpCode::Shr, r1_save, r2, dst)); }
            // BinOp::Assign is intercepted in compile_expr's Expr::Binary
            // dispatch and never reaches compile_binary (see the comment
            // there): assignment targets need a *place*, not a value, and
            // this function's prologue always reads `left` as a value.
            BinOp::Assign => unreachable!("BinOp::Assign is handled by compile_assign"),
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
        // Assign a stable module-wide field id to every field name.  Records are
        // laid out as a flat array indexed by these ids, so field access does not
        // need to know the concrete record literal that created the value.
        let mut max_field_id: u8 = 0;
        let mut field_ids = Vec::with_capacity(fields.len());
        for (name, _) in fields.iter() {
            let id = self.field_id(name)?;
            max_field_id = max_field_id.max(id);
            field_ids.push(id);
        }
        let slot_count = max_field_id.saturating_add(1);
        let dst = self.alloc_reg();
        self.emit(Instruction::new2(OpCode::RecMk, slot_count, dst));
        for (i, field_id) in field_ids.into_iter().enumerate() {
            self.emit(Instruction::new3(OpCode::RecS, dst, field_id, field_regs[i]));
        }
        Ok(dst)
    }

    fn compile_field_access(&mut self, expr: &Expr, field: &str) -> NuResult<u8> {
        // `self.field` inside an actor behavior reads from the current actor's state.
        // The parser produces `SelfRef`; `compile_behavior` also binds `self` as a
        // local variable, so accept both forms.
        let is_self = matches!(expr, Expr::SelfRef(_))
            || matches!(expr, Expr::Var(name, _) if name == "self");
        if is_self {
            let dst = self.alloc_reg();
            let field_idx = self.add_const(Constant::String(field.to_string()));
            self.emit(Instruction::new3(OpCode::StateGet,
                ((field_idx >> 8) & 0xFF) as u8,
                (field_idx & 0xFF) as u8,
                dst));
            return Ok(dst);
        }
        let obj_reg = self.compile_expr(expr)?;
        let dst = self.alloc_reg();
        // Field access is positional, keyed by the module-wide field id.
        let field_id = self.field_id(field)?;
        self.emit(Instruction::new3(OpCode::RecL, obj_reg, field_id, dst));
        Ok(dst)
    }

    fn compile_spawn(&mut self, actor_type: &Expr, init: &[(String, Expr)]) -> NuResult<u8> {
        for (_name, expr) in init {
            let _r = self.compile_expr(expr)?;
        }
        let behavior_idx = match actor_type {
            Expr::Var(name, _) => self
                .module
                .actor_metadata
                .iter()
                .find(|m| m.name == *name)
                .and_then(|m| m.behavior_indices.first().copied())
                .unwrap_or(self.module.behaviors.len()),
            _ => self.module.behaviors.len(),
        };
        let dst = self.alloc_reg();
        self.emit(Instruction::new3(
            OpCode::Spawn,
            ((behavior_idx >> 8) & 0xFF) as u8,
            (behavior_idx & 0xFF) as u8,
            dst,
        ));
        Ok(dst)
    }

    fn behavior_table_index(&self, actor_name: &str, behavior: &str) -> usize {
        let full_name = format!("{}.{}", actor_name, behavior);
        self.module
            .behaviors
            .iter()
            .position(|b| b.name == full_name)
            .or_else(|| {
                // The actor value may be stored in a variable whose name does not
                // match the actor type (e.g. `let a = spawn Agent {}`). Fall back
                // to matching any behavior with the requested suffix.
                let suffix = format!(".{}", behavior);
                self.module
                    .behaviors
                    .iter()
                    .position(|b| b.name.ends_with(&suffix))
            })
            .unwrap_or(self.module.behaviors.len())
    }

    fn compile_send(&mut self, actor: &Expr, behavior: &str, args: &[Expr]) -> NuResult<u8> {
        if args.len() > 255 {
            return Err(NuError::VMError("send has more than 255 arguments".to_string()));
        }
        let saved_next_reg = self.next_reg;
        // Reserve r0..r(n-1) for arguments and r_n for the receiver address.
        let receiver_reg = args.len() as u8;
        self.next_reg = receiver_reg + 1;
        let actual_receiver_reg = self.compile_expr(actor)?;
        if actual_receiver_reg != receiver_reg {
            self.emit(Instruction::new2(OpCode::Move, actual_receiver_reg, receiver_reg));
        }
        // Compile arguments into r0..r(n-1).  They cannot clobber receiver_reg
        // because that register is >= n.
        self.next_reg = 0;
        for (i, arg) in args.iter().enumerate() {
            let reg = self.compile_expr(arg)?;
            if reg != i as u8 {
                self.emit(Instruction::new2(OpCode::Move, reg, i as u8));
            }
        }
        self.next_reg = saved_next_reg;

        let actor_name = actor_name_from_expr(actor).unwrap_or_default();
        let behavior_idx = self.behavior_table_index(&actor_name, behavior);
        let dst = self.alloc_reg();
        self.emit(Instruction::new3(OpCode::Send, receiver_reg,
            ((behavior_idx >> 8) & 0xFF) as u8,
            (behavior_idx & 0xFF) as u8));
        self.emit(Instruction::new1(OpCode::Const0, dst));
        Ok(dst)
    }

    fn compile_ask(&mut self, actor: &Expr, behavior: &str, args: &[Expr]) -> NuResult<u8> {
        if args.len() > 255 {
            return Err(NuError::VMError("ask has more than 255 arguments".to_string()));
        }
        let saved_next_reg = self.next_reg;
        // Reserve r0..r(n-1) for arguments and r_n for the receiver address.
        // The VM stores the ask result in the receiver register, so that slot is
        // also the destination.
        let receiver_reg = args.len() as u8;
        self.next_reg = receiver_reg + 1;
        let actual_receiver_reg = self.compile_expr(actor)?;
        if actual_receiver_reg != receiver_reg {
            self.emit(Instruction::new2(OpCode::Move, actual_receiver_reg, receiver_reg));
        }
        // Compile arguments into r0..r(n-1).
        self.next_reg = 0;
        for (i, arg) in args.iter().enumerate() {
            let reg = self.compile_expr(arg)?;
            if reg != i as u8 {
                self.emit(Instruction::new2(OpCode::Move, reg, i as u8));
            }
        }
        self.next_reg = saved_next_reg;

        let actor_name = actor_name_from_expr(actor).unwrap_or_default();
        let behavior_idx = self.behavior_table_index(&actor_name, behavior);
        let dst = self.alloc_reg();
        self.emit(Instruction::new3(OpCode::Ask, receiver_reg,
            ((behavior_idx >> 8) & 0xFF) as u8,
            (behavior_idx & 0xFF) as u8));
        self.emit(Instruction::new2(OpCode::Move, receiver_reg, dst));
        Ok(dst)
    }

    fn compile_self(&mut self) -> u8 {
        let dst = self.alloc_reg();
        self.emit(Instruction::new1(op_self(), dst));
        dst
    }

    fn compile_perform(&mut self, effect: &str, op: &str, args: &[Expr]) -> NuResult<u8> {
        // Special-case workflow signal waits: compile to a dedicated opcode
        // that the runtime can suspend and resume.
        if effect == "Signal" && op == "wait" {
            if let Some(Expr::Literal(Literal::String(name), _)) = args.get(0) {
                let name_idx = self.add_const(Constant::String(name.clone()));
                let dst = self.alloc_reg();
                self.emit(Instruction::new3(OpCode::SignalWait,
                    ((name_idx >> 8) & 0xFF) as u8,
                    (name_idx & 0xFF) as u8,
                    dst));
                return Ok(dst);
            }
        }

        // Special-case LLM.ask: compile to a dedicated opcode wired to the
        // runtime's configured LLM client.
        if effect == "LLM" && op == "ask" {
            let prompt_reg = if let Some(arg) = args.get(0) {
                self.compile_expr(arg)?
            } else {
                let r = self.alloc_reg();
                self.emit(Instruction::new1(OpCode::Const0, r));
                r
            };
            let model_idx = self.add_const(Constant::String("".to_string()));
            self.emit(Instruction::new3(OpCode::LlmAsk,
                ((model_idx >> 8) & 0xFF) as u8,
                (model_idx & 0xFF) as u8,
                prompt_reg));
            return Ok(prompt_reg);
        }

        // Place effect arguments in consecutive registers r0..rn to match the
        // handler convention and to make them easy for runtime callbacks to
        // locate (e.g. Timer.sleep name/duration_ms).
        let saved_next_reg = self.next_reg;
        self.next_reg = 0;
        for (i, arg) in args.iter().enumerate().take(256) {
            let reg = self.compile_expr(arg)?;
            if reg != i as u8 {
                self.emit(Instruction::new2(OpCode::Move, reg, i as u8));
            }
        }
        self.next_reg = saved_next_reg;
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

    fn compile_emit(&mut self, event: &str, args: &[Expr]) -> NuResult<u8> {
        // Event arguments are placed in the first N registers, matching the
        // convention used by Send/Ask opcodes.
        for (i, arg) in args.iter().enumerate().take(256) {
            let reg = self.compile_expr(arg)?;
            if reg != i as u8 {
                self.emit(Instruction::new2(OpCode::Move, reg, i as u8));
            }
        }
        let event_idx = self.add_const(Constant::String(event.to_string()));
        self.emit(Instruction::new3(OpCode::Emit,
            ((event_idx >> 8) & 0xFF) as u8,
            (event_idx & 0xFF) as u8,
            args.len() as u8));
        Ok(0)
    }

    fn compile_handle(&mut self, body: &Expr, handlers: &[EffectHandler]) -> NuResult<u8> {
        // Compile handler bodies FIRST (they go after the main body + Unwind).
        // Each handler body is a sequence of: [compiled body] [Resume result_reg]
        // We remember the offset of each handler body to build the HandlerTable.

        // Save the offset where handler bodies will start (after body + Unwind).
        let _body_start = self.current_offset();

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
            Expr::FieldAccess { expr, field, .. }
                if matches!(expr.as_ref(), Expr::SelfRef(_))
                    || matches!(expr.as_ref(), Expr::Var(name, _) if name == "self") => {
                let field_idx = self.add_const(Constant::String(field.to_string()));
                self.emit(Instruction::new3(OpCode::StateSet,
                    ((field_idx >> 8) & 0xFF) as u8,
                    (field_idx & 0xFF) as u8,
                    val_reg));
                Ok(val_reg)
            }
            // `record.field = value` (non-self): write via the module-wide
            // positional field id, same layout compile_field_access reads.
            Expr::FieldAccess { expr, field, .. } => {
                let obj_reg = self.compile_expr(expr)?;
                let field_id = self.field_id(field)?;
                self.emit(Instruction::new3(OpCode::RecS, obj_reg, field_id, val_reg));
                Ok(val_reg)
            }
            // `arr[idx] = value`.
            Expr::Index { arr, idx, .. } => {
                let arr_reg = self.compile_expr(arr)?;
                let idx_reg = self.compile_expr(idx)?;
                self.emit(Instruction::new3(OpCode::ArrStore, arr_reg, idx_reg, val_reg));
                Ok(val_reg)
            }
            _ => Err(NuError::VMError(format!(
                "cannot assign to this expression: {:?}",
                target
            ))),
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
            Decl::Function {
                name,
                params,
                ret_type,
                body,
                annotations,
                ..
            } if name == "main" => {
                // Treat `fn main()` as the module entry point when there is no
                // synthetic __main wrapper.
                self.module.entry_point = Some(self.module.current_offset());
                self.compile_function(name, params, ret_type.as_ref(), body, annotations)?;
            }
            Decl::Function {
                name,
                params,
                ret_type,
                body,
                annotations,
                ..
            } => {
                self.compile_function(name, params, ret_type.as_ref(), body, annotations)?;
            }
            Decl::Actor {
                name,
                persistent,
                state_fields,
                behaviors,
                init,
                ..
            } => {
                self.compile_actor(name, *persistent, state_fields, behaviors, init, false, false, &[], None, None)?;
            }
            Decl::Agent { .. } => {
                self.compile_agent(decl)?;
            }

            Decl::TypeAlias { .. }
            | Decl::RecordType { .. }
            | Decl::VariantType { .. }
            | Decl::EffectDecl { .. }
            | Decl::Import { .. }
            | Decl::Extern { .. } => {}
            Decl::Module { decls, .. } => {
                for subdecl in decls {
                    self.compile_decl(subdecl)?;
                }
            }
            Decl::Workflow { name, items, span, .. } => {
                self.compile_workflow(name, items, *span)?;
            }
        }
        Ok(())
    }

    fn compile_function(
        &mut self,
        name: &str,
        params: &[(String, Option<crate::types::Type>)],
        _ret_type: Option<&Type>,
        body: &Expr,
        annotations: &[FunctionAnnotation],
    ) -> NuResult<()> {
        // Tool schemas for `@tool`-annotated functions are collected in
        // `collect_functions()` so they are available before agent declarations
        // are compiled.  We still validate the annotation here.
        if annotations.iter().any(|a| matches!(a, FunctionAnnotation::Tool { .. })) {
            for (param_name, param_ty) in params {
                if param_ty.is_none() {
                    return Err(NuError::ParseError {
                        msg: format!(
                            "@tool function '{}' parameter '{}' requires an explicit type",
                            name, param_name
                        ),
                        span: Span::default(),
                    });
                }
            }
        }

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

    fn compile_actor(
        &mut self,
        name: &str,
        persistent: bool,
        state_fields: &[(String, StateModel, Type, Expr)],
        behaviors: &[Behavior],
        init: &[(String, Expr)],
        is_workflow: bool,
        is_agent: bool,
        tools: &[ToolSchema],
        semantic_memory_dimensions: Option<usize>,
        procedural_memory_namespace: Option<String>,
    ) -> NuResult<()> {
        for (_field_name, expr) in init {
            let _ = self.compile_expr(expr)?;
        }
        let first_behavior_idx = self.module.behaviors.len();
        for behavior in behaviors {
            self.compile_behavior(behavior, name)?;
        }
        let behavior_indices: Vec<usize> =
            (first_behavior_idx..self.module.behaviors.len()).collect();
        let state_models: Vec<(String, StateModel)> = state_fields
            .iter()
            .map(|(field_name, model, _ty, _default)| (field_name.clone(), *model))
            .collect();
        let state_defaults: Vec<(String, Constant)> = state_fields
            .iter()
            .filter_map(|(field_name, _model, _ty, default)| {
                if let Expr::Literal(lit, _) = default {
                    Some((field_name.clone(), literal_to_constant(lit)))
                } else {
                    None
                }
            })
            .collect();
        let meta = ActorMeta {
            name: name.to_string(),
            persistent,
            state_models,
            state_defaults,
            behavior_indices,
            is_workflow,
            is_agent,
            tools: tools.to_vec(),
            semantic_memory_dimensions,
            procedural_memory_namespace,
        };
        self.module.add_actor_meta(meta);
        Ok(())
    }

    fn compile_agent(&mut self, agent: &Decl) -> NuResult<()> {
        // Extract fields from the agent declaration.  This pattern is irrefutable
        // because the caller already matched on Decl::Agent.
        let (name, model, system_prompt, tool_names, memory, semantic_memory, procedural_memory, pricing, span) = match agent {
            Decl::Agent {
                name,
                model,
                system_prompt,
                tools,
                memory,
                semantic_memory,
                procedural_memory,
                pricing,
                span,
            } => (name, model, system_prompt, tools, memory, semantic_memory, procedural_memory, *pricing, *span),
            _ => unreachable!(),
        };

        let agent_pricing = pricing.unwrap_or(crate::ast::AgentPricing {
            input: 0.0,
            output: 0.0,
        });

        let max_turns = memory.as_ref().map(|m| m.max_turns).unwrap_or(50);
        let initial_memory = serde_json::to_string(&EpisodicMemory::new(max_turns))
            .unwrap_or_else(|_| "{}".to_string());

        let semantic_memory_dimensions = semantic_memory.as_ref().map(|m| m.dimensions);
        let initial_semantic_memory = semantic_memory_dimensions
            .map(|dimensions| {
                serde_json::to_string(&crate::ai::SemanticMemory::new(dimensions, None))
                    .unwrap_or_else(|_| "{}".to_string())
            });

        let procedural_memory_namespace = procedural_memory.as_ref().map(|m| m.namespace.clone());
        let initial_procedural_memory = procedural_memory_namespace
            .as_ref()
            .map(|namespace| {
                serde_json::to_string(&crate::ai::ProceduralMemory::new(namespace.clone()))
                    .unwrap_or_else(|_| "{}".to_string())
            });

        // Agent actors keep their configuration in durable state.
        let mut state_fields: Vec<(String, StateModel, Type, Expr)> = vec![
            (
                "model".to_string(),
                StateModel::Durable,
                Type::string(),
                Expr::Literal(Literal::String(model.clone()), span),
            ),
            (
                "system_prompt".to_string(),
                StateModel::Durable,
                Type::string(),
                Expr::Literal(
                    Literal::String(system_prompt.clone().unwrap_or_default()),
                    span,
                ),
            ),
            (
                "episodic_memory".to_string(),
                StateModel::Durable,
                Type::string(),
                Expr::Literal(Literal::String(initial_memory), span),
            ),
            (
                "usage_prompt".to_string(),
                StateModel::Durable,
                Type::int(),
                Expr::Literal(Literal::Int(0), span),
            ),
            (
                "usage_completion".to_string(),
                StateModel::Durable,
                Type::int(),
                Expr::Literal(Literal::Int(0), span),
            ),
            (
                "usage_cost".to_string(),
                StateModel::Durable,
                Type::float(),
                Expr::Literal(Literal::Float(0.0), span),
            ),
            (
                "pricing_input".to_string(),
                StateModel::Durable,
                Type::float(),
                Expr::Literal(Literal::Float(agent_pricing.input), span),
            ),
            (
                "pricing_output".to_string(),
                StateModel::Durable,
                Type::float(),
                Expr::Literal(Literal::Float(agent_pricing.output), span),
            ),
        ];

        if let Some(json) = initial_semantic_memory {
            state_fields.push((
                "semantic_memory".to_string(),
                StateModel::Durable,
                Type::string(),
                Expr::Literal(Literal::String(json), span),
            ));
        }

        if let Some(json) = initial_procedural_memory {
            state_fields.push((
                "procedural_memory".to_string(),
                StateModel::Durable,
                Type::string(),
                Expr::Literal(Literal::String(json), span),
            ));
        }

        // Generated ask behavior reads agent state and performs the LLM ask.
        // The parser accepts `ask` as a behavior name after the `ask` keyword,
        // so agent actors can be invoked as `ask a ask("...")`.
        // The runtime recognizes agent actors and wires model/system/memory.
        let ask_behavior = Behavior {
            name: "ask".to_string(),
            params: vec![("prompt".to_string(), Some(Type::string()))],
            body: Expr::Block {
                exprs: vec![
                    Expr::FieldAccess {
                        expr: Box::new(Expr::SelfRef(span)),
                        field: "model".to_string(),
                        span,
                    },
                    Expr::FieldAccess {
                        expr: Box::new(Expr::SelfRef(span)),
                        field: "system_prompt".to_string(),
                        span,
                    },
                    Expr::FieldAccess {
                        expr: Box::new(Expr::SelfRef(span)),
                        field: "episodic_memory".to_string(),
                        span,
                    },
                    Expr::Perform {
                        effect: "LLM".to_string(),
                        op: "ask".to_string(),
                        args: vec![Expr::Var("prompt".to_string(), span)],
                        span,
                    },
                ],
                span,
            },
            effect: None,
            cap: Capability::Ref,
            span,
        };

        // Generated usage behavior returns cumulative usage/cost state as a
        // plain array [prompt_tokens, completion_tokens, cost] — a stable,
        // positional format for host code (e.g. integration tests) to parse
        // without depending on the record field-id allocator's ordering.
        let usage_behavior = Behavior {
            name: "usage".to_string(),
            params: vec![],
            body: Expr::Array(
                vec![
                    Expr::FieldAccess {
                        expr: Box::new(Expr::SelfRef(span)),
                        field: "usage_prompt".to_string(),
                        span,
                    },
                    Expr::FieldAccess {
                        expr: Box::new(Expr::SelfRef(span)),
                        field: "usage_completion".to_string(),
                        span,
                    },
                    Expr::FieldAccess {
                        expr: Box::new(Expr::SelfRef(span)),
                        field: "usage_cost".to_string(),
                        span,
                    },
                ],
                span,
            ),
            effect: None,
            cap: Capability::Ref,
            span,
        };

        // Generated semantic-memory behaviors.  The bodies are placeholders;
        // the runtime intercepts these behaviors and implements store/recall
        // directly against the agent's durable `semantic_memory` state field.
        let semantic_memory_behavior = if semantic_memory_dimensions.is_some() {
            let store_fact_behavior = Behavior {
                name: "store_fact".to_string(),
                params: vec![("content".to_string(), Some(Type::string()))],
                body: Expr::Literal(Literal::Unit, span),
                effect: None,
                cap: Capability::Ref,
                span,
            };
            let recall_behavior = Behavior {
                name: "recall".to_string(),
                params: vec![
                    ("query".to_string(), Some(Type::string())),
                    ("top_k".to_string(), Some(Type::int())),
                ],
                body: Expr::Literal(Literal::Unit, span),
                effect: None,
                cap: Capability::Ref,
                span,
            };
            Some((store_fact_behavior, recall_behavior))
        } else {
            None
        };

        // Generated procedural-memory behaviors.  The bodies are placeholders;
        // the runtime intercepts these behaviors and implements store/retrieve
        // directly against the agent's durable `procedural_memory` state field.
        let procedural_memory_behavior = if procedural_memory_namespace.is_some() {
            let store_pattern_behavior = Behavior {
                name: "store_pattern".to_string(),
                params: vec![
                    ("key".to_string(), Some(Type::string())),
                    ("input_pattern".to_string(), Some(Type::string())),
                    ("output_template".to_string(), Some(Type::string())),
                ],
                body: Expr::Literal(Literal::Unit, span),
                effect: None,
                cap: Capability::Ref,
                span,
            };
            let get_pattern_behavior = Behavior {
                name: "get_pattern".to_string(),
                params: vec![("key".to_string(), Some(Type::string()))],
                body: Expr::Literal(Literal::Unit, span),
                effect: None,
                cap: Capability::Ref,
                span,
            };
            let add_example_behavior = Behavior {
                name: "add_example".to_string(),
                params: vec![
                    ("task".to_string(), Some(Type::string())),
                    ("input".to_string(), Some(Type::string())),
                    ("output".to_string(), Some(Type::string())),
                ],
                body: Expr::Literal(Literal::Unit, span),
                effect: None,
                cap: Capability::Ref,
                span,
            };
            let get_examples_behavior = Behavior {
                name: "get_examples".to_string(),
                params: vec![
                    ("task".to_string(), Some(Type::string())),
                    ("query".to_string(), Some(Type::string())),
                    ("top_k".to_string(), Some(Type::int())),
                ],
                body: Expr::Literal(Literal::Unit, span),
                effect: None,
                cap: Capability::Ref,
                span,
            };
            Some((
                store_pattern_behavior,
                get_pattern_behavior,
                add_example_behavior,
                get_examples_behavior,
            ))
        } else {
            None
        };

        // Resolve tool names against the module's tool schemas.
        let mut resolved_tools = Vec::new();
        for tool_name in tool_names {
            match self.module.tools.iter().find(|t| &t.name == tool_name) {
                Some(schema) => resolved_tools.push(schema.clone()),
                None => {
                    return Err(NuError::ParseError {
                        msg: format!(
                            "Agent '{}' references unknown tool '{}' (missing @tool annotation?)",
                            name, tool_name
                        ),
                        span,
                    });
                }
            }
        }

        let mut behaviors: Vec<Behavior> = vec![ask_behavior, usage_behavior];
        if let Some((store_fact, recall)) = semantic_memory_behavior {
            behaviors.push(store_fact);
            behaviors.push(recall);
        }
        if let Some((store_pattern, get_pattern, add_example, get_examples)) = procedural_memory_behavior {
            behaviors.push(store_pattern);
            behaviors.push(get_pattern);
            behaviors.push(add_example);
            behaviors.push(get_examples);
        }

        self.compile_actor(
            name,
            true,
            &state_fields,
            &behaviors,
            &[],
            false,
            true,
            &resolved_tools,
            semantic_memory_dimensions,
            procedural_memory_namespace,
        )
    }

    fn compile_workflow(
        &mut self,
        name: &str,
        items: &[WorkflowItem],
        span: Span,
    ) -> NuResult<()> {
        // Flatten the ordered workflow items into a list of sequential behaviors.
        // Each `parallel` block becomes a synthetic step whose body runs branches
        // sequentially and emits a durable `ParallelBranchCompleted` event after
        // each branch.  A durable `parallel_progress` counter lets recovery skip
        // branches that already completed before a crash.
        let mut flattened_steps: Vec<WorkflowStep> = Vec::new();
        let mut parallel_branches: std::collections::HashMap<usize, Vec<String>> =
            std::collections::HashMap::new();
        let mut parallel_counter = 0usize;

        for item in items {
            match item {
                WorkflowItem::Step(step) => {
                    flattened_steps.push(step.clone());
                }
                WorkflowItem::Parallel(branches) => {
                    let parallel_name = format!("parallel_{}", parallel_counter);
                    parallel_counter += 1;

                    let progress_expr = Expr::FieldAccess {
                        expr: Box::new(Expr::SelfRef(span)),
                        field: "parallel_progress".to_string(),
                        span,
                    };
                    let mut body_exprs: Vec<Expr> = Vec::with_capacity(branches.len() + 1);

                    for (branch_idx, branch) in branches.iter().enumerate() {
                        let threshold = (branch_idx + 1) as i64;
                        let guard = Expr::Binary {
                            op: BinOp::Lt,
                            left: Box::new(progress_expr.clone()),
                            right: Box::new(Expr::Literal(Literal::Int(threshold), span)),
                            span,
                        };
                        let branch_block = Expr::Block {
                            exprs: vec![
                                branch.body.clone(),
                                Expr::Emit {
                                    event: "ParallelBranchCompleted".to_string(),
                                    args: vec![
                                        Expr::Literal(
                                            Literal::String(parallel_name.clone()),
                                            span,
                                        ),
                                        Expr::Literal(
                                            Literal::String(branch.name.clone()),
                                            span,
                                        ),
                                    ],
                                    span,
                                },
                            ],
                            span,
                        };
                        body_exprs.push(Expr::If {
                            cond: Box::new(guard),
                            then_branch: Box::new(branch_block),
                            else_branch: None,
                            span,
                        });
                    }

                    // Reset the parallel-progress counter once every branch has
                    // finished.  The runtime advances step_index when it records
                    // StepCompleted so that signal-waiting branches do not
                    // double-increment.
                    body_exprs.push(Expr::Assign {
                        target: Box::new(progress_expr.clone()),
                        value: Box::new(Expr::Literal(Literal::Int(0), span)),
                        span,
                    });

                    let combined_compensate = {
                        let comp_exprs: Vec<Expr> = branches
                            .iter()
                            .rev()
                            .filter_map(|b| b.compensate.clone())
                            .collect();
                        if comp_exprs.is_empty() {
                            None
                        } else {
                            Some(Expr::Block {
                                exprs: comp_exprs,
                                span,
                            })
                        }
                    };

                    flattened_steps.push(WorkflowStep {
                        name: parallel_name.clone(),
                        body: Expr::Block {
                            exprs: body_exprs,
                            span,
                        },
                        compensate: combined_compensate,
                        span,
                    });
                    parallel_branches.insert(
                        flattened_steps.len() - 1,
                        branches.iter().map(|b| b.name.clone()).collect(),
                    );
                }
            }
        }

        // A workflow is a persistent actor with one behavior per flattened step
        // plus durable step_index and parallel_progress counters.
        let state_fields: Vec<(String, StateModel, Type, Expr)> = vec![
            (
                "step_index".to_string(),
                StateModel::Durable,
                Type::int(),
                Expr::Literal(Literal::Int(0), span),
            ),
            (
                "workflow_name".to_string(),
                StateModel::Durable,
                Type::string(),
                Expr::Literal(Literal::String(name.to_string()), span),
            ),
            (
                "parallel_progress".to_string(),
                StateModel::Durable,
                Type::int(),
                Expr::Literal(Literal::Int(0), span),
            ),
        ];
        let behaviors: Vec<Behavior> = flattened_steps
            .iter()
            .map(|s| Behavior {
                name: s.name.clone(),
                params: Vec::new(),
                body: s.body.clone(),
                effect: None,
                cap: Capability::Ref,
                span: s.span,
            })
            .collect();
        let first_behavior_idx = self.module.behaviors.len();
        self.compile_actor(name, true, &state_fields, &behaviors, &[], true, false, &[], None, None)?;

        // Patch parallel-branch metadata and per-step saga compensation offsets.
        let behavior_indices: Vec<usize> =
            (first_behavior_idx..self.module.behaviors.len()).collect();
        for (i, step) in flattened_steps.iter().enumerate() {
            let behavior_idx = behavior_indices.get(i).copied().unwrap_or(first_behavior_idx + i);
            if let Some(entry) = self.module.behaviors.get_mut(behavior_idx) {
                if let Some(branches) = parallel_branches.get(&i) {
                    entry.parallel_branches = Some(branches.clone());
                }
            }
            if let Some(comp_expr) = &step.compensate {
                let comp_offset = self.compile_compensation(comp_expr)?;
                if let Some(entry) = self.module.behaviors.get_mut(behavior_idx) {
                    entry.compensate_offset = Some(comp_offset);
                }
            }
        }
        Ok(())
    }

    /// Compile a saga compensation expression as a standalone code block.
    /// Returns the code offset of the compensation body.
    fn compile_compensation(&mut self, expr: &Expr) -> NuResult<usize> {
        let saved_locals = std::mem::replace(&mut self.locals, vec![ScopeFrame::new()]);
        let saved_next_reg = self.next_reg;
        self.next_reg = 0;

        let self_reg = self.alloc_reg();
        self.emit(Instruction::new1(op_self(), self_reg));
        self.define_local("self", self_reg);

        let start_offset = self.module.current_offset();
        let body_reg = self.compile_expr(expr)?;
        self.emit(Instruction::new1(OpCode::RetVal, body_reg));

        self.locals = saved_locals;
        self.next_reg = saved_next_reg;
        Ok(start_offset)
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
            compensate_offset: None,
            parallel_branches: None,
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
        const MAX_TEMP_REG: u8 = 239;
        if self.next_reg > MAX_TEMP_REG {
            self.reg_overflow = true;
        }
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
    fn current_offset(&self) -> usize { self.module.current_offset() }

    /// Look up (or assign) the module-wide positional id for a record field
    /// name. Field access is positional, keyed by these ids, so every read
    /// and write of a given field name must resolve to the same slot.
    fn field_id(&mut self, field: &str) -> NuResult<u8> {
        if let Some(&id) = self.field_map.get(field) {
            return Ok(id);
        }
        if self.field_map.len() >= u8::MAX as usize + 1 {
            // The 256th distinct field name has no free id: field ids are a
            // single byte, so this module needs a wider encoding. Erroring
            // here is the honest outcome — silently reusing an existing id
            // (the old `saturating_add` behavior) would alias two unrelated
            // fields onto the same slot, corrupting whichever one loses.
            return Err(NuError::VMError(format!(
                "module has more than {} distinct record/tuple field names (limit for the current u8 field-id encoding); '{}' has no id left to assign",
                u8::MAX as usize + 1,
                field
            )));
        }
        let id = self.next_field_id;
        self.next_field_id = self.next_field_id.saturating_add(1);
        self.field_map.insert(field.to_string(), id);
        Ok(id)
    }

    // ===================================================================
    // Python Interop Bytecode Emission
    // ===================================================================

    // -- Python Interop — RESERVED --
    //
    // These emit methods are retained for API compatibility but emit
    // reserved opcodes that trap at runtime with a clear error message.
    // Python interop goes through the PyO3 bridge (src/python/bridge.rs),
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

/// Convert an AST literal into a bytecode constant.
fn literal_to_constant(lit: &Literal) -> Constant {
    match lit {
        Literal::Int(n) => Constant::Int(*n),
        Literal::Float(f) => Constant::Float(*f),
        Literal::String(s) => Constant::String(s.clone()),
        Literal::Bool(b) => Constant::Bool(*b),
        Literal::Nil => Constant::Nil,
        Literal::Unit => Constant::Unit,
    }
}

// Helper: create a ConstU instruction with constant pool index and destination register.
fn make_constu(idx: u16, dst: u8) -> Instruction {
    Instruction::new3(OpCode::ConstU, ((idx >> 8) & 0xFF) as u8, (idx & 0xFF) as u8, dst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn compile_source(source: &str) -> CodeModule {
        let tokens = Lexer::new(source).lex().unwrap();
        let ast = Parser::new(tokens).parse_module().unwrap();
        let mut compiler = Compiler::new("test");
        compiler.compile_module(&ast).unwrap().clone()
    }

    #[test]
    fn test_actor_meta_persistence_and_state_models() {
        let source = r#"
            persistent actor BankAccount {
                state durable balance: Int = 0
                state local temp: Int = 0
                behavior get() { self.balance }
            }
        "#;
        let module = compile_source(source);
        assert_eq!(module.actor_metadata.len(), 1);
        let meta = &module.actor_metadata[0];
        assert_eq!(meta.name, "BankAccount");
        assert!(meta.persistent);
        assert_eq!(
            meta.state_models,
            vec![
                ("balance".to_string(), StateModel::Durable),
                ("temp".to_string(), StateModel::Local),
            ]
        );
        assert_eq!(meta.behavior_indices, vec![0]);
    }

    #[test]
    fn test_non_persistent_actor_meta() {
        let source = r#"
            actor Counter {
                state count = 0
                behavior inc() { self.count + 1 }
            }
        "#;
        let module = compile_source(source);
        assert_eq!(module.actor_metadata.len(), 1);
        let meta = &module.actor_metadata[0];
        assert_eq!(meta.name, "Counter");
        assert!(!meta.persistent);
        assert_eq!(meta.state_models, vec![("count".to_string(), StateModel::Local)]);
        assert_eq!(meta.behavior_indices, vec![0]);
    }

    #[test]
    fn test_compile_spawn_references_actor_meta() {
        let source = r#"
            actor Counter {
                state count = 0
                behavior inc() { self.count + 1 }
            }
            spawn Counter { count = 0 }
        "#;
        let module = compile_source(source);
        assert_eq!(module.actor_metadata.len(), 1);
        let meta = &module.actor_metadata[0];
        assert_eq!(meta.name, "Counter");
        assert_eq!(meta.behavior_indices, vec![0]);

        let spawn_instr = module
            .instructions
            .iter()
            .find(|i| i.opcode == OpCode::Spawn)
            .expect("Expected a Spawn instruction");
        assert_eq!(spawn_instr.imm16() as usize, meta.behavior_indices[0]);
    }

    #[test]
    fn test_actor_meta_multiple_behaviors() {
        let source = r#"
            actor Counter {
                state durable count: Int = 0
                behavior get() { self.count }
                behavior inc() { self.count + 1 }
                behavior dec() { self.count - 1 }
            }
        "#;
        let module = compile_source(source);
        assert_eq!(module.actor_metadata.len(), 1);
        let meta = &module.actor_metadata[0];
        assert_eq!(meta.behavior_indices, vec![0, 1, 2]);
    }

    #[test]
    fn test_field_id_errors_past_256_distinct_field_names_instead_of_aliasing() {
        // 256 distinct field names fit exactly in the u8 id encoding; the
        // 257th must be an honest compile error, not a silently reused id
        // that aliases two unrelated fields onto the same record slot.
        //
        // Each field name lives in its own top-level function's own tiny
        // record literal (not a single 257-field record, and not `let`
        // bindings) so this only exercises field_id's cumulative,
        // module-wide id allocation — not the unrelated, much lower
        // per-function/per-module register caps (~240 temporaries per
        // function, ~14 `let` bindings per module).
        let fns: Vec<String> = (0..257)
            .map(|i| format!("fn g{i}() -> Int {{ {{ f{i}: {i} }}.f{i} }}"))
            .collect();
        let source = format!("{}\ng0()", fns.join("\n"));

        let tokens = Lexer::new(&source).lex().unwrap();
        let ast = Parser::new(tokens).parse_module().unwrap();
        let mut compiler = Compiler::new("test");
        let result = compiler.compile_module(&ast);
        assert!(
            result.is_err(),
            "the 257th distinct field name should be an honest error, not silent aliasing"
        );
    }

    #[test]
    fn test_compile_tool_annotation_generates_schema() {
        let source = r#"
            @tool(description: "Adds two integers.")
            pub fn add(x: Int, y: Int) -> Int { x + y }
        "#;
        let module = compile_source(source);
        assert_eq!(module.tools.len(), 1);
        let tool = &module.tools[0];
        assert_eq!(tool.name, "add");
        assert_eq!(tool.description, "Adds two integers.");
        assert_eq!(tool.parameters["type"], "object");
        assert_eq!(tool.parameters["properties"]["x"], serde_json::json!({"type": "integer"}));
        assert_eq!(tool.parameters["properties"]["y"], serde_json::json!({"type": "integer"}));
        assert!(tool.parameters["required"].as_array().unwrap().contains(&serde_json::json!("x")));
        assert!(tool.parameters["required"].as_array().unwrap().contains(&serde_json::json!("y")));
    }

    #[test]
    fn test_compile_agent_lowers_to_durable_actor() {
        let source = r#"
            @tool(description: "Adds two integers.")
            fn add(x: Int, y: Int) -> Int { x + y }

            agent MyAgent = {
                model: "gpt-4o",
                system_prompt: "You are helpful.",
                tools: [add],
                memory: { max_turns: 10 }
            }
        "#;
        let module = compile_source(source);
        assert_eq!(module.actor_metadata.len(), 1);
        let meta = &module.actor_metadata[0];
        assert_eq!(meta.name, "MyAgent");
        assert!(meta.persistent);
        assert!(meta.is_agent);
        assert!(!meta.is_workflow);
        assert_eq!(meta.state_models.len(), 8);
        assert!(meta.state_models.iter().all(|(_, m)| *m == StateModel::Durable));
        let field_names: std::collections::HashSet<&str> = meta
            .state_models
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert!(field_names.contains("model"));
        assert!(field_names.contains("system_prompt"));
        assert!(field_names.contains("episodic_memory"));
        assert!(field_names.contains("usage_prompt"));
        assert!(field_names.contains("usage_completion"));
        assert!(field_names.contains("usage_cost"));
        assert!(field_names.contains("pricing_input"));
        assert!(field_names.contains("pricing_output"));
        assert_eq!(meta.behavior_indices.len(), 2);
        let ask_behavior = &module.behaviors[meta.behavior_indices[0]];
        assert_eq!(ask_behavior.name, "MyAgent.ask");
        assert_eq!(ask_behavior.param_count, 1);
        let usage_behavior = &module.behaviors[meta.behavior_indices[1]];
        assert_eq!(usage_behavior.name, "MyAgent.usage");
        assert_eq!(usage_behavior.param_count, 0);
        assert_eq!(meta.tools.len(), 1);
        assert_eq!(meta.tools[0].name, "add");
    }

    #[test]
    fn test_compile_agent_unknown_tool_errors() {
        let source = r#"
            agent MyAgent = {
                model: "gpt-4o",
                tools: [nonexistent]
            }
        "#;
        let tokens = Lexer::new(source).lex().unwrap();
        let ast = Parser::new(tokens).parse_module().unwrap();
        let mut compiler = Compiler::new("test");
        assert!(compiler.compile_module(&ast).is_err());
    }
}
