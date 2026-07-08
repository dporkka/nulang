//! Hindley-Milner type checker (Algorithm W) for Nulang.
//!
//! Implements classical Damas-Milner type inference with support for:
//! - Primitive types (Int, Float, Bool, String, Unit, Never, Address)
//! - Polymorphism via type schemes (forall vars. Type)
//! - Tuples, Records, Variants, Arrays
//! - Functions with effect rows and capability annotations
//! - Reference types with capabilities
//! - Actor types
//! - Pattern matching
//! - Binary and unary operators
//!
//! The algorithm follows the standard substitution-based approach:
//! 1. `infer` computes a type and a substitution
//! 2. `mgu` (most general unifier) produces substitutions from equality constraints
//! 3. `apply_subst` propagates substitutions through types
//! 4. `generalize` creates polymorphic schemes from free variables
//! 5. `instantiate` creates fresh type variables from schemes

use crate::ast::*;
use crate::types::*;
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Substitution
// ---------------------------------------------------------------------------

/// A substitution maps type variables to types.
/// Ordered list: earlier substitutions take precedence.
pub type Substitution = Vec<(TypeVar, Type)>;

/// Apply a substitution to a type, replacing any type variables that appear
/// in the substitution with their mapped types.
fn apply_subst(ty: &Type, subst: &Substitution) -> Type {
    match ty {
        Type::Var(v) => {
            // Find the first mapping for this variable
            for (var, replacement) in subst {
                if var == v {
                    // Apply recursively in case the replacement contains vars
                    // that are also in the substitution
                    return apply_subst(replacement, subst);
                }
            }
            Type::Var(*v)
        }
        Type::Primitive(_) => ty.clone(),
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| apply_subst(t, subst)).collect()),
        Type::Record(fs) => Type::Record(
            fs.iter()
                .map(|(name, t)| (name.clone(), apply_subst(t, subst)))
                .collect(),
        ),
        Type::Variant(vs) => Type::Variant(
            vs.iter()
                .map(|(name, t)| (name.clone(), t.as_ref().map(|t| apply_subst(t, subst))))
                .collect(),
        ),
        Type::Array(t) => Type::Array(Box::new(apply_subst(t, subst))),
        Type::Function {
            param,
            ret,
            effect,
            cap,
        } => Type::Function {
            param: Box::new(apply_subst(param, subst)),
            ret: Box::new(apply_subst(ret, subst)),
            effect: effect.clone(),
            cap: *cap,
        },
        Type::Actor { state, behavior } => Type::Actor {
            state: Box::new(apply_subst(state, subst)),
            behavior: Box::new(apply_subst(behavior, subst)),
        },
        Type::App { constructor, args } => Type::App {
            constructor: Box::new(apply_subst(constructor, subst)),
            args: args.iter().map(|a| apply_subst(a, subst)).collect(),
        },
        Type::Reference { cap, inner } => Type::Reference {
            cap: *cap,
            inner: Box::new(apply_subst(inner, subst)),
        },
        Type::Scheme { vars, body } => {
            // Remove substitutions for bound variables
            let filtered: Substitution = subst
                .iter()
                .filter(|(v, _)| !vars.contains(v))
                .cloned()
                .collect();
            Type::Scheme {
                vars: vars.clone(),
                body: Box::new(apply_subst(body, &filtered)),
            }
        }
    }
}

/// Apply a substitution to a type context, returning the updated context.
/// Since TypeContext stores types at binding time and we always create fresh
/// contexts via `extend` / `clone`, substitutions are effectively applied by
/// binding already-substituted types. This function is a placeholder for
/// contexts where we need to pass through a substitution.
#[allow(unused_variables)]
fn apply_subst_to_ctx(ctx: &TypeContext, _subst: &Substitution) -> TypeContext {
    ctx.clone()
}

/// Compose two substitutions: s2 after s1.
/// Result: first apply s1, then apply s2 to the result.
/// Formally: (s2 ∘ s1)(t) = s2(s1(t))
fn compose_subst(s2: &Substitution, s1: &Substitution) -> Substitution {
    // Apply s2 to all types in s1
    let mut s1_substituted: Substitution = s1
        .iter()
        .map(|(v, t)| (*v, apply_subst(t, s2)))
        .collect();
    // Add s2 entries that don't conflict with s1
    let s1_vars: HashSet<TypeVar> = s1.iter().map(|(v, _)| *v).collect();
    for (v, t) in s2 {
        if !s1_vars.contains(v) {
            s1_substituted.push((*v, t.clone()));
        }
    }
    s1_substituted
}

// ---------------------------------------------------------------------------
// Unification (Most General Unifier)
// ---------------------------------------------------------------------------

/// Check if two effect rows are compatible (can be unified).
/// For closed rows, they must have exactly the same effects.
/// For open rows, we check that the fixed effects are compatible.
fn effect_row_compatible(e1: &EffectRow, e2: &EffectRow) -> bool {
    match (e1, e2) {
        (EffectRow::Closed(a), EffectRow::Closed(b)) => {
            let mut a_sorted = a.clone();
            let mut b_sorted = b.clone();
            a_sorted.sort_by(|x, y| format!("{:?}", x).cmp(&format!("{:?}", y)));
            b_sorted.sort_by(|x, y| format!("{:?}", x).cmp(&format!("{:?}", y)));
            a_sorted == b_sorted
        }
        (EffectRow::Open(a, _), EffectRow::Closed(b))
        | (EffectRow::Closed(b), EffectRow::Open(a, _)) => b.iter().all(|e| a.contains(e)),
        (EffectRow::Open(a, _), EffectRow::Open(b, _)) => {
            // Both open: compatible if they share the same fixed effects
            // (full row unification would require more sophisticated handling)
            a.iter().all(|e| b.contains(e)) || b.iter().all(|e| a.contains(e))
        }
    }
}

/// Compute the most general unifier of two types.
/// Returns a substitution `s` such that `apply_subst(t1, s) == apply_subst(t2, s)`.
fn mgu(t1: &Type, t2: &Type, span: Span) -> NuResult<Substitution> {
    // Handle the case where both types are the same reference
    if t1 == t2 {
        return Ok(vec![]);
    }

    match (t1, t2) {
        // Identical primitives unify trivially
        (Type::Primitive(a), Type::Primitive(b)) if a == b => Ok(vec![]),

        // Type variable unification
        (Type::Var(v), t) | (t, Type::Var(v)) => var_subst(*v, t, span),

        // Functions: unify parameters, returns, effects, and capabilities
        (
            Type::Function {
                param: p1,
                ret: r1,
                effect: e1,
                cap: c1,
            },
            Type::Function {
                param: p2,
                ret: r2,
                effect: e2,
                cap: c2,
            },
        ) => {
            if c1 != c2 {
                return Err(NuError::TypeError {
                    msg: format!(
                        "Cannot unify functions with different capabilities: {:?} vs {:?}",
                        c1, c2
                    ),
                    span,
                });
            }
            // Check effect row compatibility
            if !effect_row_compatible(e1, e2) {
                return Err(NuError::TypeError {
                    msg: format!(
                        "Cannot unify functions with incompatible effects: {:?} vs {:?}",
                        e1, e2
                    ),
                    span,
                });
            }
            let s1 = mgu(p1, p2, span)?;
            let s2 = mgu(&apply_subst(r1, &s1), &apply_subst(r2, &s1), span)?;
            Ok(compose_subst(&s2, &s1))
        }

        // Tuples
        (Type::Tuple(ts1), Type::Tuple(ts2)) => {
            if ts1.len() != ts2.len() {
                return Err(NuError::TypeError {
                    msg: format!(
                        "Cannot unify tuples of different lengths: {} vs {}",
                        ts1.len(),
                        ts2.len()
                    ),
                    span,
                });
            }
            unify_many(ts1, ts2, span)
        }

        // Records
        (Type::Record(fs1), Type::Record(fs2)) => {
            if fs1.len() != fs2.len() {
                return Err(NuError::TypeError {
                    msg: format!(
                        "Cannot unify records with different field counts: {} vs {}",
                        fs1.len(),
                        fs2.len()
                    ),
                    span,
                });
            }
            // Sort by field name and unify corresponding fields
            let mut sorted1 = fs1.clone();
            let mut sorted2 = fs2.clone();
            sorted1.sort_by(|(a, _), (b, _)| a.cmp(b));
            sorted2.sort_by(|(a, _), (b, _)| a.cmp(b));
            let mut subst = vec![];
            for ((n1, t1f), (n2, t2f)) in sorted1.iter().zip(sorted2.iter()) {
                if n1 != n2 {
                    return Err(NuError::TypeError {
                        msg: format!(
                            "Cannot unify records with different field names: '{}' vs '{}'",
                            n1, n2
                        ),
                        span,
                    });
                }
                let s = mgu(&apply_subst(t1f, &subst), &apply_subst(t2f, &subst), span)?;
                subst = compose_subst(&s, &subst);
            }
            Ok(subst)
        }

        // Arrays
        (Type::Array(t1_inner), Type::Array(t2_inner)) => mgu(t1_inner, t2_inner, span),

        // Actors
        (
            Type::Actor {
                state: s1,
                behavior: b1,
            },
            Type::Actor {
                state: s2,
                behavior: b2,
            },
        ) => {
            let s_state = mgu(s1, s2, span)?;
            let s_beh = mgu(&apply_subst(b1, &s_state), &apply_subst(b2, &s_state), span)?;
            Ok(compose_subst(&s_beh, &s_state))
        }

        // Reference types
        (Type::Reference { cap: c1, inner: i1 }, Type::Reference { cap: c2, inner: i2 }) => {
            if c1 != c2 {
                return Err(NuError::TypeError {
                    msg: format!(
                        "Cannot unify references with different capabilities: {:?} vs {:?}",
                        c1, c2
                    ),
                    span,
                });
            }
            mgu(i1, i2, span)
        }

        // Generic type application
        (Type::App { constructor: c1, args: a1 }, Type::App { constructor: c2, args: a2 }) => {
            let s1 = mgu(c1, c2, span)?;
            let applied1: Vec<Type> = a1.iter().map(|t| apply_subst(t, &s1)).collect();
            let applied2: Vec<Type> = a2.iter().map(|t| apply_subst(t, &s1)).collect();
            let s2 = unify_many_app(&applied1, &applied2, span)?;
            Ok(compose_subst(&s2, &s1))
        }

        // Anything else is a unification error
        _ => Err(NuError::TypeError {
            msg: format!("Cannot unify {:?} with {:?}", t1, t2),
            span,
        }),
    }
}

/// Unify a list of type variable / type pairs (common sub-structures).
fn unify_many_app(types1: &[Type], types2: &[Type], span: Span) -> NuResult<Substitution> {
    if types1.len() != types2.len() {
        return Err(NuError::TypeError {
            msg: format!(
                "Cannot unify type lists of different lengths: {} vs {}",
                types1.len(),
                types2.len()
            ),
            span,
        });
    }
    let mut subst = vec![];
    for (t1, t2) in types1.iter().zip(types2.iter()) {
        let s = mgu(&apply_subst(t1, &subst), &apply_subst(t2, &subst), span)?;
        subst = compose_subst(&s, &subst);
    }
    Ok(subst)
}

/// Unify two lists of types pairwise.
fn unify_many(types1: &[Type], types2: &[Type], span: Span) -> NuResult<Substitution> {
    if types1.len() != types2.len() {
        return Err(NuError::TypeError {
            msg: format!(
                "Cannot unify lists of different lengths: {} vs {}",
                types1.len(),
                types2.len()
            ),
            span,
        });
    }
    let mut subst = vec![];
    for (t1, t2) in types1.iter().zip(types2.iter()) {
        let s = mgu(&apply_subst(t1, &subst), &apply_subst(t2, &subst), span)?;
        subst = compose_subst(&s, &subst);
    }
    Ok(subst)
}

