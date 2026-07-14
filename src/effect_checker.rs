//! Effect checker and capability analyzer for Nulang.
//!
//! This module implements:
//! - Effect inference: given an expression, infer its effect row (what effects it may perform).
//! - Effect checking: verify that an expression's effects are subsumed by an allowed effect row.
//! - Capability analysis: infer the reference capability of an expression's result.
//! - Capability checking: verify capability subtyping and sendability constraints.

use crate::ast::*;
use crate::types::*;

use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Effect Row Operations
// ---------------------------------------------------------------------------

/// Check whether every effect in `sub` is present in `sup`.
///
/// For closed rows this is simple set inclusion.  For open rows we are
/// conservative: an open row on the *sup* side may contain additional effects
/// via its row variable, while an open row on the *sub* side is assumed to
/// possibly contain any effect not explicitly listed.
pub fn effect_row_subset(sub: &EffectRow, sup: &EffectRow) -> bool {
    match (sub, sup) {
        // Closed sub, closed sup: straightforward subset check.
        (EffectRow::Closed(sub_effs), EffectRow::Closed(sup_effs)) => {
            sub_effs.iter().all(|e| sup_effs.contains(e))
        }
        // Closed sub, open sup: every concrete effect in sub must be in sup's
        // concrete list (the row variable on the sup side may cover more).
        (EffectRow::Closed(sub_effs), EffectRow::Open(sup_effs, _)) => {
            sub_effs.iter().all(|e| sup_effs.contains(e))
        }
        // Open sub, closed sup: the open row *might* contain effects beyond
        // its concrete list, so it is only a subset if the concrete list
        // itself is already a subset and the open row is empty except for the
        // variable that could introduce new effects.
        (EffectRow::Open(sub_effs, _), EffectRow::Closed(sup_effs)) => {
            sub_effs.iter().all(|e| sup_effs.contains(e))
        }
        // Open sub, open sup: both row variables could introduce arbitrary
        // effects.  We only require that the concrete effects of sub are
        // contained in the concrete effects of sup.
        (EffectRow::Open(sub_effs, _), EffectRow::Open(sup_effs, _)) => {
            sub_effs.iter().all(|e| sup_effs.contains(e))
        }
    }
}

/// Union of two effect rows (non-destructive).
pub fn effect_row_union(a: &EffectRow, b: &EffectRow) -> EffectRow {
    a.clone().combine(b.clone())
}

