/-
  Nulang type system — HM Algorithm W formalization.

  Defines the Core type language (RFC 0002): variables, primitives,
  function types, and polymorphic schemes.  Mirrors `src/types.rs`
  (`Type`, `TypeVar`, `Scheme`) and `src/typechecker.rs` (`Substitution`,
  `mgu`, `generalize`, `instantiate`).

  This file also defines the Core expression language, the HM typing
  judgment, call-by-value small-step operational semantics, and proves
  the soundness theorem (progress + preservation).
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

/-- Unify a type variable with a type: occurs check, then bind. -/
def mguVar (v : Var) (τ : Ty) : Except UnifyError Subst :=
  if Ty.var v == τ then .ok Subst.empty
  else if τ.fv.contains v then .error (.occursCheck v τ)
  else .ok [(v, τ)]

/--
  Most General Unifier.  Matches `unify` / `mgu` in `src/typechecker.rs`.
  Returns `Subst` on success, `UnifyError` on failure.
-/
partial def mgu (a b : Ty) : Except UnifyError Subst :=
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
  | .unit, .unit => .ok Subst.empty
  | _, _ => .error (.mismatch a b)

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

/-- Binary operators allowed in Core. -/
inductive BinOp : Type where
| add | sub | mul | div | mod   : BinOp   -- Int → Int → Int
| eq  | neq | lt | le | gt | ge : BinOp   -- Int → Int → Bool
| and | or                      : BinOp   -- Bool → Bool → Bool
deriving BEq, Repr


/--
  The Core expression language.  Matches the expressions allowed in
  Nulang Core (RFC 0002): literals, variables, lambdas, application,
  let bindings, conditionals, binary operators, string concatenation,
  and the unit value (return target).
-/
inductive Expr : Type where
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


-- ==================================================================
-- VALUES (evaluation results)
-- ==================================================================

/--
  A value is a fully-evaluated expression.  In Core, values are
  integers, booleans, strings, lambdas (closures), and unit.
-/
inductive Value : Type where
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
  | (y, σ) :: rest => if x == y then some σ else Context.lookup rest x

/-- The empty context. -/
def Context.empty : Context := []

-- ==================================================================
-- TYPING JUDGMENT  Γ ⊢ e : τ
-- ==================================================================

/-
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
/-- Fresh variable generator used by `tVar`. -/
def defaultFresh : Nat → Var := λ n => ⟨n⟩

/-- Collect free type variables from the context. -/
def Context.freeTypeVars (Γ : Context) : List Var :=
  match Γ with
  | [] => []
  | (_, σ) :: rest => σ.body.fv ++ Context.freeTypeVars rest

/-- Return type of a binary operator. -/
def binOpResultType : BinOp → Ty
| .add | .sub | .mul | .div | .mod => .int
| .eq | .neq | .lt | .le | .gt | .ge => .bool
| .and | .or => .bool


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
    HasType ((x, Scheme.generalize (Context.freeTypeVars Γ) τ₁) :: Γ) e₂ τ₂ →
    HasType Γ (.letIn x e₁ e₂) τ₂
| tIf : ∀ {Γ e₁ e₂ e₃ τ},
    HasType Γ e₁ .bool →
    HasType Γ e₂ τ →
    HasType Γ e₃ τ →
    HasType Γ (.ifThenElse e₁ e₂ e₃) τ
| tBinOpIntArith : ∀ {Γ op e₁ e₂},
    op ∈ [.add, .sub, .mul, .div, .mod] →
    HasType Γ e₁ .int →
    HasType Γ e₂ .int →
    HasType Γ (.binOp op e₁ e₂) .int
| tBinOpIntCmp : ∀ {Γ op e₁ e₂},
    op ∈ [.eq, .neq, .lt, .le, .gt, .ge] →
    HasType Γ e₁ .int →
    HasType Γ e₂ .int →
    HasType Γ (.binOp op e₁ e₂) .bool
| tBinOpBoolLogic : ∀ {Γ op e₁ e₂},
    op ∈ [.and, .or] →
    HasType Γ e₁ .bool →
    HasType Γ e₂ .bool →
    HasType Γ (.binOp op e₁ e₂) .bool
| tStrConcat : ∀ {Γ e₁ e₂},
    HasType Γ e₁ .string →
    HasType Γ e₂ .string →
    HasType Γ (.strConcat e₁ e₂) .string
| tUnit : ∀ {Γ},
    HasType Γ .unitVal (.prim .Unit)

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
  -- NB: Lean's Int division/mods are total (x / 0 = 0, x % 0 = x by
  -- convention), so the result stays an Int literal and preservation
  -- holds. (The runtime's nil-on-div-zero is a different, untagged
  -- semantics that this Core model does not represent.)
  | .div => .litInt (n₁ / n₂)
  | .mod => .litInt (n₁ % n₂)
  | .eq  => .litBool (n₁ == n₂)
  | .neq => .litBool (n₁ != n₂)
  | .lt  => .litBool (n₁ < n₂)
  | .le  => .litBool (n₁ ≤ n₂)
  | .gt  => .litBool (n₁ > n₂)
  | .ge  => .litBool (n₁ ≥ n₂)
  | .and => .unitVal  -- unreachable: .and is for Bool operands only
  | .or  => .litBool ((n₁ != 0) || (n₂ != 0))

/-- Apply a boolean binary operator to two boolean operands, producing a literal result. -/
def binOpApplyBool (op : BinOp) (b₁ b₂ : Bool) : Expr :=
  match op with
  | .and => .litBool (b₁ && b₂)
  | .or  => .litBool (b₁ || b₂)
  | _    => .unitVal  -- unreachable for well-typed programs
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
| binOpEvalBool : ∀ {op b₁ b₂},
    op ∈ [.and, .or] →
    Step (.binOp op (.litBool b₁) (.litBool b₂))
         (binOpApplyBool op b₁ b₂)

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

/- Predicate: `e` is a value (cannot reduce further). -/


-- ==================================================================
-- TYPE SOUNDNESS THEOREMS
-- ==================================================================

/- Predicate: all type annotations in an expression contain no free type variables. -/
def annotationsClosed : Expr → Prop
| .litInt _ | .litBool _ | .litString _ | .unitVal | .var _ => True
| .lambda _ τ e => τ.fv = [] ∧ annotationsClosed e
| .app e₁ e₂ | .strConcat e₁ e₂ | .binOp _ e₁ e₂ => annotationsClosed e₁ ∧ annotationsClosed e₂
| .letIn _ e₁ e₂ => annotationsClosed e₁ ∧ annotationsClosed e₂
| .ifThenElse e₁ e₂ e₃ => annotationsClosed e₁ ∧ annotationsClosed e₂ ∧ annotationsClosed e₃