/// Create a substitution for a single type variable, with occurs check.
fn var_subst(v: TypeVar, t: &Type, span: Span) -> NuResult<Substitution> {
    match t {
        Type::Var(v2) if *v2 == v => Ok(vec![]), // t = t
        t => {
            if occurs_in(v, t) {
                return Err(NuError::TypeError {
                    msg: format!(
                        "Occurs check failed: type variable {:?} occurs in {:?}",
                        v, t
                    ),
                    span,
                });
            }
            Ok(vec![(v, t.clone())])
        }
    }
}

/// Check if a type variable occurs within a type (occurs check).
fn occurs_in(v: TypeVar, t: &Type) -> bool {
    match t {
        Type::Var(v2) => *v2 == v,
        Type::Primitive(_) => false,
        Type::Tuple(ts) => ts.iter().any(|t| occurs_in(v, t)),
        Type::Record(fs) => fs.iter().any(|(_, t)| occurs_in(v, t)),
        Type::Variant(vs) => vs.iter().any(|(_, t)| t.as_ref().map_or(false, |t| occurs_in(v, t))),
        Type::Array(t) => occurs_in(v, t),
        Type::Function { param, ret, .. } => occurs_in(v, param) || occurs_in(v, ret),
        Type::Actor { state, behavior } => occurs_in(v, state) || occurs_in(v, behavior),
        Type::App { constructor, args } => {
            occurs_in(v, constructor) || args.iter().any(|a| occurs_in(v, a))
        }
        Type::Reference { inner, .. } => occurs_in(v, inner),
        Type::Scheme { vars, body } => !vars.contains(&v) && occurs_in(v, body),
    }
}

// ---------------------------------------------------------------------------
// Instantiation
// ---------------------------------------------------------------------------

/// Instantiate a scheme by replacing all bound type variables with fresh ones.
fn instantiate(ty: &Type) -> Type {
    match ty {
        Type::Scheme { vars, body } => {
            let subst: Substitution = vars.iter().map(|v| (*v, Type::Var(TypeVar::fresh()))).collect();
            apply_subst(body, &subst)
        }
        _ => ty.clone(),
    }
}

// ---------------------------------------------------------------------------
// TypeChecker
// ---------------------------------------------------------------------------

/// Hindley-Milner type checker implementing Algorithm W.
///
/// Maintains a counter for fresh type variables and tracks context free variables
/// for proper generalization.
pub struct TypeChecker {
    /// Free type variables present in the initial context (to avoid over-generalization)
    ctx_free_vars: HashSet<TypeVar>,
}

impl TypeChecker {
    /// Create a new type checker with an empty context.
    pub fn new() -> Self {
        TypeChecker {
            ctx_free_vars: HashSet::new(),
        }
    }

    /// Type-check an entire module, returning the type of the last declaration.
    pub fn check_module(&mut self, module: &AstModule) -> NuResult<Type> {
        let mut ctx = TypeContext::new();
        let mut last_type = Type::unit();
        for decl in &module.decls {
            let (s, ty) = self.infer_decl(&ctx, decl)?;
            ctx = apply_subst_to_ctx(&ctx, &s);
            let final_ty = apply_subst(&ty, &s);
            match decl {
                Decl::Function { name, .. } => {
                    ctx.bind(name.clone(), final_ty.clone(), Capability::Ref);
                }
                Decl::Actor { name, .. } => {
                    ctx.bind(name.clone(), final_ty.clone(), Capability::Ref);
                }
                Decl::Extern { funcs, .. } => {
                    for func in funcs {
                        let param_types: Vec<Type> =
                            func.params.iter().map(|(_, t)| t.clone()).collect();
                        let param_ty = if param_types.len() == 1 {
                            param_types[0].clone()
                        } else {
                            Type::Tuple(param_types)
                        };
                        let func_ty = Type::Function {
                            param: Box::new(param_ty),
                            ret: Box::new(func.ret.clone()),
                            effect: EffectRow::singleton(Effect::FFI),
                            cap: Capability::Ref,
                        };
                        ctx.bind(func.name.clone(), func_ty, Capability::Ref);
                    }
                }
                Decl::Workflow { name, .. } => {
                    ctx.bind(name.clone(), final_ty.clone(), Capability::Ref);
                }
                Decl::Agent { name, .. } => {
                    ctx.bind(name.clone(), final_ty.clone(), Capability::Ref);
                }

                _ => {}
            }
            last_type = final_ty;
        }
        Ok(last_type)
    }

    /// Infer the type of a declaration.
    fn infer_decl(&mut self, ctx: &TypeContext, decl: &Decl) -> NuResult<(Substitution, Type)> {
        match decl {
            Decl::Function {
                name,
                params,
                ret_type,
                effect,
                body,
                span,
                ..
            } => {
                // Create fresh type variables for parameters
                let mut param_types = vec![];
                for (_param_name, param_ty) in params {
                    let pty = match param_ty {
                        Some(t) => t.clone(),
                        None => Type::Var(TypeVar::fresh()),
                    };
                    param_types.push(pty);
                }

                // Preliminary parameter type for the function binding
                let param_ty = if param_types.len() == 1 {
                    param_types[0].clone()
                } else {
                    Type::Tuple(param_types.clone())
                };

                // Fresh return type variable so the function can refer to itself
                // recursively before its body is inferred.
                let ret_var = Type::Var(TypeVar::fresh());
                let declared_effect = effect.clone().unwrap_or_else(EffectRow::empty);
                let recursive_func_ty = Type::Function {
                    param: Box::new(param_ty.clone()),
                    ret: Box::new(ret_var.clone()),
                    effect: declared_effect.clone(),
                    cap: Capability::Ref,
                };

                let mut new_ctx = ctx.clone();
                // Bind the function name so recursive calls resolve.
                new_ctx.bind(name.clone(), recursive_func_ty, Capability::Ref);
                // Bind parameters
                for (param_name, pty) in params.iter().zip(param_types.iter()) {
                    new_ctx.bind(param_name.0.clone(), pty.clone(), Capability::Ref);
                }

                // Infer body type
                let (s1, body_ty) = self.infer_expr(&new_ctx, body)?;

                // Unify the preliminary return variable with the inferred body type
                let s_rec = mgu(&apply_subst(&ret_var, &s1), &body_ty, *span)?;
                let s1 = compose_subst(&s_rec, &s1);

                // Unify with declared return type if present
                let s2 = match ret_type {
                    Some(rt) => {
                        let body_subst = apply_subst(&body_ty, &s1);
                        let rt_subst = apply_subst(rt, &s1);
                        mgu(&body_subst, &rt_subst, *span)?
                    }
                    None => vec![],
                };
                let s_combined = compose_subst(&s2, &s1);

                // Build final function type
                let param_ty = apply_subst(&param_ty, &s_combined);
                let ret_ty = apply_subst(&body_ty, &s_combined);
                let func_ty = Type::Function {
                    param: Box::new(param_ty),
                    ret: Box::new(ret_ty),
                    effect: declared_effect.clone(),
                    cap: Capability::Ref,
                };

                // Generalize and add to context
                let gen_ty = self.do_generalize(ctx, &apply_subst(&func_ty, &s_combined));
                let mut final_ctx = ctx.clone();
                final_ctx.bind(name.clone(), gen_ty, Capability::Ref);

                Ok((s_combined, func_ty))
            }
            Decl::TypeAlias { .. } => Ok((vec![], Type::unit())),
            Decl::RecordType { .. } => Ok((vec![], Type::unit())),
            Decl::VariantType { .. } => Ok((vec![], Type::unit())),
            Decl::EffectDecl { .. } => Ok((vec![], Type::unit())),
            Decl::Actor { name, behaviors, span, .. } => {
                self.infer_actor_decl(ctx, name, behaviors, *span)
            }
            Decl::Agent { .. } => {
                // An agent declaration is an opaque module-level binding with a
                // synthetic actor type, just like actors and workflows.
                let agent_ty = Type::Actor {
                    state: Box::new(Type::Var(TypeVar::fresh())),
                    behavior: Box::new(Type::Var(TypeVar::fresh())),
                };
                Ok((vec![], agent_ty))
            }
            Decl::Extern { funcs, span, .. } => {
                for func in funcs {
                    for (_name, ty) in &func.params {
                        self.validate_ffi_type(ty, *span)?;
                    }
                    self.validate_ffi_type(&func.ret, *span)?;
                }
                Ok((vec![], Type::unit()))
            }

            Decl::Module { decls, .. } => {
                let mut ctx = ctx.clone();
                let mut last = Type::unit();
                let mut all_subst = vec![];
                for d in decls {
                    let (s, ty) = self.infer_decl(&ctx, d)?;
                    ctx = apply_subst_to_ctx(&ctx, &s);
                    all_subst = compose_subst(&s, &all_subst);
                    last = ty;
                }
                Ok((all_subst, last))
            }
            Decl::Import { .. } => Ok((vec![], Type::unit())),
            Decl::Workflow {
                name: _,
                input,
                items,
                span: _,
                ..
            } => {
                // A workflow declaration is an opaque module-level binding with a
                // synthetic actor type. Each step body is type-checked in a context
                // extended with the workflow input, if one is declared.
                let mut workflow_ctx = ctx.clone();
                if let Some((input_name, input_ty)) = input {
                    workflow_ctx.bind(input_name.clone(), input_ty.clone(), Capability::Ref);
                }
                for item in items {
                    match item {
                        crate::ast::WorkflowItem::Step(step) => {
                            let (_s, _body_ty) = self.infer_expr(&workflow_ctx, &step.body)?;
                            if let Some(comp_expr) = &step.compensate {
                                let (_s, _comp_ty) = self.infer_expr(&workflow_ctx, comp_expr)?;
                            }
                        }
                        crate::ast::WorkflowItem::Parallel(branches) => {
                            for step in branches {
                                let (_s, _body_ty) = self.infer_expr(&workflow_ctx, &step.body)?;
                                if let Some(comp_expr) = &step.compensate {
                                    let (_s, _comp_ty) = self.infer_expr(&workflow_ctx, comp_expr)?;
                                }
                            }
                        }
                    }
                }
                let workflow_ty = Type::Actor {
                    state: Box::new(Type::Var(TypeVar::fresh())),
                    behavior: Box::new(Type::Var(TypeVar::fresh())),
                };
                Ok((vec![], workflow_ty))
            }
        }
    }

