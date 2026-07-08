//! AST -> HIR lowering.
//!
//! Converts the parsed, type-checked AST into the typed High-level IR.
//! Expression types fall back to `Type::unit()` when no explicit annotation
//! is available; the bytecode backend is dynamically typed, so structural
//! fidelity (not type fidelity) is what matters here.
//!
//! Control flow in expression position (`if`, `match`, `for`) lowers to
//! dedicated `RValue` variants whose sub-bodies end in a `Yield` terminator.
//! This keeps evaluation order correct when statements follow the control
//! flow expression — the old design stored `if` as a *body terminator*,
//! which reordered any code lowered after it.

use crate::ast;
use crate::ast::{Decl, Expr, Literal};
use crate::hir;
use crate::types::{Capability, EffectRow, Span, Type};

pub fn lower_module(ast: &ast::AstModule) -> hir::Module {
    let mut module = hir::Module::new(&ast.name);
    for decl in &ast.decls {
        module.decls.push(lower_decl(decl));
    }
    module
}

fn lower_decl(decl: &Decl) -> hir::Decl {
    match decl {
        Decl::Function {
            name,
            type_params,
            params,
            ret_type,
            effect,
            cap,
            body,
            annotations: _,
            public,
            span,
        } => hir::Decl::Function(hir::FunctionDef {
            name: name.clone(),
            type_params: type_params.clone(),
            params: params
                .iter()
                .map(|(n, t)| (n.clone(), resolve_type(t)))
                .collect(),
            ret: resolve_type(ret_type),
            effect: effect.clone().unwrap_or_else(EffectRow::empty),
            cap: cap.unwrap_or(Capability::Ref),
            body: lower_body(body),
            public: *public,
            span: *span,
        }),
        Decl::Actor {
            name,
            type_params,
            persistent,
            state_fields,
            behaviors,
            init,
            span,
        } => hir::Decl::Actor(hir::ActorDef {
            name: name.clone(),
            type_params: type_params.clone(),
            persistent: *persistent,
            state_fields: state_fields
                .iter()
                .map(|(n, m, t, e)| {
                    let mut body = hir::Body::new();
                    let op = lower_expr(e, &mut body);
                    (n.clone(), *m, t.clone(), op)
                })
                .collect(),
            behaviors: behaviors.iter().map(lower_behavior).collect(),
            init: init
                .iter()
                .map(|(n, e)| {
                    let mut body = hir::Body::new();
                    let op = lower_expr(e, &mut body);
                    (n.clone(), op)
                })
                .collect(),
            span: *span,
        }),
        Decl::TypeAlias {
            name,
            type_params,
            body,
            public,
            span,
        } => hir::Decl::TypeAlias {
            name: name.clone(),
            type_params: type_params.clone(),
            body: body.clone(),
            public: *public,
            span: *span,
        },
        Decl::RecordType {
            name,
            type_params,
            fields,
            public,
            span,
        } => hir::Decl::RecordType {
            name: name.clone(),
            type_params: type_params.clone(),
            fields: fields.clone(),
            public: *public,
            span: *span,
        },
        Decl::VariantType {
            name,
            type_params,
            variants,
            public,
            span,
        } => hir::Decl::VariantType {
            name: name.clone(),
            type_params: type_params.clone(),
            variants: variants.clone(),
            public: *public,
            span: *span,
        },
        Decl::EffectDecl { name, ops, span } => hir::Decl::EffectDecl {
            name: name.clone(),
            ops: ops.clone(),
            span: *span,
        },
        Decl::Extern { library, funcs, span } => hir::Decl::ExternBlock {
            library: library.clone(),
            funcs: funcs
                .iter()
                .map(|f| hir::ExternFunc {
                    name: f.name.clone(),
                    params: f
                        .params
                        .iter()
                        .map(|(n, t)| (n.clone(), t.clone()))
                        .collect(),
                    ret: f.ret.clone(),
                    span: f.span,
                })
                .collect(),
            span: *span,
        },
        Decl::Module {
            name,
            exports,
            decls,
            span,
        } => hir::Decl::Module {
            name: name.clone(),
            exports: exports.clone(),
            decls: decls.iter().map(lower_decl).collect(),
            span: *span,
        },
        Decl::Import { path, items, span } => hir::Decl::Import {
            path: path.clone(),
            items: items.clone(),
            span: *span,
        },
        Decl::Workflow { name, span, .. } => hir::Decl::Workflow {
            name: name.clone(),
            span: *span,
        },
        Decl::Agent { name, span, .. } => hir::Decl::Agent {
            name: name.clone(),
            span: *span,
        },
    }
}