/// Remove a single handled effect from a row (non-destructive).
pub fn effect_row_diff(row: &EffectRow, handled: &Effect) -> EffectRow {
    row.clone().remove(handled)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a user-written effect name (from `perform Effect.op`) into the
/// built-in [`Effect`] enum when possible, otherwise create a user-defined
/// effect.
pub fn parse_effect_name(name: &str) -> Effect {
    match name {
        "IO" => Effect::IO,
        "Net" => Effect::Net,
        "FS" => Effect::FS,
        "Rand" => Effect::Rand,
        "Time" => Effect::Time,
        "Spawn" => Effect::Spawn,
        "Send" => Effect::Send,
        "Receive" => Effect::Receive,
        "Migrate" => Effect::Migrate,
        "STM" => Effect::STM,
        "Async" => Effect::Async,
        "LLM" => Effect::LLM,
        "Cost" => Effect::Cost,
        "Event" => Effect::Event,
        "FFI" => Effect::FFI,
        other => Effect::UserDefined(other.to_string()),
    }
}

/// Collect the free (unbound) variable names in an expression.
/// `bound` accumulates the set of locally-bound names (parameters, let
/// bindings, etc.) and should not be included in the result.
fn free_vars(expr: &Expr, bound: &mut Vec<String>, acc: &mut Vec<String>) {
    match expr {
        Expr::Literal(_, _) => {}
        Expr::Var(name, _) => {
            if !bound.contains(name) && !acc.contains(name) {
                acc.push(name.clone());
            }
        }
        Expr::Lambda { params, body, .. } => {
            let mut new_bound = bound.clone();
            for (p, _) in params {
                if !new_bound.contains(p) {
                    new_bound.push(p.clone());
                }
            }
            free_vars(body, &mut new_bound, acc);
        }
        Expr::App { func, args, .. } => {
            free_vars(func, bound, acc);
            for arg in args {
                free_vars(arg, bound, acc);
            }
        }
        Expr::Let {
            name, value, body, ..
        } => {
            free_vars(value, bound, acc);
            let mut new_bound = bound.clone();
            if !new_bound.contains(name) {
                new_bound.push(name.clone());
            }
            free_vars(body, &mut new_bound, acc);
        }
        Expr::LetRec {
            name,
            params,
            value,
            body,
            ..
        } => {
            let mut new_bound = bound.clone();
            if !new_bound.contains(name) {
                new_bound.push(name.clone());
            }
            for (p, _) in params {
                if !new_bound.contains(p) {
                    new_bound.push(p.clone());
                }
            }
            free_vars(value, &mut new_bound, acc);
            free_vars(body, &mut new_bound, acc);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            free_vars(cond, bound, acc);
            free_vars(then_branch, bound, acc);
            if let Some(else_b) = else_branch {
                free_vars(else_b, bound, acc);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            free_vars(scrutinee, bound, acc);
            for (pat, arm_expr) in arms {
                let mut arm_bound = bound.clone();
                pat_bound_vars(pat, &mut arm_bound);
                free_vars(arm_expr, &mut arm_bound, acc);
            }
        }
        Expr::Block { exprs, .. } => {
            let mut block_bound = bound.clone();
            for e in exprs {
                free_vars(e, &mut block_bound, acc);
            }
        }
        Expr::Tuple(elts, _) => {
            for e in elts {
                free_vars(e, bound, acc);
            }
        }
        Expr::Record(fields, _) => {
            for (_, e) in fields {
                free_vars(e, bound, acc);
            }
        }
        Expr::FieldAccess { expr: e, .. } => {
            free_vars(e, bound, acc);
        }
        Expr::Array(elts, _) => {
            for e in elts {
                free_vars(e, bound, acc);
            }
        }
        Expr::Index { arr, idx, .. } => {
            free_vars(arr, bound, acc);
            free_vars(idx, bound, acc);
        }
        Expr::Binary { left, right, .. } => {
            free_vars(left, bound, acc);
            free_vars(right, bound, acc);
        }
        Expr::Unary { expr: e, .. } => {
            free_vars(e, bound, acc);
        }
        Expr::Assign { target, value, .. } => {
            free_vars(target, bound, acc);
            free_vars(value, bound, acc);
        }
        Expr::Spawn {
            actor_type, init, ..
        } => {
            free_vars(actor_type, bound, acc);
            for (_, e) in init {
                free_vars(e, bound, acc);
            }
        }
        Expr::Send { actor, args, .. } => {
            free_vars(actor, bound, acc);
            for arg in args {
                free_vars(arg, bound, acc);
            }
        }
        Expr::Ask { actor, args, .. } => {
            free_vars(actor, bound, acc);
            for arg in args {
                free_vars(arg, bound, acc);
            }
        }
        Expr::Receive { arms, .. } => {
            for (_, params, body_expr) in arms {
                let mut arm_bound = bound.clone();
                for p in params {
                    if !arm_bound.contains(p) {
                        arm_bound.push(p.clone());
                    }
                }
                free_vars(body_expr, &mut arm_bound, acc);
            }
        }
        Expr::SelfRef(_) => {}
        Expr::Perform { args, .. } => {
            for arg in args {
                free_vars(arg, bound, acc);
            }
        }
        Expr::Emit { args, .. } => {
            for arg in args {
                free_vars(arg, bound, acc);
            }
        }
        Expr::Handle { body, handlers, .. } => {
            free_vars(body, bound, acc);
            for h in handlers {
                let mut h_bound = bound.clone();
                for p in &h.params {
                    if !h_bound.contains(p) {
                        h_bound.push(p.clone());
                    }
                }
                free_vars(&h.body, &mut h_bound, acc);
            }
        }
        Expr::Migrate { actor, node, .. } => {
            free_vars(actor, bound, acc);
            free_vars(node, bound, acc);
        }
        Expr::CapAnnotate { expr: e, .. } => {
            free_vars(e, bound, acc);
        }
        Expr::TypeAnnotate { expr: e, .. } => {
            free_vars(e, bound, acc);
        }
        Expr::Pipe { left, right, .. } => {
            free_vars(left, bound, acc);
            free_vars(right, bound, acc);
        }
        Expr::For {
            var,
            iterable,
            body,
            ..
        } => {
            free_vars(iterable, bound, acc);
            let mut body_bound = bound.clone();
            if !body_bound.contains(var) {
                body_bound.push(var.clone());
            }
            free_vars(body, &mut body_bound, acc);
        }
        Expr::Return(Some(e), _) => {
            free_vars(e, bound, acc);
        }
        Expr::Return(None, _) => {}
        Expr::Break(_) => {}
    }
}

/// Add all variables bound by a pattern to the `bound` accumulator.
fn pat_bound_vars(pat: &Pattern, bound: &mut Vec<String>) {
    match pat {
        Pattern::Wild => {}
        Pattern::Var(name) | Pattern::Alias(name, _) => {
            if !bound.contains(name) {
                bound.push(name.clone());
            }
        }
        Pattern::Lit(_) => {}
        Pattern::Tuple(pats) => {
            for p in pats {
                pat_bound_vars(p, bound);
            }
        }
        Pattern::Record(fields) => {
            for (_, p) in fields {
                pat_bound_vars(p, bound);
            }
        }
        Pattern::Variant(_, Some(inner)) => {
            pat_bound_vars(inner, bound);
        }
        Pattern::Variant(_, None) => {}
    }
}

// ---------------------------------------------------------------------------
// Effect Context
// ---------------------------------------------------------------------------

/// Context used during effect inference.
///
/// Tracks the set of effects that are currently allowed (e.g. from a function
/// signature) as well as which handlers are installed (so that `perform`
/// operations for those effects need not appear in the final row).
#[derive(Debug, Clone)]
pub struct EffectContext {
    /// Effects that the surrounding code permits.
    pub allowed_effects: EffectRow,
    /// Effects that are currently handled by an enclosing `handle` expression.
    pub handlers: Vec<Effect>,
}

impl EffectContext {
    /// Create a new context with no allowed effects and no handlers.
    pub fn empty() -> Self {
        EffectContext {
            allowed_effects: EffectRow::empty(),
            handlers: Vec::new(),
        }
    }

    /// Create a context that allows the given effect row.
    pub fn with_allowed(allowed: EffectRow) -> Self {
        EffectContext {
            allowed_effects: allowed,
            handlers: Vec::new(),
        }
    }

    /// Extend with an additional handler (used when descending into a
    /// `handle` block).
    pub fn with_handler(&self, eff: Effect) -> Self {
        let mut ctx = self.clone();
        ctx.handlers.push(eff);
        ctx
    }
}

// ---------------------------------------------------------------------------
// Effect Checker
// ---------------------------------------------------------------------------

/// Stateful effect checker.
///
/// Accumulates error messages so that multiple violations can be reported.
pub struct EffectChecker {
    /// Accumulated diagnostics (errors + warnings).
    pub diagnostics: Vec<String>,
}

impl EffectChecker {
    /// Create a new effect checker.
    pub fn new() -> Self {
        EffectChecker {
            diagnostics: Vec::new(),
        }
    }

    /// Infer the effect row of an expression.
    ///
    /// Returns the (upper-bound) effect row describing what effects the
    /// expression may perform.
    pub fn infer_effects(&mut self, ctx: &EffectContext, expr: &Expr) -> NuResult<EffectRow> {
        match expr {
            // Literals and variables are pure.
            Expr::Literal(_, _) => Ok(EffectRow::empty()),
            Expr::Var(_, _) => Ok(EffectRow::empty()),

            // Lambda: effects are given by its annotation, or inferred from the
            // body if unannotated. If annotated, the body must not perform effects
            // beyond the annotation.
            Expr::Lambda { body, effect, .. } => {
                if let Some(ann) = effect {
                    let lambda_ctx = EffectContext::with_allowed(ann.clone());
                    self.check_effects(&lambda_ctx, body, ann)?;
                    Ok(ann.clone())
                } else {
                    self.infer_effects(ctx, body)
                }
            }

            // Application: effects of function + arguments + the implicit Call
            // effect (modelled as the function's own effect row).
            Expr::App { func, args, .. } => {
                let mut row = self.infer_effects(ctx, func)?;
                for arg in args {
                    row = effect_row_union(&row, &self.infer_effects(ctx, arg)?);
                }
                Ok(row)
            }

            // Let: effects of value + effects of body.
            Expr::Let { value, body, .. } => {
                let val_row = self.infer_effects(ctx, value)?;
                let body_row = self.infer_effects(ctx, body)?;
                Ok(effect_row_union(&val_row, &body_row))
            }

            // Let-rec: similar to let, but the binding is recursive.
            Expr::LetRec { value, body, .. } => {
                let val_row = self.infer_effects(ctx, value)?;
                let body_row = self.infer_effects(ctx, body)?;
                Ok(effect_row_union(&val_row, &body_row))
            }

            // If: union of condition, then-branch, and else-branch effects.
            Expr::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                let mut row = self.infer_effects(ctx, cond)?;
                row = effect_row_union(&row, &self.infer_effects(ctx, then_branch)?);
                if let Some(else_b) = else_branch {
                    row = effect_row_union(&row, &self.infer_effects(ctx, else_b)?);
                }
                Ok(row)
            }

            // Match: union of scrutinee and all arm effects.
            Expr::Match {
                scrutinee, arms, ..
            } => {
                let mut row = self.infer_effects(ctx, scrutinee)?;
                for (_, arm_expr) in arms {
                    row = effect_row_union(&row, &self.infer_effects(ctx, arm_expr)?);
                }
                Ok(row)
            }

            // Block: union of all sub-expression effects.
            Expr::Block { exprs, .. } => {
                let mut row = EffectRow::empty();
                for e in exprs {
                    row = effect_row_union(&row, &self.infer_effects(ctx, e)?);
                }
                Ok(row)
            }

            // Tuple: union of element effects.
            Expr::Tuple(elts, _) => {
                let mut row = EffectRow::empty();
                for e in elts {
                    row = effect_row_union(&row, &self.infer_effects(ctx, e)?);
                }
                Ok(row)
            }

            // Record: union of field effects.
            Expr::Record(fields, _) => {
                let mut row = EffectRow::empty();
                for (_, e) in fields {
                    row = effect_row_union(&row, &self.infer_effects(ctx, e)?);
                }
                Ok(row)
            }

            // Field access: effects of the base expression only.
            Expr::FieldAccess { expr: e, .. } => self.infer_effects(ctx, e),

            // Array: union of element effects.
            Expr::Array(elts, _) => {
                let mut row = EffectRow::empty();
                for e in elts {
                    row = effect_row_union(&row, &self.infer_effects(ctx, e)?);
                }
                Ok(row)
            }

            // Array index: effects of array + index expressions.
            Expr::Index { arr, idx, .. } => {
                let r1 = self.infer_effects(ctx, arr)?;
                let r2 = self.infer_effects(ctx, idx)?;
                Ok(effect_row_union(&r1, &r2))
            }

            // Binary: union of left and right.
            Expr::Binary { left, right, .. } => {
                let r1 = self.infer_effects(ctx, left)?;
                let r2 = self.infer_effects(ctx, right)?;
                Ok(effect_row_union(&r1, &r2))
            }

            // Unary: effects of the operand.
            Expr::Unary { expr: e, .. } => self.infer_effects(ctx, e),

            // Assignment: effects of target + value.
            Expr::Assign { target, value, .. } => {
                let r1 = self.infer_effects(ctx, target)?;
                let r2 = self.infer_effects(ctx, value)?;
                Ok(effect_row_union(&r1, &r2))
            }

            // Spawn: adds the Spawn effect + effects of actor type and init args.
            Expr::Spawn {
                actor_type, init, ..
            } => {
                let mut row = EffectRow::singleton(Effect::Spawn);
                row = effect_row_union(&row, &self.infer_effects(ctx, actor_type)?);
                for (_, e) in init {
                    row = effect_row_union(&row, &self.infer_effects(ctx, e)?);
                }
                Ok(row)
            }

            // Send: adds the Send effect + effects of actor and arguments.
            Expr::Send {
                actor, args, span, ..
            } => {
                let mut row = EffectRow::singleton(Effect::Send);
                row = effect_row_union(&row, &self.infer_effects(ctx, actor)?);
                for arg in args {
                    row = effect_row_union(&row, &self.infer_effects(ctx, arg)?);
                }
                // Also check that the Send capability requirement is met by
                // the actor expression (it must be sendable in some form).
                // We don't have a full type env here, so we defer to the
                // capability analyser for that.
                let _ = span;
                Ok(row)
            }

            // Ask: adds Send + Receive effects + actor and argument effects.
            Expr::Ask {
                actor, args, span, ..
            } => {
                let send_row = EffectRow::singleton(Effect::Send);
                let recv_row = EffectRow::singleton(Effect::Receive);
                let mut row = effect_row_union(&send_row, &recv_row);
                row = effect_row_union(&row, &self.infer_effects(ctx, actor)?);
                for arg in args {
                    row = effect_row_union(&row, &self.infer_effects(ctx, arg)?);
                }
                let _ = span;
                Ok(row)
            }

            // Receive: adds the Receive effect.
            Expr::Receive { arms, .. } => {
                let mut row = EffectRow::singleton(Effect::Receive);
                for (_, _, body_expr) in arms {
                    row = effect_row_union(&row, &self.infer_effects(ctx, body_expr)?);
                }
                Ok(row)
            }

            // Self reference: pure (just a variable-like read).
            Expr::SelfRef(_) => Ok(EffectRow::empty()),

            // Perform effect: adds the named effect to the row.
            Expr::Perform {
                effect,
                op,
                args,
                span,
            } => {
                let eff = parse_effect_name(effect);

                // Check whether this effect is handled by an enclosing handler.
                let is_handled = ctx.handlers.iter().any(|h| {
                    h == &eff || matches!((h, &eff), (Effect::UserDefined(a), Effect::UserDefined(b)) if a == b)
                });

                // Validate that the operation name is sensible (basic check).
                if op.is_empty() {
                    return Err(NuError::EffectError {
                        msg: format!("perform of effect '{}' has empty operation name", effect),
                        span: *span,
                    });
                }

                let mut row = if is_handled {
                    EffectRow::empty()
                } else {
                    EffectRow::singleton(eff)
                };

                // Add argument effects.
                for arg in args {
                    row = effect_row_union(&row, &self.infer_effects(ctx, arg)?);
                }

                Ok(row)
            }

            // Emit event: contributes an Event effect plus argument effects.
            Expr::Emit { args, .. } => {
                let mut row = EffectRow::singleton(Effect::Event);
                for arg in args {
                    row = effect_row_union(&row, &self.infer_effects(ctx, arg)?);
                }
                Ok(row)
            }

            // Handle: body effects minus handled effects, plus handler body effects.
            Expr::Handle {
                body,
                handlers,
                span,
            } => {
                // Compute which effects are handled.
                let mut handled_effs: Vec<Effect> = Vec::new();
                for h in handlers {
                    handled_effs.push(parse_effect_name(&h.effect_name));
                }

                // Build a context where the handled effects are registered.
                let mut inner_ctx = ctx.clone();
                for eff in &handled_effs {
                    inner_ctx.handlers.push(eff.clone());
                }

                // Infer body effects under the extended handler context.
                let mut row = self.infer_effects(&inner_ctx, body)?;

                // Remove handled effects from the resulting row.
                for eff in &handled_effs {
                    row = effect_row_diff(&row, eff);
                }

                // Add effects of each handler body.
                for h in handlers {
                    row = effect_row_union(&row, &self.infer_effects(ctx, &h.body)?);
                }

                let _ = span;
                Ok(row)
            }

            // Migrate: adds Migrate effect + actor and node effects.
            Expr::Migrate { actor, node, .. } => {
                let mut row = EffectRow::singleton(Effect::Migrate);
                row = effect_row_union(&row, &self.infer_effects(ctx, actor)?);
                row = effect_row_union(&row, &self.infer_effects(ctx, node)?);
                Ok(row)
            }

            // Capability annotation: just the inner expression's effects.
            Expr::CapAnnotate { expr: e, .. } => self.infer_effects(ctx, e),

            // Type annotation: just the inner expression's effects.
            Expr::TypeAnnotate { expr: e, .. } => self.infer_effects(ctx, e),

            // Pipe: effects of left + right.
            Expr::Pipe { left, right, .. } => {
                let r1 = self.infer_effects(ctx, left)?;
                let r2 = self.infer_effects(ctx, right)?;
                Ok(effect_row_union(&r1, &r2))
            }

            // For comprehension: effects of iterable + body.
            Expr::For {
                var,
                iterable,
                body,
                span,
            } => {
                let r1 = self.infer_effects(ctx, iterable)?;
                let r2 = self.infer_effects(ctx, body)?;
                let _ = (var, span);
                Ok(effect_row_union(&r1, &r2))
            }

            // Return: effects of the returned expression (if any).
            Expr::Return(Some(e), _) => self.infer_effects(ctx, e),
            Expr::Return(None, _) => Ok(EffectRow::empty()),

            // Break: no effects (it transfers control, doesn't perform an effect).
            Expr::Break(_) => Ok(EffectRow::empty()),
        }
    }

    /// Check that an expression's effects are subsumed by a given effect row.
    ///
    /// This infers the expression's effects and then verifies subset inclusion.
    /// On failure, a [`NuError::EffectError`] is returned.
    pub fn check_effects(
        &mut self,
        ctx: &EffectContext,
        expr: &Expr,
        allowed: &EffectRow,
    ) -> NuResult<()> {
        let inferred = self.infer_effects(ctx, expr)?;
        if !effect_row_subset(&inferred, allowed) {
            // Identify which effects are not allowed for a better error message.
            let offending: Vec<String> = inferred
                .effects()
                .iter()
                .filter(|e| !allowed.contains(e))
                .map(|e| format!("{:?}", e))
                .collect();
            let span = expr_span(expr);
            let msg = if offending.is_empty() {
                format!(
                    "effects {} are not a subset of allowed effects {}",
                    format_row(&inferred),
                    format_row(allowed)
                )
            } else {
                format!(
                    "effects {} contain disallowed effect(s): {} (allowed: {})",
                    format_row(&inferred),
                    offending.join(", "),
                    format_row(allowed)
                )
            };
            self.diagnostics.push(msg.clone());
            Err(NuError::EffectError { msg, span })
        } else {
            Ok(())
        }
    }
}

impl Default for EffectChecker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Capability Context
// ---------------------------------------------------------------------------

/// Context used during capability analysis.
///
/// Maps variable names to their reference capabilities.  The `default_cap`
/// is used when a variable is not found in the bindings (e.g. for primitives).
#[derive(Debug, Clone)]
pub struct CapContext {
    /// Explicit (name, capability) bindings in scope.
    pub bindings: Vec<(String, Capability)>,
    /// Default capability to use for unbound names (typically `Val`).
    pub default_cap: Capability,
}

impl CapContext {
    /// Create an empty context with `Val` as the default.
    pub fn new() -> Self {
        CapContext {
            bindings: Vec::new(),
            default_cap: Capability::Val,
        }
    }

    /// Look up the capability of a variable by name.
    pub fn lookup(&self, name: &str) -> Capability {
        self.bindings
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, c)| *c)
            .unwrap_or(self.default_cap)
    }

    /// Bind a new variable with the given capability, returning an extended
    /// context (non-destructive).
    pub fn with_binding(&self, name: impl Into<String>, cap: Capability) -> Self {
        let mut ctx = self.clone();
        ctx.bindings.push((name.into(), cap));
        ctx
    }

    /// Bind multiple variables at once.
    pub fn with_bindings(&self, binds: &[(String, Capability)]) -> Self {
        let mut ctx = self.clone();
        for (n, c) in binds {
            ctx.bindings.push((n.clone(), *c));
        }
        ctx
    }
}

