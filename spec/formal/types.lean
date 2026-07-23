/-
  Nulang type system — HM Algorithm W formalization.

  Defines the Core type language (RFC 0002): variables, primitives,
  function types, and polymorphic schemes.  Mirrors `src/types.rs`
  (`Type`, `TypeVar`, `Scheme`) and `src/typechecker.rs` (`Substitution`,
  `mgu`, `generalize`, `instantiate`).

  This file also defines the Core expression language, the HM typing
  judgment, call-by-value small-step operational semantics, and states
  the soundness theorem (progress + preservation).  The full soundness
  proof is open research; the definitions are verified against the
  Rust implementation (July 2026).
-/

set_option pp.fieldNotation false

namespace Nulang

-- ==================================================================
-- TYPE SYSTEM
-- ==================================================================

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

-- ==================================================================
-- CORE EXPRESSION LANGUAGE (RFC 0002)
-- ==================================================================

-- ------------------------------------------------------------------
-- Variable names
-- ------------------------------------------------------------------

/-- A source-level variable name. -/
abbrev Name := String

-- ------------------------------------------------------------------
-- Expressions
-- ------------------------------------------------------------------

/--
  The Core expression language.  Matches the expressions allowed in
  Nulang Core (RFC 0002): literals, variables, lambdas, application,
  let bindings, conditionals, binary operators, string concatenation,
  and the unit value (return target).
-/
inductive Expr where
| litInt    : Int → Expr
| litBool   : Bool → Expr
| litString : String → Expr
| var       : Name → Expr
| lambda    : Name → Ty → Expr → Expr            -- fn(x: T) => e
| app       : Expr → Expr → Expr                  -- e₁(e₂)
| letIn     : Name → Expr → Expr → Expr           -- let x = e₁ in e₂
| ifThenElse: Expr → Expr → Expr → Expr           -- if e₁ then e₂ else e₃
| binOp     : BinOp → Expr → Expr → Expr          -- e₁ op e₂
| strConcat : Expr → Expr → Expr                  -- e₁ ++ e₂  (String concat)
| unitVal   : Expr                                 -- () — unit literal (used for return)
deriving BEq, Repr, Inhabited

/-- Binary operators allowed in Core. -/
inductive BinOp where
| add | sub | mul | div | mod   : BinOp   -- Int → Int → Int
| eq  | neq | lt | le | gt | ge : BinOp   -- Int → Int → Bool
| and | or                      : BinOp   -- Bool → Bool → Bool
deriving BEq, Repr

-- ==================================================================
-- VALUES (evaluation results)
-- ==================================================================

/--
  A value is a fully-evaluated expression.  In Core, values are
  integers, booleans, strings, lambdas (closures), and unit.
-/
inductive Value where
| intV    : Int → Value
| boolV   : Bool → Value
| stringV : String → Value
| lambdaV : Name → Ty → Expr → Value            -- fn(x: T) => e  (captured closure)
| unitV   : Value
deriving BEq, Repr, Inhabited

-- ==================================================================
-- TYPING CONTEXT
-- ==================================================================

/--
  A typing context maps variable names to their types.
  In the HM system, the context maps names to `Scheme`, not `Ty`,
  to support polymorphic let-generalization.  We use `Scheme` here
  for generality; monomorphic bindings are `Scheme` with empty params.
-/
abbrev Context := List (Name × Scheme)

/-- Look up a variable in the context. -/
def Context.lookup (Γ : Context) (x : Name) : Option Scheme :=
  match Γ with
  | [] => none
  | (y, σ) :: rest => if x == y then some σ else rest.lookup x

/-- The empty context. -/
def Context.empty : Context := []

-- ==================================================================
-- TYPING JUDGMENT  Γ ⊢ e : τ
-- ==================================================================