-- ==================================================================
-- Structural lemmas
-- ==================================================================

theorem Context.lookup_append_left {Γ Δ : Context} {x : Name} {σ : Scheme}
    (h : Context.lookup Γ x = some σ) :
    Context.lookup (Γ ++ Δ) x = some σ := by
  induction Γ with
  | nil => simp [Context.lookup] at h
  | cons p Γs ih =>
      cases p with
      | mk y sy =>
          simp [Context.lookup] at h ⊢
          by_cases hxy : x = y
          · simp [hxy] at h ⊢
            exact h
          · simp [hxy] at h ⊢
            exact ih h

theorem Context.freeTypeVars_append (Γ Δ : Context) :
    Context.freeTypeVars (Γ ++ Δ) = Context.freeTypeVars Γ ++ Context.freeTypeVars Δ := by
  induction Γ with
  | nil => simp [Context.freeTypeVars]
  | cons p Γs ih =>
      cases p with
      | mk y sy =>
          simp [Context.freeTypeVars, ih, List.append_assoc]

theorem Ty.subst_nil (τ : Ty) : τ.subst [] = τ := by
  induction τ with
  | var v => simp [Ty.subst]
  | prim p => simp [Ty.subst]
  | fn a b ha hb => simp [Ty.subst, ha, hb]
  | unit => simp [Ty.subst]


theorem Scheme.instantiate_closed (σ : Scheme) (h : σ.params = []) :
    (σ.instantiate defaultFresh).1 = σ.body := by
  unfold Scheme.instantiate
  simp [h, Ty.subst_nil]

theorem Context.lookup_body_fv_subset {Γ : Context} {x : Name} {σ : Scheme}
    (h : Context.lookup Γ x = some σ) :
    σ.body.fv ⊆ Context.freeTypeVars Γ := by
  induction Γ with
  | nil => simp [Context.lookup] at h
  | cons p Γs ih =>
      cases p with
      | mk y sy =>
          simp [Context.lookup] at h
          by_cases hxy : x = y
          · simp [hxy] at h
            rw [← h]
            intro v hv
            simp [Context.freeTypeVars, List.mem_append, hv]
          · simp [hxy] at h
            intro v hv
            simp [Context.freeTypeVars, List.mem_append]
            right
            exact ih h hv

theorem Scheme.generalize_closed {Γ : Context} {τ₁ : Ty}
    (h : τ₁.fv = []) :
    (Scheme.generalize (Context.freeTypeVars Γ) τ₁).params = [] := by
  unfold Scheme.generalize
  simp [h]

abbrev Context.Mono (Γ : Context) : Prop :=
  ∀ (x : Name) (σ : Scheme), Context.lookup Γ x = some σ → σ.params = []

theorem weakening_append_closed {Γ : Context} {x : Name} {τ₀ : Ty} {e : Expr} {τ : Ty}
    (h : HasType Γ e τ) (hτ₀ : τ₀.fv = []) :
    HasType (Γ ++ [(x, ⟨[], τ₀⟩)]) e τ := by
  induction h with
  | tVar hlookup hinst =>
      exact .tVar (Context.lookup_append_left hlookup) hinst
  | tLitInt => exact .tLitInt
  | tLitBool => exact .tLitBool
  | tLitString => exact .tLitString
  | tLambda hbody ih =>
      exact .tLambda ih
  | tApp h₁ h₂ ih₁ ih₂ =>
      exact .tApp ih₁ ih₂
  | tLet h₁ h₂ ih₁ ih₂ =>
      exact .tLet ih₁ (by
        simpa [Context.freeTypeVars, Context.freeTypeVars_append, hτ₀] using ih₂)
  | tIf h₁ h₂ h₃ ih₁ ih₂ ih₃ =>
      exact .tIf ih₁ ih₂ ih₃
  | tBinOpIntArith hop h₁ h₂ ih₁ ih₂ =>
      exact .tBinOpIntArith hop ih₁ ih₂
  | tBinOpIntCmp hop h₁ h₂ ih₁ ih₂ =>
      exact .tBinOpIntCmp hop ih₁ ih₂
  | tBinOpBoolLogic hop h₁ h₂ ih₁ ih₂ =>
      exact .tBinOpBoolLogic hop ih₁ ih₂
  | tStrConcat h₁ h₂ ih₁ ih₂ =>
      exact .tStrConcat ih₁ ih₂
  | tUnit =>
      exact .tUnit