impl Default for CapContext {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Capability Analyzer
// ---------------------------------------------------------------------------

/// Stateful capability analyzer.
pub struct CapabilityAnalyzer {
    /// Accumulated diagnostics.
    pub diagnostics: Vec<String>,
}

impl CapabilityAnalyzer {
    /// Create a new capability analyzer.
    pub fn new() -> Self {
        CapabilityAnalyzer {
            diagnostics: Vec::new(),
        }
    }

    /// Infer the capability of an expression's result.
    ///
    /// Returns the most precise capability we can determine for the value
    /// produced by the expression.
    ///
    /// This is the public entry point: it runs the analysis with a fresh
    /// linear-consumption set, so consumption state never leaks between
    /// top-level calls (the frontend reuses one analyzer across declarations).
    pub fn infer_cap(&mut self, ctx: &CapContext, expr: &Expr) -> NuResult<Capability> {
        let mut consumed = HashSet::new();
        self.infer_cap_tracked(ctx, expr, &mut consumed)
    }

    /// Mark a `LinearIso` binding as consumed, erroring if it already was.
    ///
    /// Any reference to a linear binding is a *use* that moves the value:
    /// passing it to a function, sending it to an actor, storing it in a
    /// structure, returning it, or capturing it in a closure all route
    /// through `Expr::Var` (or the closure-capture rule in the Lambda arm).
    fn consume_linear(
        &mut self,
        name: &str,
        span: Span,
        consumed: &mut HashSet<String>,
    ) -> NuResult<()> {
        if !consumed.insert(name.to_string()) {
            let msg = format!(
                "linear value `{}` used after being consumed (lineariso bindings may be used at most once)",
                name
            );
            self.diagnostics.push(msg.clone());
            return Err(NuError::CapError { msg, span });
        }
        Ok(())
    }