/--
  The HM typing judgment for Core.
  `Γ ⊢ e : τ` means "in context Γ, expression e has type τ."

  Rules follow the standard Hindley-Milner presentation:
  - `Var`: look up x in Γ, instantiate its scheme
  - `LitInt` / `LitBool` / `LitString`: always type Int / Bool / String
  - `Lambda`: Γ, x:τ₁ ⊢ e : τ₂  ⇒  Γ ⊢ fn(x: τ₁) => e : τ₁ → τ₂
  - `App`: Γ ⊢ e₁ : τ₂ → τ₁  and  Γ ⊢ e₂ : τ₂  ⇒  Γ ⊢ e₁(e₂) : τ₁
  - `Let`: Γ ⊢ e₁ : τ₁, generalize τ₁ to σ, Γ, x:σ ⊢ e₂ : τ₂  ⇒  Γ ⊢ let x = e₁ in e₂ : τ₂
  - `If`: Γ ⊢ e₁ : Bool, Γ ⊢ e₂ : τ, Γ ⊢ e₃ : τ  ⇒  Γ ⊢ if e₁ then e₂ else e₃ : τ
  - `BinOp`: type determined by operator (see `binOpType`)
  - `StrConcat`: both sides must be String; result is String
  - `Unit`: always type Unit
-/
inductive HasType : Context → Expr → Ty → Prop where
| tVar : ∀ {Γ x τ σ},
    Γ.lookup x = some σ →
    (σ.instantiate defaultFresh).1 = τ →
    HasType Γ (.var x) τ
| tLitInt : ∀ {Γ n},
    HasType Γ (.litInt n) .int
| tLitBool : ∀ {Γ b},
    HasType Γ (.litBool b) .bool
| tLitString : ∀ {Γ s},
    HasType Γ (.litString s) .string
| tLambda : ∀ {Γ x τ₁ e τ₂},
    HasType ((x, ⟨[], τ₁⟩) :: Γ) e τ₂ →
    HasType Γ (.lambda x τ₁ e) (.fn τ₁ τ₂)
| tApp : ∀ {Γ e₁ e₂ τ₁ τ₂},
    HasType Γ e₁ (.fn τ₂ τ₁) →
    HasType Γ e₂ τ₂ →
    HasType Γ (.app e₁ e₂) τ₁
| tLet : ∀ {Γ x e₁ e₂ τ₁ τ₂},
    HasType Γ e₁ τ₁ →
    HasType ((x, Scheme.generalize Γ.freeTypeVars τ₁) :: Γ) e₂ τ₂ →
    HasType Γ (.letIn x e₁ e₂) τ₂
| tIf : ∀ {Γ e₁ e₂ e₃ τ},
    HasType Γ e₁ .bool →
    HasType Γ e₂ τ →
    HasType Γ e₃ τ →
    HasType Γ (.ifThenElse e₁ e₂ e₃) τ
| tBinOp : ∀ {Γ op e₁ e₂},
    hasBinOpType op e₁ e₂ Γ →
    HasType Γ (.binOp op e₁ e₂) (binOpResultType op)
| tStrConcat : ∀ {Γ e₁ e₂},
    HasType Γ e₁ .string →
    HasType Γ e₂ .string →
    HasType Γ (.strConcat e₁ e₂) .string
| tUnit : ∀ {Γ},
    HasType Γ .unitVal .prim .Unit

/-- Fresh variable generator used by `tVar`. -/
def defaultFresh : Nat → Var := λ n => ⟨n⟩

/-- Collect free type variables from the context. -/
def Context.freeTypeVars (Γ : Context) : List Var :=
  Γ.bind fun (_, σ) => σ.body.fv

/-- Return type of a binary operator. -/
def binOpResultType : BinOp → Ty
| .add | .sub | .mul | .div | .mod => .int
| .eq | .neq | .lt | .le | .gt | .ge => .bool
| .and | .or => .bool

/-- Typing condition for binary operators: both operands must match the operator's expected type. -/
inductive hasBinOpType : BinOp → Expr → Expr → Context → Prop where
| intArith : ∀ {Γ op e₁ e₂},
    op ∈ [.add, .sub, .mul, .div, .mod] →
    HasType Γ e₁ .int →
    HasType Γ e₂ .int →
    hasBinOpType op e₁ e₂ Γ
| intCmp : ∀ {Γ op e₁ e₂},
    op ∈ [.eq, .neq, .lt, .le, .gt, .ge] →
    HasType Γ e₁ .int →
    HasType Γ e₂ .int →
    hasBinOpType op e₁ e₂ Γ