theorem closed_type_under_closed_context {Γ : Context} {e : Expr} {τ : Ty}
    (h : HasType Γ e τ) (hΓ : Context.freeTypeVars Γ = [])
    (h_params : ∀ (x : Name) (σ : Scheme), Context.lookup Γ x = some σ → σ.params = [])
    (h_closed : annotationsClosed e) :
    τ.fv = [] := by
  induction h
  case tVar Γ' x' τ' σ' hlookup hinst =>
      have hparams : σ'.params = [] := h_params x' σ' hlookup
      have hinst' : (σ'.instantiate defaultFresh).1 = σ'.body :=
        Scheme.instantiate_closed σ' hparams
      have hτ : τ' = σ'.body := hinst.symm.trans hinst'
      rw [hτ]
      have hsub : σ'.body.fv ⊆ Context.freeTypeVars Γ' :=
        Context.lookup_body_fv_subset hlookup
      rw [hΓ] at hsub
      apply List.eq_nil_iff_forall_not_mem.mpr
      intro v hv
      have : v ∈ ([] : List Var) := hsub hv
      simp at this
  case tLitInt => rfl
  case tLitBool => rfl
  case tLitString => rfl
  case tLambda Γ' x' τ₁' e' τ₂' hbody ih =>
      have hτ₂ : τ₂'.fv = [] := ih
        (by simp [Context.freeTypeVars, hΓ, h_closed.1])
        (by
          intro x'' σ'' hl
          simp [Context.lookup] at hl
          by_cases hxx : x'' = x'
          · simp [hxx] at hl
            simpa using (congrArg Scheme.params hl).symm
          · simp [hxx] at hl
            exact h_params x'' σ'' hl)
        h_closed.2
      simp [Ty.fv, h_closed.1, hτ₂]
  case tApp Γ' e₁' e₂' τ₁' τ₂' h₁ h₂ ih₁ ih₂ =>
      have hfn : (Ty.fn τ₂' τ₁').fv = [] := ih₁ hΓ h_params h_closed.1
      simp [Ty.fv] at hfn
      exact hfn.2
  case tLet Γ' x' e₁' e₂' τ₁' τ₂' h₁ h₂ ih₁ ih₂ =>
      have hτ₁ : τ₁'.fv = [] := ih₁ hΓ h_params h_closed.1
      exact ih₂
        (by simp [Context.freeTypeVars, Scheme.generalize, hΓ, hτ₁])
        (by
          intro x'' σ'' hl
          simp [Context.lookup] at hl
          by_cases hxx : x'' = x'
          · simp [hxx] at hl
            have hgen : (Scheme.generalize (Context.freeTypeVars Γ') τ₁').params = [] :=
              Scheme.generalize_closed hτ₁
            simpa [hgen] using (congrArg Scheme.params hl).symm
          · simp [hxx] at hl
            exact h_params x'' σ'' hl)
        h_closed.2
  case tIf Γ' e₁' e₂' e₃' τ' h₁ h₂ h₃ ih₁ ih₂ ih₃ =>
      exact ih₂ hΓ h_params h_closed.2.1
  case tBinOpIntArith Γ' op' e₁' e₂' hop h₁ h₂ ih₁ ih₂ => rfl
  case tBinOpIntCmp Γ' op' e₁' e₂' hop h₁ h₂ ih₁ ih₂ => rfl
  case tBinOpBoolLogic Γ' op' e₁' e₂' hop h₁ h₂ ih₁ ih₂ => rfl
  case tStrConcat Γ' e₁' e₂' h₁ h₂ ih₁ ih₂ => rfl
  case tUnit => rfl

theorem value_has_closed_type {v : Expr} {τ : Ty}
    (h : HasType Context.empty v τ) (_hv : isValue v) (h_closed : annotationsClosed v) :
    τ.fv = [] := by
  exact closed_type_under_closed_context h rfl
    (by
      intro x σ hl
      simp [Context.lookup, Context.empty] at hl)
    h_closed

theorem Context.lookup_swap_head {Γ : Context} {a b : Name} {σₐ σ_b : Scheme}
    (hab : a ≠ b) :
    Context.lookup ((a, σₐ) :: (b, σ_b) :: Γ) =
    Context.lookup ((b, σ_b) :: (a, σₐ) :: Γ) := by
  funext x
  simp [Context.lookup]
  by_cases hxa : x = a
  · simp [hxa]
    by_cases hxb : x = b
    · exfalso; apply hab; exact hxa.symm.trans hxb
    · intro h
      exfalso; exact hab h
  · simp [hxa]

theorem Context.lookup_permute {Δ Γ : Context} {a b : Name} {σₐ σ_b : Scheme}
    (hab : a ≠ b) :
    Context.lookup (Δ ++ (a, σₐ) :: (b, σ_b) :: Γ) =
    Context.lookup (Δ ++ (b, σ_b) :: (a, σₐ) :: Γ) := by
  induction Δ with
  | nil => exact Context.lookup_swap_head hab
  | cons p Δs ih =>
      cases p with
      | mk y sy =>
          funext x
          simp [Context.lookup]
          by_cases hxy : x = y
          · simp [hxy]
          · simp [hxy]
            exact congrFun ih x

theorem Context.lookup_drop_shadow {Γ : Context} {x : Name} {σ σ' : Scheme} :
    Context.lookup ((x, σ') :: (x, σ) :: Γ) =
    Context.lookup ((x, σ') :: Γ) := by
  funext y
  simp [Context.lookup]
  by_cases hyx : y = x <;> simp [hyx]

theorem Context.lookup_drop_shadow_append {Δ Γ : Context} {x : Name} {σ σ' : Scheme} :
    Context.lookup (Δ ++ (x, σ') :: (x, σ) :: Γ) =
    Context.lookup (Δ ++ (x, σ') :: Γ) := by
  induction Δ with
  | nil => exact Context.lookup_drop_shadow
  | cons p Δs ih =>
      cases p with
      | mk y sy =>
          funext z
          simp [Context.lookup]
          by_cases hzy : z = y
          · simp [hzy]
          · simp [hzy]
            exact congrFun ih z

-- One-directional subcontext: lookup-mono + equal free type vars.
theorem Context.lookup_mono_extend {Γ₁ Γ₂ : Context} {x : Name} {σ₁ σ₂ : Scheme}
    (hσ : σ₁ = σ₂)
    (hmono : ∀ y σ, Context.lookup Γ₁ y = some σ → Context.lookup Γ₂ y = some σ) :
    ∀ y σ, Context.lookup ((x, σ₁) :: Γ₁) y = some σ → Context.lookup ((x, σ₂) :: Γ₂) y = some σ := by
  intro y σ hl
  simp [Context.lookup] at hl
  by_cases hy : y = x
  · subst y
    simp [Context.lookup, hσ] at hl ⊢
    exact hl
  · simp [Context.lookup, hy] at hl ⊢
    exact hmono y σ hl

theorem Context.freeTypeVars_extend {Γ₁ Γ₂ : Context} {x : Name} {σ₁ σ₂ : Scheme}
    (hσ : σ₁ = σ₂) (hftv : Context.freeTypeVars Γ₁ = Context.freeTypeVars Γ₂) :
    Context.freeTypeVars ((x, σ₁) :: Γ₁) = Context.freeTypeVars ((x, σ₂) :: Γ₂) := by
  simp [Context.freeTypeVars, hσ, hftv]

theorem HasType_subctx {Γ₁ Γ₂ : Context} {e : Expr} {τ : Ty}
    (hmono : ∀ x σ, Context.lookup Γ₁ x = some σ → Context.lookup Γ₂ x = some σ)
    (hftv : Context.freeTypeVars Γ₁ = Context.freeTypeVars Γ₂)
    (h : HasType Γ₁ e τ) :
    HasType Γ₂ e τ := by
  induction h generalizing Γ₂ with
  | tVar hlookup hinst =>
      rename_i Γ₁' x' τ' σ'
      exact .tVar (hmono x' σ' hlookup) hinst
  | tLitInt => exact .tLitInt
  | tLitBool => exact .tLitBool
  | tLitString => exact .tLitString
  | tLambda hbody ih =>
      exact .tLambda (ih
        (Context.lookup_mono_extend rfl hmono)
        (Context.freeTypeVars_extend rfl hftv))
  | tApp h₁ h₂ ih₁ ih₂ =>
      exact .tApp (ih₁ hmono hftv) (ih₂ hmono hftv)
  | tLet h₁ h₂ ih₁ ih₂ =>
      rename_i Γ₁' x' e₁ e₂ τ₁ τ₂
      have hσ : Scheme.generalize (Context.freeTypeVars Γ₁') τ₁ =
                Scheme.generalize (Context.freeTypeVars Γ₂) τ₁ := by
        rw [hftv]
      exact .tLet (ih₁ hmono hftv) (ih₂
        (Context.lookup_mono_extend hσ hmono)
        (Context.freeTypeVars_extend hσ hftv))
  | tIf h₁ h₂ h₃ ih₁ ih₂ ih₃ =>
      exact .tIf (ih₁ hmono hftv) (ih₂ hmono hftv) (ih₃ hmono hftv)
  | tBinOpIntArith hop h₁ h₂ ih₁ ih₂ =>
      exact .tBinOpIntArith hop (ih₁ hmono hftv) (ih₂ hmono hftv)
  | tBinOpIntCmp hop h₁ h₂ ih₁ ih₂ =>
      exact .tBinOpIntCmp hop (ih₁ hmono hftv) (ih₂ hmono hftv)
  | tBinOpBoolLogic hop h₁ h₂ ih₁ ih₂ =>
      exact .tBinOpBoolLogic hop (ih₁ hmono hftv) (ih₂ hmono hftv)
  | tStrConcat h₁ h₂ ih₁ ih₂ =>
      exact .tStrConcat (ih₁ hmono hftv) (ih₂ hmono hftv)
  | tUnit => exact .tUnit

theorem HasType_permute {Δ Γ : Context} {a b : Name} {σₐ σ_b : Scheme} {e : Expr} {τ : Ty}
    (hab : a ≠ b) (hσb : σ_b.body.fv = [])
    (h : HasType (Δ ++ (a, σₐ) :: (b, σ_b) :: Γ) e τ) :
    HasType (Δ ++ (b, σ_b) :: (a, σₐ) :: Γ) e τ := by
  exact HasType_subctx (Γ₁ := Δ ++ (a, σₐ) :: (b, σ_b) :: Γ) (Γ₂ := Δ ++ (b, σ_b) :: (a, σₐ) :: Γ)
    (by
      intro x σ hl
      simpa [← Context.lookup_permute hab] using hl)
    (by simp [Context.freeTypeVars, Context.freeTypeVars_append, hσb])
    h

theorem drop_shadowed_closed {Δ Γ : Context} {x : Name} {σ σ' : Scheme} {e : Expr} {τ : Ty}
    (hσ : σ.body.fv = [])
    (h : HasType (Δ ++ (x, σ') :: (x, σ) :: Γ) e τ) :
    HasType (Δ ++ (x, σ') :: Γ) e τ := by
  apply HasType_subctx
  · intro y σy hl
    have hlookup := Context.lookup_drop_shadow_append (Δ := Δ) (Γ := Γ) (x := x) (σ := σ) (σ' := σ')
    rw [← hlookup]
    exact hl
  · simp [Context.freeTypeVars, Context.freeTypeVars_append, hσ]
  · exact h

theorem lift_from_empty {Γ : Context} {v : Expr} {τ₁ : Ty}
    (hv : HasType Context.empty v τ₁)
    (hΓ : Context.freeTypeVars Γ = [])
    (_h_closed : annotationsClosed v) :
    HasType Γ v τ₁ := by
  exact HasType_subctx (Γ₁ := Context.empty) (Γ₂ := Γ)
    (by
      intro x σ hl
      simp [Context.lookup, Context.empty] at hl)
    (by simp [Context.freeTypeVars, Context.empty, hΓ])
    hv

theorem annotationsClosed_subst {e v : Expr} {x : Name}
    (he : annotationsClosed e) (hv : annotationsClosed v) :
    annotationsClosed (subst x v e) := by
  induction e with
  | litInt => trivial
  | litBool => trivial
  | litString => trivial
  | var y =>
      by_cases hxy : x = y <;> simp [subst, hxy, annotationsClosed, hv]
  | lambda y τ e' ih =>
      by_cases hxy : x = y
      · subst y
        simpa [subst] using he
      · simp [annotationsClosed, subst, hxy]
        exact ⟨he.1, ih he.2⟩
  | app e₁ e₂ ih₁ ih₂ =>
      simp [annotationsClosed, subst]
      exact ⟨ih₁ he.1, ih₂ he.2⟩
  | letIn y e₁ e₂ ih₁ ih₂ =>
      by_cases hxy : x = y
      · subst y
        simp [annotationsClosed, subst]
        exact ⟨ih₁ he.1, he.2⟩
      · simp [annotationsClosed, subst, hxy]
        exact ⟨ih₁ he.1, ih₂ he.2⟩
  | ifThenElse e₁ e₂ e₃ ih₁ ih₂ ih₃ =>
      simp [annotationsClosed, subst]
      exact ⟨ih₁ he.1, ih₂ he.2.1, ih₃ he.2.2⟩
  | binOp op e₁ e₂ ih₁ ih₂ =>
      simp [annotationsClosed, subst]
      exact ⟨ih₁ he.1, ih₂ he.2⟩
  | strConcat e₁ e₂ ih₁ ih₂ =>
      simp [annotationsClosed, subst]
      exact ⟨ih₁ he.1, ih₂ he.2⟩
  | unitVal => trivial

theorem step_preserves_closed {e e' : Expr} (hs : Step e e') (h : annotationsClosed e) :
    annotationsClosed e' := by
  induction hs
  case appFun e₁ e₁' e₂ hs₁ ih => exact ⟨ih h.1, h.2⟩
  case appArg v e₂ e₂' hv₂ hs₂ ih => exact ⟨h.1, ih h.2⟩
  case appBeta x τ₀ e v hv => exact annotationsClosed_subst h.1.2 h.2
  case letBind x e₁ e₁' e₂ hs₁ ih => exact ⟨ih h.1, h.2⟩
  case letSubst x v e₂ hv => exact annotationsClosed_subst h.2 h.1
  case ifGuard e₁ e₁' e₂ e₃ hs₁ ih => exact ⟨ih h.1, h.2.1, h.2.2⟩
  case ifTrue e₂ e₃ => exact h.2.1
  case ifFalse e₂ e₃ => exact h.2.2
  case binOpLeft op e₁ e₁' e₂ hs₁ ih => exact ⟨ih h.1, h.2⟩
  case binOpRight op v e₂ e₂' hv₂ hs₂ ih => exact ⟨h.1, ih h.2⟩
  case binOpEval op n₁ n₂ => cases op <;> simp [binOpApply, annotationsClosed]
  case binOpEvalBool op b₁ b₂ hop => cases op <;> simp [binOpApplyBool, annotationsClosed]
  case strConcatLeft e₁ e₁' e₂ hs₁ ih => exact ⟨ih h.1, h.2⟩
  case strConcatRight v e₂ e₂' hv₂ hs₂ ih => exact ⟨h.1, ih h.2⟩
  case strConcatEval s₁ s₂ => trivial

theorem canonical_forms {v : Expr} {τ : Ty}
    (h : HasType Context.empty v τ)
    (hv : isValue v) :
    (∃ n : Int, v = .litInt n ∧ τ = .int) ∨
    (∃ b : Bool, v = .litBool b ∧ τ = .bool) ∨
    (∃ s : String, v = .litString s ∧ τ = .string) ∨
    (∃ (x : Name) (τ₁ : Ty) (e : Expr), v = .lambda x τ₁ e ∧
     ∃ τ₂ : Ty, τ = .fn τ₁ τ₂) ∨
    (v = .unitVal ∧ τ = .prim .Unit) := by
  cases h
  · -- tVar: impossible, empty context
    rename_i h_lookup h_inst
    simp [Context.lookup, Context.empty] at h_lookup

  · -- tLitInt
    left; exact ⟨_, rfl, rfl⟩
  · -- tLitBool
    right; left; exact ⟨_, rfl, rfl⟩
  · -- tLitString
    right; right; left; exact ⟨_, rfl, rfl⟩
  · -- tLambda
    right; right; right; left
    exact ⟨_, _, _, rfl, _, rfl⟩
  · -- tApp
    simp [isValue] at hv
  · -- tLet
    simp [isValue] at hv
  · -- tIf
    simp [isValue] at hv
  · -- tBinOpIntArith
    simp [isValue] at hv
  · -- tBinOpIntCmp
    simp [isValue] at hv
  · -- tBinOpBoolLogic
    simp [isValue] at hv
  · -- tStrConcat
    simp [isValue] at hv
  · -- tUnit
    right; right; right; right; exact ⟨rfl, rfl⟩

theorem progress {e : Expr} {τ : Ty} (h : HasType Context.empty e τ) :
    isValue e ∨ (∃ e', Step e e') := by
  generalize hE : Context.empty = Γ₀ at h
  induction h
  case tVar Γ' x' τ' σ' hlookup hinst =>
      rw [← hE] at hlookup
      simp [Context.lookup, Context.empty] at hlookup
  case tLitInt => left; rfl
  case tLitBool => left; rfl
  case tLitString => left; rfl
  case tLambda Γ' x' τ₁' e' τ₂' hbody => left; rfl
  case tApp Γ' e₁ e₂ τ₁ τ₂ h₁ h₂ ih₁ ih₂ =>
      have h₁' : HasType Context.empty e₁ (.fn τ₂ τ₁) := by simpa [← hE] using h₁
      rcases ih₁ hE with hv₁ | ⟨e₁', step₁⟩
      · rcases ih₂ hE with hv₂ | ⟨e₂', step₂⟩
        · rcases canonical_forms h₁' hv₁ with
            ⟨n, heq, hty⟩ | ⟨b, heq, hty⟩ | ⟨s, heq, hty⟩ |
            ⟨x, τ₀, e₀, heq, hty⟩ | ⟨heq, hty⟩
          · subst e₁; cases h₁'
          · subst e₁; cases h₁'
          · subst e₁; cases h₁'
          · subst e₁
            right; exact ⟨subst x e₂ e₀, .appBeta hv₂⟩
          · subst e₁; cases h₁'
        · right; exact ⟨.app e₁ e₂', .appArg hv₁ step₂⟩
      · right; exact ⟨.app e₁' e₂, .appFun step₁⟩
  case tLet Γ' x' e₁ e₂ τ₁ τ₂ h₁ h₂ ih₁ ih₂ =>
      rcases ih₁ hE with hv₁ | ⟨e₁', step₁⟩
      · right; exact ⟨subst x' e₁ e₂, .letSubst hv₁⟩
      · right; exact ⟨.letIn x' e₁' e₂, .letBind step₁⟩
  case tIf Γ' e₁ e₂ e₃ τ' h₁ h₂ h₃ ih₁ ih₂ ih₃ =>
      have h₁' : HasType Context.empty e₁ .bool := by simpa [← hE] using h₁
      rcases ih₁ hE with hv₁ | ⟨e₁', step₁⟩
      · rcases canonical_forms h₁' hv₁ with ⟨n, heq, hty⟩ | ⟨b, heq, hty⟩ | ⟨s, heq, hty⟩ | ⟨x, τ₀, e₀, heq, hty⟩ | ⟨heq, hty⟩
        · subst e₁; cases h₁'
        · subst e₁
          right
          cases b
          · exact ⟨e₃, .ifFalse⟩
          · exact ⟨e₂, .ifTrue⟩
        · subst e₁; cases h₁'
        · subst e₁; cases h₁'
        · subst e₁; cases h₁'
      · right; exact ⟨.ifThenElse e₁' e₂ e₃, .ifGuard step₁⟩
  case tBinOpIntArith Γ' op' e₁ e₂ hop h₁ h₂ ih₁ ih₂ =>
      have h₁' : HasType Context.empty e₁ .int := by simpa [← hE] using h₁
      have h₂' : HasType Context.empty e₂ .int := by simpa [← hE] using h₂
      rcases ih₁ hE with hv₁ | ⟨e₁', step₁⟩
      · rcases ih₂ hE with hv₂ | ⟨e₂', step₂⟩
        · rcases canonical_forms h₁' hv₁ with
            ⟨n, heq, hty⟩ | ⟨b, heq, hty⟩ | ⟨s, heq, hty⟩ |
            ⟨x, τ₀, e₀, heq, hty⟩ | ⟨heq, hty⟩
          · subst e₁
            rcases canonical_forms h₂' hv₂ with
              ⟨m, heq₂, hty₂⟩ | ⟨b₂, heq₂, hty₂⟩ | ⟨s₂, heq₂, hty₂⟩ |
              ⟨x₂, τ₀₂, e₀₂, heq₂, hty₂⟩ | ⟨heq₂, hty₂⟩
            · subst e₂; right; exact ⟨binOpApply op' n m, .binOpEval⟩
            · subst e₂; cases h₂'
            · subst e₂; cases h₂'
            · subst e₂; cases h₂'
            · subst e₂; cases h₂'
          · subst e₁; cases h₁'
          · subst e₁; cases h₁'
          · subst e₁; cases h₁'
          · subst e₁; cases h₁'
        · right; exact ⟨.binOp op' e₁ e₂', .binOpRight hv₁ step₂⟩
      · right; exact ⟨.binOp op' e₁' e₂, .binOpLeft step₁⟩
  case tBinOpIntCmp Γ' op' e₁ e₂ hop h₁ h₂ ih₁ ih₂ =>
      have h₁' : HasType Context.empty e₁ .int := by simpa [← hE] using h₁
      have h₂' : HasType Context.empty e₂ .int := by simpa [← hE] using h₂
      rcases ih₁ hE with hv₁ | ⟨e₁', step₁⟩
      · rcases ih₂ hE with hv₂ | ⟨e₂', step₂⟩
        · rcases canonical_forms h₁' hv₁ with
            ⟨n, heq, hty⟩ | ⟨b, heq, hty⟩ | ⟨s, heq, hty⟩ |
            ⟨x, τ₀, e₀, heq, hty⟩ | ⟨heq, hty⟩
          · subst e₁
            rcases canonical_forms h₂' hv₂ with
              ⟨m, heq₂, hty₂⟩ | ⟨b₂, heq₂, hty₂⟩ | ⟨s₂, heq₂, hty₂⟩ |
              ⟨x₂, τ₀₂, e₀₂, heq₂, hty₂⟩ | ⟨heq₂, hty₂⟩
            · subst e₂; right; exact ⟨binOpApply op' n m, .binOpEval⟩
            · subst e₂; cases h₂'
            · subst e₂; cases h₂'
            · subst e₂; cases h₂'
            · subst e₂; cases h₂'
          · subst e₁; cases h₁'
          · subst e₁; cases h₁'
          · subst e₁; cases h₁'
          · subst e₁; cases h₁'
        · right; exact ⟨.binOp op' e₁ e₂', .binOpRight hv₁ step₂⟩
      · right; exact ⟨.binOp op' e₁' e₂, .binOpLeft step₁⟩
  case tBinOpBoolLogic Γ' op' e₁ e₂ hop h₁ h₂ ih₁ ih₂ =>
      have h₁' : HasType Context.empty e₁ .bool := by simpa [← hE] using h₁
      have h₂' : HasType Context.empty e₂ .bool := by simpa [← hE] using h₂
      rcases ih₁ hE with hv₁ | ⟨e₁', step₁⟩
      · rcases ih₂ hE with hv₂ | ⟨e₂', step₂⟩
        · rcases canonical_forms h₁' hv₁ with
            ⟨n, heq, hty⟩ | ⟨b₁, heq, hty⟩ | ⟨s, heq, hty⟩ |
            ⟨x, τ₀, e₀, heq, hty⟩ | ⟨heq, hty⟩
          · subst e₁; cases h₁'
          · subst e₁
            rcases canonical_forms h₂' hv₂ with
              ⟨m, heq₂, hty₂⟩ | ⟨b₂, heq₂, hty₂⟩ | ⟨s₂, heq₂, hty₂⟩ |
              ⟨x₂, τ₀₂, e₀₂, heq₂, hty₂⟩ | ⟨heq₂, hty₂⟩
            · subst e₂; cases h₂'
            · subst e₂; right; exact ⟨binOpApplyBool op' b₁ b₂, .binOpEvalBool hop⟩
            · subst e₂; cases h₂'
            · subst e₂; cases h₂'
            · subst e₂; cases h₂'
          · subst e₁; cases h₁'
          · subst e₁; cases h₁'
          · subst e₁; cases h₁'
        · right; exact ⟨.binOp op' e₁ e₂', .binOpRight hv₁ step₂⟩
      · right; exact ⟨.binOp op' e₁' e₂, .binOpLeft step₁⟩
  case tStrConcat Γ' e₁ e₂ h₁ h₂ ih₁ ih₂ =>
      have h₁' : HasType Context.empty e₁ .string := by simpa [← hE] using h₁
      have h₂' : HasType Context.empty e₂ .string := by simpa [← hE] using h₂
      rcases ih₁ hE with hv₁ | ⟨e₁', step₁⟩
      · rcases ih₂ hE with hv₂ | ⟨e₂', step₂⟩
        · rcases canonical_forms h₁' hv₁ with
            ⟨n, heq, hty⟩ | ⟨b, heq, hty⟩ | ⟨s₁, heq, hty⟩ |
            ⟨x, τ₀, e₀, heq, hty⟩ | ⟨heq, hty⟩
          · subst e₁; cases h₁'
          · subst e₁; cases h₁'
          · subst e₁
            rcases canonical_forms h₂' hv₂ with
              ⟨m, heq₂, hty₂⟩ | ⟨b₂, heq₂, hty₂⟩ | ⟨s₂, heq₂, hty₂⟩ |
              ⟨x₂, τ₀₂, e₀₂, heq₂, hty₂⟩ | ⟨heq₂, hty₂⟩
            · subst e₂; cases h₂'
            · subst e₂; cases h₂'
            · subst e₂; right; exact ⟨.litString (s₁ ++ s₂), .strConcatEval⟩
            · subst e₂; cases h₂'
            · subst e₂; cases h₂'
          · subst e₁; cases h₁'
          · subst e₁; cases h₁'
        · right; exact ⟨.strConcat e₁ e₂', .strConcatRight hv₁ step₂⟩
      · right; exact ⟨.strConcat e₁' e₂, .strConcatLeft step₁⟩
  case tUnit Γ' => left; rfl
theorem substitution_lemma {Γ : Context} {x : Name} {τ₁ τ₂ : Ty} {e v : Expr}
    (h : HasType ((x, ⟨[], τ₁⟩) :: Γ) e τ₂)
    (hv : HasType Context.empty v τ₁)
    (h_fv : τ₁.fv = [])
    (hΓ : Context.freeTypeVars Γ = [])
    (hΓ_mono : Context.Mono Γ)
    (h_closed_e : annotationsClosed e)
    (h_closed_v : annotationsClosed v) :
    HasType Γ (subst x v e) τ₂ := by
  induction e generalizing Γ τ₂ hΓ hΓ_mono with
  | litInt => cases h; exact .tLitInt
  | litBool => cases h; exact .tLitBool
  | litString => cases h; exact .tLitString
  | var y =>
      cases h with
      | tVar hlookup hinst =>
        rename_i σ
        by_cases hxy : x = y
        · have hyx : y = x := hxy.symm
          simp [Context.lookup, hyx] at hlookup
          have hσ : σ = ⟨[], τ₁⟩ := hlookup.symm
          have hτ : τ₂ = τ₁ := by
            rw [hσ] at hinst
            rw [Scheme.instantiate_closed ⟨[], τ₁⟩ rfl] at hinst
            exact hinst.symm
          rw [hτ]
          simpa [subst, hxy] using (lift_from_empty hv hΓ h_closed_v)
        · have hyx : ¬ y = x := fun h => hxy h.symm
          simp [Context.lookup, hyx] at hlookup
          simpa [subst, hxy] using (.tVar hlookup hinst)
  | lambda y τ₀ e' ih =>
      cases h with
      | tLambda hbody =>
        by_cases hxy : x = y
        · subst y
          have hdrop : HasType ((x, ⟨[], τ₀⟩) :: Γ) e' _ :=
            drop_shadowed_closed (Γ := Γ) (Δ := []) h_fv hbody
          simpa [subst] using (.tLambda hdrop)
        · have hyx : y ≠ x := fun h => hxy h.symm
          have hperm : HasType ((x, ⟨[], τ₁⟩) :: (y, ⟨[], τ₀⟩) :: Γ) e' _ :=
            HasType_permute (Δ := []) (Γ := Γ) hyx h_fv hbody
          have hih : HasType ((y, ⟨[], τ₀⟩) :: Γ) (subst x v e') _ :=
            ih hperm (by simp [Context.freeTypeVars, hΓ, h_closed_e.1])
              (by
                intro x' σ' hl
                simp [Context.lookup] at hl
                by_cases hxx : x' = y
                · simp [hxx] at hl
                  simpa using (congrArg Scheme.params hl).symm
                · simp [hxx] at hl
                  exact hΓ_mono x' σ' hl)
              h_closed_e.2
          simpa [subst, hxy] using (.tLambda hih)
  | app e₁ e₂ ih₁ ih₂ =>
      cases h with
      | tApp hfun harg =>
        exact .tApp (ih₁ hfun hΓ hΓ_mono h_closed_e.1) (ih₂ harg hΓ hΓ_mono h_closed_e.2)
  | letIn y e₁ e₂ ih₁ ih₂ =>
      cases h with
      | tLet h₁ h₂ =>
        rename_i τ₀
        have hτ₀ : τ₀.fv = [] :=
          closed_type_under_closed_context (Γ := (x, ⟨[], τ₁⟩) :: Γ) (e := e₁) (τ := τ₀) h₁
            (by simp [Context.freeTypeVars, hΓ, h_fv])
            (by
              intro x' σ' hl
              simp [Context.lookup] at hl
              by_cases hxx : x' = x
              · simp [hxx] at hl
                simpa using (congrArg Scheme.params hl).symm
              · simp [hxx] at hl
                exact hΓ_mono x' σ' hl)
            h_closed_e.1
        by_cases hxy : x = y
        · subst y
          have hσ : Scheme.generalize (Context.freeTypeVars Γ) τ₀ = ⟨[], τ₀⟩ := by
            unfold Scheme.generalize
            simp [hτ₀]
          have h₂' : HasType ((x, ⟨[], τ₀⟩) :: Γ) e₂ τ₂ := by
            have h2ctx : HasType ((x, Scheme.generalize (Context.freeTypeVars Γ) τ₀) :: (x, ⟨[], τ₁⟩) :: Γ) e₂ τ₂ := by
              simpa [Context.freeTypeVars, h_fv] using h₂
            have hdrop : HasType ((x, Scheme.generalize (Context.freeTypeVars Γ) τ₀) :: Γ) e₂ τ₂ :=
              drop_shadowed_closed (Γ := Γ) (Δ := []) h_fv h2ctx
            simpa [hσ] using hdrop
          simpa [subst] using (.tLet (ih₁ h₁ hΓ hΓ_mono h_closed_e.1) (by simpa [hσ] using h₂'))
        · have hyx : y ≠ x := fun h => hxy h.symm
          have hperm : HasType ((x, ⟨[], τ₁⟩) :: (y, Scheme.generalize (Context.freeTypeVars Γ) τ₀) :: Γ) e₂ τ₂ := by
            exact HasType_permute (Δ := []) (Γ := Γ) hyx h_fv (by
              simpa [Context.freeTypeVars, h_fv] using h₂)
          have hih₂ : HasType ((y, Scheme.generalize (Context.freeTypeVars Γ) τ₀) :: Γ) (subst x v e₂) τ₂ :=
            ih₂ hperm (by simp [Context.freeTypeVars, Scheme.generalize, hΓ, hτ₀])
              (by
                intro x' σ' hl
                simp [Context.lookup] at hl
                by_cases hxx : x' = y
                · simp [hxx] at hl
                  have hgen : (Scheme.generalize (Context.freeTypeVars Γ) τ₀).params = [] :=
                    Scheme.generalize_closed hτ₀
                  simpa [hgen] using (congrArg Scheme.params hl).symm
                · simp [hxx] at hl
                  exact hΓ_mono x' σ' hl)
              h_closed_e.2
          simpa [subst, hxy] using (.tLet (ih₁ h₁ hΓ hΓ_mono h_closed_e.1) hih₂)
  | ifThenElse e₁ e₂ e₃ ih₁ ih₂ ih₃ =>
      cases h with
      | tIf h₁ h₂ h₃ =>
        exact .tIf (ih₁ h₁ hΓ hΓ_mono h_closed_e.1) (ih₂ h₂ hΓ hΓ_mono h_closed_e.2.1) (ih₃ h₃ hΓ hΓ_mono h_closed_e.2.2)
  | binOp op e₁ e₂ ih₁ ih₂ =>
      cases h with
      | tBinOpIntArith hop h₁ h₂ =>
        exact .tBinOpIntArith hop (ih₁ h₁ hΓ hΓ_mono h_closed_e.1) (ih₂ h₂ hΓ hΓ_mono h_closed_e.2)
      | tBinOpIntCmp hop h₁ h₂ =>
        exact .tBinOpIntCmp hop (ih₁ h₁ hΓ hΓ_mono h_closed_e.1) (ih₂ h₂ hΓ hΓ_mono h_closed_e.2)
      | tBinOpBoolLogic hop h₁ h₂ =>
        exact .tBinOpBoolLogic hop (ih₁ h₁ hΓ hΓ_mono h_closed_e.1) (ih₂ h₂ hΓ hΓ_mono h_closed_e.2)
  | strConcat e₁ e₂ ih₁ ih₂ =>
      cases h with
      | tStrConcat h₁ h₂ =>
        exact .tStrConcat (ih₁ h₁ hΓ hΓ_mono h_closed_e.1) (ih₂ h₂ hΓ hΓ_mono h_closed_e.2)
  | unitVal => cases h; exact .tUnit

theorem preservation {e e' : Expr} {τ : Ty} (ht : HasType Context.empty e τ) (hs : Step e e')
    (h_closed : annotationsClosed e) :
    HasType Context.empty e' τ := by
  induction hs generalizing τ with
  | appFun hs₁ ih =>
      cases ht with
      | tApp hf ha => exact .tApp (ih hf h_closed.1) ha
  | appArg hv₂ hs₂ ih =>
      cases ht with
      | tApp hf ha => exact .tApp hf (ih ha h_closed.2)
  | appBeta hv =>
      rename_i x τ₀ e v
      cases ht with
      | tApp hf ha =>
          cases hf with
          | tLambda hbody =>
              exact substitution_lemma (Γ := Context.empty) hbody ha h_closed.1.1 rfl
                (by
                  intro x σ hl
                  simp [Context.lookup, Context.empty] at hl) h_closed.1.2 h_closed.2
  | letBind hs₁ ih =>
      cases ht with
      | tLet h₁ h₂ => exact .tLet (ih h₁ h_closed.1) h₂
  | letSubst hv =>
      cases ht with
      | tLet h₁ h₂ =>
        rename_i τ₁
        have hτ₁ : τ₁.fv = [] :=
          closed_type_under_closed_context h₁ rfl (by
            intro x σ hl
            simp [Context.lookup, Context.empty] at hl) h_closed.1
        have hσ : Scheme.generalize (Context.freeTypeVars Context.empty) τ₁ = ⟨[], τ₁⟩ := by
          unfold Scheme.generalize
          simp [hτ₁]
        exact substitution_lemma (Γ := Context.empty) (by simpa [hσ] using h₂) h₁ hτ₁ rfl
          (by
            intro x σ hl
            simp [Context.lookup, Context.empty] at hl) h_closed.2 h_closed.1
  | ifGuard hs₁ ih =>
      cases ht with
      | tIf h₁ h₂ h₃ => exact .tIf (ih h₁ h_closed.1) h₂ h₃
  | ifTrue =>
      cases ht with
      | tIf h₁ h₂ h₃ => exact h₂
  | ifFalse =>
      cases ht with
      | tIf h₁ h₂ h₃ => exact h₃
  | binOpLeft hs₁ ih =>
      cases ht with
      | tBinOpIntArith hop h₁ h₂ => exact .tBinOpIntArith hop (ih h₁ h_closed.1) h₂
      | tBinOpIntCmp hop h₁ h₂ => exact .tBinOpIntCmp hop (ih h₁ h_closed.1) h₂
      | tBinOpBoolLogic hop h₁ h₂ => exact .tBinOpBoolLogic hop (ih h₁ h_closed.1) h₂
  | binOpRight hv₂ hs₂ ih =>
      cases ht with
      | tBinOpIntArith hop h₁ h₂ => exact .tBinOpIntArith hop h₁ (ih h₂ h_closed.2)
      | tBinOpIntCmp hop h₁ h₂ => exact .tBinOpIntCmp hop h₁ (ih h₂ h_closed.2)
      | tBinOpBoolLogic hop h₁ h₂ => exact .tBinOpBoolLogic hop h₁ (ih h₂ h_closed.2)
  | binOpEval =>
      rename_i op n₁ n₂
      cases ht with
      | tBinOpIntArith hop h₁ h₂ =>
          have hop' : op = .add ∨ op = .sub ∨ op = .mul ∨ op = .div ∨ op = .mod := by
            simpa using hop
          rcases hop' with rfl | rfl | rfl | rfl | rfl <;> exact .tLitInt
      | tBinOpIntCmp hop h₁ h₂ =>
          have hop' : op = .eq ∨ op = .neq ∨ op = .lt ∨ op = .le ∨ op = .gt ∨ op = .ge := by
            simpa using hop
          rcases hop' with rfl | rfl | rfl | rfl | rfl | rfl <;> exact .tLitBool
      | tBinOpBoolLogic hop h₁ h₂ => cases h₁
  | binOpEvalBool hop =>
      rename_i op b₁ b₂
      cases ht with
      | tBinOpIntArith hop' h₁ h₂ => cases h₁
      | tBinOpIntCmp hop' h₁ h₂ => cases h₁
      | tBinOpBoolLogic hop' h₁ h₂ =>
          have hop₂ : op = .and ∨ op = .or := by simpa using hop'
          rcases hop₂ with rfl | rfl <;> exact .tLitBool
  | strConcatLeft hs₁ ih =>
      cases ht with
      | tStrConcat h₁ h₂ => exact .tStrConcat (ih h₁ h_closed.1) h₂
  | strConcatRight hv₂ hs₂ ih =>
      cases ht with
      | tStrConcat h₁ h₂ => exact .tStrConcat h₁ (ih h₂ h_closed.2)
  | strConcatEval =>
      cases ht with
      | tStrConcat h₁ h₂ => exact .tLitString

theorem type_soundness {e v : Expr} {τ : Ty}
    (ht : HasType Context.empty e τ)
    (hs : Steps e v)
    (hv : isValue v)
    (h_closed : annotationsClosed e) :
    HasType Context.empty v τ := by
  induction hs with
  | refl => exact ht
  | step hs₁ hs₂ ih =>
      rename_i e₁ e₂ e₃
      have h₂ : HasType Context.empty e₂ τ := preservation ht hs₁ h_closed
      have h₂cl : annotationsClosed e₂ := step_preserves_closed hs₁ h_closed
      exact ih h₂ hv h₂cl

end Nulang