    /// Recursive worker for [`infer_cap`] that tracks which `LinearIso`
    /// bindings have already been consumed along the current path.
    ///
    /// Linearity rules (conservative MVP — at-most-once use):
    /// - Referencing a variable whose capability is `LinearIso` consumes the
    ///   binding; a second reference on the same path is a `CapError`.
    /// - Branches merge conservatively: a binding is consumed after an
    ///   `if`/`match`/`receive` only if *every* fall-through path consumes
    ///   it, so a use in one branch never poisons a sibling branch.
    /// - Consuming an outer linear binding inside a `for` body errors, since
    ///   the loop may iterate more than once.
    /// - A binding that is never used is NOT an error: exactly-once
    ///   (must-use on all paths) analysis is a documented follow-up.
    fn infer_cap_tracked(
        &mut self,
        ctx: &CapContext,
        expr: &Expr,
        consumed: &mut HashSet<String>,
    ) -> NuResult<Capability> {
        match expr {
            // Literals are immutable values.
            Expr::Literal(_, _) => Ok(Capability::Val),

            // Variable: look up in the capability context. Referencing a
            // linear binding consumes it.
            Expr::Var(name, span) => {
                let cap = ctx.lookup(name);
                if cap.is_linear() {
                    self.consume_linear(name, *span, consumed)?;
                }
                Ok(cap)
            }

            // Lambda: capability is the join of all captured free variables.
            // If there are no captures, it defaults to `Val` (a pure function
            // with no mutable state is immutable).
            Expr::Lambda { params, body, span, .. } => {
                let mut free = Vec::new();
                let mut bound: Vec<String> = params.iter().map(|(n, _)| n.clone()).collect();
                free_vars(body, &mut bound, &mut free);
                if free.is_empty() {
                    Ok(Capability::Val)
                } else {
                    let mut cap = ctx.lookup(&free[0]);
                    for name in &free[1..] {
                        cap = cap.join(ctx.lookup(name));
                    }
                    // Capturing a linear binding in a closure consumes it:
                    // the closure may escape or be invoked multiple times.
                    for name in &free {
                        if ctx.lookup(name).is_linear() {
                            self.consume_linear(name, *span, consumed)?;
                        }
                    }
                    Ok(cap)
                }
            }

            // Application: conservative join of function capability and all
            // argument capabilities.
            Expr::App { func, args, .. } => {
                let mut cap = self.infer_cap_tracked(ctx, func, consumed)?;
                for arg in args {
                    cap = cap.join(self.infer_cap_tracked(ctx, arg, consumed)?);
                }
                Ok(cap)
            }

            // Let: capability of the body.
            Expr::Let {
                name, value, body, ..
            } => {
                let val_cap = self.infer_cap_tracked(ctx, value, consumed)?;
                let body_ctx = ctx.with_binding(name.clone(), val_cap);
                // Shadowing: the new binding hides any outer binding of the
                // same name. Hide the outer consumption state while analyzing
                // the body, then restore it; the inner binding's own
                // consumption is scope-local and never leaks out.
                let outer_consumed = consumed.remove(name);
                let result = self.infer_cap_tracked(&body_ctx, body, consumed);
                consumed.remove(name);
                if outer_consumed {
                    consumed.insert(name.clone());
                }
                result
            }

            // Let-rec: similar to let, but recursive.
            Expr::LetRec {
                name,
                params,
                value,
                body,
                ..
            } => {
                // Recursive binding: we approximate the binding capability as
                // the join of param capabilities (or Val if no params).
                let mut rec_cap = Capability::Val;
                for (_, _) in params {
                    rec_cap = rec_cap.join(Capability::Val);
                }
                // `name` is bound in both the value and the body; apply the
                // same shadowing discipline as `let`.
                let outer_consumed = consumed.remove(name);
                let val_ctx = ctx.with_binding(name.clone(), rec_cap);
                let result = match self.infer_cap_tracked(&val_ctx, value, consumed) {
                    Ok(val_cap) => {
                        let body_ctx = ctx.with_binding(name.clone(), val_cap);
                        self.infer_cap_tracked(&body_ctx, body, consumed)
                    }
                    Err(e) => Err(e),
                };
                consumed.remove(name);
                if outer_consumed {
                    consumed.insert(name.clone());
                }
                result
            }

            // If: join of then and else capabilities.
            Expr::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                let _ = self.infer_cap_tracked(ctx, cond, consumed)?; // cond cap not part of result
                // Branch merge: analyze each branch from the same base set,
                // then keep only the bindings consumed on *every* fall-through
                // path (a use in one branch must not poison a sibling branch;
                // a missing else branch consumes nothing).
                let base = consumed.clone();
                let then_cap = self.infer_cap_tracked(ctx, then_branch, consumed)?;
                let then_set = std::mem::replace(consumed, base);
                let else_cap = match else_branch {
                    Some(else_b) => self.infer_cap_tracked(ctx, else_b, consumed)?,
                    None => then_cap,
                };
                let else_set = std::mem::take(consumed);
                *consumed = then_set.intersection(&else_set).cloned().collect();
                Ok(then_cap.join(else_cap))
            }

            // Match: join of all arm capabilities.
            Expr::Match {
                scrutinee, arms, ..
            } => {
                let _ = self.infer_cap_tracked(ctx, scrutinee, consumed)?;
                if arms.is_empty() {
                    return Ok(Capability::Tag);
                }
                // Branch merge (same rule as `if`): a binding counts as
                // consumed after the match only if every arm consumes it.
                let base = consumed.clone();
                let mut cap = Capability::Tag;
                let mut merged: Option<HashSet<String>> = None;
                for (pat, arm_expr) in arms {
                    *consumed = base.clone();
                    let mut arm_ctx = ctx.clone();
                    add_pat_bindings(pat, &mut arm_ctx, Capability::Val);
                    // Pattern-bound names shadow outer bindings inside the
                    // arm; hide (and restore) their outer consumption state.
                    let mut pat_names = Vec::new();
                    pat_binding_names(pat, &mut pat_names);
                    let saved: Vec<(String, bool)> = pat_names
                        .iter()
                        .map(|n| (n.clone(), consumed.remove(n)))
                        .collect();
                    let arm_result = self.infer_cap_tracked(&arm_ctx, arm_expr, consumed);
                    for (n, was_consumed) in saved {
                        consumed.remove(&n);
                        if was_consumed {
                            consumed.insert(n);
                        }
                    }
                    cap = cap.join(arm_result?);
                    merged = Some(match merged {
                        None => consumed.clone(),
                        Some(m) => m.intersection(consumed).cloned().collect(),
                    });
                }
                *consumed = merged.unwrap_or(base);
                Ok(cap)
            }

            // Block: capability of the last expression (or Unit/Val if empty).
            Expr::Block { exprs, .. } => {
                if exprs.is_empty() {
                    Ok(Capability::Val)
                } else {
                    let block_ctx = ctx.clone();
                    for (i, e) in exprs.iter().enumerate() {
                        if i == exprs.len() - 1 {
                            return self.infer_cap_tracked(&block_ctx, e, consumed);
                        }
                        // Intermediate expressions may bind variables.
                        // We don't track those for now; just infer.
                        let _ = self.infer_cap_tracked(&block_ctx, e, consumed)?;
                    }
                    Ok(Capability::Val)
                }
            }

            // Tuple: join of element capabilities.
            Expr::Tuple(elts, _) => {
                let mut cap = Capability::Val;
                for e in elts {
                    cap = cap.join(self.infer_cap_tracked(ctx, e, consumed)?);
                }
                Ok(cap)
            }

            // Record: join of field capabilities.
            Expr::Record(fields, _) => {
                let mut cap = Capability::Val;
                for (_, e) in fields {
                    cap = cap.join(self.infer_cap_tracked(ctx, e, consumed)?);
                }
                Ok(cap)
            }

            // Field access: same capability as the base expression.
            Expr::FieldAccess { expr: e, .. } => self.infer_cap_tracked(ctx, e, consumed),

            // Array: join of element capabilities.
            Expr::Array(elts, _) => {
                let mut cap = Capability::Val;
                for e in elts {
                    cap = cap.join(self.infer_cap_tracked(ctx, e, consumed)?);
                }
                Ok(cap)
            }

            // Index: same capability as the array.
            Expr::Index { arr, .. } => self.infer_cap_tracked(ctx, arr, consumed),

            // Binary: join of operand capabilities.
            Expr::Binary { left, right, .. } => {
                let c1 = self.infer_cap_tracked(ctx, left, consumed)?;
                let c2 = self.infer_cap_tracked(ctx, right, consumed)?;
                Ok(c1.join(c2))
            }

            // Unary: for Ref(cap), the result has the specified capability;
            // otherwise same as operand.
            Expr::Unary { op, expr: e, .. } => {
                match op {
                    UnOp::Ref(cap) => {
                        let inner = self.infer_cap_tracked(ctx, e, consumed)?;
                        // Reference creation: the ref itself has the requested
                        // capability; we return that.  The inner expression
                        // capability is checked separately.
                        let _ = inner;
                        Ok(*cap)
                    }
                    _ => self.infer_cap_tracked(ctx, e, consumed),
                }
            }

            // Assignment: returns Unit, which is Val.
            Expr::Assign { target, value, .. } => {
                let _ = self.infer_cap_tracked(ctx, target, consumed)?;
                let _ = self.infer_cap_tracked(ctx, value, consumed)?;
                Ok(Capability::Val)
            }

            // Spawn: newly created actors are unique (Iso).
            Expr::Spawn {
                actor_type, init, ..
            } => {
                let _ = self.infer_cap_tracked(ctx, actor_type, consumed)?;
                for (_, e) in init {
                    let _ = self.infer_cap_tracked(ctx, e, consumed)?;
                }
                Ok(Capability::Iso)
            }

            // Send: returns Unit (Val).  The arguments must be sendable
            // (checked separately by check_sendable).  Passing a linear
            // value as a send argument consumes it (the spec'd linear move).
            Expr::Send { actor, args, .. } => {
                let _ = self.infer_cap_tracked(ctx, actor, consumed)?;
                for arg in args {
                    let arg_cap = self.infer_cap_tracked(ctx, arg, consumed)?;
                    // The argument to send must be sendable.
                    if !arg_cap.is_sendable() {
                        let span = expr_span(arg);
                        self.diagnostics.push(format!(
                            "send argument with capability {:?} is not sendable",
                            arg_cap
                        ));
                        return Err(NuError::CapError {
                            msg: format!(
                                "send argument must be sendable (iso, val, or tag), got {:?}",
                                arg_cap
                            ),
                            span,
                        });
                    }
                }
                Ok(Capability::Val)
            }

            // Ask: the result capability depends on what the actor returns.
            // Without type info we approximate conservatively as the join of
            // actor capability and argument capabilities.
            Expr::Ask { actor, args, .. } => {
                let mut cap = self.infer_cap_tracked(ctx, actor, consumed)?;
                for arg in args {
                    cap = cap.join(self.infer_cap_tracked(ctx, arg, consumed)?);
                }
                Ok(cap)
            }

            // Receive: the capability of a receive block is the join of all
            // arm capabilities.
            Expr::Receive { arms, .. } => {
                if arms.is_empty() {
                    return Ok(Capability::Tag);
                }
                // Branch merge (same rule as `match`): consumed-after only if
                // every arm consumes the binding.
                let base = consumed.clone();
                let mut cap = Capability::Tag;
                let mut merged: Option<HashSet<String>> = None;
                for (_, params, body_expr) in arms {
                    *consumed = base.clone();
                    let mut arm_ctx = ctx.clone();
                    // Arm parameters shadow outer bindings; hide (and restore)
                    // their outer consumption state.
                    let mut saved = Vec::new();
                    for p in params {
                        arm_ctx = arm_ctx.with_binding(p.clone(), Capability::Val);
                        saved.push((p.clone(), consumed.remove(p)));
                    }
                    let arm_result = self.infer_cap_tracked(&arm_ctx, body_expr, consumed);
                    for (n, was_consumed) in saved {
                        consumed.remove(&n);
                        if was_consumed {
                            consumed.insert(n);
                        }
                    }
                    cap = cap.join(arm_result?);
                    merged = Some(match merged {
                        None => consumed.clone(),
                        Some(m) => m.intersection(consumed).cloned().collect(),
                    });
                }
                *consumed = merged.unwrap_or(base);
                Ok(cap)
            }

            // Self reference within an actor.
            Expr::SelfRef(_) => Ok(Capability::Ref),

            // Perform effect: capability depends on what the operation returns.
            // Without a type environment, we join the capabilities of arguments.
            Expr::Perform { args, .. } => {
                let mut cap = Capability::Val;
                for arg in args {
                    cap = cap.join(self.infer_cap_tracked(ctx, arg, consumed)?);
                }
                Ok(cap)
            }

            // Emit event: returns Unit (Val).
            Expr::Emit { args, .. } => {
                let mut cap = Capability::Val;
                for arg in args {
                    cap = cap.join(self.infer_cap_tracked(ctx, arg, consumed)?);
                }
                Ok(cap)
            }

            // Handle: capability of the body (handlers don't change the value
            // capability, only the effect row).
            Expr::Handle { body, .. } => self.infer_cap_tracked(ctx, body, consumed),

            // Migrate: returns Unit (Val).
            Expr::Migrate { actor, node, .. } => {
                let _ = self.infer_cap_tracked(ctx, actor, consumed)?;
                let _ = self.infer_cap_tracked(ctx, node, consumed)?;
                Ok(Capability::Val)
            }

            // Explicit capability annotation.
            Expr::CapAnnotate { expr: inner, cap, span } => {
                let inner_cap = self.infer_cap_tracked(ctx, inner, consumed)?;
                // Re-annotating a linear value as an aliasable capability
                // would duplicate the unique value; only `lineariso`
                // (identity) and `iso` (discharging the linear obligation)
                // are permitted targets.
                if inner_cap.is_linear() && !matches!(cap, Capability::LinearIso | Capability::Iso)
                {
                    let msg = format!(
                        "cannot downgrade linear capability LinearIso to {:?}",
                        cap
                    );
                    self.diagnostics.push(msg.clone());
                    return Err(NuError::CapError { msg, span: *span });
                }
                Ok(*cap)
            }

            // Type annotation: capability of the inner expression.
            Expr::TypeAnnotate { expr: e, .. } => self.infer_cap_tracked(ctx, e, consumed),

            // Pipe: capability of the right-hand side applied to the left.
            Expr::Pipe { left, right, .. } => {
                let _ = self.infer_cap_tracked(ctx, left, consumed)?;
                self.infer_cap_tracked(ctx, right, consumed)
            }

            // For comprehension: capability of the body.
            Expr::For {
                var,
                iterable,
                body,
                span,
            } => {
                let _ = self.infer_cap_tracked(ctx, iterable, consumed)?;
                let body_ctx = ctx.with_binding(var.clone(), Capability::Val);
                let base = consumed.clone();
                // The loop variable shadows any outer binding of the same name.
                let outer_var = consumed.remove(var);
                let body_result = self.infer_cap_tracked(&body_ctx, body, consumed);
                consumed.remove(var);
                if outer_var {
                    consumed.insert(var.clone());
                }
                let body_cap = body_result?;
                // A loop body may execute more than once, so consuming an
                // outer linear binding inside the body could use it multiple
                // times along a single path — reject it outright.
                if let Some(name) = consumed.difference(&base).next() {
                    let name = name.clone();
                    let msg = format!(
                        "linear value `{}` consumed in loop body may be used more than once",
                        name
                    );
                    self.diagnostics.push(msg.clone());
                    return Err(NuError::CapError { msg, span: *span });
                }
                // The loop may also execute zero times, so body consumption
                // does not propagate past the loop.
                *consumed = base;
                Ok(body_cap)
            }

            // Return: capability of returned value.
            Expr::Return(Some(e), _) => self.infer_cap_tracked(ctx, e, consumed),
            Expr::Return(None, _) => Ok(Capability::Val),

            // Break: never returns a value, use Tag.
            Expr::Break(_) => Ok(Capability::Tag),
        }
    }

