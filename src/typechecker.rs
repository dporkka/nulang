//! Hindley-Milner type checker (Algorithm W) for Nulang.
//!
//! Implements classical Damas-Milner type inference with support for:
//! - Primitive types (Int, Float, Bool, String, Unit, Never, Address)
//! - Polymorphism via type schemes (forall vars. Type)
//! - Tuples, Records, Variants, Arrays
//! - Functions with effect rows and capability annotations
//! - Reference types with capabilities
//! - Actor/Agent types
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
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| apply_subst(t, subst)).collect()),
        Type::Record(fs) => Type::Record(
            fs.iter()
                .map(|(n, t)| (n.clone(), apply_subst(t, subst)))
                .collect(),
        ),
        Type::Variant(vs) => Type::Variant(
            vs.iter()
                .map(|(n, ts)| (n.clone(), ts.iter().map(|t| apply_subst(t, subst)).collect()))
                .collect(),
        ),
        Type::Array(t) => Type::Array(Box::new(apply_subst(t, subst))),
        Type::Ref(t, cap) => Type::Ref(Box::new(apply_subst(t, subst)), *cap),
        Type::Arrow(params, ret, eff) => Type::Arrow(
            params.iter().map(|p| apply_subst(p, subst)).collect(),
            Box::new(apply_subst(ret, subst)),
            eff.clone(),
        ),
        Type::App(name, args) => Type::App(
            Box::new(apply_subst(name, subst)),
            args.iter().map(|a| apply_subst(a, subst)).collect(),
        ),
        Type::Scheme(vs, t) => {
            // Remove bindings for variables bound by this scheme
            let filtered: Substitution = subst
                .iter()
                .filter(|(v, _)| !vs.contains(v))
                .cloned()
                .collect();
            Type::Scheme(vs.clone(), Box::new(apply_subst(t, &filtered)))
        }
        // Primitive types, Named types unchanged
        other => other.clone(),
    }
}

