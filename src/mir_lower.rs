//! HIR -> MIR lowering.
//!
//! Converts the typed High-level IR into the 3-address-code Mid-level IR.

use crate::ast::Pattern;
use crate::hir;
use crate::mir;
use crate::types::Type;

pub fn lower_module(hir: &hir::Module) -> mir::Module {
    let mut module = mir::Module::new(&hir.name);
    for decl in &hir.decls {
        match decl {
            hir::Decl::Function(f) => {
                module.functions.push(lower_function(f));
            }
            hir::Decl::Actor(a) => {
                let mut behavior_indices = Vec::new();
                for b in &a.behaviors {
                    let idx = module.functions.len();
                    module.functions.push(lower_behavior(b));
                    behavior_indices.push(idx);
                }
                let init_idx = module.functions.len();
                module.functions.push(lower_actor_init(a));
                module.actor_inits.push(mir::ActorInit {
                    actor_name: a.name.clone(),
                    behavior_indices,
                    init_function: init_idx,
                });
            }
            hir::Decl::ExternBlock { library, funcs, .. } => {
                for f in funcs {
                    module.foreign_functions.push(mir::ForeignFunction {
                        library: library.clone(),
                        symbol: f.name.clone(),
                        params: f.params.iter().map(|(_, t)| t.clone()).collect(),
                        ret: f.ret.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    module
}

fn lower_function(f: &hir::FunctionDef) -> mir::Function {
    let mut b = mir::FunctionBuilder::new(&f.name, Some(f.ret.clone()));
    let mut param_locals = Vec::new();
    for (name, ty) in &f.params {
        param_locals.push(b.add_param(name.clone(), ty.clone()));
    }
    let _ = param_locals;
    lower_body(&f.body, &mut b);
    b.build()
}

fn lower_behavior(bh: &hir::BehaviorDef) -> mir::Function {
    let mut b = mir::FunctionBuilder::behavior(&bh.name, Some(bh.ret.clone()));
    for (name, ty) in &bh.params {
        b.add_param(name.clone(), ty.clone());
    }
    lower_body(&bh.body, &mut b);
    b.build()
}

fn lower_actor_init(a: &hir::ActorDef) -> mir::Function {
    let mut b = mir::FunctionBuilder::new(format!("{}_init", a.name), Some(Type::unit()));
    lower_body(&hir::Body::new(), &mut b);
    b.build()
}

fn lower_body(body: &hir::Body, b: &mut mir::FunctionBuilder) {
    for stmt in &body.stmts {
        lower_stmt(stmt, b);
    }
    lower_terminator(&body.terminator, b);
}

fn lower_stmt(stmt: &hir::Stmt, b: &mut mir::FunctionBuilder) {
    match stmt {
        hir::Stmt::Let { name, ty, value, .. } => {
            let id = b.add_local(name.clone(), ty.clone());
            let rv = lower_rvalue(value, b);
            b.assign(id, rv);
        }
        hir::Stmt::Assign { target, value, .. } => {
            let src_rv = lower_rvalue(value, b);
            match target {
                hir::Place::Var(name, ty) => {
                    let dst = b.find_local(name).unwrap_or_else(|| b.add_local(name.clone(), ty.clone()));
                    b.assign(dst, src_rv);
                }
                hir::Place::Field { .. } | hir::Place::Index { .. } => {
                    // Field/index assignment not yet lowered to MIR.
                    let _ = src_rv;
                }
            }
        }
        hir::Stmt::StateSet { field, value, .. } => {
            let _ = field;
            let _ = lower_operand(value, b);
        }
        hir::Stmt::Emit { event, args, .. } => {
            let arg_ids: Vec<_> = args.iter().map(|a| lower_operand(a, b)).collect();
            b.emit(mir::Stmt::Emit {
                event: event.clone(),
                args: arg_ids,
            });
        }
    }
}

fn lower_rvalue(rv: &hir::RValue, b: &mut mir::FunctionBuilder) -> mir::RValue {
    use crate::bytecode::Constant;
    match rv {
        hir::RValue::Use(op) => mir::RValue::Load(lower_operand(op, b)),
        hir::RValue::Literal(lit, _) => mir::RValue::Const(literal_to_constant(lit)),
        hir::RValue::Binary(op, l, r, _) => {
            let lid = lower_operand(l, b);
            let rid = lower_operand(r, b);
            mir::RValue::Binary(*op, lid, rid)
        }
        hir::RValue::Unary(op, e, _) => {
            let id = lower_operand(e, b);
            mir::RValue::Unary(*op, id)
        }
        hir::RValue::Call { func, args, .. } => {
            let func_ref = match func {
                hir::Operand::Var(name, _) => mir::FuncRef::Named(name.clone()),
                _ => mir::FuncRef::Named("unknown".to_string()),
            };
            let aids: Vec<_> = args.iter().map(|a| lower_operand(a, b)).collect();
            mir::RValue::Call { func: func_ref, args: aids }
        }
        hir::RValue::Tuple(elems, _) => {
            mir::RValue::Tuple(elems.iter().map(|e| lower_operand(e, b)).collect())
        }
        hir::RValue::Record(fields, _) => {
            mir::RValue::Record(fields.iter().map(|(n, e)| (n.clone(), lower_operand(e, b))).collect())
        }
        hir::RValue::Array(elems, _) => {
            let len = b.add_temp(Type::int());
            b.assign(len, mir::RValue::Const(Constant::Int(elems.len() as i64)));
            mir::RValue::Array { len }
        }
        hir::RValue::FieldAccess { base, field, .. } => {
            let _ = field;
            let id = lower_operand(base, b);
            mir::RValue::LoadField { obj: id, field: 0 }
        }
        hir::RValue::Index { base, idx, .. } => {
            let arr = lower_operand(base, b);
            let i = lower_operand(idx, b);
            mir::RValue::ArrayLoad { arr, idx: i }
        }
        hir::RValue::Spawn { actor_type, .. } => {
            let _ = actor_type;
            let init = b.add_temp(Type::unit());
            mir::RValue::Spawn { behavior_idx: 0, init }
        }
        hir::RValue::Send { actor, behavior: _, args, .. } => {
            let a = lower_operand(actor, b);
            let aargs: Vec<_> = args.iter().map(|x| lower_operand(x, b)).collect();
            mir::RValue::Send {
                actor: a,
                behavior_id: 0,
                args: aargs,
            }
        }
        hir::RValue::Ask { actor, behavior: _, args, .. } => {
            let a = lower_operand(actor, b);
            let aargs: Vec<_> = args.iter().map(|x| lower_operand(x, b)).collect();
            mir::RValue::Ask {
                actor: a,
                behavior_id: 0,
                args: aargs,
            }
        }
        hir::RValue::SelfRef(_) => mir::RValue::SelfRef,
        hir::RValue::Perform { effect, op, args, .. } => {
            let _ = effect;
            let _ = op;
            let aargs: Vec<_> = args.iter().map(|x| lower_operand(x, b)).collect();
            mir::RValue::Perform {
                effect_id: 0,
                op_id: 0,
                args: aargs,
            }
        }
        hir::RValue::Handle { .. } => mir::RValue::SelfRef,
        hir::RValue::Receive { .. } => mir::RValue::SelfRef,
        hir::RValue::Migrate { actor, node, .. } => {
            let a = lower_operand(actor, b);
            let n = lower_operand(node, b);
            mir::RValue::Tuple(vec![a, n])
        }
        hir::RValue::CapCheck { operand, required } => {
            let id = lower_operand(operand, b);
            mir::RValue::CapabilityCheck {
                val: id,
                required: *required,
            }
        }
        hir::RValue::FFICall { symbol: _, args, .. } => {
            let aargs: Vec<_> = args.iter().map(|x| lower_operand(x, b)).collect();
            mir::RValue::FFICall {
                idx: 0,
                args: aargs,
            }
        }
        hir::RValue::Closure { .. } => mir::RValue::SelfRef,
    }
}

fn lower_operand(op: &hir::Operand, b: &mut mir::FunctionBuilder) -> mir::LocalId {
    match op {
        hir::Operand::Var(name, ty) => {
            // Try to find existing local by name; otherwise create it.
            if let Some(id) = b.find_local(name) {
                id
            } else {
                b.add_local(name.clone(), ty.clone())
            }
        }
        hir::Operand::Literal(lit, _) => {
            let id = b.add_temp(literal_type(lit));
            b.assign(id, mir::RValue::Const(literal_to_constant(lit)));
            id
        }
        hir::Operand::Unit => {
            let id = b.add_temp(Type::unit());
            b.assign(id, mir::RValue::Const(crate::bytecode::Constant::Int(0)));
            id
        }
    }
}

fn lower_terminator(term: &hir::Terminator, b: &mut mir::FunctionBuilder) {
    match term {
        hir::Terminator::Return(val) => {
            let id = val.as_ref().map(|op| lower_operand(op, b));
            b.terminate(mir::Terminator::Return(id));
        }
        hir::Terminator::If { cond, result, then_body, else_body } => {
            let cid = lower_operand(cond, b);
            let then_block = b.create_block();
            let else_block = b.create_block();
            let return_block = b.create_block();
            let result_id = b.find_local(result).unwrap_or_else(|| b.add_local(result.clone(), Type::unit()));

            b.terminate(mir::Terminator::Branch {
                cond: cid,
                then_: then_block,
                else_: else_block,
            });

            b.switch_to(then_block);
            lower_body(then_body, b);
            b.terminate(mir::Terminator::Jump(return_block));

            b.switch_to(else_block);
            if let Some(eb) = else_body {
                lower_body(eb, b);
            }
            b.terminate(mir::Terminator::Jump(return_block));

            b.switch_to(return_block);
            b.terminate(mir::Terminator::Return(Some(result_id)));
        }
        hir::Terminator::Match { scrutinee, result, arms } => {
            let entry_block = b.current_block();
            let sid = lower_operand(scrutinee, b);
            let result_id = b.find_local(result).unwrap_or_else(|| b.add_local(result.clone(), Type::unit()));
            let return_block = b.create_block();

            // Build arm blocks first.
            let mut arm_blocks = Vec::new();
            let mut default_arm: Option<mir::BlockId> = None;
            for (pat, arm_body) in arms {
                let arm_block = b.create_block();
                b.switch_to(arm_block);
                lower_body(arm_body, b);
                b.terminate(mir::Terminator::Jump(return_block));
                if matches!(pat, Pattern::Wild) {
                    default_arm = Some(arm_block);
                } else {
                    arm_blocks.push((pat.clone(), arm_block));
                }
            }

            // Build a chain of comparison blocks, one per literal arm.
            let default_block = default_arm.unwrap_or(return_block);
            let mut next_block = default_block;
            let mut cmp_blocks = Vec::new();
            for (pat, arm_block) in arm_blocks.into_iter().rev() {
                if let Some(lit) = pattern_literal(&pat) {
                    let cmp_block = b.create_block();
                    b.switch_to(cmp_block);
                    let lit_id = b.add_temp(literal_type(&lit));
                    b.assign(lit_id, mir::RValue::Const(literal_to_constant(&lit)));
                    let cond_id = b.add_temp(Type::bool());
                    b.assign(cond_id, mir::RValue::Binary(crate::ast::BinOp::Eq, sid, lit_id));
                    b.terminate(mir::Terminator::Branch {
                        cond: cond_id,
                        then_: arm_block,
                        else_: next_block,
                    });
                    cmp_blocks.push(cmp_block);
                    next_block = cmp_block;
                }
            }

            // The original block jumps into the comparison chain.
            b.switch_to(entry_block);
            if let Some(first_cmp) = cmp_blocks.last().copied() {
                b.terminate(mir::Terminator::Jump(first_cmp));
            } else if let Some(def) = default_arm {
                b.terminate(mir::Terminator::Jump(def));
            } else {
                b.terminate(mir::Terminator::Jump(return_block));
            }

            b.switch_to(return_block);
            b.terminate(mir::Terminator::Return(Some(result_id)));
        }
        hir::Terminator::Block(bodies) => {
            for body in bodies {
                lower_body(body, b);
            }
        }
        hir::Terminator::Break => {
            b.terminate(mir::Terminator::Unwind);
        }
    }
}

fn literal_to_constant(lit: &crate::ast::Literal) -> crate::bytecode::Constant {
    use crate::ast::Literal;
    match lit {
        Literal::Int(n) => crate::bytecode::Constant::Int(*n),
        Literal::Float(f) => crate::bytecode::Constant::Float(*f),
        Literal::String(s) => crate::bytecode::Constant::String(s.clone()),
        Literal::Bool(b) => crate::bytecode::Constant::Bool(*b),
        Literal::Nil | Literal::Unit => crate::bytecode::Constant::Int(0),
    }
}

fn literal_type(lit: &crate::ast::Literal) -> crate::types::Type {
    use crate::types::PrimitiveType;
    match lit {
        crate::ast::Literal::Int(_) => Type::Primitive(PrimitiveType::Int),
        crate::ast::Literal::Float(_) => Type::Primitive(PrimitiveType::Float),
        crate::ast::Literal::String(_) => Type::Primitive(PrimitiveType::String),
        crate::ast::Literal::Bool(_) => Type::Primitive(PrimitiveType::Bool),
        crate::ast::Literal::Nil | crate::ast::Literal::Unit => Type::Primitive(PrimitiveType::Unit),
    }
}

fn pattern_literal(pat: &Pattern) -> Option<crate::ast::Literal> {
    match pat {
        Pattern::Lit(lit) => Some(lit.clone()),
        _ => None,
    }
}
