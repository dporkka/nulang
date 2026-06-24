//! AST -> Bytecode compiler.

use crate::ast::*;
use crate::bytecode::*;
use crate::types::{Capability, EffectRow, Type};

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Compiler state
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct LocalVar {
    name: String,
    reg: u8,
}

#[derive(Debug)]
pub struct Compiler {
    module: Module,
    locals: Vec<LocalVar>,
    next_reg: u8,
    loop_breaks: Vec<Vec<u32>>, // stack of break-jump PCs per loop
    current_actor: Option<String>,
}

impl Compiler {
    pub fn new(module_name: String) -> Self {
        Compiler {
            module: Module::new(module_name),
            locals: Vec::new(),
            next_reg: 1, // r0 reserved for temporaries / return value
            loop_breaks: Vec::new(),
            current_actor: None,
        }
    }

    pub fn finish(self) -> Module {
        self.module
    }

    pub fn module(&self) -> &Module {
        &self.module
    }

    // -- Variable management --

    fn alloc_reg(&mut self) -> u8 {
        let r = self.next_reg;
        self.next_reg += 1;
        assert!(self.next_reg <= 255, "register overflow");
        r
    }

    fn free_reg(&mut self, _r: u8) {
        // Simple bump allocator - in production, use a register pool
    }

    fn find_local(&self, name: &str) -> Option<u8> {
        self.locals.iter().rev().find(|l| l.name == name).map(|l| l.reg)
    }

    fn push_local(&mut self, name: String, reg: u8) {
        self.locals.push(LocalVar { name, reg });
    }

    fn pop_local(&mut self) -> Option<LocalVar> {
        self.locals.pop()
    }

    // -- Entry point --

    pub fn compile_module(&mut self, module: &ast::Module) {
        for decl in &module.decls {
            self.compile_decl(decl);
        }
        self.module.emit(OpCode::Halt, 0, 0, 0);
    }

    fn compile_decl(&mut self, decl: &Decl) {
        match decl {
            Decl::Fun { name, params, body, .. } => {
                let saved_locals = self.locals.clone();
                let saved_reg = self.next_reg;
                self.locals.clear();
                self.next_reg = 1;

                // Parameters start at r1
                for (pname, _) in params {
                    let reg = self.alloc_reg();
                    self.push_local(pname.clone(), reg);
                }

                let entry = self.module.instructions.len() as u32;
                let ret_reg = self.compile_expr(body);
                self.module.emit(OpCode::Ret, ret_reg, 0, 0);

                self.module.behavior_table.push(BehaviorTableEntry {
                    name: name.clone(),
                    param_count: params.len() as u8,
                    entry_point: entry,
                    effect_annotation: None,
                });

                self.locals = saved_locals;
                self.next_reg = saved_reg;
            }
            Decl::Actor { def, .. } => {
                self.current_actor = Some(def.name.clone());
                for behavior in &def.behaviors {
                    self.compile_behavior(&def.name, behavior);
                }
                self.current_actor = None;
            }
            Decl::Agent { def, .. } => {
                self.current_actor = Some(def.name.clone());
                for behavior in &def.behaviors {
                    self.compile_behavior(&def.name, behavior);
                }
                self.current_actor = None;
            }
            _ => {}
        }
    }

    fn compile_behavior(&mut self, actor_name: &str, behavior: &Behavior) {
        let saved_locals = self.locals.clone();
        let saved_reg = self.next_reg;
        self.locals.clear();
        self.next_reg = 1;

        for (pname, _) in &behavior.params {
            let reg = self.alloc_reg();
            self.push_local(pname.clone(), reg);
        }

        let entry = self.module.instructions.len() as u32;
        let ret_reg = self.compile_expr(&behavior.body);
        self.module.emit(OpCode::Ret, ret_reg, 0, 0);

        self.module.behavior_table.push(BehaviorTableEntry {
            name: format!("{}.{}", actor_name, behavior.name),
            param_count: behavior.params.len() as u8,
            entry_point: entry,
            effect_annotation: None,
        });

        self.locals = saved_locals;
        self.next_reg = saved_reg;
    }

    // -- Expression compiler --