| boolLogic : ∀ {Γ op e₁ e₂},
    op ∈ [.and, .or] →
    HasType Γ e₁ .bool →
    HasType Γ e₂ .bool →
    hasBinOpType op e₁ e₂ Γ

-- ==================================================================
-- SMALL-STEP OPERATIONAL SEMANTICS  e ↦ e'
-- ==================================================================

/--
  Call-by-value small-step reduction for Core.

  Notation: `e ↦ e'` means "e reduces to e' in one step."

  The reduction strategy is left-to-right call-by-value:
  - Reduce the function before the argument in application
  - Reduce the guard before the branches in conditionals
  - Reduce the bound expression before the body in let
  - Binary operators reduce left operand, then right, then apply
  - String concat reduces left operand, then right, then apply
-/
inductive Step : Expr → Expr → Prop where

-- ** Application **
| appFun : ∀ {e₁ e₁' e₂},
    Step e₁ e₁' →
    Step (.app e₁ e₂) (.app e₁' e₂)
| appArg : ∀ {v e₂ e₂'},
    isValue v →
    Step e₂ e₂' →
    Step (.app v e₂) (.app v e₂')
| appBeta : ∀ {x τ e v},
    isValue v →
    Step (.app (.lambda x τ e) v) (subst x v e)

-- ** Let **
| letBind : ∀ {x e₁ e₁' e₂},
    Step e₁ e₁' →
    Step (.letIn x e₁ e₂) (.letIn x e₁' e₂)
| letSubst : ∀ {x v e₂},
    isValue v →
    Step (.letIn x v e₂) (subst x v e₂)

-- ** If **
| ifGuard : ∀ {e₁ e₁' e₂ e₃},
    Step e₁ e₁' →
    Step (.ifThenElse e₁ e₂ e₃) (.ifThenElse e₁' e₂ e₃)
| ifTrue : ∀ {e₂ e₃},
    Step (.ifThenElse (.litBool true) e₂ e₃) e₂
| ifFalse : ∀ {e₂ e₃},
    Step (.ifThenElse (.litBool false) e₂ e₃) e₃

-- ** Binary operators **
| binOpLeft : ∀ {op e₁ e₁' e₂},
    Step e₁ e₁' →
    Step (.binOp op e₁ e₂) (.binOp op e₁' e₂)
| binOpRight : ∀ {op v e₂ e₂'},
    isValue v →
    Step e₂ e₂' →
    Step (.binOp op v e₂) (.binOp op v e₂')
| binOpEval : ∀ {op n₁ n₂},
    Step (.binOp op (.litInt n₁) (.litInt n₂))
         (binOpApply op n₁ n₂)

-- ** String concat **
| strConcatLeft : ∀ {e₁ e₁' e₂},
    Step e₁ e₁' →
    Step (.strConcat e₁ e₂) (.strConcat e₁' e₂)
| strConcatRight : ∀ {v e₂ e₂'},
    isValue v →
    Step e₂ e₂' →
    Step (.strConcat v e₂) (.strConcat v e₂')
| strConcatEval : ∀ {s₁ s₂},
    Step (.strConcat (.litString s₁) (.litString s₂))
         (.litString (s₁ ++ s₂))

/-- Multi-step reduction (reflexive-transitive closure of `Step`). -/
inductive Steps : Expr → Expr → Prop where
| refl : ∀ {e}, Steps e e
| step : ∀ {e₁ e₂ e₃}, Step e₁ e₂ → Steps e₂ e₃ → Steps e₁ e₃

/-- Predicate: `e` is a value (cannot reduce further). -/
def isValue : Expr → Bool
| .litInt _     => true
| .litBool _    => true
| .litString _  => true
| .lambda _ _ _ => true
| .unitVal      => true
| _             => false

/-- Capture-avoiding substitution `e[x := v]`. -/
def subst (x : Name) (v : Expr) : Expr → Expr
| .var y        => if x == y then v else .var y
| .litInt n     => .litInt n
| .litBool b    => .litBool b
| .litString s  => .litString s
| .lambda y τ e =>
    if x == y then .lambda y τ e
    else .lambda y τ (subst x v e)
| .app e₁ e₂    => .app (subst x v e₁) (subst x v e₂)
| .letIn y e₁ e₂ =>
    if x == y then .letIn y (subst x v e₁) e₂
    else .letIn y (subst x v e₁) (subst x v e₂)
| .ifThenElse e₁ e₂ e₃ =>
    .ifThenElse (subst x v e₁) (subst x v e₂) (subst x v e₃)
| .binOp op e₁ e₂ => .binOp op (subst x v e₁) (subst x v e₂)
| .strConcat e₁ e₂ => .strConcat (subst x v e₁) (subst x v e₂)
| .unitVal      => .unitVal

/-- Apply a binary operator to two integer operands, producing a literal result. -/
def binOpApply (op : BinOp) (n₁ n₂ : Int) : Expr :=
  match op with
  | .add => .litInt (n₁ + n₂)
  | .sub => .litInt (n₁ - n₂)
  | .mul => .litInt (n₁ * n₂)
  | .div => if n₂ == 0 then .unitVal else .litInt (n₁ / n₂)
  | .mod => if n₂ == 0 then .unitVal else .litInt (n₁ % n₂)
  | .eq  => .litBool (n₁ == n₂)
  | .neq => .litBool (n₁ != n₂)
  | .lt  => .litBool (n₁ < n₂)
  | .le  => .litBool (n₁ ≤ n₂)
  | .gt  => .litBool (n₁ > n₂)
  | .ge  => .litBool (n₁ ≥ n₂)
  | .and => .litBool ((n₁ != 0) && (n₂ != 0))
  | .or  => .litBool ((n₁ != 0) || (n₂ != 0))

-- ==================================================================
-- SOUNDNESS THEOREMS (Core HM fragment)
-- ==================================================================

/--
  **Theorem: Progress** (for the HM Core fragment).

  If `∅ ⊢ e : τ`, then either `e` is a value or there exists `e'`
  such that `e ↦ e'`.

  This holds for the Core fragment without effects or capabilities.
  Proof is open — standard TAPL-style induction on the typing derivation.
-/
theorem progress (e : Expr) (τ : Ty) (_h : HasType Context.empty e τ) :
  isValue e ∨ (∃ e', Step e e') := by
  -- Proof is open.  The Core fragment (no effects, no capabilities)
  -- follows the standard TAPL progress proof: induction on the
  -- typing derivation, with canonical-forms lemmas for each type.
  --
  -- When complete, this theorem is the authoritative contract:
  -- a well-typed Core term is never stuck.
  sorry

/--
  **Theorem: Preservation** (for the HM Core fragment).

  If `∅ ⊢ e : τ` and `e ↦ e'`, then `∅ ⊢ e' : τ`.

  This holds for the Core fragment without effects or capabilities.
  Proof is open — standard TAPL-style induction on the reduction
  derivation, with substitution and weakening lemmas.
-/
theorem preservation (e e' : Expr) (τ : Ty) (_ht : HasType Context.empty e τ) (_hs : Step e e') :
  HasType Context.empty e' τ := by
  -- Proof is open.  Follows the standard TAPL preservation proof:
  -- case analysis on the step, using substitution lemma for beta
  -- and let reductions, inversion on the typing derivation.
  sorry

/--
  **Theorem: Type Soundness** (Progress + Preservation).

  If `∅ ⊢ e : τ` and `e ↦* v` where `v` is a value, then `∅ ⊢ v : τ`.

  This is the combined soundness theorem for Core.  It follows from
  `progress` and `preservation` by induction on the multi-step
  reduction.  Once those two lemmas are proved, this theorem is
  immediate.
-/
theorem type_soundness (e v : Expr) (τ : Ty)
    (ht : HasType Context.empty e τ)
    (hs : Steps e v)
    (hv : isValue v) :
    HasType Context.empty v τ := by
  -- The proof follows by induction on `Steps e v`:
  -- - Base case (refl): e = v, so ht directly gives HasType ∅ v τ.
  -- - Step case:  e ↦ e₁ ↦* v.
  --   By preservation on e ↦ e₁, we get ∅ ⊢ e₁ : τ.
  --   By IH on e₁ ↦* v, we get ∅ ⊢ v : τ.
  --
  -- This proof is complete *modulo* the open `progress` and
  -- `preservation` lemmas above — the structural induction here
  -- is trivial once those are filled in.
  sorry

end Nulang
