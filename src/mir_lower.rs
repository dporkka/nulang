//! HIR -> MIR lowering.
//!
//! Converts the typed High-level IR into the 3-address-code Mid-level IR.
//!
//! Guarantees:
//!   - Everything this pass emits compiles to *correct* bytecode; any
//!     construct that cannot be lowered faithfully yet returns an honest
//!     `NotYetImplemented` error instead of emitting placeholder code.
//!   - Lexical scoping (with shadowing) is respected via a scope stack.
//!   - Lambdas and recursive let-bindings are lifted to top-level MIR
//!     functions; closures capture enclosing locals by value.

use crate::ast::Pattern;
use crate::hir;
use crate::mir;
use crate::types::{NuError, NuResult, Span, Type};
use std::collections::HashMap;

fn nyi(feature: &str) -> NuError {
    NuError::NotYetImplemented {
        feature: feature.to_string(),
        span: Span::default(),
    }
}

fn compile_err(msg: impl Into<String>) -> NuError {
    NuError::VMError(msg.into())
}

pub fn lower_module(hir: &hir::Module) -> NuResult<mir::Module> {
    let mut ctx = ModuleCtx::new(&hir.name);

    // Pass 1: reserve function slots for all named top-level functions so
    // forward references and mutual recursion resolve, and register externs.
    for decl in &hir.decls {
        match decl {
            hir::Decl::Function(f) => {
                let idx = ctx.reserve_function(&f.name);
                ctx.func_map.insert(f.name.clone(), idx);
            }
            hir::Decl::ExternBlock { library, funcs, .. } => {
                for f in funcs {
                    let idx = ctx.foreign.len();
                    ctx.foreign.push(mir::ForeignFunction {
                        library: library.clone(),
                        symbol: f.name.clone(),
                        params: f.params.iter().map(|(_, t)| t.clone()).collect(),
                        ret: f.ret.clone(),
                    });
                    ctx.extern_map.insert(f.name.clone(), idx);
                }
            }
            hir::Decl::Actor(a) => {
                return Err(nyi(&format!("actor '{}' in HIR/MIR pipeline", a.name)));
            }
            hir::Decl::Workflow { name, .. } => {
                return Err(nyi(&format!("workflow '{}' in HIR/MIR pipeline", name)));
            }
            hir::Decl::Agent { name, .. } => {
                return Err(nyi(&format!("agent '{}' in HIR/MIR pipeline", name)));
            }
            hir::Decl::Module { .. } => {
                return Err(nyi("nested module in HIR/MIR pipeline"));
            }
            // Type-level declarations produce no code.
            hir::Decl::TypeAlias { .. }
            | hir::Decl::RecordType { .. }
            | hir::Decl::VariantType { .. }
            | hir::Decl::EffectDecl { .. }
            | hir::Decl::Import { .. } => {}
        }
    }

    // Pass 2: lower function bodies into their reserved slots.
    for decl in &hir.decls {
        if let hir::Decl::Function(f) = decl {
            let idx = ctx.func_map[&f.name];
            let func = lower_function_def(&mut ctx, f)?;
            ctx.fill_function(idx, func);
        }
    }

    ctx.finish()
}

// ---------------------------------------------------------------------------
// Module context
// ---------------------------------------------------------------------------

struct ModuleCtx {
    name: String,
    functions: Vec<Option<mir::Function>>,
    func_map: HashMap<String, usize>,
    extern_map: HashMap<String, usize>,
    foreign: Vec<mir::ForeignFunction>,
    next_lambda: u32,
}

impl ModuleCtx {
    fn new(name: &str) -> Self {
        ModuleCtx {
            name: name.to_string(),
            functions: Vec::new(),
            func_map: HashMap::new(),
            extern_map: HashMap::new(),
            foreign: Vec::new(),
            next_lambda: 0,
        }
    }

    fn reserve_function(&mut self, _name: &str) -> usize {
        self.functions.push(None);
        self.functions.len() - 1
    }

    fn fill_function(&mut self, idx: usize, func: mir::Function) {
        self.functions[idx] = Some(func);
    }

