//! AST -> HIR lowering.
//!
//! Converts the parsed, type-checked AST into the typed High-level IR.
//! For v0.2, expression types fall back to `Type::unit()` when no explicit
//! annotation is available; the structural pipeline is the primary goal.

use crate::ast;
use crate::ast::{BinOp, Decl, Expr, Literal};
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
        } => {
            // Flatten module decls for now.
            let mut module_decl = hir::Decl::Module {
                name: name.clone(),
                exports: exports.clone(),
                decls: Vec::new(),
                span: *span,
            };
            for d in decls {
                if let hir::Decl::Module { .. } = module_decl {
                    if let hir::Decl::Module { decls: ref mut inner, .. } = module_decl {
                        inner.push(lower_decl(d));
                    }
                }
            }
            module_decl
        }
        Decl::Import { path, items, span } => hir::Decl::Import {
            path: path.clone(),
            items: items.clone(),
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

pub fn lower_body(expr: &Expr) -> hir::Body {
    let mut body = hir::Body::new();
    let op = lower_expr(expr, &mut body);
    // If the expression did not already set a terminator (e.g. return/break),
    // make the body's value the return value.
    if matches!(body.terminator, hir::Terminator::Return(None)) {
        if is_unit_operand(&op) {
            body.set_terminator(hir::Terminator::Return(None));
        } else {
            body.set_terminator(hir::Terminator::Return(Some(op)));
        }
    }
    body
}

fn is_unit_operand(op: &hir::Operand) -> bool {
    matches!(op, hir::Operand::Unit)
}

/// Lower an expression into a sequence of statements in `body`, returning an
/// operand that represents the expression's value.
pub fn lower_expr(expr: &Expr, body: &mut hir::Body) -> hir::Operand {
    match expr {
        Expr::Literal(lit, _span) => {
            let ty = literal_type(lit);
            hir::Operand::Literal(lit.clone(), ty)
        }
        Expr::Var(name, _span) => hir::Operand::Var(name.clone(), Type::unit()),
        Expr::SelfRef(_) => hir::Operand::Var("self".to_string(), Type::unit()),
        Expr::TypeAnnotate { expr, ty, span } => {
            let mut inner = hir::Body::new();
            let op = lower_expr(expr, &mut inner);
            for stmt in inner.stmts {
                body.push(stmt);
            }
            body.set_terminator(inner.terminator);
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Use(op),
                span: *span,
            });
            hir::Operand::Var(temp, ty.clone())
        }
        Expr::CapAnnotate { expr, .. } => lower_expr(expr, body),
        Expr::Lambda { params, body: lb, effect: _, span } => {
            let lambda_body = lower_body(lb);
            let ty = Type::unit();
            let captures = free_vars(expr);
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
            let ty = Type::unit();
            body.push(hir::Stmt::Let {
                name: name.clone(),
                ty: ty.clone(),
                value: hir::RValue::Closure {
                    params: params
                        .iter()
                        .map(|(n, t)| (n.clone(), resolve_type(t)))
                        .collect(),
                    body: Box::new(func_body),
                    captures: Vec::new(),
                    ty: ty.clone(),
                },
                span: *span,
            });
            lower_expr(b, body)
        }
        Expr::If { cond, then_branch, else_branch, span } => {
            let cond_op = lower_expr(cond, body);
            let ty = Type::unit();
            let result_name = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: result_name.clone(),
                ty: ty.clone(),
                value: hir::RValue::Use(hir::Operand::Unit),
                span: *span,
            });

            let mut then_body = hir::Body::new();
            let then_op = lower_expr(then_branch, &mut then_body);
            then_body.push(hir::Stmt::Assign {
                target: hir::Place::Var(result_name.clone(), ty.clone()),
                value: hir::RValue::Use(then_op),
                span: *span,
            });
            then_body.set_terminator(hir::Terminator::Return(None));

            let else_body = else_branch
                .as_ref()
                .map(|e| {
                    let mut eb = hir::Body::new();
                    let else_op = lower_expr(e, &mut eb);
                    eb.push(hir::Stmt::Assign {
                        target: hir::Place::Var(result_name.clone(), ty.clone()),
                        value: hir::RValue::Use(else_op),
                        span: *span,
                    });
                    eb.set_terminator(hir::Terminator::Return(None));
                    Box::new(eb)
                });

            body.set_terminator(hir::Terminator::If {
                cond: cond_op,
                result: result_name.clone(),
                then_body: Box::new(then_body),
                else_body,
            });
            hir::Operand::Var(result_name, ty)
        }
        Expr::Match { scrutinee, arms, span } => {
            let scrut_op = lower_expr(scrutinee, body);
            let ty = Type::unit();
            let result_name = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: result_name.clone(),
                ty: ty.clone(),
                value: hir::RValue::Use(hir::Operand::Unit),
                span: *span,
            });

            let arms_hir: Vec<_> = arms
                .iter()
                .map(|(pat, e)| {
                    let mut arm_body = hir::Body::new();
                    let arm_op = lower_expr(e, &mut arm_body);
                    arm_body.push(hir::Stmt::Assign {
                        target: hir::Place::Var(result_name.clone(), ty.clone()),
                        value: hir::RValue::Use(arm_op),
                        span: *span,
                    });
                    arm_body.set_terminator(hir::Terminator::Return(None));
                    (pat.clone(), Box::new(arm_body))
                })
                .collect();

            body.set_terminator(hir::Terminator::Match {
                scrutinee: scrut_op,
                result: result_name.clone(),
                arms: arms_hir,
            });
            hir::Operand::Var(result_name, ty)
        }
        Expr::Block { exprs, span: _ } => {
            let mut last = hir::Operand::Unit;
            for e in exprs {
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
            let ty = binary_type(op, &l, &r);
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
        Expr::For { var: _, iterable, body: b, span } => {
            let iop = lower_expr(iterable, body);
            let _loop_body = lower_body(b);
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Use(iop),
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Pipe { left, right, span } => {
            let lop = lower_expr(left, body);
            let rop = lower_expr(right, body);
            let ty = Type::unit();
            let temp = fresh_temp_name();
            body.push(hir::Stmt::Let {
                name: temp.clone(),
                ty: ty.clone(),
                value: hir::RValue::Call {
                    func: rop,
                    args: vec![lop],
                    ty: ty.clone(),
                },
                span: *span,
            });
            hir::Operand::Var(temp, ty)
        }
        Expr::Return(val, _span) => {
            let op = val
                .as_ref()
                .map(|e| lower_expr(e, body))
                .unwrap_or(hir::Operand::Unit);
            body.set_terminator(hir::Terminator::Return(Some(op)));
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

fn binary_type(op: &BinOp, _l: &hir::Operand, _r: &hir::Operand) -> Type {
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

static mut TEMP_COUNTER: u32 = 0;

fn fresh_temp_name() -> String {
    unsafe {
        let n = TEMP_COUNTER;
        TEMP_COUNTER += 1;
        format!("__tmp{}", n)
    }
}

fn free_vars(_expr: &Expr) -> Vec<String> {
    // TODO: implement proper free-variable capture for closures.
    Vec::new()
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
                public: true,
                span: Span::default(),
            }],
        };
        let hir = lower_module(&ast);
        assert_eq!(hir.decls.len(), 1);
    }
}