    /// Check that a capability is a subtype of another.
    ///
    /// Returns `Ok(())` if `sub <: sup`, otherwise emits a [`NuError::CapError`].
    pub fn check_cap_sub(&mut self, sub: Capability, sup: Capability, span: Span) -> NuResult<()> {
        if sub.is_subtype_of(sup) {
            Ok(())
        } else {
            let msg = format!("capability {:?} is not a subtype of {:?}", sub, sup);
            self.diagnostics.push(msg.clone());
            Err(NuError::CapError { msg, span })
        }
    }

    /// Check that a capability is sendable (can cross an actor boundary).
    ///
    /// Sendable capabilities are `Iso`, `Val`, and `Tag`.
    pub fn check_sendable(&mut self, cap: Capability, span: Span) -> NuResult<()> {
        if cap.is_sendable() {
            Ok(())
        } else {
            let msg = format!(
                "capability {:?} is not sendable (must be iso, val, or tag)",
                cap
            );
            self.diagnostics.push(msg.clone());
            Err(NuError::CapError { msg, span })
        }
    }

    /// Check sendability of an expression's result.
    ///
    /// Infers the expression's capability and then checks that it is sendable.
    pub fn check_expr_sendable(&mut self, ctx: &CapContext, expr: &Expr) -> NuResult<()> {
        let cap = self.infer_cap(ctx, expr)?;
        let span = expr_span(expr);
        self.check_sendable(cap, span)
    }
}

impl Default for CapabilityAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the source span from any expression node.
fn expr_span(expr: &Expr) -> Span {
    match expr {
        Expr::Literal(_, s) => *s,
        Expr::Var(_, s) => *s,
        Expr::Lambda { span, .. } => *span,
        Expr::App { span, .. } => *span,
        Expr::Let { span, .. } => *span,
        Expr::LetRec { span, .. } => *span,
        Expr::If { span, .. } => *span,
        Expr::Match { span, .. } => *span,
        Expr::Block { span, .. } => *span,
        Expr::Tuple(_, s) => *s,
        Expr::Record(_, s) => *s,
        Expr::FieldAccess { span, .. } => *span,
        Expr::Array(_, s) => *s,
        Expr::Index { span, .. } => *span,
        Expr::Binary { span, .. } => *span,
        Expr::Unary { span, .. } => *span,
        Expr::Assign { span, .. } => *span,
        Expr::Spawn { span, .. } => *span,
        Expr::Send { span, .. } => *span,
        Expr::Ask { span, .. } => *span,
        Expr::Receive { span, .. } => *span,
        Expr::SelfRef(s) => *s,
        Expr::Emit { span, .. } => *span,
        Expr::Perform { span, .. } => *span,
        Expr::Handle { span, .. } => *span,
        Expr::Migrate { span, .. } => *span,
        Expr::CapAnnotate { span, .. } => *span,
        Expr::TypeAnnotate { span, .. } => *span,
        Expr::Pipe { span, .. } => *span,
        Expr::For { span, .. } => *span,
        Expr::Return(_, s) => *s,
        Expr::Break(s) => *s,
    }
}

/// Format an effect row for diagnostic messages.
fn format_row(row: &EffectRow) -> String {
    let effs: Vec<String> = row.effects().iter().map(|e| format!("{:?}", e)).collect();
    match row {
        EffectRow::Closed(_) => format!("{{{}}}", effs.join(", ")),
        EffectRow::Open(_, r) => format!("{{{}, |p{}}}", effs.join(", "), r.0),
    }
}

/// Add pattern-bound variables to the capability context with a given
/// default capability.
fn add_pat_bindings(pat: &Pattern, ctx: &mut CapContext, cap: Capability) {
    match pat {
        Pattern::Wild => {}
        Pattern::Var(name) | Pattern::Alias(name, _) => {
            ctx.bindings.push((name.clone(), cap));
        }
        Pattern::Lit(_) => {}
        Pattern::Tuple(pats) => {
            for p in pats {
                add_pat_bindings(p, ctx, cap);
            }
        }
        Pattern::Record(fields) => {
            for (_, p) in fields {
                add_pat_bindings(p, ctx, cap);
            }
        }
        Pattern::Variant(_, Some(inner)) => {
            add_pat_bindings(inner, ctx, cap);
        }
        Pattern::Variant(_, None) => {}
    }
}