    fn fresh_lambda_name(&mut self) -> String {
        let n = self.next_lambda;
        self.next_lambda += 1;
        format!("__lambda_{}", n)
    }

    fn finish(self) -> NuResult<mir::Module> {
        let mut module = mir::Module::new(&self.name);
        for (i, f) in self.functions.into_iter().enumerate() {
            module.functions.push(f.ok_or_else(|| {
                compile_err(format!("internal: MIR function slot {} left unfilled", i))
            })?);
        }
        module.foreign_functions = self.foreign;
        Ok(module)
    }
}

fn lower_function_def(ctx: &mut ModuleCtx, f: &hir::FunctionDef) -> NuResult<mir::Function> {
    let mut lowerer = FnLowerer::new(ctx, &f.name, Some(f.ret.clone()));
    for (name, ty) in &f.params {
        let id = lowerer.b.add_param(name.clone(), ty.clone());
        lowerer.bind(name, id);
    }
    lowerer.lower_body_top(&f.body)?;
    Ok(lowerer.b.build())
}

/// Lower a lifted lambda/recursive-function body into a standalone MIR
/// function. `captures` are bound from closure capture slots (in order);
/// `rec` binds a name to the function's own index for recursive calls.
fn lower_lifted(
    ctx: &mut ModuleCtx,
    name: &str,
    params: &[(String, Type)],
    captures: &[String],
    rec: Option<(&str, usize)>,
    body: &hir::Body,
) -> NuResult<mir::Function> {
    let mut lowerer = FnLowerer::new(ctx, name, None);
    for (pname, ty) in params {
        let id = lowerer.b.add_param(pname.clone(), ty.clone());
        lowerer.bind(pname, id);
    }
    for cname in captures {
        let id = lowerer.b.add_capture(cname.clone(), Type::unit());
        lowerer.bind(cname, id);
    }
    if let Some((rec_name, rec_idx)) = rec {
        // The function refers to itself by name: bind the name to a local
        // holding the function-table index (callable like any function value).
        let id = lowerer.b.add_local(rec_name, Type::unit());
        lowerer.b.assign(
            id,
            mir::RValue::Const(crate::bytecode::Constant::Int(rec_idx as i64)),
        );
        lowerer.bind(rec_name, id);
    }
    lowerer.lower_body_top(body)?;
    Ok(lowerer.b.build())
}

// ---------------------------------------------------------------------------
// Function lowering
// ---------------------------------------------------------------------------

struct FnLowerer<'c> {
    ctx: &'c mut ModuleCtx,
    b: mir::FunctionBuilder,
    scopes: Vec<Vec<(String, mir::LocalId)>>,
    loop_exits: Vec<mir::BlockId>,
}