    /// Infer the type of an expression (Algorithm W).
    /// Returns (substitution, inferred_type).
    pub fn infer_expr(&mut self, ctx: &TypeContext, expr: &Expr) -> NuResult<(Substitution, Type)> {
        match expr {
            // Literals: exact primitive type
            Expr::Literal(lit, span) => self.infer_literal(lit, *span),

            // Variables: look up in context, instantiate scheme
            Expr::Var(name, span) => self.infer_var(ctx, name, *span),

            // Lambda: introduce fresh type vars for params, infer body
            Expr::Lambda {
                params,
                body,
                effect,
                span,
            } => self.infer_lambda(ctx, params, body, effect.as_ref(), *span),

            // Application: infer function, infer arg, unify, return result
            Expr::App { func, args, span } => self.infer_app(ctx, func, args, *span),

            // Let binding: infer value, generalize, extend context, infer body
            Expr::Let {
                name,
                value,
                body,
                span,
            } => self.infer_let(ctx, name, value, body, *span),

            // Let-rec: recursive binding
            Expr::LetRec {
                name,
                params,
                value,
                body,
                span,
            } => self.infer_letrec(ctx, name, params, value, body, *span),

            // If: condition must be Bool, branches must match
            Expr::If {
                cond,
                then_branch,
                else_branch,
                span,
            } => self.infer_if(ctx, cond, then_branch, else_branch.as_ref(), *span),

            // Binary operators: type-specific rules
            Expr::Binary { op, left, right, span } => self.infer_binary(ctx, *op, left, right, *span),

            // Unary operators
            Expr::Unary { op, expr, span } => self.infer_unary(ctx, *op, expr, *span),

            // Tuple: infer each element
            Expr::Tuple(exprs, span) => self.infer_tuple(ctx, exprs, *span),

            // Record literal: infer each field
            Expr::Record(fields, span) => self.infer_record(ctx, fields, *span),

            // Field access: look up field in record type
            Expr::FieldAccess { expr, field, span } => self.infer_field_access(ctx, expr, field, *span),

            // Array literal
            Expr::Array(elems, span) => self.infer_array(ctx, elems, *span),

            // Array index
            Expr::Index { arr, idx, span } => self.infer_index(ctx, arr, idx, *span),

            // Pattern matching
            Expr::Match {
                scrutinee,
                arms,
                span,
            } => self.infer_match(ctx, scrutinee, arms, *span),

            // Block: sequence of expressions
            Expr::Block { exprs, span } => self.infer_block(ctx, exprs, *span),

            // Spawn actor
            Expr::Spawn {
                actor_type,
                init: _,
                span,
            } => self.infer_spawn(ctx, actor_type, *span),

            // Send message
            Expr::Send {
                actor,
                behavior,
                args,
                span,
            } => self.infer_send(ctx, actor, behavior, args, *span),

            // Ask request
            Expr::Ask {
                actor,
                behavior: _,
                args: _,
                span,
            } => self.infer_ask(ctx, actor, *span),

            // Receive
            Expr::Receive { .. } => {
                let ret_ty = Type::Var(TypeVar::fresh());
                Ok((vec![], ret_ty))
            }

            // Self reference
            Expr::SelfRef(_) => Ok((vec![], Type::Var(TypeVar::fresh()))),

            // Perform effect
            Expr::Perform { effect, op: _, args, span } => {
                self.infer_perform(ctx, effect, args, *span)
            }

            // Emit event
            Expr::Emit { args, .. } => {
                let mut subst = Vec::new();
                for arg in args {
                    let ctx_sub = apply_subst_to_ctx(ctx, &subst);
                    let (s, _ty) = self.infer_expr(&ctx_sub, arg)?;
                    subst = compose_subst(&s, &subst);
                }
                Ok((subst, Type::unit()))
            }

            // Handle effect
            Expr::Handle {
                body,
                handlers,
                span,
            } => self.infer_handle(ctx, body, handlers, *span),

            // Migrate actor
            Expr::Migrate { actor, node: _, span } => {
                let (s1, actor_ty) = self.infer_expr(ctx, actor)?;
                // Actor must be an actor type
                match &actor_ty {
                    Type::Actor { .. } => Ok((s1, actor_ty)),
                    _ => {
                        let actor_var = TypeVar::fresh();
                        let s2 = mgu(
                            &apply_subst(&actor_ty, &s1),
                            &Type::Actor {
                                state: Box::new(Type::Var(actor_var)),
                                behavior: Box::new(Type::Var(TypeVar::fresh())),
                            },
                            *span,
                        )?;
                        let actor_subst = apply_subst(&actor_ty, &compose_subst(&s2, &s1));
                        Ok((compose_subst(&s2, &s1), actor_subst))
                    }
                }
            }

            // Capability annotation
            Expr::CapAnnotate { expr, cap, span: _ } => {
                let (s, ty) = self.infer_expr(ctx, expr)?;
                // Wrap in reference type with the given capability
                let ref_ty = Type::Reference {
                    cap: *cap,
                    inner: Box::new(apply_subst(&ty, &s)),
                };
                Ok((s, ref_ty))
            }

            // Type annotation
            Expr::TypeAnnotate { expr, ty, span } => {
                let (s1, inferred) = self.infer_expr(ctx, expr)?;
                let s2 = mgu(&apply_subst(&inferred, &s1), ty, *span)?;
                Ok((compose_subst(&s2, &s1), apply_subst(ty, &compose_subst(&s2, &s1))))
            }

            // Pipe operator: x |> f  ===  f(x), and x |> f(a, b) === f(x, a, b)
            Expr::Pipe { left, right, span } => {
                let (s1, left_ty) = self.infer_expr(ctx, left)?;
                let ctx1 = apply_subst_to_ctx(ctx, &s1);

                // If the right side is already a function application, prepend
                // the piped value as the first argument. This matches the
                // compiler's pipe lowering and supports multi-arg functions.
                if let Expr::App { func, args, .. } = right.as_ref() {
                    let mut new_args = vec![left.as_ref().clone()];
                    new_args.extend(args.iter().cloned());
                    let app = Expr::App {
                        func: func.clone(),
                        args: new_args,
                        span: *span,
                    };
                    let (s2, ty) = self.infer_expr(&ctx1, &app)?;
                    let final_subst = compose_subst(&s2, &s1);
                    return Ok((final_subst.clone(), apply_subst(&ty, &final_subst)));
                }

                let (s2, right_ty) = self.infer_expr(&ctx1, right)?;
                // right should be a function taking left's type
                let result_var = Type::Var(TypeVar::fresh());
                let expected = Type::Function {
                    param: Box::new(apply_subst(&left_ty, &compose_subst(&s2, &s1))),
                    ret: Box::new(result_var.clone()),
                    effect: EffectRow::empty(),
                    cap: Capability::Ref,
                };
                let s3 = mgu(&apply_subst(&right_ty, &s2), &expected, *span)?;
                let final_subst = compose_subst(&s3, &compose_subst(&s2, &s1));
                Ok((final_subst.clone(), apply_subst(&result_var, &final_subst)))
            }

            // For comprehension
            Expr::For {
                var,
                iterable,
                body,
                span,
            } => self.infer_for(ctx, var, iterable, body, *span),

            // Return
            Expr::Return(expr, _span) => {
                if let Some(e) = expr {
                    self.infer_expr(ctx, e)
                } else {
                    Ok((vec![], Type::unit()))
                }
            }

            // Break
            Expr::Break(_) => {
                let fresh = Type::Var(TypeVar::fresh());
                Ok((vec![], fresh))
            }

            // Assignment: target must be a reference
            Expr::Assign { target, value, span } => {
                let (s1, target_ty) = self.infer_expr(ctx, target)?;
                let ctx1 = apply_subst_to_ctx(ctx, &s1);
                let (s2, value_ty) = self.infer_expr(&ctx1, value)?;
                // Unify target (should be a reference) with value
                let s3 = mgu(
                    &apply_subst(&target_ty, &compose_subst(&s2, &s1)),
                    &Type::Reference {
                        cap: Capability::Ref,
                        inner: Box::new(apply_subst(&value_ty, &s2)),
                    },
                    *span,
                )?;
                let final_subst = compose_subst(&s3, &compose_subst(&s2, &s1));
                Ok((final_subst, Type::unit()))
            }
        }
    }

    // -----------------------------------------------------------------------
    // Inference helpers for each expression form
    // -----------------------------------------------------------------------

    /// Infer the type of a literal.
    fn infer_literal(&mut self, lit: &Literal, _span: Span) -> NuResult<(Substitution, Type)> {
        let ty = match lit {
            Literal::Int(_) => Type::int(),
            Literal::Float(_) => Type::float(),
            Literal::String(_) => Type::string(),
            Literal::Bool(_) => Type::bool(),
            Literal::Nil => Type::unit(), // TODO: dedicated nil type
            Literal::Unit => Type::unit(),
        };
        Ok((vec![], ty))
    }

    /// Infer the type of a variable reference.
    fn infer_var(&mut self, ctx: &TypeContext, name: &str, span: Span) -> NuResult<(Substitution, Type)> {
        match ctx.lookup(name) {
            Some((ty, _cap)) => {
                let instantiated = instantiate(ty);
                Ok((vec![], instantiated))
            }
            None => Err(NuError::TypeError {
                msg: format!("Unbound variable: '{}'", name),
                span,
            }),
        }
    }

    /// Infer the type of a lambda expression.
    fn infer_lambda(
        &mut self,
        ctx: &TypeContext,
        params: &[(String, Option<Type>)],
        body: &Expr,
        effect: Option<&EffectRow>,
        _span: Span,
    ) -> NuResult<(Substitution, Type)> {
        let mut new_ctx = ctx.clone();
        let mut param_types = vec![];
        for (param_name, param_ty) in params {
            let pty = match param_ty {
                Some(t) => t.clone(),
                None => Type::Var(TypeVar::fresh()),
            };
            new_ctx.bind(param_name.clone(), pty.clone(), Capability::Ref);
            param_types.push(pty);
        }

        let (s, ret_ty) = self.infer_expr(&new_ctx, body)?;

        let param_ty = if param_types.len() == 1 {
            apply_subst(&param_types[0], &s)
        } else {
            Type::Tuple(param_types.iter().map(|t| apply_subst(t, &s)).collect())
        };

        let func_ty = Type::Function {
            param: Box::new(param_ty),
            ret: Box::new(apply_subst(&ret_ty, &s)),
            effect: effect.cloned().unwrap_or_else(EffectRow::empty),
            cap: Capability::Ref,
        };

        Ok((s, func_ty))
    }

    /// Infer the type of a function application.
    fn infer_app(
        &mut self,
        ctx: &TypeContext,
        func: &Expr,
        args: &[Expr],
        span: Span,
    ) -> NuResult<(Substitution, Type)> {
        let (s1, func_ty) = self.infer_expr(ctx, func)?;
        let mut subst = s1;
        let mut arg_types = vec![];

        // Infer each argument
        for arg in args {
            let ctx_sub = apply_subst_to_ctx(ctx, &subst);
            let (s_arg, arg_ty) = self.infer_expr(&ctx_sub, arg)?;
            subst = compose_subst(&s_arg, &subst);
            arg_types.push(apply_subst(&arg_ty, &subst));
        }

        // Create a fresh result type
        let result_ty = Type::Var(TypeVar::fresh());

        // Build expected function type
        let param_ty = if arg_types.len() == 1 {
            arg_types[0].clone()
        } else {
            Type::Tuple(arg_types)
        };

        // Preserve the function's effect row instead of forcing it to empty.
        // If the function type is not yet known, use a fresh open row so that
        // row-polymorphic functions can still unify.
        let func_ty_subst = apply_subst(&func_ty, &subst);
        let expected_effect = match &func_ty_subst {
            Type::Function { effect, .. } => effect.clone(),
            _ => EffectRow::Open(vec![], Region::fresh()),
        };

        let expected = Type::Function {
            param: Box::new(param_ty),
            ret: Box::new(result_ty.clone()),
            effect: expected_effect,
            cap: Capability::Ref,
        };

        // Unify
        let s2 = mgu(&func_ty_subst, &expected, span)?;
        let final_subst = compose_subst(&s2, &subst);

        Ok((final_subst.clone(), apply_subst(&result_ty, &final_subst)))
    }