/// Collect the variable names bound by a pattern (for scope/shadowing
/// bookkeeping in the linear-consumption tracker).
fn pat_binding_names(pat: &Pattern, acc: &mut Vec<String>) {
    match pat {
        Pattern::Wild | Pattern::Lit(_) => {}
        Pattern::Var(name) | Pattern::Alias(name, _) => acc.push(name.clone()),
        Pattern::Tuple(pats) => {
            for p in pats {
                pat_binding_names(p, acc);
            }
        }
        Pattern::Record(fields) => {
            for (_, p) in fields {
                pat_binding_names(p, acc);
            }
        }
        Pattern::Variant(_, Some(inner)) => pat_binding_names(inner, acc),
        Pattern::Variant(_, None) => {}
    }
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to create a zero span.
    fn s() -> Span {
        Span::default()
    }

    // -----------------------------------------------------------------------
    // Effect row operation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_effect_row_subset_closed() {
        let a = EffectRow::Closed(vec![Effect::IO, Effect::FS]);
        let b = EffectRow::Closed(vec![Effect::IO, Effect::FS, Effect::Net]);
        assert!(effect_row_subset(&a, &b)); // {IO, FS} ⊆ {IO, FS, Net}
        assert!(!effect_row_subset(&b, &a)); // {IO, FS, Net} ⊄ {IO, FS}
    }

    #[test]
    fn test_effect_row_subset_empty() {
        let empty = EffectRow::empty();
        let row = EffectRow::Closed(vec![Effect::IO]);
        assert!(effect_row_subset(&empty, &row)); // {} ⊆ {IO}
        assert!(effect_row_subset(&empty, &empty)); // {} ⊆ {}
        assert!(!effect_row_subset(&row, &empty)); // {IO} ⊄ {}
    }

    #[test]
    fn test_effect_row_subset_open() {
        let closed = EffectRow::Closed(vec![Effect::IO]);
        let open = EffectRow::Open(vec![Effect::IO], Region::fresh());
        assert!(effect_row_subset(&closed, &open));
    }

    #[test]
    fn test_effect_row_union() {
        let a = EffectRow::Closed(vec![Effect::IO]);
        let b = EffectRow::Closed(vec![Effect::FS]);
        let u = effect_row_union(&a, &b);
        assert!(u.contains(&Effect::IO));
        assert!(u.contains(&Effect::FS));
    }

    #[test]
    fn test_effect_row_union_dedup() {
        let a = EffectRow::Closed(vec![Effect::IO, Effect::FS]);
        let b = EffectRow::Closed(vec![Effect::FS, Effect::Net]);
        let u = effect_row_union(&a, &b);
        // Both IO and FS and Net should be present.
        assert!(u.contains(&Effect::IO));
        assert!(u.contains(&Effect::FS));
        assert!(u.contains(&Effect::Net));
    }

    #[test]
    fn test_effect_row_diff() {
        let row = EffectRow::Closed(vec![Effect::IO, Effect::FS, Effect::Net]);
        let diff = effect_row_diff(&row, &Effect::FS);
        assert!(diff.contains(&Effect::IO));
        assert!(!diff.contains(&Effect::FS));
        assert!(diff.contains(&Effect::Net));
    }

    #[test]
    fn test_effect_row_diff_open() {
        let row = EffectRow::Open(vec![Effect::IO, Effect::FS], Region::fresh());
        let diff = effect_row_diff(&row, &Effect::FS);
        assert!(diff.contains(&Effect::IO));
        assert!(!diff.contains(&Effect::FS));
    }

    // -----------------------------------------------------------------------
    // Effect parsing
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_effect_name_builtin() {
        assert_eq!(parse_effect_name("IO"), Effect::IO);
        assert_eq!(parse_effect_name("Net"), Effect::Net);
        assert_eq!(parse_effect_name("FS"), Effect::FS);
        assert_eq!(parse_effect_name("Spawn"), Effect::Spawn);
        assert_eq!(parse_effect_name("Async"), Effect::Async);
        assert_eq!(parse_effect_name("LLM"), Effect::LLM);
    }

    #[test]
    fn test_parse_effect_name_user_defined() {
        assert_eq!(
            parse_effect_name("MyEffect"),
            Effect::UserDefined("MyEffect".to_string())
        );
    }

    #[test]
    fn test_parse_effect_name_ffi() {
        assert_eq!(parse_effect_name("FFI"), Effect::FFI);
    }

    // -----------------------------------------------------------------------
    // Effect inference tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_infer_literal_is_pure() {
        let mut checker = EffectChecker::new();
        let ctx = EffectContext::empty();
        let lit = Expr::Literal(Literal::Int(42), s());
        let row = checker.infer_effects(&ctx, &lit).unwrap();
        assert_eq!(row, EffectRow::empty());
    }

    #[test]
    fn test_infer_var_is_pure() {
        let mut checker = EffectChecker::new();
        let ctx = EffectContext::empty();
        let var = Expr::Var("x".to_string(), s());
        let row = checker.infer_effects(&ctx, &var).unwrap();
        assert_eq!(row, EffectRow::empty());
    }

    #[test]
    fn test_infer_lambda_is_pure() {
        let mut checker = EffectChecker::new();
        let ctx = EffectContext::empty();
        let lam = Expr::Lambda {
            params: vec![("x".to_string(), None)],
            body: Box::new(Expr::Var("x".to_string(), s())),
            effect: None,
            span: s(),
        };
        let row = checker.infer_effects(&ctx, &lam).unwrap();
        assert_eq!(row, EffectRow::empty());
    }

    #[test]
    fn test_infer_perform_io() {
        let mut checker = EffectChecker::new();
        let ctx = EffectContext::empty();
        let perform = Expr::Perform {
            effect: "IO".to_string(),
            op: "print".to_string(),
            args: vec![Expr::Literal(Literal::String("hello".to_string()), s())],
            span: s(),
        };
        let row = checker.infer_effects(&ctx, &perform).unwrap();
        assert!(row.contains(&Effect::IO));
        assert!(!row.contains(&Effect::FS));
    }

    #[test]
    fn test_infer_spawn_effect() {
        let mut checker = EffectChecker::new();
        let ctx = EffectContext::empty();
        let spawn = Expr::Spawn {
            actor_type: Box::new(Expr::Var("MyActor".to_string(), s())),
            init: vec![],
            span: s(),
        };
        let row = checker.infer_effects(&ctx, &spawn).unwrap();
        assert!(row.contains(&Effect::Spawn));
    }

    #[test]
    fn test_infer_send_effect() {
        let mut checker = EffectChecker::new();
        let ctx = EffectContext::empty();
        let send = Expr::Send {
            actor: Box::new(Expr::Var("a".to_string(), s())),
            behavior: "foo".to_string(),
            args: vec![Expr::Literal(Literal::Int(1), s())],
            span: s(),
        };
        let row = checker.infer_effects(&ctx, &send).unwrap();
        assert!(row.contains(&Effect::Send));
    }

    #[test]
    fn test_infer_ask_effect() {
        let mut checker = EffectChecker::new();
        let ctx = EffectContext::empty();
        let ask = Expr::Ask {
            actor: Box::new(Expr::Var("a".to_string(), s())),
            behavior: "foo".to_string(),
            args: vec![],
            span: s(),
        };
        let row = checker.infer_effects(&ctx, &ask).unwrap();
        assert!(row.contains(&Effect::Send));
        assert!(row.contains(&Effect::Receive));
    }

    #[test]
    fn test_infer_let_combines_effects() {
        let mut checker = EffectChecker::new();
        let ctx = EffectContext::empty();
        let let_expr = Expr::Let {
            name: "x".to_string(),
            ty: None,
            value: Box::new(Expr::Perform {
                effect: "FS".to_string(),
                op: "read".to_string(),
                args: vec![],
                span: s(),
            }),
            body: Box::new(Expr::Perform {
                effect: "Net".to_string(),
                op: "get".to_string(),
                args: vec![],
                span: s(),
            }),
            span: s(),
        };
        let row = checker.infer_effects(&ctx, &let_expr).unwrap();
        assert!(row.contains(&Effect::FS));
        assert!(row.contains(&Effect::Net));
    }

    #[test]
    fn test_infer_if_combines_effects() {
        let mut checker = EffectChecker::new();
        let ctx = EffectContext::empty();
        let if_expr = Expr::If {
            cond: Box::new(Expr::Literal(Literal::Bool(true), s())),
            then_branch: Box::new(Expr::Perform {
                effect: "IO".to_string(),
                op: "print".to_string(),
                args: vec![],
                span: s(),
            }),
            else_branch: Some(Box::new(Expr::Perform {
                effect: "FS".to_string(),
                op: "read".to_string(),
                args: vec![],
                span: s(),
            })),
            span: s(),
        };
        let row = checker.infer_effects(&ctx, &if_expr).unwrap();
        assert!(row.contains(&Effect::IO));
        assert!(row.contains(&Effect::FS));
    }

    #[test]
    fn test_infer_handle_removes_effect() {
        let mut checker = EffectChecker::new();
        let ctx = EffectContext::empty();
        let handle_expr = Expr::Handle {
            body: Box::new(Expr::Perform {
                effect: "IO".to_string(),
                op: "print".to_string(),
                args: vec![Expr::Literal(Literal::String("hi".to_string()), s())],
                span: s(),
            }),
            handlers: vec![EffectHandler {
                effect_name: "IO".to_string(),
                op_name: "print".to_string(),
                params: vec!["msg".to_string()],
                body: Expr::Literal(Literal::Unit, s()),
                resume: false,
            }],
            span: s(),
        };
        let row = checker.infer_effects(&ctx, &handle_expr).unwrap();
        // The IO effect should be handled (removed from the body row).
        assert!(!row.contains(&Effect::IO));
    }

    #[test]
    fn test_check_effects_passes() {
        let mut checker = EffectChecker::new();
        let ctx = EffectContext::with_allowed(EffectRow::Closed(vec![Effect::IO, Effect::FS]));
        let expr = Expr::Perform {
            effect: "IO".to_string(),
            op: "print".to_string(),
            args: vec![],
            span: s(),
        };
        assert!(checker
            .check_effects(&ctx, &expr, &ctx.allowed_effects)
            .is_ok());
    }

    #[test]
    fn test_check_effects_fails() {
        let mut checker = EffectChecker::new();
        let ctx = EffectContext::with_allowed(EffectRow::Closed(vec![Effect::IO]));
        let expr = Expr::Perform {
            effect: "FS".to_string(),
            op: "read".to_string(),
            args: vec![],
            span: Span::new(0, 10, 1, 1),
        };
        let result = checker.check_effects(&ctx, &expr, &ctx.allowed_effects);
        assert!(result.is_err());
        match result.unwrap_err() {
            NuError::EffectError { msg, .. } => {
                assert!(
                    msg.contains("FS"),
                    "error message should mention FS: {}",
                    msg
                );
            }
            other => panic!("expected EffectError, got {:?}", other),
        }
    }

    #[test]
    fn test_perform_empty_op_name_errors() {
        let mut checker = EffectChecker::new();
        let ctx = EffectContext::empty();
        let perform = Expr::Perform {
            effect: "IO".to_string(),
            op: "".to_string(),
            args: vec![],
            span: s(),
        };
        let result = checker.infer_effects(&ctx, &perform);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Capability analysis tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_cap_literal_is_val() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new();
        let lit = Expr::Literal(Literal::Int(42), s());
        let cap = analyzer.infer_cap(&ctx, &lit).unwrap();
        assert_eq!(cap, Capability::Val);
    }

    #[test]
    fn test_cap_var_lookup() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new().with_binding("x", Capability::Iso);
        let var = Expr::Var("x".to_string(), s());
        let cap = analyzer.infer_cap(&ctx, &var).unwrap();
        assert_eq!(cap, Capability::Iso);
    }

    #[test]
    fn test_cap_var_default() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new();
        let var = Expr::Var("unknown".to_string(), s());
        let cap = analyzer.infer_cap(&ctx, &var).unwrap();
        assert_eq!(cap, Capability::Val); // default
    }

    #[test]
    fn test_cap_lambda_no_captures() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new();
        let lam = Expr::Lambda {
            params: vec![("x".to_string(), None)],
            body: Box::new(Expr::Var("x".to_string(), s())),
            effect: None,
            span: s(),
        };
        let cap = analyzer.infer_cap(&ctx, &lam).unwrap();
        assert_eq!(cap, Capability::Val);
    }

    #[test]
    fn test_cap_lambda_with_capture() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new().with_binding("y", Capability::Ref);
        let lam = Expr::Lambda {
            params: vec![("x".to_string(), None)],
            body: Box::new(Expr::Binary {
                op: BinOp::Add,
                left: Box::new(Expr::Var("x".to_string(), s())),
                right: Box::new(Expr::Var("y".to_string(), s())),
                span: s(),
            }),
            effect: None,
            span: s(),
        };
        let cap = analyzer.infer_cap(&ctx, &lam).unwrap();
        assert_eq!(cap, Capability::Ref);
    }

    #[test]
    fn test_cap_spawn_is_iso() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new();
        let spawn = Expr::Spawn {
            actor_type: Box::new(Expr::Var("A".to_string(), s())),
            init: vec![],
            span: s(),
        };
        let cap = analyzer.infer_cap(&ctx, &spawn).unwrap();
        assert_eq!(cap, Capability::Iso);
    }

    #[test]
    fn test_cap_annotate() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new();
        let expr = Expr::CapAnnotate {
            expr: Box::new(Expr::Literal(Literal::Int(1), s())),
            cap: Capability::Iso,
            span: s(),
        };
        let cap = analyzer.infer_cap(&ctx, &expr).unwrap();
        assert_eq!(cap, Capability::Iso);
    }

    #[test]
    fn test_check_cap_sub_passes() {
        let mut analyzer = CapabilityAnalyzer::new();
        // Val <: Box (val can be read as box)
        assert!(analyzer
            .check_cap_sub(Capability::Val, Capability::Box, s())
            .is_ok());
        // Tag <: Iso (tag is bottom of the lattice)
        assert!(analyzer
            .check_cap_sub(Capability::Tag, Capability::Iso, s())
            .is_ok());
        // Ref <: Box (ref can be read as box)
        assert!(analyzer
            .check_cap_sub(Capability::Ref, Capability::Box, s())
            .is_ok());
    }

    #[test]
    fn test_check_cap_sub_fails() {
        let mut analyzer = CapabilityAnalyzer::new();
        let result = analyzer.check_cap_sub(Capability::Ref, Capability::Val, s());
        assert!(result.is_err());
    }

    #[test]
    fn test_check_sendable_passes() {
        let mut analyzer = CapabilityAnalyzer::new();
        assert!(analyzer.check_sendable(Capability::Iso, s()).is_ok());
        assert!(analyzer.check_sendable(Capability::Val, s()).is_ok());
        assert!(analyzer.check_sendable(Capability::Tag, s()).is_ok());
    }

    #[test]
    fn test_check_sendable_fails() {
        let mut analyzer = CapabilityAnalyzer::new();
        assert!(analyzer.check_sendable(Capability::Ref, s()).is_err());
        assert!(analyzer.check_sendable(Capability::Box, s()).is_err());
    }

    #[test]
    fn test_cap_ref_creation() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new();
        let expr = Expr::Unary {
            op: UnOp::Ref(Capability::Iso),
            expr: Box::new(Expr::Literal(Literal::Int(42), s())),
            span: s(),
        };
        let cap = analyzer.infer_cap(&ctx, &expr).unwrap();
        assert_eq!(cap, Capability::Iso);
    }

    #[test]
    fn test_cap_binary_join() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new();
        // A binary expression where we just need to check the join works.
        let expr = Expr::Binary {
            op: BinOp::Add,
            left: Box::new(Expr::Literal(Literal::Int(1), s())),
            right: Box::new(Expr::Literal(Literal::Int(2), s())),
            span: s(),
        };
        let cap = analyzer.infer_cap(&ctx, &expr).unwrap();
        // Val join Val = Val
        assert_eq!(cap, Capability::Val);
    }

    #[test]
    fn test_cap_send_checks_sendable() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new().with_binding("a", Capability::Iso);
        // Send with a non-sendable argument should fail.
        let send = Expr::Send {
            actor: Box::new(Expr::Var("a".to_string(), s())),
            behavior: "foo".to_string(),
            args: vec![Expr::Var("ref_var".to_string(), s())],
            span: s(),
        };
        // ref_var defaults to Val (sendable), so it passes. Let's test with
        // a non-sendable binding.
        let ctx2 = ctx.with_binding("ref_var", Capability::Ref);
        let result = analyzer.infer_cap(&ctx2, &send);
        assert!(result.is_err(), "send with ref argument should fail");
    }

    #[test]
    fn test_cap_self_ref_is_ref() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new();
        let expr = Expr::SelfRef(s());
        let cap = analyzer.infer_cap(&ctx, &expr).unwrap();
        assert_eq!(cap, Capability::Ref);
    }

    #[test]
    fn test_cap_break_is_tag() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new();
        let expr = Expr::Break(s());
        let cap = analyzer.infer_cap(&ctx, &expr).unwrap();
        assert_eq!(cap, Capability::Tag);
    }

    #[test]
    fn test_effect_context_with_handler() {
        let ctx = EffectContext::with_allowed(EffectRow::Closed(vec![Effect::IO]));
        let ctx2 = ctx.with_handler(Effect::IO);
        assert_eq!(ctx2.handlers.len(), 1);
        assert!(ctx2.handlers.contains(&Effect::IO));
    }

    #[test]
    fn test_cap_context_lookup_and_binding() {
        let ctx = CapContext::new().with_binding("x", Capability::Iso);
        assert_eq!(ctx.lookup("x"), Capability::Iso);
        assert_eq!(ctx.lookup("unknown"), Capability::Val); // default

        let ctx2 = ctx.with_binding("y", Capability::Ref);
        assert_eq!(ctx2.lookup("y"), Capability::Ref);
        assert_eq!(ctx2.lookup("x"), Capability::Iso);
    }

    #[test]
    fn test_infer_migrate_effect() {
        let mut checker = EffectChecker::new();
        let ctx = EffectContext::empty();
        let migrate = Expr::Migrate {
            actor: Box::new(Expr::Var("a".to_string(), s())),
            node: Box::new(Expr::Literal(Literal::String("node1".to_string()), s())),
            span: s(),
        };
        let row = checker.infer_effects(&ctx, &migrate).unwrap();
        assert!(row.contains(&Effect::Migrate));
    }

    #[test]
    fn test_infer_receive_effect() {
        let mut checker = EffectChecker::new();
        let ctx = EffectContext::empty();
        let receive = Expr::Receive {
            arms: vec![(
                "Msg".to_string(),
                vec!["x".to_string()],
                Expr::Var("x".to_string(), s()),
            )],
            span: s(),
        };
        let row = checker.infer_effects(&ctx, &receive).unwrap();
        assert!(row.contains(&Effect::Receive));
    }

    #[test]
    fn test_infer_perform_user_defined() {
        let mut checker = EffectChecker::new();
        let ctx = EffectContext::empty();
        let perform = Expr::Perform {
            effect: "Logger".to_string(),
            op: "log".to_string(),
            args: vec![Expr::Literal(Literal::String("msg".to_string()), s())],
            span: s(),
        };
        let row = checker.infer_effects(&ctx, &perform).unwrap();
        assert!(row.contains(&Effect::UserDefined("Logger".to_string())));
    }

    #[test]
    fn test_infer_lambda_effect_annotation_satisfied() {
        let mut checker = EffectChecker::new();
        let ctx = EffectContext::empty();
        let lam = Expr::Lambda {
            params: vec![("x".to_string(), None)],
            body: Box::new(Expr::Perform {
                effect: "IO".to_string(),
                op: "print".to_string(),
                args: vec![Expr::Var("x".to_string(), s())],
                span: s(),
            }),
            effect: Some(EffectRow::Closed(vec![Effect::IO])),
            span: s(),
        };
        let row = checker.infer_effects(&ctx, &lam).unwrap();
        assert_eq!(row, EffectRow::Closed(vec![Effect::IO]));
    }

    #[test]
    fn test_infer_lambda_effect_annotation_violated() {
        let mut checker = EffectChecker::new();
        let ctx = EffectContext::empty();
        let lam = Expr::Lambda {
            params: vec![("x".to_string(), None)],
            body: Box::new(Expr::Perform {
                effect: "FS".to_string(),
                op: "read".to_string(),
                args: vec![],
                span: s(),
            }),
            effect: Some(EffectRow::Closed(vec![Effect::IO])),
            span: s(),
        };
        let result = checker.infer_effects(&ctx, &lam);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // LinearIso consumption tracking (at-most-once use)
    // -----------------------------------------------------------------------

    // Helpers for building linearity test expressions.
    fn lvar(name: &str) -> Expr {
        Expr::Var(name.to_string(), s())
    }

    fn call1(func: &str, arg: Expr) -> Expr {
        Expr::App {
            func: Box::new(Expr::Var(func.to_string(), s())),
            args: vec![arg],
            span: s(),
        }
    }

    fn send_m(arg: Expr) -> Expr {
        Expr::Send {
            actor: Box::new(lvar("a")),
            behavior: "m".to_string(),
            args: vec![arg],
            span: s(),
        }
    }

    #[test]
    fn test_lineariso_used_once_ok() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new().with_binding("x", Capability::LinearIso);
        let cap = analyzer.infer_cap(&ctx, &call1("f", lvar("x"))).unwrap();
        assert_eq!(cap, Capability::Val); // Val (f) join LinearIso (x)
        assert!(analyzer.diagnostics.is_empty());
    }

    #[test]
    fn test_lineariso_used_twice_errors() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new().with_binding("x", Capability::LinearIso);
        let expr = Expr::Block {
            exprs: vec![call1("f", lvar("x")), call1("g", lvar("x"))],
            span: s(),
        };
        let result = analyzer.infer_cap(&ctx, &expr);
        match result {
            Err(NuError::CapError { msg, .. }) => {
                assert!(msg.contains("x"), "error should name the binding: {}", msg);
                assert!(msg.contains("linear"), "error should mention linearity: {}", msg);
            }
            other => panic!("expected CapError, got {:?}", other),
        }
        assert!(!analyzer.diagnostics.is_empty());
    }

    #[test]
    fn test_lineariso_never_used_ok() {
        // At-most-once MVP: an unused linear binding is NOT an error.
        // Exactly-once (must-use on all paths) analysis is a follow-up.
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new().with_binding("x", Capability::LinearIso);
        let expr = Expr::Literal(Literal::Int(1), s());
        assert!(analyzer.infer_cap(&ctx, &expr).is_ok());
    }

    #[test]
    fn test_lineariso_consumed_on_both_branches_then_used_errors() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new().with_binding("x", Capability::LinearIso);
        // if c then f(x) else g(x); h(x) — both branches consume x, so the
        // binding is consumed after the if and the later use must fail.
        let expr = Expr::Block {
            exprs: vec![
                Expr::If {
                    cond: Box::new(Expr::Literal(Literal::Bool(true), s())),
                    then_branch: Box::new(call1("f", lvar("x"))),
                    else_branch: Some(Box::new(call1("g", lvar("x")))),
                    span: s(),
                },
                call1("h", lvar("x")),
            ],
            span: s(),
        };
        assert!(analyzer.infer_cap(&ctx, &expr).is_err());
    }

    #[test]
    fn test_lineariso_consumed_on_one_branch_then_used_ok() {
        // Conservative merge: a binding is consumed after an if only if ALL
        // fall-through paths consume it. The else branch here does not, so
        // the later use is fine.
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new().with_binding("x", Capability::LinearIso);
        let expr = Expr::Block {
            exprs: vec![
                Expr::If {
                    cond: Box::new(Expr::Literal(Literal::Bool(true), s())),
                    then_branch: Box::new(call1("f", lvar("x"))),
                    else_branch: Some(Box::new(Expr::Literal(Literal::Int(0), s()))),
                    span: s(),
                },
                call1("h", lvar("x")),
            ],
            span: s(),
        };
        assert!(analyzer.infer_cap(&ctx, &expr).is_ok());
    }

    #[test]
    fn test_lineariso_sent_once_ok() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new()
            .with_binding("a", Capability::Iso)
            .with_binding("x", Capability::LinearIso);
        let cap = analyzer.infer_cap(&ctx, &send_m(lvar("x"))).unwrap();
        assert_eq!(cap, Capability::Val);
    }

    #[test]
    fn test_lineariso_sent_twice_errors() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new()
            .with_binding("a", Capability::Iso)
            .with_binding("x", Capability::LinearIso);
        // Sending a linear value consumes it (the spec'd linear move), so the
        // second send of the same binding must fail.
        let expr = Expr::Block {
            exprs: vec![send_m(lvar("x")), send_m(lvar("x"))],
            span: s(),
        };
        assert!(analyzer.infer_cap(&ctx, &expr).is_err());
    }

    #[test]
    fn test_lineariso_downgrade_to_ref_errors() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new().with_binding("x", Capability::LinearIso);
        let expr = Expr::CapAnnotate {
            expr: Box::new(lvar("x")),
            cap: Capability::Ref,
            span: s(),
        };
        let result = analyzer.infer_cap(&ctx, &expr);
        match result {
            Err(NuError::CapError { msg, .. }) => {
                assert!(msg.contains("downgrade"), "unexpected message: {}", msg);
            }
            other => panic!("expected CapError, got {:?}", other),
        }
    }

    #[test]
    fn test_lineariso_promote_to_iso_consumes() {
        let ctx = CapContext::new().with_binding("x", Capability::LinearIso);
        let promote = Expr::CapAnnotate {
            expr: Box::new(lvar("x")),
            cap: Capability::Iso,
            span: s(),
        };
        // Promoting lineariso to iso discharges the linear obligation.
        let mut analyzer = CapabilityAnalyzer::new();
        let cap = analyzer.infer_cap(&ctx, &promote).unwrap();
        assert_eq!(cap, Capability::Iso);
        // ...but it still consumes the binding: a later use must fail.
        let mut analyzer = CapabilityAnalyzer::new();
        let expr = Expr::Block {
            exprs: vec![promote, call1("f", lvar("x"))],
            span: s(),
        };
        assert!(analyzer.infer_cap(&ctx, &expr).is_err());
    }

    #[test]
    fn test_lineariso_captured_by_closure_consumes() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new().with_binding("x", Capability::LinearIso);
        let lam = Expr::Lambda {
            params: vec![("y".to_string(), None)],
            body: Box::new(lvar("x")),
            effect: None,
            span: s(),
        };
        let expr = Expr::Block {
            exprs: vec![lam, call1("f", lvar("x"))],
            span: s(),
        };
        assert!(analyzer.infer_cap(&ctx, &expr).is_err());
    }

    #[test]
    fn test_lineariso_consumed_in_for_body_errors() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new().with_binding("x", Capability::LinearIso);
        let expr = Expr::For {
            var: "i".to_string(),
            iterable: Box::new(Expr::Array(
                vec![Expr::Literal(Literal::Int(1), s())],
                s(),
            )),
            body: Box::new(call1("f", lvar("x"))),
            span: s(),
        };
        assert!(analyzer.infer_cap(&ctx, &expr).is_err());
    }

    #[test]
    fn test_lineariso_shadowed_by_let_ok() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new().with_binding("x", Capability::LinearIso);
        // let x = 1 in { f(x); g(x) } — the inner (Val) binding shadows the
        // linear outer one, so both uses are fine and the outer binding is
        // not consumed.
        let expr = Expr::Let {
            name: "x".to_string(),
            ty: None,
            value: Box::new(Expr::Literal(Literal::Int(1), s())),
            body: Box::new(Expr::Block {
                exprs: vec![call1("f", lvar("x")), call1("g", lvar("x"))],
                span: s(),
            }),
            span: s(),
        };
        assert!(analyzer.infer_cap(&ctx, &expr).is_ok());
    }

    #[test]
    fn test_iso_used_twice_ok() {
        // Regression: non-linear capabilities are unaffected.
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new().with_binding("x", Capability::Iso);
        let expr = Expr::Block {
            exprs: vec![call1("f", lvar("x")), call1("g", lvar("x"))],
            span: s(),
        };
        assert!(analyzer.infer_cap(&ctx, &expr).is_ok());
    }
}
