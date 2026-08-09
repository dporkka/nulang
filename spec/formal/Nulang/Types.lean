/-
Nulang Formal Semantics: Type Language
========================================

Formalizes the Nulang type language, substitution, the occurs check,
and the most-general-unifier (mgu) — the core of Hindley-Milner type
inference.
-/

/- ------------------------------------------------------------------
   Type Variables
   ------------------------------------------------------------------ -/

/-- A type variable is a natural number (compare: `TypeVar(u64)` in Rust). -/
abbrev TypeVar := Nat

/- ------------------------------------------------------------------
   Type Language (simplified)
   ------------------------------------------------------------------ -/

/--
The type language.  We use `Ty` instead of `Type` to avoid shadowing
Lean's built-in `Type` universe.
-/
inductive Ty where
  | var  (v : TypeVar)
  | int
  | bool
  | string
  | unit
  | nil
  | fn   (param : Ty) (ret : Ty)
  | tuple (ts : List Ty)
  deriving BEq, Repr, Inhabited

/- ------------------------------------------------------------------
   Free Type Variables
   ------------------------------------------------------------------ -/

/-- The set of type variables occurring in a type. -/
def freeVars : Ty → List TypeVar
  | .var v    => [v]
  | .fn p r   => freeVars p ++ freeVars r
  | .tuple ts => ts.bind freeVars
  | _         => []

/- ------------------------------------------------------------------
   Substitution
   ------------------------------------------------------------------ -/

/--
A substitution is a finite map from type variables to types.
Represented as an association list.

A substitution σ is *idempotent* if ∀v. σ(v) applied to σ = σ(v).
The `mgu` function always returns an idempotent substitution.
-/
abbrev Subst := List (TypeVar × Ty)

/-- The empty substitution. -/
def substEmpty : Subst := []

/-- Apply a substitution to a type variable (lookup). -/
def substApplyVar (σ : Subst) (v : TypeVar) : Ty :=
  match σ.lookup v with
  | some t => t
  | none   => .var v

/-- Apply a substitution to a type (recursive traversal). -/
def substApply (σ : Subst) : Ty → Ty
  | .var v    => substApplyVar σ v
  | .fn p r   => .fn (substApply σ p) (substApply σ r)
  | .tuple ts => .tuple (ts.map (substApply σ))
  | t         => t  -- ground types unchanged

/-- Compose two substitutions: σ₂ ∘ σ₁ = apply σ₂ to σ₁'s codomains, then append σ₂. -/
def substCompose (σ₂ : Subst) (σ₁ : Subst) : Subst :=
  let applied := σ₁.map (λ (v, t) => (v, substApply σ₂ t))
  applied ++ σ₂

/- ------------------------------------------------------------------
   Occurs Check
   ------------------------------------------------------------------ -/

/--
`occurs v t` is true when type variable `v` appears free in type `t`.
Prevents cyclic substitutions like `'a ↦ 'a -> Int`.
-/
def occurs (v : TypeVar) : Ty → Bool
  | .var w    => v == w
  | .fn p r   => occurs v p || occurs v r
  | .tuple ts => ts.any (occurs v)
  | _         => false

/- ------------------------------------------------------------------
   Most General Unifier (MGU)
   ------------------------------------------------------------------ -/

/--
`mgu s t` computes the most general unifier of types `s` and `t`,
returning `some σ` such that `substApply σ s = substApply σ t`,
or `none` if `s` and `t` are not unifiable.

Implements Algorithm W's unification:
- var-v:       equal → empty, distinct → bind
- var-t:       occurs check → bind, fail otherwise
- ground-g:    equal → empty, distinct → fail
- fn-fn:       unify params, then unify returns, compose
- tuple-tuple: unify elementwise (same length), compose
-/
partial def mgu : Ty → Ty → Option Subst
  -- Both are type variables: bind or identity
  | .var v, .var w =>
    if v == w then some substEmpty
    else some [(v, .var w)]

  -- Variable + non-variable: occurs check then bind
  | .var v, t =>
    if occurs v t then none
    else some [(v, t)]

  -- Non-variable + variable: symmetric
  | t, .var v =>
    if occurs v t then none
    else some [(v, t)]

  -- Ground types: must be identical
  | .int,    .int    => some substEmpty
  | .bool,   .bool   => some substEmpty
  | .string, .string => some substEmpty
  | .unit,   .unit   => some substEmpty
  | .nil,    .nil    => some substEmpty

  -- Function types: unify parameter and return
  | .fn p₁ r₁, .fn p₂ r₂ =>
    match mgu p₁ p₂ with
    | none => none
    | some σ₁ =>
      match mgu (substApply σ₁ r₁) (substApply σ₁ r₂) with
      | none => none
      | some σ₂ => some (substCompose σ₂ σ₁)

  -- Tuple types: unify elementwise
  | .tuple ts₁, .tuple ts₂ =>
    if ts₁.length != ts₂.length then none
    else go ts₁ ts₂ substEmpty
  where
    go : List Ty → List Ty → Subst → Option Subst
    | [], [], σ => some σ
    | t₁::ts₁, t₂::ts₂, σ =>
      match mgu (substApply σ t₁) (substApply σ t₂) with
      | none => none
      | some σ' => go ts₁ ts₂ (substCompose σ' σ)
    | _, _, _ => none

  -- All other pairs: not unifiable
  | _, _ => none

/- ------------------------------------------------------------------
   Termination Note
   ------------------------------------------------------------------ -/

/-
`mgu` is marked `partial` because Lean cannot automatically prove
termination.  The termination argument:

1. Each recursive call reduces the *structural size* of at least one
   argument:
   - `fn p r` → recursive calls on `p` and `r` (proper subterms)
   - `tuple ts` → recursive calls on elements (proper subterms)
   - `var v, t` → terminates immediately (no recursion)

2. The occurs check ensures substituting a variable never introduces
   a cycle; without it, `mgu 'a ('a -> Int)` would substitute
   `'a ↦ 'a -> Int` and recurse indefinitely.

3. Substitution composition does not increase structural size of terms
   involved in recursive calls.

A full termination proof uses `WellFounded` recursion on the
lexicographic product of type sizes.  Planned for the machine-checked
soundness proof.
-/