    /// Infer the type of a let binding.
    fn infer_let(
        &mut self,
        ctx: &TypeContext,
        name: &str,
        value: &Expr,
        body: &Expr,
        _span: Span,
    ) -> NuResult<(Substitution, Type)> {
        // For let-bound lambdas that reference themselves (e.g.
        // `let fac = fn(n) ... fac(n-1) ... in ...`), make the binding name
        // available inside the lambda body with a fresh type variable.
        if matches!(value, Expr::Lambda { .. }) {
            let rec_var = Type::Var(TypeVar::fresh());
            let ctx_with_rec = ctx.extend(name.to_string(), rec_var.clone(), Capability::Ref);
            let (s1, val_ty) = self.infer_expr(&ctx_with_rec, value)?;
            let s2 = mgu(&apply_subst(&rec_var, &s1), &apply_subst(&val_ty, &s1), Span::default())?;
            let s_combined = compose_subst(&s2, &s1);
            let gen_ty = self.do_generalize(ctx, &apply_subst(&val_ty, &s_combined));
            let new_ctx = ctx.extend(name.to_string(), gen_ty, Capability::Ref);
            let (s3, body_ty) = self.infer_expr(&new_ctx, body)?;
            let final_subst = compose_subst(&s3, &s_combined);
            return Ok((final_subst.clone(), apply_subst(&body_ty, &final_subst)));
        }

        // Infer the binding value
        let (s1, val_ty) = self.infer_expr(ctx, value)?;

        // Generalize the value type
        let gen_ty = self.do_generalize(ctx, &apply_subst(&val_ty, &s1));

        // Extend context with generalized type
        let new_ctx = ctx.extend(name.to_string(), gen_ty, Capability::Ref);

        // Infer body with extended context
        let (s2, body_ty) = self.infer_expr(&new_ctx, body)?;

        let final_subst = compose_subst(&s2, &s1);
        Ok((final_subst.clone(), apply_subst(&body_ty, &final_subst)))
    }

    /// Infer the type of a recursive let binding.
    fn infer_letrec(
        &mut self,
        ctx: &TypeContext,
        name: &str,
        params: &[(String, Option<Type>)],
        value: &Expr,
        body: &Expr,
        _span: Span,
    ) -> NuResult<(Substitution, Type)> {
        // Create a fresh type variable for the recursive function
        let rec_var = Type::Var(TypeVar::fresh());
        let ctx_with_rec = ctx.extend(name.to_string(), rec_var.clone(), Capability::Ref);

        // Infer the value with the recursive binding in scope
        let mut new_ctx = ctx_with_rec.clone();
        let mut param_types = vec![];
        for (param_name, param_ty) in params {
            let pty = match param_ty {
                Some(t) => t.clone(),
                None => Type::Var(TypeVar::fresh()),
            };
            new_ctx.bind(param_name.clone(), pty.clone(), Capability::Ref);
            param_types.push(pty);
        }

        let (s1, val_ty) = self.infer_expr(&new_ctx, value)?;

        // Build the function type from the value
        let func_ty = match &val_ty {
            Type::Function { .. } => val_ty.clone(),
            _ => {
                let param_ty = if param_types.len() == 1 {
                    param_types[0].clone()
                } else {
                    Type::Tuple(param_types)
                };
                Type::Function {
                    param: Box::new(param_ty),
                    ret: Box::new(val_ty.clone()),
                    effect: EffectRow::empty(),
                    cap: Capability::Ref,
                }
            }
        };

        // Unify the recursive variable with the inferred function type
        let s2 = mgu(&apply_subst(&rec_var, &s1), &apply_subst(&func_ty, &s1), Span::default())?;
        let s_combined = compose_subst(&s2, &s1);

        // Generalize
        let gen_ty = self.do_generalize(ctx, &apply_subst(&func_ty, &s_combined));
        let final_ctx = ctx.extend(name.to_string(), gen_ty, Capability::Ref);

        // Infer body
        let (s3, body_ty) = self.infer_expr(&final_ctx, body)?;
        let final_subst = compose_subst(&s3, &s_combined);

        Ok((final_subst.clone(), apply_subst(&body_ty, &final_subst)))
    }

    /// Infer the type of an if expression.
    fn infer_if(
        &mut self,
        ctx: &TypeContext,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: Option<&Box<Expr>>,
        span: Span,
    ) -> NuResult<(Substitution, Type)> {
        // Infer condition - must be Bool
        let (s1, cond_ty) = self.infer_expr(ctx, cond)?;
        let s_cond = mgu(&apply_subst(&cond_ty, &s1), &Type::bool(), span)?;
        let s1 = compose_subst(&s_cond, &s1);

        // Infer then branch
        let ctx1 = apply_subst_to_ctx(ctx, &s1);
        let (s2, then_ty) = self.infer_expr(&ctx1, then_branch)?;
        let s2 = compose_subst(&s2, &s1);

        // Infer else branch or use Unit
        let (s3, else_ty) = match else_branch {
            Some(else_expr) => {
                let ctx2 = apply_subst_to_ctx(ctx, &s2);
                let (s3, else_ty) = self.infer_expr(&ctx2, else_expr)?;
                (compose_subst(&s3, &s2), else_ty)
            }
            None => (s2.clone(), Type::unit()),
        };

        // Unify then and else branches
        let s4 = mgu(
            &apply_subst(&then_ty, &s3),
            &apply_subst(&else_ty, &s3),
            span,
        )?;
        let final_subst = compose_subst(&s4, &s3);

        Ok((final_subst.clone(), apply_subst(&then_ty, &final_subst)))
    }

    /// Infer the type of a binary operator expression.
    fn infer_binary(
        &mut self,
        ctx: &TypeContext,
        op: BinOp,
        left: &Expr,
        right: &Expr,
        span: Span,
    ) -> NuResult<(Substitution, Type)> {
        use BinOp::*;

        match op {
            // Arithmetic: numeric -> numeric
            Add | Sub | Mul | Div | Mod => {
                let (s1, left_ty) = self.infer_expr(ctx, left)?;
                let ctx1 = apply_subst_to_ctx(ctx, &s1);
                let (s2, right_ty) = self.infer_expr(&ctx1, right)?;

                let num_var = Type::Var(TypeVar::fresh());
                let s3 = mgu(&apply_subst(&left_ty, &s1), &num_var, span)?;
                let s_combined = compose_subst(&s3, &compose_subst(&s2, &s1));

                let s4 = mgu(&apply_subst(&right_ty, &s_combined), &apply_subst(&num_var, &s_combined), span)?;
                let final_subst = compose_subst(&s4, &s_combined);

                Ok((final_subst.clone(), apply_subst(&num_var, &final_subst)))
            }

            // Comparison: comparable -> Bool
            Eq | Ne | Lt | Le | Gt | Ge => {
                let (s1, left_ty) = self.infer_expr(ctx, left)?;
                let ctx1 = apply_subst_to_ctx(ctx, &s1);
                let (s2, right_ty) = self.infer_expr(&ctx1, right)?;

                let combined = compose_subst(&s2, &s1);
                let s3 = mgu(&apply_subst(&right_ty, &combined), &apply_subst(&left_ty, &combined), span)?;
                let final_subst = compose_subst(&s3, &combined);

                Ok((final_subst, Type::bool()))
            }

            // Boolean: Bool -> Bool
            And | Or => {
                let (s1, left_ty) = self.infer_expr(ctx, left)?;
                let s_left = mgu(&left_ty, &Type::bool(), span)?;
                let s1 = compose_subst(&s_left, &s1);

                let ctx1 = apply_subst_to_ctx(ctx, &s1);
                let (s2, right_ty) = self.infer_expr(&ctx1, right)?;
                let combined = compose_subst(&s2, &s1);
                let s_right = mgu(&apply_subst(&right_ty, &combined), &Type::bool(), span)?;
                let final_subst = compose_subst(&s_right, &combined);

                Ok((final_subst, Type::bool()))
            }

            // Bitwise: Int -> Int
            BitAnd | BitOr | BitXor | Shl | Shr => {
                let (s1, left_ty) = self.infer_expr(ctx, left)?;
                let s_left = mgu(&left_ty, &Type::int(), span)?;
                let s1 = compose_subst(&s_left, &s1);

                let ctx1 = apply_subst_to_ctx(ctx, &s1);
                let (s2, right_ty) = self.infer_expr(&ctx1, right)?;
                let combined = compose_subst(&s2, &s1);
                let s_right = mgu(&apply_subst(&right_ty, &combined), &Type::int(), span)?;
                let final_subst = compose_subst(&s_right, &combined);

                Ok((final_subst, Type::int()))
            }

            // Assignment (should be handled in Assign expr, but here for completeness)
            Assign => {
                let (s1, left_ty) = self.infer_expr(ctx, left)?;
                let ctx1 = apply_subst_to_ctx(ctx, &s1);
                let (s2, right_ty) = self.infer_expr(&ctx1, right)?;
                let s3 = mgu(&apply_subst(&right_ty, &s2), &apply_subst(&left_ty, &s1), span)?;
                let final_subst = compose_subst(&s3, &compose_subst(&s2, &s1));
                Ok((final_subst, Type::unit()))
            }

            // Pipe (should be handled in Pipe expr)
            Pipe => {
                let (s1, left_ty) = self.infer_expr(ctx, left)?;
                let ctx1 = apply_subst_to_ctx(ctx, &s1);
                let (s2, right_ty) = self.infer_expr(&ctx1, right)?;
                let result_var = Type::Var(TypeVar::fresh());
                let expected = Type::Function {
                    param: Box::new(apply_subst(&left_ty, &compose_subst(&s2, &s1))),
                    ret: Box::new(result_var.clone()),
                    effect: EffectRow::empty(),
                    cap: Capability::Ref,
                };
                let s3 = mgu(&apply_subst(&right_ty, &s2), &expected, span)?;
                let final_subst = compose_subst(&s3, &compose_subst(&s2, &s1));
                Ok((final_subst.clone(), apply_subst(&result_var, &final_subst)))
            }
        }
    }

    /// Infer the type of a unary operator expression.
    fn infer_unary(
        &mut self,
        ctx: &TypeContext,
        op: UnOp,
        expr: &Expr,
        span: Span,
    ) -> NuResult<(Substitution, Type)> {
        use UnOp::*;

        let (s, ty) = self.infer_expr(ctx, expr)?;

        match op {
            // Negation: numeric -> numeric
            Neg => {
                let num_var = Type::Var(TypeVar::fresh());
                let s2 = mgu(&apply_subst(&ty, &s), &num_var, span)?;
                let final_subst = compose_subst(&s2, &s);
                Ok((final_subst.clone(), apply_subst(&num_var, &final_subst)))
            }
            // Boolean not: Bool -> Bool
            Not => {
                let s2 = mgu(&apply_subst(&ty, &s), &Type::bool(), span)?;
                let final_subst = compose_subst(&s2, &s);
                Ok((final_subst, Type::bool()))
            }
            // Dereference: &cap T -> T
            Deref => {
                let inner_var = Type::Var(TypeVar::fresh());
                let s2 = mgu(
                    &apply_subst(&ty, &s),
                    &Type::Reference {
                        cap: Capability::Ref,
                        inner: Box::new(inner_var.clone()),
                    },
                    span,
                )?;
                let final_subst = compose_subst(&s2, &s);
                Ok((final_subst.clone(), apply_subst(&inner_var, &final_subst)))
            }
            // Reference: T -> &cap T
            Ref(cap) => {
                let ref_ty = Type::Reference {
                    cap,
                    inner: Box::new(apply_subst(&ty, &s)),
                };
                Ok((s, ref_ty))
            }
        }
    }