    fn compile_expr(&mut self, expr: &Expr) -> u8 {
        match expr {
            Expr::Literal(lit, _) => self.compile_literal(lit),
            Expr::Var(name, _) => {
                if let Some(reg) = self.find_local(name) {
                    let dst = self.alloc_reg();
                    self.module.emit(OpCode::Move, dst, reg, 0);
                    dst
                } else {
                    // Global - load from constants
                    let dst = self.alloc_reg();
                    let name_idx = self.module.add_string(name.clone());
                    self.module.emit(OpCode::LoadConst, dst,
                        ((name_idx >> 8) & 0xFF) as u8,
                        (name_idx & 0xFF) as u8);
                    dst
                }
            }
            Expr::Let { name, value, body, .. } => {
                let val_reg = self.compile_expr(value);
                self.push_local(name.clone(), val_reg);
                let result = self.compile_expr(body);
                self.pop_local();
                result
            }
            Expr::LetRec { name, params, value, body, .. } => {
                // Allocate register for the recursive function
                let fun_reg = self.alloc_reg();
                self.push_local(name.clone(), fun_reg);

                // Compile the function body with parameters
                let saved_locals = self.locals.clone();
                let saved_reg = self.next_reg;
                self.locals.clear();
                self.next_reg = 1;

                for (pname, _) in params {
                    let reg = self.alloc_reg();
                    self.push_local(pname.clone(), reg);
                }

                let entry = self.module.instructions.len() as u32;
                let ret_reg = self.compile_expr(value);
                self.module.emit(OpCode::Ret, ret_reg, 0, 0);

                self.module.behavior_table.push(BehaviorTableEntry {
                    name: name.clone(),
                    param_count: params.len() as u8,
                    entry_point: entry,
                    effect_annotation: None,
                });

                self.locals = saved_locals;
                self.next_reg = saved_reg;

                // Now compile the body
                let result = self.compile_expr(body);
                self.pop_local();
                result
            }
            Expr::If { cond, then_branch, else_branch, .. } => {
                let cond_reg = self.compile_expr(cond);
                let jump_else = self.module.instructions.len() as u32;
                self.module.emit(OpCode::JumpIfNot, cond_reg, 0, 0); // patched later

                let then_reg = self.compile_expr(then_branch);
                let jump_end = self.module.instructions.len() as u32;
                self.module.emit(OpCode::Jump, 0, 0, 0); // patched later

                let else_pc = self.module.instructions.len() as u32;
                let else_reg = else_branch.as_ref()
                    .map(|e| self.compile_expr(e))
                    .unwrap_or_else(|| {
                        let r = self.alloc_reg();
                        self.module.emit(OpCode::LoadNull, r, 0, 0);
                        r
                    });

                let end_pc = self.module.instructions.len() as u32;

                // Patch jumps
                let else_offset = (else_pc as i32 - jump_else as i32 - 1) as i16;
                self.module.patch_jump(jump_else, else_offset);
                let end_offset = (end_pc as i32 - jump_end as i32 - 1) as i16;
                self.module.patch_jump(jump_end, end_offset);

                // Move result to a single register
                let result_reg = self.alloc_reg();
                self.module.emit(OpCode::Move, result_reg, then_reg, 0);
                result_reg
            }
            Expr::Lambda { params, body, .. } => {
                let saved_locals = self.locals.clone();
                let saved_reg = self.next_reg;
                self.locals.clear();
                self.next_reg = 1;

                for (pname, _) in params {
                    let reg = self.alloc_reg();
                    self.push_local(pname.clone(), reg);
                }

                let entry = self.module.instructions.len() as u32;
                let ret_reg = self.compile_expr(body);
                self.module.emit(OpCode::Ret, ret_reg, 0, 0);

                let lambda_name = format!("__lambda_{}", entry);
                self.module.behavior_table.push(BehaviorTableEntry {
                    name: lambda_name,
                    param_count: params.len() as u8,
                    entry_point: entry,
                    effect_annotation: None,
                });

                self.locals = saved_locals;
                self.next_reg = saved_reg;

                let dst = self.alloc_reg();
                dst
            }
            Expr::App { func, args, .. } => {
                let func_reg = self.compile_expr(func);
                let mut arg_regs = Vec::new();
                for arg in args {
                    arg_regs.push(self.compile_expr(arg));
                }
                let dst = self.alloc_reg();

                if !arg_regs.is_empty() {
                    // Move args to consecutive registers starting at func_reg+1
                    for (i, &arg_reg) in arg_regs.iter().enumerate() {
                        let target = func_reg + 1 + i as u8;
                        if arg_reg != target {
                            self.module.emit(OpCode::Move, target, arg_reg, 0);
                        }
                    }
                }

                self.module.emit(OpCode::Call, dst, func_reg,
                    (func_reg + args.len() as u8));
                dst
            }
            Expr::Block { exprs, .. } => {
                let mut last_reg = 0;
                for (i, e) in exprs.iter().enumerate() {
                    let is_last = i == exprs.len() - 1;
                    if is_last {
                        last_reg = self.compile_expr(e);
                    } else {
                        self.compile_expr(e);
                    }
                }
                last_reg
            }
            Expr::Binary { op, left, right, .. } => {
                let l = self.compile_expr(left);
                let r = self.compile_expr(right);
                let dst = self.alloc_reg();
                let opcode = match op {
                    BinOp::Add => OpCode::Add,
                    BinOp::Sub => OpCode::Sub,
                    BinOp::Mul => OpCode::Mul,
                    BinOp::Div => OpCode::Div,
                    BinOp::Mod => OpCode::Mod,
                    BinOp::Eq => OpCode::Eq,
                    BinOp::Ne => OpCode::Ne,
                    BinOp::Lt => OpCode::Lt,
                    BinOp::Le => OpCode::Le,
                    BinOp::Gt => OpCode::Gt,
                    BinOp::Ge => OpCode::Ge,
                    BinOp::And => OpCode::And,
                    BinOp::Or => OpCode::Or,
                    BinOp::Cons => OpCode::Cons,
                    _ => OpCode::Add,
                };
                self.module.emit(opcode, dst, l, r);
                dst
            }
            Expr::Tuple(elems, _) => {
                let mut regs = Vec::new();
                for e in elems {
                    regs.push(self.compile_expr(e));
                }
                let dst = self.alloc_reg();
                if !regs.is_empty() {
                    for (i, &reg) in regs.iter().enumerate() {
                        self.module.emit(OpCode::Move, dst + 1 + i as u8, reg, 0);
                    }
                    self.module.emit(OpCode::NewTuple, dst, dst + 1,
                        (dst + regs.len() as u8));
                } else {
                    self.module.emit(OpCode::LoadNull, dst, 0, 0);
                }
                dst
            }
            Expr::Record(fields, _) => {
                let dst = self.alloc_reg();
                let mut field_regs = Vec::new();
                for (name, expr) in fields {
                    let reg = self.compile_expr(expr);
                    field_regs.push((name.clone(), reg));
                }
                for (_name, reg) in field_regs {
                    // Store field name in string table
                    self.module.emit(OpCode::Move, dst + 1, reg, 0);
                }
                self.module.emit(OpCode::NewRecord, dst, field_regs.len() as u8, 0);
                dst
            }
            Expr::FieldAccess { expr, field, .. } => {
                let obj = self.compile_expr(expr);
                let dst = self.alloc_reg();
                let field_idx = self.module.add_string(field.clone());
                self.module.emit(OpCode::FieldGet, dst, obj,
                    ((field_idx >> 8) & 0xFF) as u8);
                dst
            }
            Expr::Array(elems, _) => {
                let dst = self.alloc_reg();
                for (i, e) in elems.iter().enumerate() {
                    let reg = self.compile_expr(e);
                    self.module.emit(OpCode::Move, dst + 1 + i as u8, reg, 0);
                }
                self.module.emit(OpCode::NewArray, dst, dst + 1,
                    (dst + elems.len() as u8));
                dst
            }
            Expr::Unary { op, expr, .. } => {
                let operand = self.compile_expr(expr);
                let dst = self.alloc_reg();
                let opcode = match op {
                    UnOp::Neg => OpCode::Neg,
                    UnOp::Not => OpCode::Not,
                };
                self.module.emit(opcode, dst, operand, 0);
                dst
            }
            Expr::Match { scrutinee, arms, .. } => {
                let scrut_reg = self.compile_expr(scrutinee);
                let mut end_jumps = Vec::new();
                let dst = self.alloc_reg();

                for (pattern, arm_body) in arms {
                    // Simple pattern: just Var for now
                    if let Pattern::Var(name) = pattern {
                        self.push_local(name.clone(), scrut_reg);
                        let arm_reg = self.compile_expr(arm_body);
                        self.module.emit(OpCode::Move, dst, arm_reg, 0);
                        self.pop_local();
                        break; // Only handle first arm for now
                    }
                }

                dst
            }
            Expr::Spawn { actor_type, init, .. } => {
                let type_reg = self.compile_expr(actor_type);
                let dst = self.alloc_reg();

                // Compile init args
                for (i, (_name, expr)) in init.iter().enumerate() {
                    let reg = self.compile_expr(expr);
                    self.module.emit(OpCode::Move, dst + 2 + i as u8, reg, 0);
                }

                self.module.emit(OpCode::Spawn, dst, type_reg,
                    (dst + 2 + init.len() as u8));
                dst
            }
            Expr::Send { actor, behavior, args, .. } => {
                let actor_reg = self.compile_expr(actor);
                let beh_idx = self.module.add_string(behavior.clone());

                let mut arg_regs = Vec::new();
                for arg in args {
                    arg_regs.push(self.compile_expr(arg));
                }

                for (i, &reg) in arg_regs.iter().enumerate() {
                    self.module.emit(OpCode::Move, actor_reg + 2 + i as u8, reg, 0);
                }

                self.module.emit(OpCode::Send, 0, actor_reg,
                    ((beh_idx >> 8) & 0xFF) as u8);
                0
            }
            Expr::Ask { actor, behavior, args, .. } => {
                let actor_reg = self.compile_expr(actor);
                let dst = self.alloc_reg();
                let beh_idx = self.module.add_string(behavior.clone());

                let mut arg_regs = Vec::new();
                for arg in args {
                    arg_regs.push(self.compile_expr(arg));
                }

                for (i, &reg) in arg_regs.iter().enumerate() {
                    self.module.emit(OpCode::Move, dst + 1 + i as u8, reg, 0);
                }

                self.module.emit(OpCode::Ask, dst, actor_reg,
                    ((beh_idx >> 8) & 0xFF) as u8);
                dst
            }
            Expr::SelfRef(_) => {
                let dst = self.alloc_reg();
                self.module.emit(OpCode::SelfAddr, dst, 0, 0);
                dst
            }
            Expr::Perform { effect, op, args, .. } => {
                let dst = self.alloc_reg();
                let eff_idx = self.module.add_string(effect.clone());
                let op_idx = self.module.add_string(op.clone());

                for (i, arg) in args.iter().enumerate() {
                    let reg = self.compile_expr(arg);
                    self.module.emit(OpCode::Move, dst + 1 + i as u8, reg, 0);
                }

                self.module.emit(OpCode::Perform, dst,
                    ((eff_idx >> 8) & 0xFF) as u8,
                    ((op_idx >> 8) & 0xFF) as u8);
                dst
            }
            Expr::Handle { body, handlers, .. } => {
                // Compile handlers setup
                let _handler_pcs: Vec<u32> = handlers.iter().map(|h| {
                    let op_idx = self.module.add_string(h.op.clone());
                    self.module.instructions.len() as u32
                }).collect();

                let body_reg = self.compile_expr(body);

                // Pop handlers
                for _ in handlers {
                    self.module.emit(OpCode::PopHandler, 0, 0, 0);
                }

                body_reg
            }
            Expr::Pipe { left, right, .. } => {
                let l = self.compile_expr(left);
                let r = self.compile_expr(right);
                let dst = self.alloc_reg();
                self.module.emit(OpCode::Call, dst, r, l);
                dst
            }
            Expr::Try { body, catch_arms: _, .. } => {
                self.compile_expr(body)
            }
            Expr::Await { expr, .. } => {
                self.compile_expr(expr)
            }
            Expr::Migrate { actor, node, .. } => {
                let actor_reg = self.compile_expr(actor);
                let node_reg = self.compile_expr(node);
                let dst = self.alloc_reg();
                self.module.emit(OpCode::Migrate, dst, actor_reg, node_reg);
                dst
            }
            Expr::CapAnnotate { expr, .. } => {
                self.compile_expr(expr)
            }
            Expr::TypeAnnotate { expr, .. } => {
                self.compile_expr(expr)
            }
            Expr::Assign { target, value, .. } => {
                let val_reg = self.compile_expr(value);
                if let Expr::Var(name, _) = target.as_ref() {
                    if let Some(reg) = self.find_local(name) {
                        self.module.emit(OpCode::Move, reg, val_reg, 0);
                        reg
                    } else {
                        val_reg
                    }
                } else {
                    val_reg
                }
            }
            Expr::ActorDef(_, _) | Expr::AgentDef(_, _) => {
                let dst = self.alloc_reg();
                self.module.emit(OpCode::LoadNull, dst, 0, 0);
                dst
            }
            Expr::Receive { .. } => {
                let dst = self.alloc_reg();
                self.module.emit(OpCode::LoadNull, dst, 0, 0);
                dst
            }
            Expr::Index { .. } => {
                let dst = self.alloc_reg();
                self.module.emit(OpCode::LoadNull, dst, 0, 0);
                dst
            }
        }
    }

