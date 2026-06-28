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
        Expr::Handle { body, handlers, .. } => {
            free_vars(body, bound, acc);
            for h in handlers {
                let mut h_bound = bound.clone();
                for p in &h.params { h_bound.insert(p.clone()); }
                free_vars(&h.body, &h_bound, acc);
            }
        }
        Expr::Perform { args, .. } => {
            for a in args { free_vars(a, bound, acc); }
        }
        Expr::Spawn { behaviors, .. } => {
            for b in behaviors {
                for (_, e) in &b.patterns { free_vars(e, bound, acc); }
            }
        }
        Expr::Send { args, .. } | Expr::Ask { args, .. } => {
            for a in args { free_vars(a, bound, acc); }
        }
        _ => {}
    }
}