    /// Infer the type of a tuple expression.
    fn infer_tuple(
        &mut self,
        ctx: &TypeContext,
        exprs: &[Expr],
        _span: Span,
    ) -> NuResult<(Substitution, Type)> {
        let mut subst = vec![];
        let mut types = vec![];
        for expr in exprs {
            let ctx_sub = apply_subst_to_ctx(ctx, &subst);
            let (s, ty) = self.infer_expr(&ctx_sub, expr)?;
            subst = compose_subst(&s, &subst);
            types.push(apply_subst(&ty, &subst));
        }
        Ok((subst, Type::Tuple(types)))
    }

    /// Infer the type of a record expression.
    fn infer_record(
        &mut self,
        ctx: &TypeContext,
        fields: &[(String, Expr)],
        _span: Span,
    ) -> NuResult<(Substitution, Type)> {
        let mut subst = vec![];
        let mut field_types = vec![];
        for (name, expr) in fields {
            let ctx_sub = apply_subst_to_ctx(ctx, &subst);
            let (s, ty) = self.infer_expr(&ctx_sub, expr)?;
            subst = compose_subst(&s, &subst);
            field_types.push((name.clone(), apply_subst(&ty, &subst)));
        }
        Ok((subst, Type::Record(field_types)))
    }

    /// Infer the type of a field access expression.
    fn infer_field_access(
        &mut self,
        ctx: &TypeContext,
        expr: &Expr,
        field: &str,
        span: Span,
    ) -> NuResult<(Substitution, Type)> {
        // Pipeline built-in namespace: Pipeline.new / Pipeline.stage.
        if let Expr::Var(base, _) = expr {
            if base == "Pipeline" {
                if field == "new" {
                    let func_ty = Type::Function {
                        param: Box::new(Type::Tuple(vec![])),
                        ret: Box::new(Type::int()),
                        effect: EffectRow::empty(),
                        cap: Capability::Ref,
                    };
                    return Ok((vec![], func_ty));
                }
                if field == "stage" {
                    let actor_ty = Type::Actor {
                        state: Box::new(Type::Var(TypeVar::fresh())),
                        behavior: Box::new(Type::Var(TypeVar::fresh())),
                    };
                    let func_ty = Type::Function {
                        param: Box::new(Type::Tuple(vec![
                            Type::int(),
                            Type::string(),
                            actor_ty,
                            Type::string(),
                        ])),
                        ret: Box::new(Type::int()),
                        effect: EffectRow::empty(),
                        cap: Capability::Ref,
                    };
                    return Ok((vec![], func_ty));
                }
            }
        }

        // Pipeline instance method: <pipeline-id>.run(input)
        if field == "run" {
            let (s1, receiver_ty) = self.infer_expr(ctx, expr)?;
            let s_receiver = mgu(&apply_subst(&receiver_ty, &s1), &Type::int(), span)?;
            let final_subst = compose_subst(&s_receiver, &s1);
            let func_ty = Type::Function {
                param: Box::new(Type::string()),
                ret: Box::new(Type::string()),
                effect: EffectRow::empty(),
                cap: Capability::Ref,
            };
            return Ok((final_subst, func_ty));
        }

        let (s1, record_ty) = self.infer_expr(ctx, expr)?;
        let record_ty_resolved = apply_subst(&record_ty, &s1);

        match record_ty_resolved {
            Type::Record(ref fields) => {
                if let Some((_, field_ty)) = fields.iter().find(|(name, _)| name == field) {
                    return Ok((s1, field_ty.clone()));
                }
                Err(NuError::TypeError {
                    msg: format!("Field '{}' not found in record type", field),
                    span,
                })
            }
            _ => {
                let field_var = Type::Var(TypeVar::fresh());
                let expected = Type::Record(vec![(field.to_string(), field_var.clone())]);
                let s2 = mgu(&record_ty_resolved, &expected, span)?;
                let final_subst = compose_subst(&s2, &s1);
                Ok((final_subst.clone(), apply_subst(&field_var, &final_subst)))
            }
        }
    }

    /// Infer the type of an array expression.
    fn infer_array(
        &mut self,
        ctx: &TypeContext,
        elems: &[Expr],
        _span: Span,
    ) -> NuResult<(Substitution, Type)> {
        if elems.is_empty() {
            let elem_var = Type::Var(TypeVar::fresh());
            return Ok((vec![], Type::Array(Box::new(elem_var))));
        }

        let mut subst = vec![];
        let (s1, first_ty) = self.infer_expr(ctx, &elems[0])?;
        subst = s1;

        for elem in &elems[1..] {
            let ctx_sub = apply_subst_to_ctx(ctx, &subst);
            let (s, ty) = self.infer_expr(&ctx_sub, elem)?;
            let s_unify = mgu(&apply_subst(&ty, &s), &apply_subst(&first_ty, &subst), Span::default())?;
            subst = compose_subst(&s_unify, &compose_subst(&s, &subst));
        }

        Ok((subst.clone(), Type::Array(Box::new(apply_subst(&first_ty, &subst)))))
    }

    /// Infer the type of an array index expression.
    fn infer_index(
        &mut self,
        ctx: &TypeContext,
        arr: &Expr,
        idx: &Expr,
        span: Span,
    ) -> NuResult<(Substitution, Type)> {
        let (s1, arr_ty) = self.infer_expr(ctx, arr)?;
        let ctx1 = apply_subst_to_ctx(ctx, &s1);
        let (s2, idx_ty) = self.infer_expr(&ctx1, idx)?;

        // Index must be Int
        let s_idx = mgu(&apply_subst(&idx_ty, &s2), &Type::int(), span)?;
        let s_combined = compose_subst(&s_idx, &compose_subst(&s2, &s1));

        // Array type
        let elem_var = Type::Var(TypeVar::fresh());
        let s_arr = mgu(
            &apply_subst(&arr_ty, &s_combined),
            &Type::Array(Box::new(elem_var.clone())),
            span,
        )?;
        let final_subst = compose_subst(&s_arr, &s_combined);

        Ok((final_subst.clone(), apply_subst(&elem_var, &final_subst)))
    }

    /// Infer the type of a pattern match expression.
    fn infer_match(
        &mut self,
        ctx: &TypeContext,
        scrutinee: &Expr,
        arms: &[(Pattern, Expr)],
        span: Span,
    ) -> NuResult<(Substitution, Type)> {
        // Infer scrutinee type
        let (s1, scrut_ty) = self.infer_expr(ctx, scrutinee)?;

        // Infer each arm
        let mut subst = s1;
        let mut arm_types = vec![];

        for (pattern, arm_expr) in arms {
            let ctx_sub = apply_subst_to_ctx(ctx, &subst);
            // Bind pattern variables to the context
            let pattern_ctx = self.bind_pattern(&ctx_sub, pattern, &apply_subst(&scrut_ty, &subst))?;
            let (s_arm, arm_ty) = self.infer_expr(&pattern_ctx, arm_expr)?;
            subst = compose_subst(&s_arm, &subst);
            arm_types.push(apply_subst(&arm_ty, &subst));
        }

        if arm_types.is_empty() {
            return Err(NuError::TypeError {
                msg: "Match expression with no arms".to_string(),
                span,
            });
        }

        // Unify all arm types
        let first_arm = arm_types[0].clone();
        let mut final_subst = subst;
        for arm_ty in &arm_types[1..] {
            let s = mgu(&apply_subst(arm_ty, &final_subst), &apply_subst(&first_arm, &final_subst), span)?;
            final_subst = compose_subst(&s, &final_subst);
        }

        Ok((final_subst.clone(), apply_subst(&first_arm, &final_subst)))
    }

    /// Bind pattern variables into a new context.
    fn bind_pattern(
        &mut self,
        ctx: &TypeContext,
        pattern: &Pattern,
        scrut_ty: &Type,
    ) -> NuResult<TypeContext> {
        match pattern {
            Pattern::Wild => Ok(ctx.clone()),
            Pattern::Var(name) => Ok(ctx.extend(name.clone(), scrut_ty.clone(), Capability::Ref)),
            Pattern::Lit(lit) => {
                let lit_ty = match lit {
                    Literal::Int(_) => Type::int(),
                    Literal::Float(_) => Type::float(),
                    Literal::String(_) => Type::string(),
                    Literal::Bool(_) => Type::bool(),
                    Literal::Nil => Type::unit(), // TODO: dedicated nil type
                    Literal::Unit => Type::unit(),
                };
                let _ = mgu(scrut_ty, &lit_ty, Span::default())?;
                Ok(ctx.clone())
            }
            Pattern::Tuple(pats) => {
                match scrut_ty {
                    Type::Tuple(tys) if tys.len() == pats.len() => {
                        let mut new_ctx = ctx.clone();
                        for (pat, ty) in pats.iter().zip(tys.iter()) {
                            new_ctx = self.bind_pattern(&new_ctx, pat, ty)?;
                        }
                        Ok(new_ctx)
                    }
                    _ => {
                        // Create fresh type vars for tuple elements
                        let mut new_ctx = ctx.clone();
                        for pat in pats {
                            let elem_ty = Type::Var(TypeVar::fresh());
                            new_ctx = self.bind_pattern(&new_ctx, pat, &elem_ty)?;
                        }
                        Ok(new_ctx)
                    }
                }
            }
            Pattern::Record(pats) => {
                match scrut_ty {
                    Type::Record(fields) => {
                        let mut new_ctx = ctx.clone();
                        let field_map: std::collections::HashMap<String, Type> = fields.iter()
                            .map(|(n, t)| (n.clone(), t.clone()))
                            .collect();
                        for (field_name, pat) in pats {
                            if let Some(ty) = field_map.get(field_name) {
                                new_ctx = self.bind_pattern(&new_ctx, pat, ty)?;
                            } else {
                                let fresh = Type::Var(TypeVar::fresh());
                                new_ctx = self.bind_pattern(&new_ctx, pat, &fresh)?;
                            }
                        }
                        Ok(new_ctx)
                    }
                    _ => {
                        let mut new_ctx = ctx.clone();
                        for (_, pat) in pats {
                            let fresh = Type::Var(TypeVar::fresh());
                            new_ctx = self.bind_pattern(&new_ctx, pat, &fresh)?;
                        }
                        Ok(new_ctx)
                    }
                }
            }
            Pattern::Variant(name, pat) => {
                match scrut_ty {
                    Type::Variant(variants) => {
                        let mut new_ctx = ctx.clone();
                        if let Some((_, Some(ty))) = variants.iter().find(|(n, _)| n == name) {
                            if let Some(p) = pat {
                                new_ctx = self.bind_pattern(&new_ctx, p, ty)?;
                            }
                        }
                        Ok(new_ctx)
                    }
                    _ => {
                        if let Some(p) = pat {
                            let fresh = Type::Var(TypeVar::fresh());
                            self.bind_pattern(ctx, p, &fresh)
                        } else {
                            Ok(ctx.clone())
                        }
                    }
                }
            }
            Pattern::Alias(name, pat) => {
                let mut new_ctx = ctx.extend(name.clone(), scrut_ty.clone(), Capability::Ref);
                new_ctx = self.bind_pattern(&new_ctx, pat, scrut_ty)?;
                Ok(new_ctx)
            }
        }
    }

    /// Infer the type of a block expression.
    fn infer_block(
        &mut self,
        ctx: &TypeContext,
        exprs: &[Expr],
        _span: Span,
    ) -> NuResult<(Substitution, Type)> {
        if exprs.is_empty() {
            return Ok((vec![], Type::unit()));
        }

        let mut subst = vec![];
        let mut last_ty = Type::unit();
        for expr in exprs {
            let ctx_sub = apply_subst_to_ctx(ctx, &subst);
            let (s, ty) = self.infer_expr(&ctx_sub, expr)?;
            subst = compose_subst(&s, &subst);
            last_ty = ty;
        }
        Ok((subst.clone(), apply_subst(&last_ty, &subst)))
    }