    fn compile_literal(&mut self, lit: &Literal) -> u8 {
        let dst = self.alloc_reg();
        match lit {
            Literal::Int(n) => {
                let idx = self.module.add_constant(Constant::Int(*n));
                self.module.emit(OpCode::LoadConst, dst,
                    ((idx >> 8) & 0xFF) as u8,
                    (idx & 0xFF) as u8);
            }
            Literal::Float(n) => {
                let idx = self.module.add_constant(Constant::Float(*n));
                self.module.emit(OpCode::LoadConst, dst,
                    ((idx >> 8) & 0xFF) as u8,
                    (idx & 0xFF) as u8);
            }
            Literal::String(s) => {
                let idx = self.module.add_constant(Constant::String(s.clone()));
                self.module.emit(OpCode::LoadConst, dst,
                    ((idx >> 8) & 0xFF) as u8,
                    (idx & 0xFF) as u8);
            }
            Literal::Bool(b) => {
                let idx = self.module.add_constant(Constant::Bool(*b));
                self.module.emit(OpCode::LoadConst, dst,
                    ((idx >> 8) & 0xFF) as u8,
                    (idx & 0xFF) as u8);
            }
            Literal::Unit => {
                self.module.emit(OpCode::LoadNull, dst, 0, 0);
            }
        }
        dst
    }
}