fn lower_behavior(b: &ast::Behavior) -> hir::BehaviorDef {
    hir::BehaviorDef {
        name: b.name.clone(),
        params: b
            .params
            .iter()
            .map(|(n, t)| (n.clone(), resolve_type(t)))
            .collect(),
        ret: Type::unit(),
        effect: b.effect.clone().unwrap_or_else(EffectRow::empty),
        cap: b.cap,
        body: lower_body(&b.body),
        span: b.span,
    }
}

/// Lower an expression into a fresh body that yields the expression's value.
pub fn lower_body(expr: &Expr) -> hir::Body {
    let mut body = hir::Body::new();
    let op = lower_expr(expr, &mut body);
    if !body.is_terminated() {
        body.set_terminator(hir::Terminator::Yield(op));
    }
    body
}

/// Lower an expression into a sequence of statements in `body`, returning an
/// operand that represents the expression's value.
pub fn lower_expr(expr: &Expr, body: &mut hir::Body) -> hir::Operand {
    if body.is_terminated() {
        // Dead code after an explicit `return`/`break`: don't lower it.
        return hir::Operand::Unit;
    }
    match expr {
        Expr::Literal(lit, _span) => {
            let ty = literal_type(lit);
            hir::Operand::Literal(lit.clone(), ty)
        }
        Expr::Var(name, _span) => hir::Operand::Var(name.clone(), Type::unit()),
        Expr::SelfRef(_) => hir::Operand::Var("self".to_string(), Type::unit()),
        Expr::TypeAnnotate { expr, .. } => lower_expr(expr, body),
        Expr::CapAnnotate { expr, .. } => lower_expr(expr, body),
        Expr::Lambda { params, body: lb, effect: _, span } => {
            let lambda_body = lower_body(lb);
            let ty = Type::unit();
            let captures = lambda_captures(params, lb);
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Closure {
                    params: params
                        .iter()
                        .map(|(n, t)| (n.clone(), resolve_type(t)))
                        .collect(),
                    body: Box::new(lambda_body),
                    captures,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::App { func, args, span } => {
            let fop = lower_expr(func, body);
            let aops: Vec<_> = args.iter().map(|a| lower_expr(a, body)).collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Call {
                    func: fop,
                    args: aops,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Let { name, value, body: b, span } => {
            // Let-bound lambdas may reference themselves (`let fac = fn(n) ...
            // fac(n-1)`); lower them like `let rec` so the self-reference
            // resolves. Non-self-referencing lambdas stay ordinary closures so
            // they can capture the enclosing scope.
            if let Expr::Lambda { params, body: lam_body, .. } = value.as_ref() {
                if lambda_references(name, params, lam_body) {
                    let func_body = lower_body(lam_body);
                    body.push(hir::Stmt::Let {
                        name: name.clone(),
                        ty: Type::unit(),
                        value: hir::RValue::RecClosure {
                            name: name.clone(),
                            params: params
                                .iter()
                                .map(|(n, t)| (n.clone(), resolve_type(t)))
                                .collect(),
                            body: Box::new(func_body),
                            ty: Type::unit(),
                        },
                        span: *span,
                    });
                    return lower_expr(b, body);
                }
            }
            let vop = lower_expr(value, body);
            let ty = vop.ty();
            body.push(hir::Stmt::Let {
                name: name.clone(),
                ty: ty.clone(),
                value: hir::RValue::Use(vop),
                span: *span,
            });
            lower_expr(b, body)
        }
        Expr::LetRec { name, params, value, body: b, span } => {
            let func_body = lower_body(value);
            body.push(hir::Stmt::Let {
                name: name.clone(),
                ty: Type::unit(),
                value: hir::RValue::RecClosure {
                    name: name.clone(),
                    params: params
                        .iter()
                        .map(|(n, t)| (n.clone(), resolve_type(t)))
                        .collect(),
                    body: Box::new(func_body),
                    ty: Type::unit(),
                },
                span: *span,
            });
            lower_expr(b, body)
        }
        Expr::If { cond, then_branch, else_branch, span } => {
            let cond_op = lower_expr(cond, body);
            let ty = Type::unit();
            let temp = fresh_temp_name();
            let then_body = lower_body(then_branch);
            let else_body = else_branch.as_ref().map(|e| Box::new(lower_body(e)));
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::If {
                    cond: cond_op,
                    then_body: Box::new(then_body),
                    else_body,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Match { scrutinee, arms, span } => {
            let scrut_op = lower_expr(scrutinee, body);
            let ty = Type::unit();
            let temp = fresh_temp_name();
            let arms_hir: Vec<_> = arms
                .iter()
                .map(|(pat, e)| (pat.clone(), Box::new(lower_body(e))))
                .collect();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Match {
                    scrutinee: scrut_op,
                    arms: arms_hir,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Block { exprs, span: _ } => {
            let mut last = hir::Operand::Unit;
            for e in exprs {
                if body.is_terminated() {
                    break;
                }
                last = lower_expr(e, body);
            }
            last
        }
        Expr::Tuple(elems, span) => {
            let ops: Vec<_> = elems.iter().map(|e| lower_expr(e, body)).collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Tuple(ops, ty.clone()),
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Record(fields, span) => {
            let fs: Vec<_> = fields
                .iter()
                .map(|(n, e)| (n.clone(), lower_expr(e, body)))
                .collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Record(fs, ty.clone()),
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::FieldAccess { expr, field, span } => {
            let base = lower_expr(expr, body);
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::FieldAccess {
                    base,
                    field: field.clone(),
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Array(elems, span) => {
            let ops: Vec<_> = elems.iter().map(|e| lower_expr(e, body)).collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Array(ops, ty.clone()),
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Index { arr, idx, span } => {
            let aop = lower_expr(arr, body);
            let iop = lower_expr(idx, body);
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Index {
                    base: aop,
                    idx: iop,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Binary { op, left, right, span } => {
            let l = lower_expr(left, body);
            let r = lower_expr(right, body);
            let ty = binary_type(op);
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Binary(*op, l, r, ty.clone()),
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Unary { op, expr, span } => {
            let e = lower_expr(expr, body);
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Unary(*op, e, ty.clone()),
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Assign { target, value, span } => {
            let val = lower_expr(value, body);
            let place = lower_place(target, body);
            body.push(hir::Stmt::Assign {
                target: place,
                value: hir::RValue::Use(val),
                span: *span,
            });
            hir::Operand::Unit
        }
        Expr::Spawn { actor_type, init, span } => {
            let name = actor_name_from_expr(actor_type).unwrap_or_default();
            let init_ops: Vec<_> = init
                .iter()
                .map(|(n, e)| (n.clone(), lower_expr(e, body)))
                .collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Spawn {
                    actor_type: name,
                    init: init_ops,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Send { actor, behavior, args, span } => {
            let aop = lower_expr(actor, body);
            let aops: Vec<_> = args.iter().map(|a| lower_expr(a, body)).collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Send {
                    actor: aop,
                    behavior: behavior.clone(),
                    args: aops,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Ask { actor, behavior, args, span } => {
            let aop = lower_expr(actor, body);
            let aops: Vec<_> = args.iter().map(|a| lower_expr(a, body)).collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Ask {
                    actor: aop,
                    behavior: behavior.clone(),
                    args: aops,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Perform { effect, op, args, span } => {
            let aops: Vec<_> = args.iter().map(|a| lower_expr(a, body)).collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Perform {
                    effect: effect.clone(),
                    op: op.clone(),
                    args: aops,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Handle { body: hb, handlers, span } => {
            let hbody = lower_body(hb);
            let hs: Vec<_> = handlers
                .iter()
                .map(|h| hir::EffectHandler {
                    effect_name: h.effect_name.clone(),
                    op_name: h.op_name.clone(),
                    params: h
                        .params
                        .iter()
                        .map(|p| (p.clone(), Type::unit()))
                        .collect(),
                    resume: h.resume,
                    body: Box::new(lower_body(&h.body)),
                    span: *span,
                })
                .collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Handle {
                    body: Box::new(hbody),
                    handlers: hs,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Receive { arms, span } => {
            let arms_hir: Vec<_> = arms
                .iter()
                .map(|(name, params, e)| {
                    (
                        name.clone(),
                        params.clone(),
                        Box::new(lower_body(e)),
                    )
                })
                .collect();
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Receive {
                    arms: arms_hir,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Migrate { actor, node, span } => {
            let aop = lower_expr(actor, body);
            let nop = lower_expr(node, body);
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Migrate {
                    actor: aop,
                    node: nop,
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Emit { event, args, span } => {
            let aops: Vec<_> = args.iter().map(|a| lower_expr(a, body)).collect();
            body.push(hir::Stmt::Emit {
                event: event.clone(),
                args: aops,
                span: *span,
            });
            hir::Operand::Unit
        }
        Expr::For { var, iterable, body: b, span } => {
            let iop = lower_expr(iterable, body);
            let loop_body = lower_body(b);
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: Type::unit(),
                value: hir::RValue::For {
                    var: var.clone(),
                    iterable: iop,
                    body: Box::new(loop_body),
                },
                span: *span,
            });
            hir::Operand::Var(temp, Type::unit())
        }
        Expr::Pipe { left, right, span } => {
            // Lower `x |> f(a, b)` to `f(x, a, b)`, matching the stable
            // compiler's pipe semantics.
            let app = match right.as_ref() {
                Expr::App { func, args, span: app_span } => {
                    let mut new_args = vec![left.as_ref().clone()];
                    new_args.extend(args.iter().cloned());
                    Expr::App {
                        func: func.clone(),
                        args: new_args,
                        span: *app_span,
                    }
                }
                _ => Expr::App {
                    func: right.clone(),
                    args: vec![left.as_ref().clone()],
                    span: *span,
                },
            };
            lower_expr(&app, body)
        }
        Expr::Return(val, _span) => {
            let op = val.as_ref().map(|e| lower_expr(e, body));
            body.set_terminator(hir::Terminator::FnReturn(op));
            hir::Operand::Unit
        }
        Expr::Break(_) => {
            body.set_terminator(hir::Terminator::Break);
            hir::Operand::Unit
        }
    }
}

fn lower_place(expr: &Expr, body: &mut hir::Body) -> hir::Place {
    match expr {
        Expr::Var(name, _) => hir::Place::Var(name.clone(), Type::unit()),
        Expr::FieldAccess { expr, field, span: _ } => {
            let base = lower_place(expr, body);
            hir::Place::Field {
                base: Box::new(base),
                field: field.clone(),
                ty: Type::unit(),
            }
        }
        Expr::Index { arr, idx, span: _ } => {
            let base = lower_place(arr, body);
            let idx_op = lower_expr(idx, body);
            hir::Place::Index {
                base: Box::new(base),
                idx: idx_op,
                ty: Type::unit(),
            }
        }
        _ => {
            let op = lower_expr(expr, body);
            let name = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: name.clone(),
                ty: op.ty(),
                value: hir::RValue::Use(op),
                span: Span::default(),
            });
            hir::Place::Var(name, Type::unit())
        }
    }
}

/// Free variables of a lambda (candidates for capture). The MIR lowering
/// filters this against what is actually in scope.
fn lambda_captures(params: &[(String, Option<Type>)], body: &Expr) -> Vec<String> {
    let bound: std::collections::HashSet<String> =
        params.iter().map(|(n, _)| n.clone()).collect();
    let mut free = std::collections::HashSet::new();
    crate::compiler::free_vars(body, &bound, &mut free);
    let mut captures: Vec<String> = free.into_iter().collect();
    captures.sort(); // deterministic ordering shared with codegen
    captures
}

/// Does a let-bound lambda reference its own binding name?
fn lambda_references(name: &str, params: &[(String, Option<Type>)], body: &Expr) -> bool {
    lambda_captures(params, body).iter().any(|c| c == name)
}

fn resolve_type(ty: &Option<Type>) -> Type {
    ty.clone().unwrap_or_else(Type::unit)
}

fn literal_type(lit: &Literal) -> Type {
    use crate::types::PrimitiveType;
    match lit {
        Literal::Int(_) => Type::Primitive(PrimitiveType::Int),
        Literal::Float(_) => Type::Primitive(PrimitiveType::Float),
        Literal::String(_) => Type::Primitive(PrimitiveType::String),
        Literal::Bool(_) => Type::Primitive(PrimitiveType::Bool),
        Literal::Nil => Type::Primitive(PrimitiveType::Unit),
        Literal::Unit => Type::Primitive(PrimitiveType::Unit),
    }
}

fn binary_type(op: &ast::BinOp) -> Type {
    use crate::ast::BinOp;
    use crate::types::PrimitiveType;
    match op {
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::And | BinOp::Or => {
            Type::Primitive(PrimitiveType::Bool)
        }
        _ => Type::Primitive(PrimitiveType::Int),
    }
}

fn actor_name_from_expr(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Var(name, _) => Some(name.clone()),
        _ => None,
    }
}

static TEMP_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn fresh_temp_name() -> String {
    let n = TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("__tmp{}", n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lower_literal() {
        let ast = ast::AstModule {
            name: "test".to_string(),
            decls: vec![Decl::Function {
                name: "__main".to_string(),
                type_params: vec![],
                params: vec![],
                ret_type: Some(Type::int()),
                effect: None,
                cap: None,
                body: Expr::Literal(Literal::Int(42), Span::default()),
                annotations: vec![],
                public: true,
                span: Span::default(),
            }],
        };
        let hir = lower_module(&ast);
        assert_eq!(hir.decls.len(), 1);
    }

    #[test]
    fn test_lower_if_is_expression_positioned() {
        // `let x = if c then 1 else 2 in x` must keep the if as an RValue so
        // statements after it stay in evaluation order.
        let source_body = Expr::Let {
            name: "x".to_string(),
            value: Box::new(Expr::If {
                cond: Box::new(Expr::Literal(Literal::Bool(true), Span::default())),
                then_branch: Box::new(Expr::Literal(Literal::Int(1), Span::default())),
                else_branch: Some(Box::new(Expr::Literal(Literal::Int(2), Span::default()))),
                span: Span::default(),
            }),
            body: Box::new(Expr::Var("x".to_string(), Span::default())),
            span: Span::default(),
        };
        let body = lower_body(&source_body);
        // The if lowers to a Let stmt with an RValue::If, then x's Let, and
        // the body yields x.
        assert!(matches!(body.terminator, hir::Terminator::Yield(_)));
        assert!(body
            .stmts
            .iter()
            .any(|s| matches!(s, hir::Stmt::Let { value: hir::RValue::If { .. }, .. })));
    }
}