    /// Infer actor declaration.
    fn infer_actor_decl(
        &mut self,
        ctx: &TypeContext,
        _name: &str,
        behaviors: &[Behavior],
        _span: Span,
    ) -> NuResult<(Substitution, Type)> {
        // Check each behavior
        for behavior in behaviors {
            let mut behavior_ctx = ctx.clone();
            let mut param_types = vec![];
            for (param_name, param_ty) in &behavior.params {
                let pty = match param_ty {
                    Some(t) => t.clone(),
                    None => Type::Var(TypeVar::fresh()),
                };
                behavior_ctx.bind(param_name.clone(), pty.clone(), behavior.cap);
                param_types.push(pty);
            }
            let (_s, _body_ty) = self.infer_expr(&behavior_ctx, &behavior.body)?;
            // We could store behavior types here
        }

        let actor_ty = Type::Actor {
            state: Box::new(Type::Var(TypeVar::fresh())),
            behavior: Box::new(Type::Var(TypeVar::fresh())),
        };
        Ok((vec![], actor_ty))
    }

    /// Infer spawn expression.
    fn infer_spawn(
        &mut self,
        ctx: &TypeContext,
        actor_type: &Expr,
        span: Span,
    ) -> NuResult<(Substitution, Type)> {
        let (s, actor_ty) = self.infer_expr(ctx, actor_type)?;
        match &actor_ty {
            Type::Actor { .. } => Ok((s, actor_ty.clone())),
            _ => {
                // Try to unify with Actor type
                let fresh_actor = Type::Actor {
                    state: Box::new(Type::Var(TypeVar::fresh())),
                    behavior: Box::new(Type::Var(TypeVar::fresh())),
                };
                let s2 = mgu(&actor_ty, &fresh_actor, span)?;
                let final_subst = compose_subst(&s2, &s);
                Ok((final_subst, fresh_actor))
            }
        }
    }

    /// Infer send expression.
    fn infer_send(
        &mut self,
        ctx: &TypeContext,
        actor: &Expr,
        _behavior: &str,
        args: &[Expr],
        span: Span,
    ) -> NuResult<(Substitution, Type)> {
        let (s1, actor_ty) = self.infer_expr(ctx, actor)?;

        // Actor must be an actor type
        let actor_var = TypeVar::fresh();
        let fresh_actor = Type::Actor {
            state: Box::new(Type::Var(actor_var)),
            behavior: Box::new(Type::Var(TypeVar::fresh())),
        };
        let s2 = mgu(&apply_subst(&actor_ty, &s1), &fresh_actor, span)?;
        let s_combined = compose_subst(&s2, &s1);

        // Infer argument types
        let mut subst = s_combined;
        for arg in args {
            let ctx_sub = apply_subst_to_ctx(ctx, &subst);
            let (s_arg, _arg_ty) = self.infer_expr(&ctx_sub, arg)?;
            subst = compose_subst(&s_arg, &subst);
        }

        // Send returns Unit
        Ok((subst, Type::unit()))
    }

    /// Infer ask expression.
    fn infer_ask(&mut self, ctx: &TypeContext, actor: &Expr, span: Span) -> NuResult<(Substitution, Type)> {
        let (s1, actor_ty) = self.infer_expr(ctx, actor)?;

        let fresh_actor = Type::Actor {
            state: Box::new(Type::Var(TypeVar::fresh())),
            behavior: Box::new(Type::Var(TypeVar::fresh())),
        };
        let s2 = mgu(&actor_ty, &fresh_actor, span)?;
        let subst = compose_subst(&s2, &s1);

        // Ask returns a fresh type (the behavior's return type)
        let ret_var = Type::Var(TypeVar::fresh());
        Ok((subst, ret_var))
    }

    /// Infer perform expression.
    fn infer_perform(
        &mut self,
        ctx: &TypeContext,
        _effect: &str,
        args: &[Expr],
        _span: Span,
    ) -> NuResult<(Substitution, Type)> {
        let mut subst = vec![];
        for arg in args {
            let ctx_sub = apply_subst_to_ctx(ctx, &subst);
            let (s, _ty) = self.infer_expr(&ctx_sub, arg)?;
            subst = compose_subst(&s, &subst);
        }
        // Perform returns a fresh type variable
        let ret_var = Type::Var(TypeVar::fresh());
        Ok((subst, ret_var))
    }

    /// Infer handle expression.
    fn infer_handle(
        &mut self,
        ctx: &TypeContext,
        body: &Expr,
        handlers: &[EffectHandler],
        _span: Span,
    ) -> NuResult<(Substitution, Type)> {
        // Infer body type
        let (mut subst, body_ty) = self.infer_expr(ctx, body)?;

        // Each handler body must produce a value compatible with the body's type.
        for h in handlers {
            let mut handler_ctx = apply_subst_to_ctx(ctx, &subst);
            for p in &h.params {
                handler_ctx.bind(p.clone(), Type::Var(TypeVar::fresh()), Capability::Ref);
            }
            let (s, handler_ty) = self.infer_expr(&handler_ctx, &h.body)?;
            let handler_ty_subst = apply_subst(&handler_ty, &s);
            let body_ty_subst = apply_subst(&body_ty, &compose_subst(&s, &subst));
            let s_unify = mgu(&handler_ty_subst, &body_ty_subst, Span::default())?;
            subst = compose_subst(&s_unify, &compose_subst(&s, &subst));
        }

        Ok((subst.clone(), apply_subst(&body_ty, &subst)))
    }

    /// Infer for comprehension.
    fn infer_for(
        &mut self,
        ctx: &TypeContext,
        var: &str,
        iterable: &Expr,
        body: &Expr,
        span: Span,
    ) -> NuResult<(Substitution, Type)> {
        // Infer iterable type (should be array-like)
        let (s1, iter_ty) = self.infer_expr(ctx, iterable)?;

        let elem_var = Type::Var(TypeVar::fresh());
        let s2 = mgu(
            &apply_subst(&iter_ty, &s1),
            &Type::Array(Box::new(elem_var.clone())),
            span,
        )?;
        let s_combined = compose_subst(&s2, &s1);

        // Bind the loop variable
        let body_ctx = ctx.extend(var.to_string(), apply_subst(&elem_var, &s_combined), Capability::Ref);

        // Infer body
        let (s3, _body_ty) = self.infer_expr(&body_ctx, body)?;
        let final_subst = compose_subst(&s3, &s_combined);

        // For returns Unit
        Ok((final_subst, Type::unit()))
    }

    // -----------------------------------------------------------------------
    // Generalization with tracked context vars
    // -----------------------------------------------------------------------

    /// Validate that a type is usable as an FFI parameter/return type in the MVP.
    /// Only primitive Int, Float, Bool, String, and Unit are supported.
    fn validate_ffi_type(&self, ty: &Type, span: Span) -> NuResult<()> {
        match ty {
            Type::Primitive(PrimitiveType::Int)
            | Type::Primitive(PrimitiveType::Float)
            | Type::Primitive(PrimitiveType::Bool)
            | Type::Primitive(PrimitiveType::String)
            | Type::Primitive(PrimitiveType::Unit) => Ok(()),
            _ => Err(NuError::TypeError {
                msg: format!(
                    "Unsupported FFI type: {:?}. Only Int, Float, Bool, String, and Unit are allowed in this MVP.",
                    ty
                ),
                span,
            }),
        }
    }

    /// Generalize a type by abstracting over free variables not in the context.
    fn do_generalize(&self, ctx: &TypeContext, ty: &Type) -> Type {
        let ty_fv: HashSet<TypeVar> = ty.free_vars().into_iter().collect();
        let ctx_fv = self.get_ctx_free_vars(ctx);
        let gen_vars: Vec<TypeVar> = ty_fv.difference(&ctx_fv).copied().collect();

        if gen_vars.is_empty() {
            ty.clone()
        } else {
            Type::Scheme {
                vars: gen_vars,
                body: Box::new(ty.clone()),
            }
        }
    }

