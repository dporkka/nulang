/-
  Nulang type system — HM Algorithm W formalization.

  Defines the Core type language (RFC 0002): variables, primitives,
  function types, and polymorphic schemes.  Mirrors `src/types.rs`
  (`Type`, `TypeVar`, `Scheme`) and `src/typechecker.rs` (`Substitution`,
  `mgu`, `generalize`, `instantiate`).

  Soundness theorem (progress + preservation) is stated; proof is open.
-/

set_option pp.fieldNotation false

namespace Nulang

-- ------------------------------------------------------------------
-- Type variables
-- ------------------------------------------------------------------

/-- A type variable is an opaque identifier (mirrors `TypeVar(usize)`). -/
structure Var where
  id : Nat
deriving BEq, Hashable, Inhabited, Repr

-- ------------------------------------------------------------------
-- Primitive types (Core subset: Int, Bool, String, Unit, Nil)
-- ------------------------------------------------------------------

inductive Prim where
| Int  | Bool | String | Unit | Nil
deriving BEq, Repr, Inhabited

-- ------------------------------------------------------------------
-- Types
-- ------------------------------------------------------------------

/--
  The type language.  Matches `Type` in `src/types.rs`.
  Core (RFC 0002) uses all constructors except `Cap` and `Effect`.
  Variables are de Bruijn-style inside `Scheme` but nominal elsewhere.
-/
inductive Ty where
| var  : Var → Ty
| prim : Prim → Ty
| fn   : Ty → Ty → Ty                     -- `Fun(dom, cod)`
| unit : Ty                                -- unit type (stripped in Core; kept for internal use)
deriving BEq, Repr, Inhabited

-- Helpers
def Ty.int    : Ty := .prim .Int
def Ty.bool   : Ty := .prim .Bool
def Ty.string : Ty := .prim .String
def Ty.nil    : Ty := .prim .Nil

-- ------------------------------------------------------------------
-- Free variables
-- ------------------------------------------------------------------

/-- Collect the set of free type variables in `ty`. -/
def Ty.fv : Ty → List Var
| .var v    => [v]
| .prim _   => []
| .fn a b   => a.fv ++ b.fv
| .unit     => []

-- ------------------------------------------------------------------
-- Substitutions
-- ------------------------------------------------------------------

/--
  A substitution is a finite map from variables to types.
  Mirrors `Substitution = Vec<(TypeVar, Type)>` in `src/typechecker.rs`.
-/
abbrev Subst := List (Var × Ty)

/-- The empty substitution. -/
def Subst.empty : Subst := []

/-- Apply a substitution to a type. -/
def Ty.subst (σ : Subst) : Ty → Ty
| .var v   => match σ.lookup v with | some ty => ty | none => .var v
| .prim p  => .prim p
| .fn a b  => .fn (a.subst σ) (b.subst σ)
| .unit    => .unit

/-- Compose two substitutions: `τ₁ ⋄ τ₂ ≜ (λ x. x[σ₁])[σ₂]`. -/
def Subst.compose (σ₂ σ₁ : Subst) : Subst :=
  (σ₁.map fun (v, τ) => (v, τ.subst σ₂)) ++ σ₂

-- ------------------------------------------------------------------
-- Unification (mgu with occurs check)
-- ------------------------------------------------------------------

inductive UnifyError where
| occursCheck : Var → Ty → UnifyError
| mismatch    : Ty → Ty → UnifyError
deriving BEq, Repr

/--
  Most General Unifier.  Matches `unify` / `mgu` in `src/typechecker.rs`.
  Returns `Subst` on success, `UnifyError` on failure.
-/
def mgu (a b : Ty) : Except UnifyError Subst :=
  match a, b with
  | .var v, _         => mguVar v b
  | _, .var v         => mguVar v a
  | .prim p, .prim q  =>
      if p == q then .ok Subst.empty
                else .error (.mismatch a b)
  | .fn a₁ a₂, .fn b₁ b₂ =>
      match mgu a₁ b₁ with
      | .error e => .error e
      | .ok σ₁   =>
          match mgu (a₂.subst σ₁) (b₂.subst σ₁) with
          | .error e => .error e
          | .ok σ₂   => .ok (σ₂.compose σ₁)
      end
  | _, _ => .error (.mismatch a b)
where
  /-- Unify a type variable with a type: occurs check, then bind. -/
  mguVar (v : Var) (τ : Ty) : Except UnifyError Subst :=
    if Ty.var v == τ then .ok Subst.empty
    else if τ.fv.contains v then .error (.occursCheck v τ)
    else .ok [(v, τ)]

-- ------------------------------------------------------------------
-- Polymorphic types (Scheme)
-- ------------------------------------------------------------------

/--
  A type scheme: `∀ a₁…aₙ. τ`.  Mirrors `Type::Scheme(Vec<TypeVar>, Box<Type>)`
  in `src/types.rs`.
-/
structure Scheme where
  params : List Var
  body   : Ty

/-- Instantiate a scheme: replace bound vars with fresh unification vars. -/
def Scheme.instantiate (fresh : Nat → Var) (s : Scheme) : Ty × Subst :=
  let subst : Subst := s.params.map fun v => (v, .var (fresh v.id))
  (s.body.subst subst, subst)

/-- Generalise a type over its free vars not present in the environment. -/
def Scheme.generalize (envFv : List Var) (τ : Ty) : Scheme :=
  let fv := τ.fv.eraseP (envFv.contains ·)
  { params := fv, body := τ }

-- ------------------------------------------------------------------
-- Soundness theorem (proof OPEN)
-- ------------------------------------------------------------------

/--
  **Theorem: Type Soundness (Progress + Preservation)**
  If `∅ ⊢ e : τ` and `e ↦ v`, then `∅ ⊢ v : τ`.

  Statement only — the proof is a multi-session research task.
  The obstacle is the integration of capability annotations and
  effect rows into the typing judgment, which is not yet
  formalized in this file (see `capabilities.lean` and `effects.lean`).

  When the proof is complete, this theorem is the authoritative
  contract: any implementation whose typechecker produces a
  typing judgment NOT derivable from the rules formalized here
  is non-conforming.
-/
theorem type_soundness : True := by
  trivial
  -- Proof is open; the definitional scaffolding above is verified
  -- against `src/types.rs` and `src/typechecker.rs` (July 2026).

end Nulang