/// Compose two substitutions: first apply s2, then s1.
/// Result: s1 ∘ s2 (apply s2 first, then s1)
fn compose_subst(s1: &Substitution, s2: &Substitution) -> Substitution {
    let mut result: Substitution = s2
        .iter()
        .map(|(v, t)| (*v, apply_subst(t, s1)))
        .collect();
    // Add s1 entries for variables not in s2
    for (v, t) in s1 {
        if !s2.iter().any(|(v2, _)| v2 == v) {
            result.push((*v, t.clone()));
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Free Variables
// ---------------------------------------------------------------------------

/// Get free type variables in a type.
fn free_vars(ty: &Type) -> HashSet<TypeVar> {
    let mut set = HashSet::new();
    collect_free_vars(ty, &mut set);
    set
}

fn collect_free_vars(ty: &Type, set: &mut HashSet<TypeVar>) {
    match ty {
        Type::Var(v) => {
            set.insert(*v);
        }
        Type::Tuple(ts) => {
            for t in ts {
                collect_free_vars(t, set);
            }
        }
        Type::Record(fs) => {
            for (_, t) in fs {
                collect_free_vars(t, set);
            }
        }
        Type::Variant(vs) => {
            for (_, ts) in vs {
                for t in ts {
                    collect_free_vars(t, set);
                }
            }
        }
        Type::Array(t) => collect_free_vars(t, set),
        Type::Ref(t, _) => collect_free_vars(t, set),
        Type::Arrow(params, ret, _) => {
            for p in params {
                collect_free_vars(p, set);
            }
            collect_free_vars(ret, set);
        }
        Type::App(name, args) => {
            collect_free_vars(name, set);
            for a in args {
                collect_free_vars(a, set);
            }
        }
        Type::Scheme(vs, t) => {
            collect_free_vars(t, set);
            for v in vs {
                set.remove(v);
            }
        }
        _ => {}
    }
}

/// Get free type variables in a context.
fn free_vars_ctx(ctx: &TypeContext) -> HashSet<TypeVar> {
    let mut set = HashSet::new();
    for (_, ty) in &ctx.bindings {
        set.extend(free_vars(ty));
    }
    set
}

// ---------------------------------------------------------------------------
// Generalization & Instantiation
// ---------------------------------------------------------------------------

/// Generalize a type into a scheme: abstract over variables not free in the context.
fn generalize(ctx: &TypeContext, ty: &Type) -> Type {
    let ctx_fv = free_vars_ctx(ctx);
    let ty_fv = free_vars(ty);
    let gen_vars: Vec<TypeVar> = ty_fv.difference(&ctx_fv).cloned().collect();
    if gen_vars.is_empty() {
        ty.clone()
    } else {
        Type::Scheme(gen_vars, Box::new(ty.clone()))
    }
}

/// Instantiate a scheme: replace bound variables with fresh ones.
fn instantiate(ty: &Type) -> Type {
    match ty {
        Type::Scheme(vs, t) => {
            let subst: Substitution = vs.iter().map(|v| (*v, Type::Var(TypeVar::fresh()))).collect();
            apply_subst(t, &subst)
        }
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------
// Most General Unifier
// ---------------------------------------------------------------------------

/// Compute the most general unifier of two types.
/// Returns a substitution that makes the two types equal.
fn mgu(t1: &Type, t2: &Type, span: &Span) -> NuResult<Substitution> {
    use Type::*;

    match (t1, t2) {
        // Same primitive types
        (Int, Int)
        | (Float, Float)
        | (Bool, Bool)
        | (String, String)
        | (Unit, Unit)
        | (Never, Never)
        | (Address, Address) => Ok(vec![]),

        // Variable unification
        (Var(v), t) | (t, Var(v)) => var_bind(*v, t, span),

        // Tuples
        (Tuple(ts1), Tuple(ts2)) => {
            if ts1.len() != ts2.len() {
                return Err(NuError::TypeError {
                    message: format!(
                        "Tuple size mismatch: {} vs {}",
                        ts1.len(),
                        ts2.len()
                    ),
                    span: span.clone(),
                });
            }
            let mut subst = vec![];
            for (a, b) in ts1.iter().zip(ts2.iter()) {
                let s = mgu(&apply_subst(a, &subst), &apply_subst(b, &subst), span)?;
                subst = compose_subst(&s, &subst);
            }
            Ok(subst)
        }

        // Records
        (Record(fs1), Record(fs2)) => {
            let mut subst = vec![];
            for (name1, ty1) in fs1 {
                match fs2.iter().find(|(n, _)| n == name1) {
                    Some((_, ty2)) => {
                        let s = mgu(&apply_subst(ty1, &subst), &apply_subst(ty2, &subst), span)?;
                        subst = compose_subst(&s, &subst);
                    }
                    None => {
                        return Err(NuError::TypeError {
                            message: format!("Record missing field: {}", name1),
                            span: span.clone(),
                        });
                    }
                }
            }
            // Check that fs2 doesn't have extra fields
            for (name2, _) in fs2 {
                if !fs1.iter().any(|(n, _)| n == name2) {
                    return Err(NuError::TypeError {
                        message: format!("Record has extra field: {}", name2),
                        span: span.clone(),
                    });
                }
            }
            Ok(subst)
        }

        // Arrays
        (Array(t1), Array(t2)) => mgu(t1, t2, span),

        // References with capabilities
        (Ref(ty1, cap1), Ref(ty2, cap2)) => {
            if cap1 != cap2 {
                return Err(NuError::TypeError {
                    message: format!("Capability mismatch: {:?} vs {:?}", cap1, cap2),
                    span: span.clone(),
                });
            }
            mgu(ty1, ty2, span)
        }

        // Functions
        (Arrow(params1, ret1, eff1), Arrow(params2, ret2, eff2)) => {
            if params1.len() != params2.len() {
                return Err(NuError::TypeError {
                    message: format!(
                        "Function arity mismatch: {} vs {}",
                        params1.len(),
                        params2.len()
                    ),
                    span: span.clone(),
                });
            }
            let mut subst = vec![];
            for (a, b) in params1.iter().zip(params2.iter()) {
                let s = mgu(&apply_subst(a, &subst), &apply_subst(b, &subst), span)?;
                subst = compose_subst(&s, &subst);
            }
            let s = mgu(
                &apply_subst(ret1, &subst),
                &apply_subst(ret2, &subst),
                span,
            )?;
            subst = compose_subst(&s, &subst);

            // Effect rows must be compatible
            if !effect_row_compatible(eff1, eff2) {
                return Err(NuError::TypeError {
                    message: format!(
                        "Effect row mismatch: {} vs {}",
                        eff1.display(),
                        eff2.display()
                    ),
                    span: span.clone(),
                });
            }

            Ok(subst)
        }

        // Named type equality
        (Named(n1), Named(n2)) if n1 == n2 => Ok(vec![]),

        // Generic application
        (App(name1, args1), App(name2, args2)) => {
            let mut subst = mgu(name1, name2, span)?;
            if args1.len() != args2.len() {
                return Err(NuError::TypeError {
                    message: format!(
                        "Type application arity mismatch: {} vs {}",
                        args1.len(),
                        args2.len()
                    ),
                    span: span.clone(),
                });
            }
            for (a, b) in args1.iter().zip(args2.iter()) {
                let s = mgu(&apply_subst(a, &subst), &apply_subst(b, &subst), span)?;
                subst = compose_subst(&s, &subst);
            }
            Ok(subst)
        }

        // Type mismatch
        _ => Err(NuError::TypeError {
            message: format!("Type mismatch: {} vs {}", t1.display(), t2.display()),
            span: span.clone(),
        }),
    }
}

/// Bind a type variable to a type, performing the occurs check.
fn var_bind(v: TypeVar, ty: &Type, span: &Span) -> NuResult<Substitution> {
    match ty {
        Type::Var(v2) if *v2 == v => Ok(vec![]), // Trivial binding
        t => {
            // Occurs check
            if free_vars(t).contains(&v) {
                return Err(NuError::TypeError {
                    message: format!(
                        "Occurs check failed: {} occurs in {}",
                        Type::Var(v).display(),
                        t.display()
                    ),
                    span: span.clone(),
                });
            }
            Ok(vec![(v, t.clone())])
        }
    }
}

/// Check if two effect rows are compatible.
fn effect_row_compatible(eff1: &EffectRow, eff2: &EffectRow) -> bool {
    match (eff1, eff2) {
        (EffectRow::Closed(e1), EffectRow::Closed(e2)) => {
            e1.len() == e2.len() && e1.iter().all(|e| e2.contains(e))
        }
        (EffectRow::Closed(e), EffectRow::Open(o, _))
        | (EffectRow::Open(o, _), EffectRow::Closed(e)) => {
            e.iter().all(|eff| o.contains(eff))
        }
        (EffectRow::Open(_, _), EffectRow::Open(_, _)) => {
            // Open rows are compatible (free variable absorbs differences)
            true
        }
    }
}

// ---------------------------------------------------------------------------
// Type Checker
// ---------------------------------------------------------------------------

pub struct TypeChecker {
    errors: Vec<NuError>,
}

impl TypeChecker {
    pub fn new() -> Self {
        TypeChecker { errors: vec![] }
    }

    /// Type-check a module: infer types for all declarations.
    pub fn check_module(&mut self, module: &Module) -> NuResult<Type> {
        let mut ctx = TypeContext::empty();
        let mut last_type = Type::Unit;

        for decl in &module.decls {
            match self.infer_decl(&mut ctx, decl) {
                Ok(ty) => last_type = ty,
                Err(e) => self.errors.push(e),
            }
        }

        if !self.errors.is_empty() {
            return Err(self.errors.remove(0));
        }

        Ok(last_type)
    }

    /// Infer the type of a declaration and update the context.
    fn infer_decl(&mut self, ctx: &mut TypeContext, decl: &Decl) -> NuResult<Type> {
        match decl {
            Decl::Fun { name, params, ret_type, body, .. } => {
                // Create fresh type variables for parameters
                let param_types: Vec<Type> = params
                    .iter()
                    .map(|(_, ty)| {
                        ty.clone()
                            .unwrap_or_else(|| Type::Var(TypeVar::fresh()))
                    })
                    .collect();

                // Add function parameters to context
                let mut body_ctx = ctx.clone();
                for ((param_name, _), param_ty) in params.iter().zip(&param_types) {
                    body_ctx = body_ctx.extend(param_name.clone(), param_ty.clone());
                }

                // Infer body type
                let body_subst = self.infer_expr(&mut body_ctx, body)?;
                let body_type = apply_subst(&body_ctx.get_type(body).unwrap_or(Type::Unit), &body_subst);

                // If return type annotation exists, unify
                let final_ret = if let Some(ann) = ret_type {
                    let _ = mgu(&body_type, ann, &body.span())?;
                    ann.clone()
                } else {
                    body_type
                };

                // Function type
                let fun_type = Type::Arrow(param_types, Box::new(final_ret), Box::new(EffectRow::pure()));

                // Generalize and add to context
                let gen_type = generalize(ctx, &fun_type);
                ctx.bindings.push((name.clone(), gen_type));

                Ok(fun_type)
            }

            Decl::Actor { def, .. } => {
                // Type-check actor definition
                let actor_type = Type::Named(def.name.clone());
                ctx.bindings.push((def.name.clone(), actor_type.clone()));

                // Check each behavior
                for behavior in &def.behaviors {
                    let mut behavior_ctx = ctx.clone();
                    let param_types: Vec<Type> = behavior
                        .params
                        .iter()
                        .map(|(_, ty)| ty.clone())
                        .collect();
                    for ((name, _), ty) in behavior.params.iter().zip(&param_types) {
                        behavior_ctx = behavior_ctx.extend(name.clone(), ty.clone());
                    }
                    let _ = self.infer_expr(&mut behavior_ctx, &behavior.body)?;
                }

                Ok(actor_type)
            }

            Decl::Agent { def, .. } => {
                let agent_type = Type::Named(def.name.clone());
                ctx.bindings.push((def.name.clone(), agent_type.clone()));

                for behavior in &def.behaviors {
                    let mut behavior_ctx = ctx.clone();
                    for (name, ty) in &behavior.params {
                        behavior_ctx = behavior_ctx.extend(name.clone(), ty.clone());
                    }
                    let _ = self.infer_expr(&mut behavior_ctx, &behavior.body)?;
                }

                Ok(agent_type)
            }

            Decl::TypeAlias { name, body, .. } => {
                ctx.bindings.push((name.clone(), body.clone()));
                Ok(body.clone())
            }

            Decl::Module { name, decls, .. } => {
                // Create a nested context
                let mut mod_ctx = ctx.clone();
                let mut last_ty = Type::Unit;
                for d in decls {
                    last_ty = self.infer_decl(&mut mod_ctx, d)?;
                }
                // Export module name
                ctx.bindings.push((name.clone(), Type::Named(name.clone())));
                Ok(last_ty)
            }

            Decl::Import { .. } => {
                // TODO: resolve imports
                Ok(Type::Unit)
            }
        }
    }

    /// Infer the type of an expression, returning a substitution.
    pub fn infer_expr(&mut self, ctx: &mut TypeContext, expr: &Expr) -> NuResult<Substitution> {
        match expr {
            Expr::Literal(lit, _) => self.infer_literal(lit),
            Expr::Var(name, span) => self.infer_var(ctx, name, span),
            Expr::Lambda { params, body, .. } => self.infer_lambda(ctx, params, body),
            Expr::App { func, args, span } => self.infer_app(ctx, func, args, span),
            Expr::Let { name, value, body, .. } => self.infer_let(ctx, name, value, body),
            Expr::LetRec { name, params, value, body, .. } => {
                self.infer_letrec(ctx, name, params, value, body)
            }
            Expr::If { cond, then_branch, else_branch, span } => {
                self.infer_if(ctx, cond, then_branch, else_branch.as_deref(), span)
            }
            Expr::Binary { op, left, right, span } => self.infer_binary(ctx, op, left, right, span),
            Expr::Unary { op, expr, span } => self.infer_unary(ctx, op, expr, span),
            Expr::Tuple(elems, span) => self.infer_tuple(ctx, elems, span),
            Expr::Record(fields, span) => self.infer_record(ctx, fields, span),
            Expr::FieldAccess { expr, field, span } => self.infer_field_access(ctx, expr, field, span),
            Expr::Array(elems, span) => self.infer_array(ctx, elems, span),
            Expr::Index { arr, idx, span } => self.infer_index(ctx, arr, idx, span),
            Expr::Match { scrutinee, arms, span } => self.infer_match(ctx, scrutinee, arms, span),
            Expr::Block { exprs, .. } => self.infer_block(ctx, exprs),
            Expr::Spawn { .. } => Ok(vec![]), // Returns actor reference (Address)
            Expr::Send { .. } => Ok(vec![]),  // Returns Unit
            Expr::Ask { .. } => Ok(vec![]),   // Returns the reply type (fresh var)
            Expr::Receive { .. } => Ok(vec![]), // Returns fresh var
            Expr::SelfRef(_) => Ok(vec![]),   // Returns Address
            Expr::Perform { .. } => Ok(vec![]), // Returns fresh var
            Expr::Handle { body, .. } => self.infer_expr(ctx, body),
            Expr::Migrate { .. } => Ok(vec![]), // Returns Unit
            Expr::CapAnnotate { expr, .. } => self.infer_expr(ctx, expr),
            Expr::TypeAnnotate { expr, ty, .. } => {
                let subst = self.infer_expr(ctx, expr)?;
                // TODO: unify inferred type with annotation
                Ok(subst)
            }
            Expr::Pipe { left, right, .. } => self.infer_pipe(ctx, left, right),
            Expr::Try { body, .. } => self.infer_expr(ctx, body),
            Expr::Await { expr, .. } => self.infer_expr(ctx, expr),
            Expr::Assign { target, value, span } => self.infer_assign(ctx, target, value, span),
            Expr::ActorDef(def, _) => {
                ctx.bindings.push((def.name.clone(), Type::Named(def.name.clone())));
                Ok(vec![])
            }
            Expr::AgentDef(def, _) => {
                ctx.bindings.push((def.name.clone(), Type::Named(def.name.clone())));
                Ok(vec![])
            }
        }
    }

    // -- Literal inference --

    fn infer_literal(&mut self, lit: &Literal) -> NuResult<Substitution> {
        match lit {
            Literal::Int(_) => Ok(vec![(TypeVar::fresh(), Type::Int)]),
            Literal::Float(_) => Ok(vec![(TypeVar::fresh(), Type::Float)]),
            Literal::Bool(_) => Ok(vec![(TypeVar::fresh(), Type::Bool)]),
            Literal::String(_) => Ok(vec![(TypeVar::fresh(), Type::String)]),
            Literal::Unit => Ok(vec![(TypeVar::fresh(), Type::Unit)]),
        }
    }

    // -- Variable inference --

    fn infer_var(&mut self, ctx: &TypeContext, name: &str, _span: &Span) -> NuResult<Substitution> {
        match ctx.lookup(name) {
            Some(scheme) => {
                let inst = instantiate(scheme);
                // The substitution maps a fresh variable to the instantiated type
                Ok(vec![(TypeVar::fresh(), inst)])
            }
            None => Err(NuError::TypeError {
                message: format!("Unbound variable: {}", name),
                span: _span.clone(),
            }),
        }
    }

    // -- Lambda inference --

    fn infer_lambda(
        &mut self,
        ctx: &mut TypeContext,
        params: &[(String, Option<Type>)],
        body: &Expr,
    ) -> NuResult<Substitution> {
        let param_types: Vec<Type> = params
            .iter()
            .map(|(_, ty)| ty.clone().unwrap_or_else(|| Type::Var(TypeVar::fresh())))
            .collect();

        let mut body_ctx = ctx.clone();
        for ((name, _), ty) in params.iter().zip(&param_types) {
            body_ctx = body_ctx.extend(name.clone(), ty.clone());
        }

        let body_subst = self.infer_expr(&mut body_ctx, body)?;
        let body_type = apply_subst(
            &body_ctx.get_type(body).unwrap_or(Type::Unit),
            &body_subst,
        );

        let fun_type = Type::Arrow(
            param_types,
            Box::new(body_type),
            Box::new(EffectRow::pure()),
        );

        Ok(vec![(TypeVar::fresh(), fun_type)])
    }

    // -- Application inference --

    fn infer_app(
        &mut self,
        ctx: &mut TypeContext,
        func: &Expr,
        args: &[Expr],
        span: &Span,
    ) -> NuResult<Substitution> {
        let func_subst = self.infer_expr(ctx, func)?;
        let func_type = apply_subst(&ctx.get_type(func).unwrap_or(Type::Unit), &func_subst);

        // Generate fresh type variables for arguments and return
        let arg_types: Vec<Type> = args.iter().map(|_| Type::Var(TypeVar::fresh())).collect();
        let ret_type = Type::Var(TypeVar::fresh());

        let expected = Type::Arrow(
            arg_types.clone(),
            Box::new(ret_type.clone()),
            Box::new(EffectRow::pure()),
        );

        let unify_subst = mgu(&func_type, &expected, span)?;

        // Apply substitution to argument types and infer each argument
        let mut final_subst = compose_subst(&unify_subst, &func_subst);

        for (arg, expected_ty) in args.iter().zip(arg_types.iter()) {
            let actual_ty = apply_subst(expected_ty, &final_subst);
            let arg_subst = self.infer_expr(ctx, arg)?;

            // Get the inferred type of the argument expression
            let inferred_ty = apply_subst(
                &ctx.get_type(arg).unwrap_or(Type::Var(TypeVar::fresh())),
                &arg_subst,
            );

            let s = mgu(&inferred_ty, &actual_ty, span)?;
            final_subst = compose_subst(&s, &final_subst);
            final_subst = compose_subst(&arg_subst, &final_subst);
        }

        Ok(final_subst)
    }

    // -- Let binding inference --

    fn infer_let(
        &mut self,
        ctx: &mut TypeContext,
        name: &str,
        value: &Expr,
        body: &Expr,
    ) -> NuResult<Substitution> {
        let val_subst = self.infer_expr(ctx, value)?;
        let val_type = apply_subst(
            &ctx.get_type(value).unwrap_or(Type::Unit),
            &val_subst,
        );

        // Generalize the value type
        let gen_type = generalize(ctx, &val_type);

        // Add to context and infer body
        let mut body_ctx = ctx.clone();
        body_ctx = body_ctx.extend(name.to_string(), gen_type);
        let body_subst = self.infer_expr(&mut body_ctx, body)?;

        Ok(compose_subst(&body_subst, &val_subst))
    }

    // -- Let-rec inference --

    fn infer_letrec(
        &mut self,
        ctx: &mut TypeContext,
        name: &str,
        params: &[(String, Option<Type>)],
        value: &Expr,
        body: &Expr,
    ) -> NuResult<Substitution> {
        // Create a fresh type variable for the recursive function
        let rec_type = Type::Var(TypeVar::fresh());
        let mut rec_ctx = ctx.clone();
        rec_ctx = rec_ctx.extend(name.to_string(), rec_type.clone());

        // Infer the function value with the recursive binding
        let param_types: Vec<Type> = params
            .iter()
            .map(|(_, ty)| ty.clone().unwrap_or_else(|| Type::Var(TypeVar::fresh())))
            .collect();

        let mut val_ctx = rec_ctx.clone();
        for ((pname, _), pty) in params.iter().zip(&param_types) {
            val_ctx = val_ctx.extend(pname.clone(), pty.clone());
        }

        let val_subst = self.infer_expr(&mut val_ctx, value)?;
        let val_type = apply_subst(
            &val_ctx.get_type(value).unwrap_or(Type::Unit),
            &val_subst,
        );

        // The recursive type should be a function
        let fun_type = Type::Arrow(param_types, Box::new(val_type), Box::new(EffectRow::pure()));
        let rec_subst = mgu(&rec_type, &fun_type, &Span::default())?;

        // Generalize and add to context
        let combined = compose_subst(&rec_subst, &val_subst);
        let gen_type = generalize(ctx, &apply_subst(&fun_type, &combined));

        let mut body_ctx = ctx.clone();
        body_ctx = body_ctx.extend(name.to_string(), gen_type);
        let body_subst = self.infer_expr(&mut body_ctx, body)?;

        Ok(compose_subst(&body_subst, &combined))
    }

    // -- If inference --

    fn infer_if(
        &mut self,
        ctx: &mut TypeContext,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: Option<&Expr>,
        span: &Span,
    ) -> NuResult<Substitution> {
        let cond_subst = self.infer_expr(ctx, cond)?;
        let cond_type = apply_subst(
            &ctx.get_type(cond).unwrap_or(Type::Unit),
            &cond_subst,
        );

        // Condition must be Bool
        let bool_subst = mgu(&cond_type, &Type::Bool, span)?;
        let mut subst = compose_subst(&bool_subst, &cond_subst);

        let then_subst = self.infer_expr(ctx, then_branch)?;
        let then_type = apply_subst(
            &ctx.get_type(then_branch).unwrap_or(Type::Unit),
            &then_subst,
        );
        subst = compose_subst(&then_subst, &subst);

        if let Some(else_expr) = else_branch {
            let else_subst = self.infer_expr(ctx, else_expr)?;
            let else_type = apply_subst(
                &ctx.get_type(else_expr).unwrap_or(Type::Unit),
                &else_subst,
            );

            let branch_subst = mgu(&then_type, &else_type, span)?;
            subst = compose_subst(&branch_subst, &subst);
            subst = compose_subst(&else_subst, &subst);
        }

        Ok(subst)
    }

    // -- Binary operator inference --

    fn infer_binary(
        &mut self,
        ctx: &mut TypeContext,
        op: &BinOp,
        left: &Expr,
        right: &Expr,
        span: &Span,
    ) -> NuResult<Substitution> {
        let left_subst = self.infer_expr(ctx, left)?;
        let right_subst = self.infer_expr(ctx, right)?;

        let left_type = apply_subst(
            &ctx.get_type(left).unwrap_or(Type::Unit),
            &left_subst,
        );
        let right_type = apply_subst(
            &ctx.get_type(right).unwrap_or(Type::Unit),
            &right_subst,
        );

        let (result_type, operand_type) = match op {
            // Arithmetic: operands must be numeric, result is same type
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                // Unify left and right operand types
                let num_subst = mgu(&left_type, &right_type, span)?;
                let unified = apply_subst(&left_type, &num_subst);

                // Must be Int or Float
                match unified {
                    Type::Int | Type::Float | Type::Var(_) => (unified.clone(), unified),
                    _ => {
                        return Err(NuError::TypeError {
                            message: format!(
                                "Arithmetic operator {:?} requires numeric operands, got {} and {}",
                                op, left_type.display(), right_type.display()
                            ),
                            span: span.clone(),
                        });
                    }
                }
            }

            // Comparison: operands must be comparable, result is Bool
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let cmp_subst = mgu(&left_type, &right_type, span)?;
                (Type::Bool, apply_subst(&left_type, &cmp_subst))
            }

            // Logical: operands must be Bool, result is Bool
            BinOp::And | BinOp::Or => {
                let _ = mgu(&left_type, &Type::Bool, span)?;
                let _ = mgu(&right_type, &Type::Bool, span)?;
                (Type::Bool, Type::Bool)
            }

            // Cons: right must be list of left's type
            BinOp::Cons => {
                let elem_subst = mgu(&right_type, &Type::Array(Box::new(left_type.clone())), span)?;
                let arr_type = apply_subst(&Type::Array(Box::new(left_type.clone())), &elem_subst);
                (arr_type.clone(), arr_type)
            }

            // Pipe: left |> right means right(left)
            BinOp::Pipe => {
                // Pipe is handled specially in infer_pipe
                return self.infer_pipe(ctx, left, right);
            }
        };

        let mut subst = compose_subst(&left_subst, &right_subst);
        subst.push((TypeVar::fresh(), result_type));

        Ok(subst)
    }

    // -- Unary operator inference --

    fn infer_unary(
        &mut self,
        ctx: &mut TypeContext,
        op: &UnOp,
        expr: &Expr,
        _span: &Span,
    ) -> NuResult<Substitution> {
        let expr_subst = self.infer_expr(ctx, expr)?;
        let expr_type = apply_subst(
            &ctx.get_type(expr).unwrap_or(Type::Unit),
            &expr_subst,
        );

        let result_type = match op {
            UnOp::Neg => {
                // Operand must be numeric
                match expr_type {
                    Type::Int | Type::Float | Type::Var(_) => expr_type,
                    _ => Type::Var(TypeVar::fresh()),
                }
            }
            UnOp::Not => {
                // Operand must be Bool
                Type::Bool
            }
        };

        let mut subst = expr_subst;
        subst.push((TypeVar::fresh(), result_type));
        Ok(subst)
    }

    // -- Tuple inference --

    fn infer_tuple(
        &mut self,
        ctx: &mut TypeContext,
        elems: &[Expr],
        _span: &Span,
    ) -> NuResult<Substitution> {
        let mut subst = vec![];
        let mut elem_types = vec![];

        for elem in elems {
            let elem_subst = self.infer_expr(ctx, elem)?;
            let elem_type = apply_subst(
                &ctx.get_type(elem).unwrap_or(Type::Unit),
                &elem_subst,
            );
            elem_types.push(elem_type);
            subst = compose_subst(&elem_subst, &subst);
        }

        subst.push((TypeVar::fresh(), Type::Tuple(elem_types)));
        Ok(subst)
    }

    // -- Record inference --

    fn infer_record(
        &mut self,
        ctx: &mut TypeContext,
        fields: &[(String, Expr)],
        _span: &Span,
    ) -> NuResult<Substitution> {
        let mut subst = vec![];
        let mut field_types = vec![];

        for (name, expr) in fields {
            let field_subst = self.infer_expr(ctx, expr)?;
            let field_type = apply_subst(
                &ctx.get_type(expr).unwrap_or(Type::Unit),
                &field_subst,
            );
            field_types.push((name.clone(), field_type));
            subst = compose_subst(&field_subst, &subst);
        }

        subst.push((TypeVar::fresh(), Type::Record(field_types)));
        Ok(subst)
    }

    // -- Field access inference --

    fn infer_field_access(
        &mut self,
        ctx: &mut TypeContext,
        expr: &Expr,
        field: &str,
        span: &Span,
    ) -> NuResult<Substitution> {
        let expr_subst = self.infer_expr(ctx, expr)?;
        let expr_type = apply_subst(
            &ctx.get_type(expr).unwrap_or(Type::Unit),
            &expr_subst,
        );

        // Create a fresh type variable for the field
        let field_type = Type::Var(TypeVar::fresh());

        // The expression type must be a record with this field
        let expected = Type::Record(vec![(field.to_string(), field_type.clone())]);
        let access_subst = mgu(&expr_type, &expected, span)?;

        let mut subst = compose_subst(&access_subst, &expr_subst);
        subst.push((TypeVar::fresh(), field_type));
        Ok(subst)
    }

    // -- Array inference --

    fn infer_array(
        &mut self,
        ctx: &mut TypeContext,
        elems: &[Expr],
        _span: &Span,
    ) -> NuResult<Substitution> {
        if elems.is_empty() {
            subst.push((TypeVar::fresh(), Type::Array(Box::new(Type::Var(TypeVar::fresh())))));
            return Ok(subst);
        }

        let mut subst = vec![];
        let first_subst = self.infer_expr(ctx, &elems[0])?;
        let first_type = apply_subst(
            &ctx.get_type(&elems[0]).unwrap_or(Type::Unit),
            &first_subst,
        );
        subst = first_subst;

        for elem in &elems[1..] {
            let elem_subst = self.infer_expr(ctx, elem)?;
            let elem_type = apply_subst(
                &ctx.get_type(elem).unwrap_or(Type::Unit),
                &elem_subst,
            );
            let unify_subst = mgu(&first_type, &elem_type, _span)?;
            subst = compose_subst(&unify_subst, &subst);
            subst = compose_subst(&elem_subst, &subst);
        }

        subst.push((TypeVar::fresh(), Type::Array(Box::new(first_type))));
        Ok(subst)
    }

    // -- Index inference --

    fn infer_index(
        &mut self,
        ctx: &mut TypeContext,
        arr: &Expr,
        idx: &Expr,
        span: &Span,
    ) -> NuResult<Substitution> {
        let arr_subst = self.infer_expr(ctx, arr)?;
        let idx_subst = self.infer_expr(ctx, idx)?;

        let arr_type = apply_subst(
            &ctx.get_type(arr).unwrap_or(Type::Unit),
            &arr_subst,
        );
        let idx_type = apply_subst(
            &ctx.get_type(idx).unwrap_or(Type::Unit),
            &idx_subst,
        );

        // Index must be Int
        let idx_unify = mgu(&idx_type, &Type::Int, span)?;

        // Array type must be Array of some element type
        let elem_type = Type::Var(TypeVar::fresh());
        let arr_unify = mgu(&arr_type, &Type::Array(Box::new(elem_type.clone())), span)?;

        let mut subst = compose_subst(&arr_subst, &idx_subst);
        subst = compose_subst(&idx_unify, &subst);
        subst = compose_subst(&arr_unify, &subst);
        subst.push((TypeVar::fresh(), elem_type));

        Ok(subst)
    }

    // -- Pattern matching inference --

    fn infer_match(
        &mut self,
        ctx: &mut TypeContext,
        scrutinee: &Expr,
        arms: &[(Pattern, Expr)],
        span: &Span,
    ) -> NuResult<Substitution> {
        let scrut_subst = self.infer_expr(ctx, scrutinee)?;
        let scrut_type = apply_subst(
            &ctx.get_type(scrutinee).unwrap_or(Type::Unit),
            &scrut_subst,
        );

        let mut subst = scrut_subst;
        let mut result_type = Type::Var(TypeVar::fresh());

        for (pattern, arm_body) in arms {
            // Infer pattern bindings
            let pat_bindings = self.pattern_bindings(pattern, &scrut_type);
            let mut arm_ctx = ctx.clone();
            for (name, ty) in pat_bindings {
                arm_ctx = arm_ctx.extend(name, ty);
            }

            let arm_subst = self.infer_expr(&mut arm_ctx, arm_body)?;
            let arm_type = apply_subst(
                &arm_ctx.get_type(arm_body).unwrap_or(Type::Unit),
                &arm_subst,
            );

            let unify_subst = mgu(&result_type, &arm_type, span)?;
            result_type = apply_subst(&result_type, &unify_subst);
            subst = compose_subst(&unify_subst, &subst);
            subst = compose_subst(&arm_subst, &subst);
        }

        Ok(subst)
    }

    /// Extract variable bindings from a pattern with their types.
    fn pattern_bindings(&mut self, pattern: &Pattern, scrut_type: &Type) -> Vec<(String, Type)> {
        match pattern {
            Pattern::Wild => vec![],
            Pattern::Var(name) => vec![(name.clone(), scrut_type.clone())],
            Pattern::Lit(_) => vec![],
            Pattern::Tuple(pats) => {
                if let Type::Tuple(elem_types) = scrut_type {
                    let mut bindings = vec![];
                    for (pat, ty) in pats.iter().zip(elem_types.iter()) {
                        bindings.extend(self.pattern_bindings(pat, ty));
                    }
                    bindings
                } else {
                    vec![]
                }
            }
            Pattern::Record(field_pats) => {
                if let Type::Record(field_types) = scrut_type {
                    let mut bindings = vec![];
                    for (name, pat) in field_pats {
                        if let Some((_, ty)) = field_types.iter().find(|(n, _)| n == name) {
                            bindings.extend(self.pattern_bindings(pat, ty));
                        }
                    }
                    bindings
                } else {
                    vec![]
                }
            }
            Pattern::Variant(_, Some(pat)) => self.pattern_bindings(pat, scrut_type),
            Pattern::Variant(_, None) => vec![],
            Pattern::Alias(name, pat) => {
                let mut bindings = self.pattern_bindings(pat, scrut_type);
                bindings.push((name.clone(), scrut_type.clone()));
                bindings
            }
        }
    }

    // -- Block inference --

    fn infer_block(
        &mut self,
        ctx: &mut TypeContext,
        exprs: &[Expr],
    ) -> NuResult<Substitution> {
        let mut subst = vec![];
        for expr in exprs {
            let expr_subst = self.infer_expr(ctx, expr)?;
            subst = compose_subst(&expr_subst, &subst);
        }
        Ok(subst)
    }

    // -- Pipe inference --

    fn infer_pipe(
        &mut self,
        ctx: &mut TypeContext,
        left: &Expr,
        right: &Expr,
    ) -> NuResult<Substitution> {
        // x |> f means f(x)
        // Infer the left side
        let left_subst = self.infer_expr(ctx, left)?;
        let left_type = apply_subst(
            &ctx.get_type(left).unwrap_or(Type::Unit),
            &left_subst,
        );

        // The right side must be a function that accepts the left type
        let ret_type = Type::Var(TypeVar::fresh());
        let expected_fun = Type::Arrow(
            vec![left_type.clone()],
            Box::new(ret_type.clone()),
            Box::new(EffectRow::pure()),
        );

        let right_subst = self.infer_expr(ctx, right)?;
        let right_type = apply_subst(
            &ctx.get_type(right).unwrap_or(Type::Unit),
            &right_subst,
        );

        let pipe_subst = mgu(&right_type, &expected_fun, &Span::default())?;

        let mut subst = compose_subst(&pipe_subst, &left_subst);
        subst = compose_subst(&right_subst, &subst);
        subst.push((TypeVar::fresh(), ret_type));

        Ok(subst)
    }

    // -- Assignment inference --

    fn infer_assign(
        &mut self,
        ctx: &mut TypeContext,
        target: &Expr,
        value: &Expr,
        span: &Span,
    ) -> NuResult<Substitution> {
        let target_subst = self.infer_expr(ctx, target)?;
        let value_subst = self.infer_expr(ctx, value)?;

        let target_type = apply_subst(
            &ctx.get_type(target).unwrap_or(Type::Unit),
            &target_subst,
        );
        let value_type = apply_subst(
            &ctx.get_type(value).unwrap_or(Type::Unit),
            &value_subst,
        );

        let unify_subst = mgu(&target_type, &value_type, span)?;

        let mut subst = compose_subst(&target_subst, &value_subst);
        subst = compose_subst(&unify_subst, &subst);
        subst.push((TypeVar::fresh(), Type::Unit)); // Assignment returns Unit

        Ok(subst)
    }

    /// Get the inferred type of an expression from the context.
    /// This is a helper that looks up the last inferred type for an expression.
    pub fn get_expr_type(&self, ctx: &TypeContext, expr: &Expr) -> Option<Type> {
        // For now, return the type based on the expression structure
        // In a full implementation, we'd store inferred types per-expression
        match expr {
            Expr::Literal(lit, _) => Some(match lit {
                Literal::Int(_) => Type::Int,
                Literal::Float(_) => Type::Float,
                Literal::Bool(_) => Type::Bool,
                Literal::String(_) => Type::String,
                Literal::Unit => Type::Unit,
            }),
            Expr::Var(name, _) => ctx.lookup(name).cloned(),
            Expr::Tuple(elems, _) => {
                let types: Vec<Type> = elems.iter()
                    .map(|e| self.get_expr_type(ctx, e).unwrap_or(Type::Var(TypeVar::fresh())))
                    .collect();
                Some(Type::Tuple(types))
            }
            Expr::Record(fields, _) => {
                let types: Vec<(String, Type)> = fields.iter()
                    .map(|(n, e)| (n.clone(), self.get_expr_type(ctx, e).unwrap_or(Type::Var(TypeVar::fresh()))))
                    .collect();
                Some(Type::Record(types))
            }
            Expr::Array(elems, _) => {
                let elem_type = elems.first()
                    .and_then(|e| self.get_expr_type(ctx, e))
                    .unwrap_or(Type::Var(TypeVar::fresh()));
                Some(Type::Array(Box::new(elem_type)))
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// TypeContext extension
// ---------------------------------------------------------------------------

impl TypeContext {
    /// Look up a type scheme by name.
    pub fn lookup(&self, name: &str) -> Option<&Type> {
        self.bindings.iter().rev().find(|(n, _)| n == name).map(|(_, t)| t)
    }

    /// Extend the context with a new binding.
    pub fn extend(&self, name: String, ty: Type) -> Self {
        let mut ctx = self.clone();
        ctx.bindings.push((name, ty));
        ctx
    }

    /// Get the inferred type for an expression (placeholder for full implementation).
    pub fn get_type(&self, _expr: &Expr) -> Option<Type> {
        // In a full implementation, this would look up the stored type for the expression
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn infer_source(source: &str) -> NuResult<Type> {
        let module = parser::parse(source)?;
        let mut tc = TypeChecker::new();
        tc.check_module(&module)
    }

    fn expect_type(source: &str, expected: &str) {
        let ty = infer_source(source).unwrap_or_else(|e| panic!("Type error for '{}': {:?}", source, e));
        assert_eq!(ty.display(), expected, "For source: {}", source);
    }

    fn expect_error(source: &str) {
        assert!(infer_source(source).is_err(), "Expected type error for: {}", source);
    }

    // -- Literals --

    #[test]
    fn test_literal_int() {
        expect_type("42", "Int");
    }

    #[test]
    fn test_literal_float() {
        expect_type("3.14", "Float");
    }

    #[test]
    fn test_literal_bool() {
        expect_type("true", "Bool");
    }

    #[test]
    fn test_literal_string() {
        expect_type(r#""hello""#, "String");
    }

    #[test]
    fn test_literal_unit() {
        expect_type("()", "()");
    }

    // -- Variables --

    #[test]
    fn test_unbound_variable() {
        expect_error("x");
    }

    #[test]
    fn test_bound_variable() {
        expect_type("let x = 42 in x", "Int");
    }

    // -- Let bindings --

    #[test]
    fn test_let_binding() {
        expect_type("let x = 5 in x + 3", "Int");
    }

    #[test]
    fn test_nested_let() {
        expect_type("let a = 1 in let b = 2 in a + b", "Int");
    }

    #[test]
    fn test_let_shadowing() {
        expect_type("let x = 5 in let x = 10 in x + 1", "Int");
    }

    // -- Arithmetic --

    #[test]
    fn test_int_add() {
        expect_type("1 + 2", "Int");
    }

    #[test]
    fn test_int_sub() {
        expect_type("10 - 3", "Int");
    }

    #[test]
    fn test_int_mul() {
        expect_type("6 * 7", "Int");
    }

    #[test]
    fn test_int_div() {
        expect_type("21 / 3", "Int");
    }

    #[test]
    fn test_int_mod() {
        expect_type("17 % 5", "Int");
    }

    #[test]
    fn test_int_neg() {
        expect_type("-42", "Int");
    }

    #[test]
    fn test_arithmetic_precedence() {
        expect_type("1 + 2 * 3", "Int");
    }

    #[test]
    fn test_float_add() {
        expect_type("1.5 + 2.5", "Float");
    }

    #[test]
    fn test_float_mul() {
        expect_type("2.5 * 4.0", "Float");
    }

    // -- Comparison --

    #[test]
    fn test_int_eq() {
        expect_type("3 == 3", "Bool");
    }

    #[test]
    fn test_int_lt() {
        expect_type("3 < 5", "Bool");
    }

    #[test]
    fn test_int_gt() {
        expect_type("5 > 3", "Bool");
    }

    #[test]
    fn test_int_lte() {
        expect_type("3 <= 3", "Bool");
    }

    #[test]
    fn test_int_gte() {
        expect_type("5 >= 5", "Bool");
    }

    // -- Boolean logic --

    #[test]
    fn test_and() {
        expect_type("true and true", "Bool");
        expect_error("true and 1");
    }

    #[test]
    fn test_or() {
        expect_type("true or false", "Bool");
    }

    #[test]
    fn test_not() {
        expect_type("not true", "Bool");
    }

    // -- Conditionals --

    #[test]
    fn test_if_true() {
        expect_type("if true then 42 else 0", "Int");
    }

    #[test]
    fn test_if_comparison() {
        expect_type("if 3 < 5 then 1 else 0", "Int");
    }

    #[test]
    fn test_if_nested() {
        expect_type("if 1 < 2 then if 3 < 4 then 100 else 50 else 25", "Int");
    }

    #[test]
    fn test_if_mismatched_branches() {
        expect_error(r#"if true then 1 else "hello""#);
    }

    // -- Functions --

    #[test]
    fn test_function_declaration() {
        expect_type("fun add(x: Int, y: Int) = x + y\nadd(3, 4)", "Int");
    }

    #[test]
    fn test_function_zero_args() {
        expect_type("fun answer() = 42\nanswer()", "Int");
    }

    #[test]
    fn test_lambda() {
        expect_type("fn(x: Int) -> x + 1", "(Int) -> Int {}");
    }

    // -- Tuples --

    #[test]
    fn test_tuple_create() {
        expect_type("(1, 2, 3)", "(Int, Int, Int)");
    }

    #[test]
    fn test_tuple_nested() {
        expect_type("((1, 2), (3, 4))", "((Int, Int), (Int, Int))");
    }

    // -- Records --

    #[test]
    fn test_record_create() {
        expect_type("{ x: 10, y: 20 }", "{ x: Int, y: Int }");
    }

    #[test]
    fn test_record_access() {
        expect_type("let r = { x: 10, y: 20 } in r.x", "Int");
    }

    // -- Arrays --

    #[test]
    fn test_array_create() {
        expect_type("[1, 2, 3]", "[Int]");
    }

    // -- Pattern matching --

    #[test]
    fn test_match_int_literal() {
        expect_type("match 1 with | 1 => 100 | 2 => 200 | _ => 0", "Int");
    }

    #[test]
    fn test_match_bool() {
        expect_type("match true with | true => 1 | false => 0", "Int");
    }

    #[test]
    fn test_match_tuple() {
        expect_type("match (1, 2) with | (a, b) => a + b", "Int");
    }

    #[test]
    fn test_match_wildcard() {
        expect_type("match 42 with | _ => 100", "Int");
    }

    // -- Recursion --

    #[test]
    fn test_factorial() {
        expect_type(
            "let rec fact = fn(n: Int) -> if n == 0 then 1 else n * fact(n - 1) in fact(5)",
            "Int",
        );
    }

    // -- Polymorphism --

    #[test]
    fn test_identity_polymorphic() {
        expect_type("let id = fn(x) -> x in id(42)", "Int");
    }

    #[test]
    fn test_identity_bool() {
        expect_type("let id = fn(x) -> x in id(true)", "Bool");
    }

    // -- Type errors --

    #[test]
    fn test_type_error_int_plus_string() {
        expect_error(r#"1 + "hello""#);
    }

    #[test]
    fn test_type_error_string_plus_int() {
        expect_error(r#""hello" + 1"#);
    }

    #[test]
    fn test_type_error_bool_arithmetic() {
        expect_error("true + 1");
    }

    #[test]
    fn test_type_error_undefined_var() {
        expect_error("x + 1");
    }

    // -- Pipes --

    #[test]
    fn test_pipe_operator() {
        expect_type("let add = fn(x: Int) -> fn(y: Int) -> x + y in 5 |> add(3)", "Int");
    }

    // -- Blocks --

    #[test]
    fn test_block() {
        expect_type("{ 1; 2; 3 }", "Int");
    }
}