    /// Get free type variables from the context.
    fn get_ctx_free_vars(&self, _ctx: &TypeContext) -> HashSet<TypeVar> {
        // Combine tracked context vars with any vars we can discover
        let vars = self.ctx_free_vars.clone();
        // We can't iterate TypeContext directly, so we rely on the tracked set
        // The tracked set is updated when we add bindings
        vars
    }
}

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to create a span
    fn sp() -> Span {
        Span::new(0, 0, 1, 1)
    }

    // Helper to create an int literal expression
    fn int_lit(n: i64) -> Expr {
        Expr::Literal(Literal::Int(n), sp())
    }

    // Helper to create a bool literal
    fn bool_lit(b: bool) -> Expr {
        Expr::Literal(Literal::Bool(b), sp())
    }

    // Helper to create a string literal
    fn string_lit(s: &str) -> Expr {
        Expr::Literal(Literal::String(s.to_string()), sp())
    }

    // Helper to create a variable expression
    fn var(name: &str) -> Expr {
        Expr::Var(name.to_string(), sp())
    }

    // Helper to create a lambda
    fn lambda(param: &str, body: Expr) -> Expr {
        Expr::Lambda {
            params: vec![(param.to_string(), None)],
            body: Box::new(body),
            effect: None,
            span: sp(),
        }
    }

    // Helper to create application
    fn app(func: Expr, arg: Expr) -> Expr {
        Expr::App {
            func: Box::new(func),
            args: vec![arg],
            span: sp(),
        }
    }

    // Helper for let binding
    fn let_(name: &str, value: Expr, body: Expr) -> Expr {
        Expr::Let {
            name: name.to_string(),
            value: Box::new(value),
            body: Box::new(body),
            span: sp(),
        }
    }

    // Helper for if
    fn if_(cond: Expr, then_: Expr, else_: Option<Expr>) -> Expr {
        Expr::If {
            cond: Box::new(cond),
            then_branch: Box::new(then_),
            else_branch: else_.map(Box::new),
            span: sp(),
        }
    }

    // Helper for binary op
    fn bin(op: BinOp, left: Expr, right: Expr) -> Expr {
        Expr::Binary {
            op,
            left: Box::new(left),
            right: Box::new(right),
            span: sp(),
        }
    }

    // Helper for tuple
    fn tuple(exprs: Vec<Expr>) -> Expr {
        Expr::Tuple(exprs, sp())
    }

    // Helper for record
    fn record(fields: Vec<(&str, Expr)>) -> Expr {
        Expr::Record(
            fields.into_iter().map(|(n, e)| (n.to_string(), e)).collect(),
            sp(),
        )
    }

    // Helper for field access
    fn field(expr: Expr, name: &str) -> Expr {
        Expr::FieldAccess {
            expr: Box::new(expr),
            field: name.to_string(),
            span: sp(),
        }
    }

    // Helper to set up context with a typed binding
    fn ctx_with(name: &str, ty: Type) -> TypeContext {
        let mut ctx = TypeContext::new();
        ctx.bind(name.to_string(), ty, Capability::Ref);
        ctx
    }

    // -----------------------------------------------------------------------
    // Test: Literals
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_int_literal() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        let (s, ty) = tc.infer_expr(&ctx, &int_lit(42)).unwrap();
        assert!(s.is_empty());
        assert_eq!(ty, Type::int());
    }

    #[test]
    fn test_infer_bool_literal() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        let (s, ty) = tc.infer_expr(&ctx, &bool_lit(true)).unwrap();
        assert!(s.is_empty());
        assert_eq!(ty, Type::bool());
    }

    #[test]
    fn test_infer_string_literal() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        let (s, ty) = tc.infer_expr(&ctx, &string_lit("hello")).unwrap();
        assert!(s.is_empty());
        assert_eq!(ty, Type::string());
    }

    #[test]
    fn test_infer_float_literal() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        let expr = Expr::Literal(Literal::Float(3.14), sp());
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert!(s.is_empty());
        assert_eq!(ty, Type::float());
    }

    #[test]
    fn test_infer_unit_literal() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        let expr = Expr::Literal(Literal::Unit, sp());
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert!(s.is_empty());
        assert_eq!(ty, Type::unit());
    }

    // -----------------------------------------------------------------------
    // Test: Variables
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_bound_variable() {
        let mut tc = TypeChecker::new();
        let ctx = ctx_with("x", Type::int());
        let (s, ty) = tc.infer_expr(&ctx, &var("x")).unwrap();
        assert!(s.is_empty());
        assert_eq!(ty, Type::int());
    }

    #[test]
    fn test_infer_unbound_variable() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        let result = tc.infer_expr(&ctx, &var("undefined"));
        assert!(result.is_err());
    }

    #[test]
    fn test_infer_polymorphic_variable() {
        let mut tc = TypeChecker::new();
        // Bind 'id' as a polymorphic scheme: forall a. a -> a
        let a = TypeVar(100);
        let scheme = Type::Scheme {
            vars: vec![a],
            body: Box::new(Type::Function {
                param: Box::new(Type::Var(a)),
                ret: Box::new(Type::Var(a)),
                effect: EffectRow::empty(),
                cap: Capability::Ref,
            }),
        };
        let ctx = ctx_with("id", scheme);
        let (s, ty) = tc.infer_expr(&ctx, &var("id")).unwrap();
        assert!(s.is_empty());
        // Should be instantiated to a fresh function type
        match ty {
            Type::Function { param, ret, .. } => {
                // param and ret should be the same fresh variable
                assert_eq!(*param, *ret);
            }
            _ => panic!("Expected function type, got {:?}", ty),
        }
    }

    // -----------------------------------------------------------------------
    // Test: Lambda
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_identity_lambda() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        let expr = lambda("x", var("x"));
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert!(s.is_empty());
        match ty {
            Type::Function { param, ret, .. } => {
                assert_eq!(*param, *ret);
            }
            _ => panic!("Expected function type, got {:?}", ty),
        }
    }

    #[test]
    fn test_infer_const_lambda() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        let expr = lambda("x", int_lit(42));
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert!(s.is_empty());
        match ty {
            Type::Function { param: _, ret, .. } => {
                assert_eq!(*ret, Type::int());
            }
            _ => panic!("Expected function type, got {:?}", ty),
        }
    }

    // -----------------------------------------------------------------------
    // Test: Application
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_app_identity() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // (fn x => x)(42)
        let expr = app(lambda("x", var("x")), int_lit(42));
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        // Should infer Int (applying identity to 42)
        assert_eq!(apply_subst(&ty, &s), Type::int());
    }

    #[test]
    fn test_infer_app_const() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // (fn x => 42)("hello")
        let expr = app(lambda("x", int_lit(42)), string_lit("hello"));
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        // Should infer Int (const function ignores its argument)
        assert_eq!(apply_subst(&ty, &s), Type::int());
    }

    // -----------------------------------------------------------------------
    // Test: Let bindings
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_simple_let() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // let x = 42 in x
        let expr = let_("x", int_lit(42), var("x"));
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert_eq!(apply_subst(&ty, &s), Type::int());
    }

    #[test]
    fn test_infer_let_with_usage() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // let x = 42 in x + 1
        let expr = let_(
            "x",
            int_lit(42),
            bin(BinOp::Add, var("x"), int_lit(1)),
        );
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert_eq!(apply_subst(&ty, &s), Type::int());
    }

    #[test]
    fn test_infer_let_polymorphism() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // let id = fn x => x in (id(42), id(true))
        // This tests that 'id' is polymorphic
        let id = lambda("x", var("x"));
        let body = tuple(vec![app(var("id"), int_lit(42)), app(var("id"), bool_lit(true))]);
        let expr = let_("id", id, body);
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        match apply_subst(&ty, &s) {
            Type::Tuple(ts) if ts.len() == 2 => {
                assert_eq!(ts[0], Type::int());
                assert_eq!(ts[1], Type::bool());
            }
            other => panic!("Expected Tuple[Int, Bool], got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test: If expressions
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_if_then_else() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // if true then 42 else 0
        let expr = if_(bool_lit(true), int_lit(42), Some(int_lit(0)));
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert_eq!(apply_subst(&ty, &s), Type::int());
    }

    #[test]
    fn test_infer_if_with_condition_error() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // if 42 then 0 else 1 (condition must be bool)
        let expr = if_(int_lit(42), int_lit(0), Some(int_lit(1)));
        let result = tc.infer_expr(&ctx, &expr);
        assert!(result.is_err());
    }

    #[test]
    fn test_infer_if_branch_mismatch() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // if true then 42 else "hello" (branch mismatch)
        let expr = if_(bool_lit(true), int_lit(42), Some(string_lit("hello")));
        let result = tc.infer_expr(&ctx, &expr);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Test: Binary operators
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_binop_arithmetic() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // 1 + 2
        let expr = bin(BinOp::Add, int_lit(1), int_lit(2));
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert_eq!(apply_subst(&ty, &s), Type::int());
    }

    #[test]
    fn test_infer_binop_comparison() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // 1 < 2
        let expr = bin(BinOp::Lt, int_lit(1), int_lit(2));
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert_eq!(apply_subst(&ty, &s), Type::bool());
    }

    #[test]
    fn test_infer_binop_boolean() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // true && false
        let expr = bin(BinOp::And, bool_lit(true), bool_lit(false));
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert_eq!(apply_subst(&ty, &s), Type::bool());
    }

    #[test]
    fn test_infer_binop_bitwise() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // 1 & 2
        let expr = bin(BinOp::BitAnd, int_lit(1), int_lit(2));
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert_eq!(apply_subst(&ty, &s), Type::int());
    }

    #[test]
    fn test_infer_binop_boolean_error() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // 1 && 2 (must be bool)
        let expr = bin(BinOp::And, int_lit(1), int_lit(2));
        let result = tc.infer_expr(&ctx, &expr);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Test: Unary operators
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_unary_neg() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // -42
        let expr = Expr::Unary {
            op: UnOp::Neg,
            expr: Box::new(int_lit(42)),
            span: sp(),
        };
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        // Negation on Int should give Int
        assert_eq!(apply_subst(&ty, &s), Type::int());
    }

    #[test]
    fn test_infer_unary_not() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // !true
        let expr = Expr::Unary {
            op: UnOp::Not,
            expr: Box::new(bool_lit(true)),
            span: sp(),
        };
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert_eq!(apply_subst(&ty, &s), Type::bool());
    }

    // -----------------------------------------------------------------------
    // Test: Tuples
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_tuple() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // (42, true, "hello")
        let expr = tuple(vec![int_lit(42), bool_lit(true), string_lit("hello")]);
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        match apply_subst(&ty, &s) {
            Type::Tuple(ts) => {
                assert_eq!(ts.len(), 3);
                assert_eq!(ts[0], Type::int());
                assert_eq!(ts[1], Type::bool());
                assert_eq!(ts[2], Type::string());
            }
            other => panic!("Expected tuple, got {:?}", other),
        }
    }

    #[test]
    fn test_infer_empty_tuple() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        let expr = tuple(vec![]);
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        match apply_subst(&ty, &s) {
            Type::Tuple(ts) => assert!(ts.is_empty()),
            other => panic!("Expected empty tuple, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test: Records
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_record() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // { x: 42, y: true }
        let expr = record(vec![("x", int_lit(42)), ("y", bool_lit(true))]);
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        match apply_subst(&ty, &s) {
            Type::Record(fields) => {
                assert_eq!(fields.len(), 2);
                // Fields may be in any order
                let field_map: std::collections::HashMap<String, Type> =
                    fields.into_iter().collect();
                assert_eq!(field_map.get("x"), Some(&Type::int()));
                assert_eq!(field_map.get("y"), Some(&Type::bool()));
            }
            other => panic!("Expected record, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test: Field access
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_field_access() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // { x: 42, y: true }.x
        let rec = record(vec![("x", int_lit(42)), ("y", bool_lit(true))]);
        let expr = field(rec, "x");
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert_eq!(apply_subst(&ty, &s), Type::int());
    }

    // -----------------------------------------------------------------------
    // Test: Recursive functions (let rec)
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_letrec() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // let rec fact n = if n == 0 then 1 else n * fact(n - 1) in fact(5)
        // (simplified: let rec f x = x in f(42))
        let body = var("x");
        let rec_expr = Expr::LetRec {
            name: "f".to_string(),
            params: vec![("x".to_string(), None)],
            value: Box::new(body),
            body: Box::new(app(var("f"), int_lit(42))),
            span: sp(),
        };
        let (s, ty) = tc.infer_expr(&ctx, &rec_expr).unwrap();
        assert_eq!(apply_subst(&ty, &s), Type::int());
    }

    // -----------------------------------------------------------------------
    // Test: Block
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_block() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // { 1; 2; 3 }
        let expr = Expr::Block {
            exprs: vec![int_lit(1), int_lit(2), int_lit(3)],
            span: sp(),
        };
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert_eq!(apply_subst(&ty, &s), Type::int());
    }

    // -----------------------------------------------------------------------
    // Test: Array
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_array() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // [1, 2, 3]
        let expr = Expr::Array(vec![int_lit(1), int_lit(2), int_lit(3)], sp());
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        match apply_subst(&ty, &s) {
            Type::Array(elem_ty) => {
                assert_eq!(*elem_ty, Type::int());
            }
            other => panic!("Expected array, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test: Pattern matching
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_match_wildcard() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // match 42 { | _ => 0 }
        let expr = Expr::Match {
            scrutinee: Box::new(int_lit(42)),
            arms: vec![(Pattern::Wild, int_lit(0))],
            span: sp(),
        };
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert_eq!(apply_subst(&ty, &s), Type::int());
    }

    #[test]
    fn test_infer_match_variable() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // match 42 { | x => x }
        let expr = Expr::Match {
            scrutinee: Box::new(int_lit(42)),
            arms: vec![(Pattern::Var("x".to_string()), var("x"))],
            span: sp(),
        };
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert_eq!(apply_subst(&ty, &s), Type::int());
    }

    // -----------------------------------------------------------------------
    // Test: Pipe operator
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_pipe() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // 42 |> (fn x => x)
        let expr = Expr::Pipe {
            left: Box::new(int_lit(42)),
            right: Box::new(lambda("x", var("x"))),
            span: sp(),
        };
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert_eq!(apply_subst(&ty, &s), Type::int());
    }

    // -----------------------------------------------------------------------
    // Test: Type annotation
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_type_annotate() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // 42 : Int
        let expr = Expr::TypeAnnotate {
            expr: Box::new(int_lit(42)),
            ty: Type::int(),
            span: sp(),
        };
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert_eq!(apply_subst(&ty, &s), Type::int());
    }

    #[test]
    fn test_infer_type_annotate_error() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // 42 : Bool (wrong annotation)
        let expr = Expr::TypeAnnotate {
            expr: Box::new(int_lit(42)),
            ty: Type::bool(),
            span: sp(),
        };
        let result = tc.infer_expr(&ctx, &expr);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Test: Polymorphism
    // -----------------------------------------------------------------------

    #[test]
    fn test_polymorphism_twice() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // let f = fn x => x in let a = f(1) in let b = f(true) in (a, b)
        let f = lambda("x", var("x"));
        let inner = let_(
            "b",
            app(var("f"), bool_lit(true)),
            tuple(vec![var("a"), var("b")]),
        );
        let middle = let_("a", app(var("f"), int_lit(1)), inner);
        let expr = let_("f", f, middle);
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        match apply_subst(&ty, &s) {
            Type::Tuple(ts) => {
                assert_eq!(ts[0], Type::int());
                assert_eq!(ts[1], Type::bool());
            }
            other => panic!("Expected Tuple[Int, Bool], got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test: Substitution operations
    // -----------------------------------------------------------------------

    #[test]
    fn test_compose_subst() {
        let v1 = TypeVar(1);
        let v2 = TypeVar(2);
        let s1 = vec![(v1, Type::int())];
        let s2 = vec![(v2, Type::Var(v1))];
        let composed = compose_subst(&s2, &s1);
        // Applying composed to v2 should give Int (v1 -> Int, then v2 -> v1)
        let ty = apply_subst(&Type::Var(v2), &composed);
        assert_eq!(ty, Type::int());
    }

    #[test]
    fn test_mgu_same_type() {
        let t1 = Type::int();
        let t2 = Type::int();
        let s = mgu(&t1, &t2, sp()).unwrap();
        assert!(s.is_empty());
    }

    #[test]
    fn test_mgu_var_type() {
        let v = TypeVar(1);
        let t = Type::int();
        let s = mgu(&Type::Var(v), &t, sp()).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].0, v);
        assert_eq!(s[0].1, Type::int());
    }

    #[test]
    fn test_mgu_function() {
        let v1 = TypeVar(1);
        let v2 = TypeVar(2);
        let f1 = Type::Function {
            param: Box::new(Type::Var(v1)),
            ret: Box::new(Type::Var(v1)),
            effect: EffectRow::empty(),
            cap: Capability::Ref,
        };
        let f2 = Type::Function {
            param: Box::new(Type::int()),
            ret: Box::new(Type::Var(v2)),
            effect: EffectRow::empty(),
            cap: Capability::Ref,
        };
        let s = mgu(&f1, &f2, sp()).unwrap();
        let result = apply_subst(&Type::Var(v2), &s);
        assert_eq!(result, Type::int());
    }

    #[test]
    fn test_occurs_check() {
        let v = TypeVar(1);
        let t = Type::Function {
            param: Box::new(Type::Var(v)),
            ret: Box::new(Type::Var(v)),
            effect: EffectRow::empty(),
            cap: Capability::Ref,
        };
        let result = mgu(&Type::Var(v), &t, sp());
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Test: Module checking
    // -----------------------------------------------------------------------

    #[test]
    fn test_check_empty_module() {
        let mut tc = TypeChecker::new();
        let module = AstModule {
            name: "test".to_string(),
            decls: vec![],
        };
        let ty = tc.check_module(&module).unwrap();
        assert_eq!(ty, Type::unit());
    }

    #[test]
    fn test_check_module_with_function() {
        let mut tc = TypeChecker::new();
        let module = AstModule {
            name: "test".to_string(),
            decls: vec![Decl::Function {
                name: "add1".to_string(),
                type_params: vec![],
                params: vec![("x".to_string(), Some(Type::int()))],
                ret_type: Some(Type::int()),
                effect: None,
                cap: None,
                body: bin(BinOp::Add, var("x"), int_lit(1)),
                annotations: vec![],
                public: true,
                span: sp(),
            }],
        };
        let ty = tc.check_module(&module).unwrap();
        match ty {
            Type::Function { param, ret, .. } => {
                assert_eq!(*param, Type::int());
                assert_eq!(*ret, Type::int());
            }
            other => panic!("Expected function type, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test: Generalization and Instantiation
    // -----------------------------------------------------------------------

    #[test]
    fn test_instantiate_scheme() {
        let a = TypeVar(100);
        let scheme = Type::Scheme {
            vars: vec![a],
            body: Box::new(Type::Function {
                param: Box::new(Type::Var(a)),
                ret: Box::new(Type::Var(a)),
                effect: EffectRow::empty(),
                cap: Capability::Ref,
            }),
        };
        let instantiated = instantiate(&scheme);
        match instantiated {
            Type::Function { param, ret, .. } => {
                // After instantiation, param and ret should be equal fresh vars
                assert_eq!(*param, *ret);
                // And different from the original
                assert_ne!(*param, Type::Var(a));
            }
            _ => panic!("Expected function type"),
        }
    }

    // -----------------------------------------------------------------------
    // Test: Reference types
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_ref() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        // ref(42)
        let expr = Expr::Unary {
            op: UnOp::Ref(Capability::Ref),
            expr: Box::new(int_lit(42)),
            span: sp(),
        };
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        match apply_subst(&ty, &s) {
            Type::Reference { cap, inner } => {
                assert_eq!(cap, Capability::Ref);
                assert_eq!(*inner, Type::int());
            }
            other => panic!("Expected reference type, got {:?}", other),
        }
    }

    #[test]
    fn test_infer_deref() {
        let mut tc = TypeChecker::new();
        let ctx = ctx_with(
            "x",
            Type::Reference {
                cap: Capability::Ref,
                inner: Box::new(Type::int()),
            },
        );
        let expr = Expr::Unary {
            op: UnOp::Deref,
            expr: Box::new(var("x")),
            span: sp(),
        };
        let (s, ty) = tc.infer_expr(&ctx, &expr).unwrap();
        assert_eq!(apply_subst(&ty, &s), Type::int());
    }

    // -----------------------------------------------------------------------
    // Test: Effect row handling in application and declarations
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_app_preserves_lambda_effect() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        let lam = Expr::Lambda {
            params: vec![("x".to_string(), Some(Type::int()))],
            body: Box::new(var("x")),
            effect: Some(EffectRow::Closed(vec![Effect::IO])),
            span: sp(),
        };
        let app = Expr::App {
            func: Box::new(lam),
            args: vec![int_lit(1)],
            span: sp(),
        };
        let (s, ty) = tc.infer_expr(&ctx, &app).unwrap();
        assert_eq!(apply_subst(&ty, &s), Type::int());
    }

    #[test]
    fn test_infer_function_decl_with_effect() {
        let mut tc = TypeChecker::new();
        let module = AstModule {
            name: "test".to_string(),
            decls: vec![Decl::Function {
                name: "io_fn".to_string(),
                type_params: vec![],
                params: vec![("x".to_string(), Some(Type::int()))],
                ret_type: Some(Type::int()),
                effect: Some(EffectRow::Closed(vec![Effect::IO])),
                cap: None,
                body: bin(BinOp::Add, var("x"), int_lit(1)),
                annotations: vec![],
                public: true,
                span: sp(),
            }],
        };
        let ty = tc.check_module(&module).unwrap();
        match ty {
            Type::Function { effect, .. } => {
                assert!(effect.contains(&Effect::IO));
            }
            other => panic!("Expected function type, got {:?}", other),
        }
    }

    #[test]
    fn test_infer_handle_checks_handler_body() {
        let mut tc = TypeChecker::new();
        let ctx = TypeContext::new();
        let handle_ok = Expr::Handle {
            body: Box::new(int_lit(42)),
            handlers: vec![EffectHandler {
                effect_name: "IO".to_string(),
                op_name: "print".to_string(),
                params: vec!["msg".to_string()],
                body: int_lit(0),
                resume: false,
            }],
            span: sp(),
        };
        let (s, ty) = tc.infer_expr(&ctx, &handle_ok).unwrap();
        assert_eq!(apply_subst(&ty, &s), Type::int());

        let handle_bad = Expr::Handle {
            body: Box::new(int_lit(42)),
            handlers: vec![EffectHandler {
                effect_name: "IO".to_string(),
                op_name: "print".to_string(),
                params: vec!["msg".to_string()],
                body: string_lit("oops"),
                resume: false,
            }],
            span: sp(),
        };
        assert!(tc.infer_expr(&ctx, &handle_bad).is_err());
    }

    #[test]
    fn test_extern_function_available_with_ffi_effect() {
        let module = AstModule {
            name: "main".to_string(),
            decls: vec![
                Decl::Extern {
                    library: "libm.so.6".to_string(),
                    funcs: vec![ExternFunc {
                        name: "sqrt".to_string(),
                        params: vec![("x".to_string(), Type::float())],
                        ret: Type::float(),
                        span: sp(),
                    }],
                    span: sp(),
                },
                Decl::Function {
                    name: "use_sqrt".to_string(),
                    type_params: vec![],
                    params: vec![],
                    ret_type: None,
                    effect: None,
                    cap: None,
                    body: Expr::App {
                        func: Box::new(Expr::Var("sqrt".to_string(), sp())),
                        args: vec![Expr::Literal(Literal::Float(4.0), sp())],
                        span: sp(),
                    },
                    annotations: vec![],
                    public: false,
                    span: sp(),
                },
            ],
        };
        let mut tc = TypeChecker::new();
        let ty = tc.check_module(&module).unwrap();
        // use_sqrt is a parameterless function returning Float.
        match ty {
            Type::Function { param, ret, .. } => {
                assert_eq!(*param, Type::Tuple(vec![]));
                assert_eq!(*ret, Type::float());
            }
            other => panic!("Expected function type, got {:?}", other),
        }
    }

    #[test]
    fn test_extern_function_type_has_ffi_effect() {
        let mut tc = TypeChecker::new();
        let mut ctx = TypeContext::new();
        let extern_ty = Type::Function {
            param: Box::new(Type::float()),
            ret: Box::new(Type::float()),
            effect: EffectRow::singleton(Effect::FFI),
            cap: Capability::Ref,
        };
        ctx.bind("sqrt", extern_ty, Capability::Ref);
        let (_s, ty) = tc.infer_expr(&ctx, &var("sqrt")).unwrap();
        match ty {
            Type::Function { effect, .. } => {
                assert!(effect.contains(&Effect::FFI));
            }
            other => panic!("Expected function type, got {:?}", other),
        }
    }

    #[test]
    fn test_extern_unsupported_param_type_errors() {
        let module = AstModule {
            name: "main".to_string(),
            decls: vec![Decl::Extern {
                library: "lib".to_string(),
                funcs: vec![ExternFunc {
                    name: "bad".to_string(),
                    params: vec![("x".to_string(), Type::Array(Box::new(Type::int())))],
                    ret: Type::int(),
                    span: sp(),
                }],
                span: sp(),
            }],
        };
        let mut tc = TypeChecker::new();
        let result = tc.check_module(&module);
        assert!(result.is_err());
        match result.unwrap_err() {
            NuError::TypeError { msg, .. } => {
                assert!(msg.contains("Unsupported FFI type"));
                assert!(msg.contains("Array"));
            }
            other => panic!("Expected TypeError, got {:?}", other),
        }
    }

    #[test]
    fn test_extern_unsupported_return_type_errors() {
        let module = AstModule {
            name: "main".to_string(),
            decls: vec![Decl::Extern {
                library: "lib".to_string(),
                funcs: vec![ExternFunc {
                    name: "bad".to_string(),
                    params: vec![("x".to_string(), Type::int())],
                    ret: Type::Record(vec![("a".to_string(), Type::int())]),
                    span: sp(),
                }],
                span: sp(),
            }],
        };
        let mut tc = TypeChecker::new();
        let result = tc.check_module(&module);
        assert!(result.is_err());
        match result.unwrap_err() {
            NuError::TypeError { msg, .. } => {
                assert!(msg.contains("Unsupported FFI type"));
            }
            other => panic!("Expected TypeError, got {:?}", other),
        }
    }
}