// ---------------------------------------------------------------------------
// Convenience function
// ---------------------------------------------------------------------------

pub fn compile(module: &ast::Module) -> Module {
    let mut compiler = Compiler::new(module.name.clone());
    compiler.compile_module(module);
    compiler.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;
    use crate::types::Span;

    fn s() -> Span { Span { start: 0, end: 0, line: 1, col: 1 } }

    #[test]
    fn test_compile_literal() {
        let m = ast::Module {
            name: "test".to_string(),
            decls: vec![],
            span: s(),
        };
        let mut c = Compiler::new("test".to_string());
        c.compile_module(&m);
        let module = c.finish();
        assert!(!module.instructions.is_empty());
    }

    #[test]
    fn test_compile_function() {
        let m = ast::Module {
            name: "test".to_string(),
            decls: vec![
                Decl::Fun {
                    name: "add".to_string(),
                    type_params: vec![],
                    params: vec![("x".to_string(), None), ("y".to_string(), None)],
                    ret_type: None,
                    effect: None,
                    body: Expr::Binary {
                        op: BinOp::Add,
                        left: Box::new(Expr::Var("x".to_string(), s())),
                        right: Box::new(Expr::Var("y".to_string(), s())),
                        span: s(),
                    },
                    span: s(),
                }
            ],
            span: s(),
        };
        let module = compile(&m);
        assert!(!module.behavior_table.is_empty());
        assert_eq!(module.behavior_table[0].name, "add");
    }
}