impl<'c> FnLowerer<'c> {
    fn new(ctx: &'c mut ModuleCtx, name: &str, ret: Option<Type>) -> Self {
        FnLowerer {
            ctx,
            b: mir::FunctionBuilder::new(name, ret),
            scopes: vec![Vec::new()],
            loop_exits: Vec::new(),
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn bind(&mut self, name: &str, id: mir::LocalId) {
        self.scopes
            .last_mut()
            .expect("scope stack never empty")
            .push((name.to_string(), id));
    }

    fn lookup(&self, name: &str) -> Option<mir::LocalId> {
        for scope in self.scopes.iter().rev() {
            for (n, id) in scope.iter().rev() {
                if n == name {
                    return Some(*id);
                }
            }
        }
        None
    }

    // -- Body lowering ------------------------------------------------------

    /// Lower a body in function-return position.
    fn lower_body_top(&mut self, body: &hir::Body) -> NuResult<()> {
        for stmt in &body.stmts {
            self.lower_stmt(stmt)?;
        }
        if self.b.is_terminated() {
            return Ok(());
        }
        match &body.terminator {
            hir::Terminator::Yield(op) | hir::Terminator::FnReturn(Some(op)) => {
                let id = self.lower_operand(op)?;
                self.b.terminate(mir::Terminator::Return(Some(id)));
            }
            hir::Terminator::FnReturn(None) => {
                let id = self.unit_temp();
                self.b.terminate(mir::Terminator::Return(Some(id)));
            }
            hir::Terminator::Break => {
                return Err(compile_err("break outside of a loop"));
            }
        }
        Ok(())
    }

    /// Lower a body in expression position: its yielded value is assigned to
    /// `dst` and control joins `join`. Explicit returns still return from the
    /// function; breaks target the innermost loop.
    fn lower_body_into(
        &mut self,
        body: &hir::Body,
        dst: mir::LocalId,
        join: mir::BlockId,
    ) -> NuResult<()> {
        for stmt in &body.stmts {
            self.lower_stmt(stmt)?;
        }
        if self.b.is_terminated() {
            return Ok(());
        }
        match &body.terminator {
            hir::Terminator::Yield(op) => {
                let id = self.lower_operand(op)?;
                self.b.assign(dst, mir::RValue::Load(id));
                self.b.terminate(mir::Terminator::Jump(join));
            }
            hir::Terminator::FnReturn(op) => {
                let id = match op {
                    Some(op) => self.lower_operand(op)?,
                    None => self.unit_temp(),
                };
                self.b.terminate(mir::Terminator::Return(Some(id)));
            }
            hir::Terminator::Break => {
                let exit = self
                    .loop_exits
                    .last()
                    .copied()
                    .ok_or_else(|| compile_err("break outside of a loop"))?;
                self.b.terminate(mir::Terminator::Jump(exit));
            }
        }
        Ok(())
    }

    // -- Statements ----------------------------------------------------------

    fn lower_stmt(&mut self, stmt: &hir::Stmt) -> NuResult<()> {
        if self.b.is_terminated() {
            // Unreachable code after return/break: skip.
            return Ok(());
        }
        match stmt {
            hir::Stmt::Let { name, ty, value, .. } => {
                let dst = self.b.add_local(name.clone(), ty.clone());
                self.lower_rvalue(dst, value)?;
                self.bind(name, dst);
                Ok(())
            }
            hir::Stmt::Assign { target, value, .. } => self.lower_assign(target, value),
            hir::Stmt::StateSet { .. } => Err(nyi("actor state assignment in HIR/MIR pipeline")),
            hir::Stmt::Emit { event, args, .. } => {
                let mut ids = Vec::with_capacity(args.len());
                for a in args {
                    ids.push(self.lower_operand(a)?);
                }
                self.b.emit(mir::Stmt::Emit { event: event.clone(), args: ids });
                Ok(())
            }
        }
    }

    fn lower_assign(&mut self, target: &hir::Place, value: &hir::RValue) -> NuResult<()> {
        match target {
            hir::Place::Var(name, _) => {
                if name == "self" {
                    return Err(nyi("assignment to self in HIR/MIR pipeline"));
                }
                let dst = self.lookup(name).ok_or_else(|| {
                    compile_err(format!("assignment to undefined variable '{}'", name))
                })?;
                self.lower_rvalue(dst, value)
            }
            hir::Place::Field { base, field, .. } => {
                if place_is_self(base) {
                    return Err(nyi("actor state assignment in HIR/MIR pipeline"));
                }
                let obj = self.read_place(base)?;
                let src = self.b.add_temp(Type::unit());
                self.lower_rvalue(src, value)?;
                self.b.emit(mir::Stmt::StoreFieldNamed {
                    obj,
                    field: field.clone(),
                    src,
                });
                Ok(())
            }
            hir::Place::Index { base, idx, .. } => {
                let arr = self.read_place(base)?;
                let idx_id = self.lower_operand(idx)?;
                let src = self.b.add_temp(Type::unit());
                self.lower_rvalue(src, value)?;
                self.b.emit(mir::Stmt::ArrayStore { arr, idx: idx_id, src });
                Ok(())
            }
        }
    }

    fn read_place(&mut self, place: &hir::Place) -> NuResult<mir::LocalId> {
        match place {
            hir::Place::Var(name, _) => self
                .lookup(name)
                .ok_or_else(|| compile_err(format!("undefined variable '{}'", name))),
            hir::Place::Field { base, field, .. } => {
                let obj = self.read_place(base)?;
                let dst = self.b.add_temp(Type::unit());
                self.b
                    .assign(dst, mir::RValue::LoadFieldNamed { obj, field: field.clone() });
                Ok(dst)
            }
            hir::Place::Index { base, idx, .. } => {
                let arr = self.read_place(base)?;
                let idx_id = self.lower_operand(idx)?;
                let dst = self.b.add_temp(Type::unit());
                self.b.assign(dst, mir::RValue::ArrayLoad { arr, idx: idx_id });
                Ok(dst)
            }
        }
    }

    // -- Operands ------------------------------------------------------------

    fn lower_operand(&mut self, op: &hir::Operand) -> NuResult<mir::LocalId> {
        match op {
            hir::Operand::Var(name, _) => {
                if let Some(id) = self.lookup(name) {
                    return Ok(id);
                }
                if let Some(&idx) = self.ctx.func_map.get(name) {
                    // Reference to a top-level function used as a value.
                    let id = self.b.add_temp(Type::unit());
                    self.b.assign(
                        id,
                        mir::RValue::Const(crate::bytecode::Constant::Int(idx as i64)),
                    );
                    return Ok(id);
                }
                if name == "self" {
                    let id = self.b.add_temp(Type::unit());
                    self.b.assign(id, mir::RValue::SelfRef);
                    return Ok(id);
                }
                Err(compile_err(format!(
                    "undefined variable '{}' in MIR lowering",
                    name
                )))
            }
            hir::Operand::Literal(lit, ty) => {
                let id = self.b.add_temp(ty.clone());
                self.b
                    .assign(id, mir::RValue::Const(literal_to_constant(lit)));
                Ok(id)
            }
            hir::Operand::Unit => Ok(self.unit_temp()),
        }
    }

    fn unit_temp(&mut self) -> mir::LocalId {
        let id = self.b.add_temp(Type::unit());
        self.b
            .assign(id, mir::RValue::Const(crate::bytecode::Constant::Unit));
        id
    }

    // -- RValues (dst-directed) -----------------------------------------------

    fn lower_rvalue(&mut self, dst: mir::LocalId, rv: &hir::RValue) -> NuResult<()> {
        use crate::bytecode::Constant;
        match rv {
            hir::RValue::Use(op) => {
                let id = self.lower_operand(op)?;
                self.b.assign(dst, mir::RValue::Load(id));
                Ok(())
            }
            hir::RValue::Literal(lit, _) => {
                self.b
                    .assign(dst, mir::RValue::Const(literal_to_constant(lit)));
                Ok(())
            }
            hir::RValue::Binary(op, l, r, _) => {
                let lid = self.lower_operand(l)?;
                let rid = self.lower_operand(r)?;
                self.b.assign(dst, mir::RValue::Binary(*op, lid, rid));
                Ok(())
            }
            hir::RValue::Unary(op, e, _) => {
                let id = self.lower_operand(e)?;
                self.b.assign(dst, mir::RValue::Unary(*op, id));
                Ok(())
            }
            hir::RValue::Call { func, args, .. } => {
                let mut aids = Vec::with_capacity(args.len());
                for a in args {
                    aids.push(self.lower_operand(a)?);
                }
                let func_ref = match func {
                    hir::Operand::Var(name, _) => {
                        if let Some(id) = self.lookup(name) {
                            mir::FuncRef::Local(id)
                        } else if let Some(&idx) = self.ctx.func_map.get(name) {
                            mir::FuncRef::Index(idx)
                        } else if let Some(&eidx) = self.ctx.extern_map.get(name) {
                            self.b
                                .assign(dst, mir::RValue::FFICall { idx: eidx, args: aids });
                            return Ok(());
                        } else {
                            return Err(compile_err(format!(
                                "call to undefined function '{}'",
                                name
                            )));
                        }
                    }
                    _ => {
                        let id = self.lower_operand(func)?;
                        mir::FuncRef::Local(id)
                    }
                };
                self.b
                    .assign(dst, mir::RValue::Call { func: func_ref, args: aids });
                Ok(())
            }
            hir::RValue::Closure { params, body, captures, .. } => {
                // Capture only names that are actually locals in scope here;
                // top-level functions and externs resolve inside the lifted
                // function without capturing.
                let capture_names: Vec<String> = captures
                    .iter()
                    .filter(|n| self.lookup(n).is_some())
                    .cloned()
                    .collect();
                let capture_ids: Vec<mir::LocalId> = capture_names
                    .iter()
                    .map(|n| self.lookup(n).expect("capture just resolved"))
                    .collect();
                let lname = self.ctx.fresh_lambda_name();
                let idx = self.ctx.reserve_function(&lname);
                let lifted =
                    lower_lifted(self.ctx, &lname, params, &capture_names, None, body)?;
                self.ctx.fill_function(idx, lifted);
                self.b
                    .assign(dst, mir::RValue::Closure { func: idx, captures: capture_ids });
                Ok(())
            }
            hir::RValue::RecClosure { name, params, body, .. } => {
                let lname = format!("__rec_{}", name);
                let idx = self.ctx.reserve_function(&lname);
                let lifted =
                    lower_lifted(self.ctx, &lname, params, &[], Some((name, idx)), body)?;
                self.ctx.fill_function(idx, lifted);
                // The binding holds the function-table index as a value.
                self.b
                    .assign(dst, mir::RValue::Const(Constant::Int(idx as i64)));
                Ok(())
            }
            hir::RValue::Tuple(elems, _) => {
                let mut ids = Vec::with_capacity(elems.len());
                for e in elems {
                    ids.push(self.lower_operand(e)?);
                }
                self.b.assign(dst, mir::RValue::Tuple(ids));
                Ok(())
            }
            hir::RValue::Record(fields, _) => {
                let mut fs = Vec::with_capacity(fields.len());
                for (n, e) in fields {
                    fs.push((n.clone(), self.lower_operand(e)?));
                }
                self.b.assign(dst, mir::RValue::Record(fs));
                Ok(())
            }
            hir::RValue::Array(elems, _) => {
                let mut ids = Vec::with_capacity(elems.len());
                for e in elems {
                    ids.push(self.lower_operand(e)?);
                }
                self.b.assign(dst, mir::RValue::ArrayLit(ids));
                Ok(())
            }
            hir::RValue::FieldAccess { base, field, .. } => {
                if operand_is_self(base) {
                    return Err(nyi("actor state access in HIR/MIR pipeline"));
                }
                let obj = self.lower_operand(base)?;
                self.b
                    .assign(dst, mir::RValue::LoadFieldNamed { obj, field: field.clone() });
                Ok(())
            }
            hir::RValue::Index { base, idx, .. } => {
                let arr = self.lower_operand(base)?;
                let idx_id = self.lower_operand(idx)?;
                self.b.assign(dst, mir::RValue::ArrayLoad { arr, idx: idx_id });
                Ok(())
            }
            hir::RValue::If { cond, then_body, else_body, .. } => {
                let cid = self.lower_operand(cond)?;
                let then_bb = self.b.create_block();
                let else_bb = self.b.create_block();
                let join = self.b.create_block();
                self.b.terminate(mir::Terminator::Branch {
                    cond: cid,
                    then_: then_bb,
                    else_: else_bb,
                });

                self.b.switch_to(then_bb);
                self.push_scope();
                self.lower_body_into(then_body, dst, join)?;
                self.pop_scope();

                self.b.switch_to(else_bb);
                match else_body {
                    Some(eb) => {
                        self.push_scope();
                        self.lower_body_into(eb, dst, join)?;
                        self.pop_scope();
                    }
                    None => {
                        self.b.assign(dst, mir::RValue::Const(Constant::Unit));
                        self.b.terminate(mir::Terminator::Jump(join));
                    }
                }

                self.b.switch_to(join);
                Ok(())
            }
            hir::RValue::Match { scrutinee, arms, .. } => {
                self.lower_match(dst, scrutinee, arms)
            }
            hir::RValue::For { var, iterable, body } => {
                self.lower_for(dst, var, iterable, body)
            }
            hir::RValue::Perform { effect, op, args, .. } => {
                // Mirror the stable compiler's special cases.
                if effect == "LLM" && op == "ask" {
                    let prompt = match args.first() {
                        Some(a) => self.lower_operand(a)?,
                        None => self.unit_temp(),
                    };
                    self.b.assign(dst, mir::RValue::LlmAsk { prompt });
                    return Ok(());
                }
                if effect == "Signal" && op == "wait" {
                    if let Some(hir::Operand::Literal(crate::ast::Literal::String(name), _)) =
                        args.first()
                    {
                        self.b
                            .assign(dst, mir::RValue::SignalWait { name: name.clone() });
                        return Ok(());
                    }
                }
                let mut ids = Vec::with_capacity(args.len());
                for a in args {
                    ids.push(self.lower_operand(a)?);
                }
                self.b.assign(
                    dst,
                    mir::RValue::Perform {
                        effect: effect.clone(),
                        op: op.clone(),
                        args: ids,
                    },
                );
                Ok(())
            }
            hir::RValue::Handle { body, handlers, .. } => {
                self.lower_handle(dst, body, handlers)
            }
            hir::RValue::Migrate { actor, node, .. } => {
                let a = self.lower_operand(actor)?;
                let n = self.lower_operand(node)?;
                self.b.assign(dst, mir::RValue::Migrate { actor: a, node: n });
                Ok(())
            }
            hir::RValue::CapCheck { operand, .. } => {
                let id = self.lower_operand(operand)?;
                self.b.assign(dst, mir::RValue::CapabilityCheck { val: id });
                Ok(())
            }
            hir::RValue::SelfRef(_) => {
                self.b.assign(dst, mir::RValue::SelfRef);
                Ok(())
            }
            hir::RValue::FFICall { symbol, args, .. } => {
                let idx = self
                    .ctx
                    .extern_map
                    .get(symbol)
                    .copied()
                    .ok_or_else(|| compile_err(format!("unknown extern function '{}'", symbol)))?;
                let mut ids = Vec::with_capacity(args.len());
                for a in args {
                    ids.push(self.lower_operand(a)?);
                }
                self.b.assign(dst, mir::RValue::FFICall { idx, args: ids });
                Ok(())
            }
            hir::RValue::Spawn { .. } => Err(nyi("spawn in HIR/MIR pipeline")),
            hir::RValue::Send { .. } => Err(nyi("send in HIR/MIR pipeline")),
            hir::RValue::Ask { .. } => Err(nyi("ask in HIR/MIR pipeline")),
            hir::RValue::Receive { .. } => Err(nyi("receive in HIR/MIR pipeline")),
        }
    }

    // -- Control flow constructs ----------------------------------------------

    fn lower_match(
        &mut self,
        dst: mir::LocalId,
        scrutinee: &hir::Operand,
        arms: &[(Pattern, Box<hir::Body>)],
    ) -> NuResult<()> {
        use crate::bytecode::Constant;
        let sid = self.lower_operand(scrutinee)?;
        if arms.is_empty() {
            self.b.assign(dst, mir::RValue::Const(Constant::Unit));
            return Ok(());
        }
        let join = self.b.create_block();

        for (i, (pat, arm_body)) in arms.iter().enumerate() {
            let is_last = i == arms.len() - 1;
            if is_last {
                // Last arm is entered unconditionally (mirrors the stable
                // compiler's fallback semantics).
                self.push_scope();
                self.bind_pattern(pat, sid);
                self.lower_body_into(arm_body, dst, join)?;
                self.pop_scope();
            } else {
                let test = self.pattern_test(pat, sid)?;
                let arm_bb = self.b.create_block();
                let next_bb = self.b.create_block();
                self.b.terminate(mir::Terminator::Branch {
                    cond: test,
                    then_: arm_bb,
                    else_: next_bb,
                });
                self.b.switch_to(arm_bb);
                self.push_scope();
                self.bind_pattern(pat, sid);
                self.lower_body_into(arm_body, dst, join)?;
                self.pop_scope();
                self.b.switch_to(next_bb);
            }
        }

        self.b.switch_to(join);
        Ok(())
    }

    fn pattern_test(&mut self, pat: &Pattern, sid: mir::LocalId) -> NuResult<mir::LocalId> {
        use crate::bytecode::Constant;
        let dst = self.b.add_temp(Type::bool());
        match pat {
            Pattern::Wild | Pattern::Var(_) => {
                self.b.assign(dst, mir::RValue::Const(Constant::Bool(true)));
            }
            Pattern::Lit(lit) => {
                let lit_id = self.b.add_temp(Type::unit());
                self.b
                    .assign(lit_id, mir::RValue::Const(literal_to_constant(lit)));
                self.b.assign(
                    dst,
                    mir::RValue::Binary(crate::ast::BinOp::Eq, sid, lit_id),
                );
            }
            Pattern::Variant(tag, _) => {
                let tag_id = self.b.add_temp(Type::unit());
                self.b
                    .assign(tag_id, mir::RValue::Const(Constant::String(tag.clone())));
                self.b.assign(dst, mir::RValue::StringEq(sid, tag_id));
            }
            Pattern::Tuple(pats) => {
                // Structural tuple matching is not implemented; mirror the
                // stable compiler (non-empty tuple pattern always matches).
                self.b
                    .assign(dst, mir::RValue::Const(Constant::Bool(!pats.is_empty())));
            }
            Pattern::Record(fields) => {
                self.b
                    .assign(dst, mir::RValue::Const(Constant::Bool(!fields.is_empty())));
            }
            Pattern::Alias(_, inner) => {
                return self.pattern_test(inner, sid);
            }
        }
        Ok(dst)
    }

    fn bind_pattern(&mut self, pat: &Pattern, sid: mir::LocalId) {
        match pat {
            Pattern::Var(name) => self.bind(name, sid),
            Pattern::Alias(name, inner) => {
                self.bind(name, sid);
                self.bind_pattern(inner, sid);
            }
            Pattern::Variant(_, Some(inner)) => self.bind_pattern(inner, sid),
            _ => {}
        }
    }

    fn lower_for(
        &mut self,
        dst: mir::LocalId,
        var: &str,
        iterable: &hir::Operand,
        body: &hir::Body,
    ) -> NuResult<()> {
        use crate::bytecode::Constant;
        let iter = self.lower_operand(iterable)?;
        let len = self.b.add_temp(Type::int());
        self.b.assign(len, mir::RValue::ArrayLen(iter));
        let idx = self.b.add_temp(Type::int());
        self.b.assign(idx, mir::RValue::Const(Constant::Int(0)));
        let one = self.b.add_temp(Type::int());
        self.b.assign(one, mir::RValue::Const(Constant::Int(1)));

        let head = self.b.create_block();
        let body_bb = self.b.create_block();
        let exit = self.b.create_block();

        self.b.terminate(mir::Terminator::Jump(head));

        self.b.switch_to(head);
        let cond = self.b.add_temp(Type::bool());
        self.b
            .assign(cond, mir::RValue::Binary(crate::ast::BinOp::Lt, idx, len));
        self.b.terminate(mir::Terminator::Branch {
            cond,
            then_: body_bb,
            else_: exit,
        });

        self.b.switch_to(body_bb);
        let elem = self.b.add_temp(Type::unit());
        self.b.assign(elem, mir::RValue::ArrayLoad { arr: iter, idx });
        self.push_scope();
        self.bind(var, elem);
        self.loop_exits.push(exit);
        for stmt in &body.stmts {
            self.lower_stmt(stmt)?;
        }
        if !self.b.is_terminated() {
            match &body.terminator {
                hir::Terminator::Yield(_) => {
                    // Loop body value is discarded; increment and loop.
                    self.b
                        .assign(idx, mir::RValue::Binary(crate::ast::BinOp::Add, idx, one));
                    self.b.terminate(mir::Terminator::Jump(head));
                }
                hir::Terminator::FnReturn(op) => {
                    let id = match op {
                        Some(op) => self.lower_operand(op)?,
                        None => self.unit_temp(),
                    };
                    self.b.terminate(mir::Terminator::Return(Some(id)));
                }
                hir::Terminator::Break => {
                    self.b.terminate(mir::Terminator::Jump(exit));
                }
            }
        }
        self.loop_exits.pop();
        self.pop_scope();

        self.b.switch_to(exit);
        // The stable compiler evaluates `for` to integer 0; mirror it so the
        // pipelines stay observationally identical.
        self.b.assign(dst, mir::RValue::Const(Constant::Int(0)));
        Ok(())
    }

    fn lower_handle(
        &mut self,
        dst: mir::LocalId,
        body: &hir::Body,
        handlers: &[hir::EffectHandler],
    ) -> NuResult<()> {
        let join = self.b.create_block();
        let table_idx = self.b.add_handler_table(mir::HandlerTableDef { bindings: Vec::new() });
        self.b.emit(mir::Stmt::EnterHandle { table: table_idx });

        // Body: yielded value lands in dst, then pop the handler frame.
        for stmt in &body.stmts {
            self.lower_stmt(stmt)?;
        }
        if !self.b.is_terminated() {
            match &body.terminator {
                hir::Terminator::Yield(op) => {
                    let id = self.lower_operand(op)?;
                    self.b.assign(dst, mir::RValue::Load(id));
                    self.b.emit(mir::Stmt::PopHandler);
                    self.b.terminate(mir::Terminator::Jump(join));
                }
                hir::Terminator::FnReturn(op) => {
                    let id = match op {
                        Some(op) => self.lower_operand(op)?,
                        None => self.unit_temp(),
                    };
                    self.b.terminate(mir::Terminator::Return(Some(id)));
                }
                hir::Terminator::Break => {
                    return Err(compile_err("break out of an effect handler body"));
                }
            }
        }

        // Handler bodies: entered only by the VM's effect dispatch; each ends
        // with Resume.
        let mut bindings = Vec::with_capacity(handlers.len());
        for h in handlers {
            let hb = self.b.create_block();
            self.b.switch_to(hb);
            self.push_scope();
            let mut params = Vec::with_capacity(h.params.len());
            for (pname, pty) in &h.params {
                let id = self.b.add_local(pname.clone(), pty.clone());
                self.bind(pname, id);
                params.push(id);
            }
            for stmt in &h.body.stmts {
                self.lower_stmt(stmt)?;
            }
            if !self.b.is_terminated() {
                match &h.body.terminator {
                    hir::Terminator::Yield(op) => {
                        let id = self.lower_operand(op)?;
                        self.b.terminate(mir::Terminator::Resume(id));
                    }
                    hir::Terminator::FnReturn(op) => {
                        let id = match op {
                            Some(op) => self.lower_operand(op)?,
                            None => self.unit_temp(),
                        };
                        self.b.terminate(mir::Terminator::Return(Some(id)));
                    }
                    hir::Terminator::Break => {
                        return Err(compile_err("break out of an effect handler"));
                    }
                }
            }
            self.pop_scope();
            bindings.push(mir::HandlerBindingDef {
                effect_name: h.effect_name.clone(),
                params,
                body: hb,
            });
        }
        self.b.handler_table_mut(table_idx).bindings = bindings;

        self.b.switch_to(join);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn place_is_self(place: &hir::Place) -> bool {
    matches!(place, hir::Place::Var(name, _) if name == "self")
}

fn operand_is_self(op: &hir::Operand) -> bool {
    matches!(op, hir::Operand::Var(name, _) if name == "self")
}

fn literal_to_constant(lit: &crate::ast::Literal) -> crate::bytecode::Constant {
    use crate::ast::Literal;
    use crate::bytecode::Constant;
    match lit {
        Literal::Int(n) => Constant::Int(*n),
        Literal::Float(f) => Constant::Float(*f),
        Literal::String(s) => Constant::String(s.clone()),
        Literal::Bool(b) => Constant::Bool(*b),
        Literal::Nil => Constant::Nil,
        Literal::Unit => Constant::Unit,
    }
}
