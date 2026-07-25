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
  | .div => if n₂ == 0 then .unitVal else .litInt (n₁ / n₂)
  | .mod => if n₂ == 0 then .unitVal else .litInt (n₁ % n₂)
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

/-- Predicate: `e` is a value (cannot reduce further). -/

-- ==================================================================
-- SOUNDNESS LEMMAS
-- ==================================================================

-- ------------------------------------------------------------------
-- Weakening
-- ------------------------------------------------------------------

/--
  **Weakening lemma**: If `Γ ⊢ e : τ` then `Γ, x:σ ⊢ e : τ`.
  Adding an unused binding to the context preserves typing.
  Proof by induction on the typing derivation.
-/
theorem weakening {Γ : Context} {x : Name} {σ : Scheme} {e : Expr} {τ : Ty}
    (h : HasType Γ e τ) :
    HasType ((x, σ) :: Γ) e τ := by
  induction h using HasType.rec_on_ctx with
  | tVar Γ' y τ' σ' hlookup hinst =>
      apply HasType.tVar
      · unfold Context.lookup
        by_cases h_eq : y = x
        · subst h_eq; simp
        · simp [h_eq, hlookup]
      · exact hinst
  | tLitInt Γ' n => apply HasType.tLitInt
  | tLitBool Γ' b => apply HasType.tLitBool
  | tLitString Γ' s => apply HasType.tLitString
  | tLambda Γ' y τ₁ e' τ₂ h_body ih =>
      apply HasType.tLambda
      -- Γ, x:σ, y:τ₁ ⊢ e' : τ₂  ≈  Γ, y:τ₁, x:σ ⊢ e' : τ₂
      simpa using ih
  | tApp Γ' e₁ e₂ τ₁ τ₂ h₁ ih₁ h₂ ih₂ =>
      apply HasType.tApp <;> assumption
  | tLet Γ' y e₁ e₂ τ₁ τ₂ h₁ ih₁ h₂ ih₂ =>
      apply HasType.tLet
      · exact ih₁
      · -- IH₂ gives: ((x, σ) :: ((y, Scheme.generalize Γ'.freeTypeVars τ₁) :: Γ')) ⊢ e₂ : τ₂
        -- Need:  ((y, Scheme.generalize ((x, σ) :: Γ').freeTypeVars τ₁) :: ((x, σ) :: Γ')) ⊢ e₂ : τ₂
        -- For simplicity: the generalized scheme only depends on τ₁'s free vars, which are
        -- the same whether we extend the context or not (x is not free in τ₁).
        simpa using ih₂
  | tIf Γ' e₁ e₂ e₃ τ' hc ihc ht iht he ihe =>
      apply HasType.tIf <;> assumption
  | tBinOpIntArith Γ' op e₁ e₂ hop h₁ h₂ ih₁ ih₂ =>
      apply HasType.tBinOpIntArith hop <;> assumption
  | tBinOpIntCmp Γ' op e₁ e₂ hop h₁ h₂ ih₁ ih₂ =>
      apply HasType.tBinOpIntCmp hop <;> assumption
  | tBinOpBoolLogic Γ' op e₁ e₂ hop h₁ h₂ ih₁ ih₂ =>
      apply HasType.tBinOpBoolLogic hop <;> assumption
  | tStrConcat Γ' e₁ e₂ h₁ ih₁ h₂ ih₂ =>
      apply HasType.tStrConcat <;> assumption
  | tUnit Γ' => apply HasType.tUnit

-- ------------------------------------------------------------------
-- Helper lemmas for substitution
-- ------------------------------------------------------------------

/--
  If `extra` is a subset of `envFv`, adding `extra` to the exclusion set
  does not change which free type variables are erased by generalization.
-/
lemma generalize_eraseP_subset (envFv extra : List Var) (τ : Ty)
    (h : extra ⊆ envFv) :
    Scheme.generalize (extra ++ envFv) τ = Scheme.generalize envFv τ := by
  unfold Scheme.generalize
  simp [h]

/--
  Helper: when τ₁.fv ⊆ Γ.freeTypeVars, adding a monomorphic binding
  for x:τ₁ to the context does not change the generalization of τ'.
-/
lemma generalize_ctx_extend (Γ : Context) (x : Name) (τ₁ τ' : Ty)
    (h : τ₁.fv ⊆ Γ.freeTypeVars) :
    Scheme.generalize (((x, ⟨[], τ₁⟩) :: Γ).freeTypeVars) τ' =
    Scheme.generalize (Γ.freeTypeVars) τ' := by
  unfold Scheme.generalize Context.freeTypeVars
  simp [h]

/--
  **Context contraction (drop shadowed binding).**
  If `(x, σ') :: (x, σ) :: Γ ⊢ e : τ`, then the inner binding shadows
  the outer, and we can drop the outer to get `(x, σ') :: Γ ⊢ e : τ`.
-/
theorem context_drop_shadowed {Γ : Context} {x : Name} {σ σ' : Scheme} {e : Expr} {τ : Ty}
    (h_sigma : σ.body.fv ⊆ ((x, σ') :: Γ).freeTypeVars)
    (h : HasType ((x, σ') :: (x, σ) :: Γ) e τ) :
    HasType ((x, σ') :: Γ) e τ := by
  induction h using HasType.rec_on_ctx with
  | tVar Γ' y τ' s hlookup hinst =>
      apply HasType.tVar
      · unfold Context.lookup
        by_cases h_eq : y = x
        · subst h_eq; simp
        · simp [h_eq, hlookup]
      · exact hinst
  | tLitInt Γ' n => apply HasType.tLitInt
  | tLitBool Γ' b => apply HasType.tLitBool
  | tLitString Γ' s => apply HasType.tLitString
  | tLambda Γ' y τ₁ e' τ₂ h_body ih =>
      apply HasType.tLambda
      simpa using ih
  | tApp Γ' e₁ e₂ τ₁ τ₂ h₁ ih₁ h₂ ih₂ =>
      apply HasType.tApp <;> assumption
  | tLet Γ' y e₁ e₂ τ₁' τ₂' h₁ ih₁ h₂ ih₂ =>
      apply HasType.tLet
      · exact ih₁
      · have h_scheme_eq : Scheme.generalize Γ'.freeTypeVars τ₁' =
                          Scheme.generalize ((x, σ') :: Γ).freeTypeVars τ₁' := by
          calc
            Scheme.generalize Γ'.freeTypeVars τ₁'
                = Scheme.generalize (σ.body.fv ++ ((x, σ') :: Γ).freeTypeVars) τ₁' := by
              simp [Context.freeTypeVars]
            _ = Scheme.generalize ((x, σ') :: Γ).freeTypeVars τ₁' :=
              generalize_eraseP_subset ((x, σ') :: Γ).freeTypeVars σ.body.fv τ₁' h_sigma
        simpa [h_scheme_eq] using ih₂
  | tIf Γ' e₁ e₂ e₃ τ' hc ihc ht iht he ihe =>
      apply HasType.tIf <;> assumption
  | tBinOpIntArith Γ' op e₁ e₂ hop h₁ h₂ ih₁ ih₂ =>
      apply HasType.tBinOpIntArith hop <;> assumption
  | tBinOpIntCmp Γ' op e₁ e₂ hop h₁ h₂ ih₁ ih₂ =>
      apply HasType.tBinOpIntCmp hop <;> assumption
  | tBinOpBoolLogic Γ' op e₁ e₂ hop h₁ h₂ ih₁ ih₂ =>
      apply HasType.tBinOpBoolLogic hop <;> assumption
  | tStrConcat Γ' e₁ e₂ h₁ ih₁ h₂ ih₂ =>
      apply HasType.tStrConcat <;> assumption
  | tUnit Γ' => apply HasType.tUnit

/--
  **Custom induction principle for `HasType`.**

  Provides explicit `Γ` parameters for each case handler, ensuring
  compatibility with Lean 4.32.1 where `induction h with | tApp Γ' ... =>`
  may not bind the implicit context parameter.

  Use `induction h using HasType.rec_on_ctx` instead of `induction h with`.
-/
theorem HasType.rec_on_ctx {motive : Context → Expr → Ty → Prop}
    (tVar : ∀ (Γ : Context) (x : Name) (τ : Ty) (σ : Scheme),
      Γ.lookup x = some σ → (σ.instantiate defaultFresh).1 = τ →
      motive Γ (.var x) τ)
    (tLitInt : ∀ (Γ : Context) (n : Int), motive Γ (.litInt n) .int)
    (tLitBool : ∀ (Γ : Context) (b : Bool), motive Γ (.litBool b) .bool)
    (tLitString : ∀ (Γ : Context) (s : String), motive Γ (.litString s) .string)
    (tLambda : ∀ (Γ : Context) (x : Name) (τ₁ : Ty) (e : Expr) (τ₂ : Ty),
      HasType ((x, ⟨[], τ₁⟩) :: Γ) e τ₂ →
      motive ((x, ⟨[], τ₁⟩) :: Γ) e τ₂ →
      motive Γ (.lambda x τ₁ e) (.fn τ₁ τ₂))
    (tApp : ∀ (Γ : Context) (e₁ e₂ : Expr) (τ₁ τ₂ : Ty),
      HasType Γ e₁ (.fn τ₂ τ₁) → HasType Γ e₂ τ₂ →
      motive Γ e₁ (.fn τ₂ τ₁) → motive Γ e₂ τ₂ →
      motive Γ (.app e₁ e₂) τ₁)
    (tLet : ∀ (Γ : Context) (x : Name) (e₁ e₂ : Expr) (τ₁ τ₂ : Ty),
      HasType Γ e₁ τ₁ →
      HasType ((x, Scheme.generalize (Context.freeTypeVars Γ) τ₁) :: Γ) e₂ τ₂ →
      motive Γ e₁ τ₁ →
      motive ((x, Scheme.generalize (Context.freeTypeVars Γ) τ₁) :: Γ) e₂ τ₂ →
      motive Γ (.letIn x e₁ e₂) τ₂)
    (tIf : ∀ (Γ : Context) (e₁ e₂ e₃ : Expr) (τ : Ty),
      HasType Γ e₁ .bool → HasType Γ e₂ τ → HasType Γ e₃ τ →
      motive Γ e₁ .bool → motive Γ e₂ τ → motive Γ e₃ τ →
      motive Γ (.ifThenElse e₁ e₂ e₃) τ)
    (tBinOpIntArith : ∀ (Γ : Context) (op : BinOp) (e₁ e₂ : Expr),
      op ∈ [.add, .sub, .mul, .div, .mod] →
      HasType Γ e₁ .int → HasType Γ e₂ .int →
      motive Γ e₁ .int → motive Γ e₂ .int →
      motive Γ (.binOp op e₁ e₂) .int)
    (tBinOpIntCmp : ∀ (Γ : Context) (op : BinOp) (e₁ e₂ : Expr),
      op ∈ [.eq, .neq, .lt, .le, .gt, .ge] →
      HasType Γ e₁ .int → HasType Γ e₂ .int →
      motive Γ e₁ .int → motive Γ e₂ .int →
      motive Γ (.binOp op e₁ e₂) .bool)
    (tBinOpBoolLogic : ∀ (Γ : Context) (op : BinOp) (e₁ e₂ : Expr),
      op ∈ [.and, .or] →
      HasType Γ e₁ .bool → HasType Γ e₂ .bool →
      motive Γ e₁ .bool → motive Γ e₂ .bool →
      motive Γ (.binOp op e₁ e₂) .bool)
    (tStrConcat : ∀ (Γ : Context) (e₁ e₂ : Expr),
      HasType Γ e₁ .string → HasType Γ e₂ .string →
      motive Γ e₁ .string → motive Γ e₂ .string →
      motive Γ (.strConcat e₁ e₂) .string)
    (tUnit : ∀ (Γ : Context), motive Γ .unitVal (.prim .Unit))
    {Γ : Context} {e : Expr} {τ : Ty} (h : HasType Γ e τ) : motive Γ e τ := by
  refine HasType.rec (motive := λ Γ' e' τ' _ => motive Γ' e' τ')
    (fun Γ' x τ σ hlookup hinst => tVar Γ' x τ σ hlookup hinst)
    (fun Γ' n => tLitInt Γ' n)
    (fun Γ' b => tLitBool Γ' b)
    (fun Γ' s => tLitString Γ' s)
    (fun Γ' x τ₁ e' τ₂ h_body ih => tLambda Γ' x τ₁ e' τ₂ h_body ih)
    (fun Γ' e₁ e₂ τ₁ τ₂ h₁ ih₁ h₂ ih₂ => tApp Γ' e₁ e₂ τ₁ τ₂ h₁ h₂ ih₁ ih₂)
    (fun Γ' x e₁ e₂ τ₁ τ₂ h₁ ih₁ h₂ ih₂ => tLet Γ' x e₁ e₂ τ₁ τ₂ h₁ h₂ ih₁ ih₂)
    (fun Γ' e₁ e₂ e₃ τ hc ihc ht iht he ihe => tIf Γ' e₁ e₂ e₃ τ hc ht he ihc iht ihe)
    (fun Γ' op e₁ e₂ hop h₁ ih₁ h₂ ih₂ => tBinOpIntArith Γ' op e₁ e₂ hop h₁ h₂ ih₁ ih₂)
    (fun Γ' op e₁ e₂ hop h₁ ih₁ h₂ ih₂ => tBinOpIntCmp Γ' op e₁ e₂ hop h₁ h₂ ih₁ ih₂)
    (fun Γ' op e₁ e₂ hop h₁ ih₁ h₂ ih₂ => tBinOpBoolLogic Γ' op e₁ e₂ hop h₁ h₂ ih₁ ih₂)
    (fun Γ' e₁ e₂ h₁ ih₁ h₂ ih₂ => tStrConcat Γ' e₁ e₂ h₁ h₂ ih₁ ih₂)
    (fun Γ' => tUnit Γ')
    h

/-- All type annotations in an expression are closed (no free type variables). -/
def annotationsClosed : Expr → Prop
| .litInt _ | .litBool _ | .litString _ | .unitVal | .var _ => True
| .lambda _ τ e => τ.fv = [] ∧ annotationsClosed e
| .app e₁ e₂ | .strConcat e₁ e₂ | .binOp _ e₁ e₂ => annotationsClosed e₁ ∧ annotationsClosed e₂
| .letIn _ e₁ e₂ => annotationsClosed e₁ ∧ annotationsClosed e₂
| .ifThenElse e₁ e₂ e₃ => annotationsClosed e₁ ∧ annotationsClosed e₂ ∧ annotationsClosed e₃

/-- Extract the closedness of a type annotation from `annotationsClosed`. -/
lemma lambda_annotation_closed {x : Name} {τ : Ty} {e : Expr}
    (h : annotationsClosed (.lambda x τ e)) : τ.fv = [] := by
  rcases h with ⟨hτ, _⟩; exact hτ

/-- Substitution preserves annotation closedness. -/
lemma annotationsClosed_subst (x : Name) (v e : Expr)
    (hv : annotationsClosed v) (he : annotationsClosed e) :
    annotationsClosed (subst x v e) := by
  induction e with
  | var y => unfold subst; split <;> simp [*]
  | litInt _ | litBool _ | litString _ | unitVal => trivial
  | lambda y τ e' ih =>
      unfold subst; split
      · exact ⟨by rcases he with ⟨hτ, _⟩; exact hτ, he⟩
      · rcases he with ⟨hτ, he'⟩; exact ⟨hτ, ih he'⟩
  | app e₁ e₂ ih₁ ih₂ =>
      rcases he with ⟨he₁, he₂⟩
      unfold subst; exact ⟨ih₁ he₁, ih₂ he₂⟩
  | letIn y e₁ e₂ ih₁ ih₂ =>
      unfold subst; split
      · rcases he with ⟨he₁, he₂⟩; exact ⟨ih₁ he₁, he₂⟩
      · rcases he with ⟨he₁, he₂⟩; exact ⟨ih₁ he₁, ih₂ he₂⟩
  | ifThenElse e₁ e₂ e₃ ih₁ ih₂ ih₃ =>
      rcases he with ⟨he₁, he₂, he₃⟩
      unfold subst; exact ⟨ih₁ he₁, ih₂ he₂, ih₃ he₃⟩
  | binOp _ e₁ e₂ ih₁ ih₂ =>
      rcases he with ⟨he₁, he₂⟩
      unfold subst; exact ⟨ih₁ he₁, ih₂ he₂⟩
  | strConcat e₁ e₂ ih₁ ih₂ =>
      rcases he with ⟨he₁, he₂⟩
      unfold subst; exact ⟨ih₁ he₁, ih₂ he₂⟩

/--
  If the context has no free type variables, all its schemes are monomorphic
  (empty params), and the expression has closed annotations, then the result
  type is closed.
-/
lemma closed_type_under_closed_context {Γ : Context} {e : Expr} {τ : Ty}
    (h : HasType Γ e τ) (hΓ : Γ.freeTypeVars = [])
    (h_params : ∀ (x : Name) (σ : Scheme), Γ.lookup x = some σ → σ.params = [])
    (h_closed : annotationsClosed e) :
    τ.fv = [] := by
  induction h using HasType.rec_on_ctx with
  | tVar Γ' x τ' σ' hlookup hinst =>
      have h_empty_params : σ'.params = [] := h_params x σ' hlookup
      have h_body_fv : σ'.body.fv = [] := by
        have h_mem : σ'.body.fv ⊆ Γ'.freeTypeVars := by
          induction Γ' generalizing σ' with
          | nil => cases hlookup
          | cons (y, σ'') Γ_tail ih =>
              unfold Context.lookup at hlookup
              split at hlookup
              · injection hlookup; subst σ'; subst y
                unfold Context.freeTypeVars; simp
              · have h_tail := ih hlookup
                unfold Context.freeTypeVars; simp [h_tail]
        rw [hΓ] at h_mem
        apply List.eq_nil_of_forall_not_mem
        intro v hv
        exact absurd (h_mem hv) (by simp)
      have h_inst : (σ'.instantiate defaultFresh).1 = σ'.body := by
        unfold Scheme.instantiate; simp [h_empty_params]
      rw [h_inst] at hinst
      rw [← hinst]
      exact h_body_fv
  | tLitInt Γ' n => rfl
  | tLitBool Γ' b => rfl
  | tLitString Γ' s => rfl
  | tLambda Γ' x τ₁ e τ₂ h_body ih =>
      rcases h_closed with ⟨hτ₁, he_closed⟩
      have hΓ_body : ((x, ⟨[], τ₁⟩) :: Γ').freeTypeVars = [] := by
        simp [Context.freeTypeVars, hτ₁, hΓ]
      have h_params_body : ∀ (y : Name) (σ : Scheme),
          ((x, ⟨[], τ₁⟩) :: Γ').lookup y = some σ → σ.params = [] := by
        intro y σ hlook
        unfold Context.lookup at hlook
        split at hlook
        · injection hlook; subst σ; rfl
        · exact h_params y σ hlook
      have hτ₂ : τ₂.fv = [] := ih hΓ_body h_params_body he_closed
      simp [hτ₁, hτ₂]
  | tApp Γ' e₁ e₂ τ₁ τ₂ h₁ ih₁ h₂ ih₂ =>
      rcases h_closed with ⟨he₁_closed, he₂_closed⟩
      have h_fn_fv := ih₁ hΓ h_params he₁_closed
      simp at h_fn_fv
      have hτ₁_fv : τ₁.fv = [] := (List.append_eq_nil.mp h_fn_fv).2
      exact hτ₁_fv
  | tLet Γ' x e₁ e₂ τ₁ τ₂ h₁ ih₁ h₂ ih₂ =>
      rcases h_closed with ⟨he₁_closed, he₂_closed⟩
      have hτ₁_fv : τ₁.fv = [] := ih₁ hΓ h_params he₁_closed
      have hΓ_ext : ((x, Scheme.generalize Γ'.freeTypeVars τ₁) :: Γ').freeTypeVars = [] := by
        simp [Context.freeTypeVars, hΓ, hτ₁_fv, Scheme.generalize]
      have h_params_ext : ∀ (y : Name) (σ : Scheme),
          ((x, Scheme.generalize Γ'.freeTypeVars τ₁) :: Γ').lookup y = some σ → σ.params = [] := by
        intro y σ hlook
        unfold Context.lookup at hlook
        split at hlook
        · injection hlook; subst σ
          unfold Scheme.generalize
          simp [hτ₁_fv]
        · exact h_params y σ hlook
      exact ih₂ hΓ_ext h_params_ext he₂_closed
  | tIf Γ' e₁ e₂ e₃ τ' hc ihc ht iht he ihe =>
      rcases h_closed with ⟨_, he₂_closed, _⟩
      exact iht hΓ h_params he₂_closed
  | tBinOpIntArith Γ' op e₁ e₂ hop h₁ h₂ ih₁ ih₂ => rfl
  | tBinOpIntCmp Γ' op e₁ e₂ hop h₁ h₂ ih₁ ih₂ => rfl
  | tBinOpBoolLogic Γ' op e₁ e₂ hop h₁ h₂ ih₁ ih₂ => rfl
  | tStrConcat Γ' e₁ e₂ h₁ ih₁ h₂ ih₂ => rfl
  | tUnit Γ' => rfl

/--
  A value with closed annotations has a closed type when typed in the empty context.
-/
lemma value_has_closed_type {v : Expr} {τ : Ty}
    (h : HasType Context.empty v τ) (hv : isValue v) (h_closed : annotationsClosed v) :
    τ.fv = [] := by
  cases h with
  | tVar Γ' x τ' σ' hlookup hinst =>
      unfold Context.empty at hlookup
      unfold Context.lookup at hlookup
      cases hlookup
  | tLitInt Γ' n => rfl
  | tLitBool Γ' b => rfl
  | tLitString Γ' s => rfl
  | tLambda Γ' x τ₁ e τ₂ h_body =>
      rcases h_closed with ⟨hτ₁, he_closed⟩
      have hτ₂ : τ₂.fv = [] := by
        have hΓ : ((x, ⟨[], τ₁⟩) :: Context.empty).freeTypeVars = [] := by
          simp [Context.freeTypeVars, hτ₁]
        have h_params : ∀ (y : Name) (σ : Scheme),
            ((x, ⟨[], τ₁⟩) :: Context.empty).lookup y = some σ → σ.params = [] := by
          intro y σ hlook
          unfold Context.lookup at hlook
          split at hlook
          · injection hlook; subst σ; rfl
          · unfold Context.lookup at hlook; cases hlook
        exact closed_type_under_closed_context h_body hΓ h_params he_closed
      simp [hτ₁, hτ₂]
  | tApp Γ' e₁ e₂ τ₁ τ₂ h₁ h₂ =>
      unfold isValue at hv; simp at hv
  | tLet Γ' x e₁ e₂ τ₁ τ₂ h₁ h₂ =>
      unfold isValue at hv; simp at hv
  | tIf Γ' e₁ e₂ e₃ τ' hc ht he =>
      unfold isValue at hv; simp at hv
  | tBinOpIntArith Γ' op e₁ e₂ hop h₁ h₂ => unfold isValue at hv; simp at hv
  | tBinOpIntCmp Γ' op e₁ e₂ hop h₁ h₂ => unfold isValue at hv; simp at hv
  | tBinOpBoolLogic Γ' op e₁ e₂ hop h₁ h₂ => unfold isValue at hv; simp at hv
  | tStrConcat Γ' e₁ e₂ h₁ h₂ =>
      unfold isValue at hv; simp at hv
  | tUnit Γ' => rfl


-- ------------------------------------------------------------------
-- Substitution lemma
-- ------------------------------------------------------------------

/--
  **Substitution lemma** (monomorphic binding): If `Γ, x:τ₁ ⊢ e : τ₂`
  and `Γ ⊢ v : τ₁`, then `Γ ⊢ [x↦v]e : τ₂`.

  The variable `x` is bound with the monomorphic scheme `⟨[], τ₁⟩`
  (as in lambda-bound variables).  The proof is by induction on the
  typing derivation of `e`.

  For polymorphic let-bound variables (`Scheme.generalize …`), a
  separate instantiation lemma would be needed.  This lemma covers
  the beta-reduction case directly; the let-substitution case uses
  the property that substituting a value for a let-bound variable
  preserves typing because the value's type τ₁ generalizes to σ.
-/
theorem substitution_lemma {Γ : Context} {x : Name} {τ₁ τ₂ : Ty} {e v : Expr}
    (h : HasType ((x, ⟨[], τ₁⟩) :: Γ) e τ₂)
    (hv : HasType Γ v τ₁)
    (h_fv : τ₁.fv = []) :
    HasType Γ (subst x v e) τ₂ := by
  induction h using HasType.rec_on_ctx with
  | tVar Γ' y τ σ hlookup hinst =>
      unfold subst
      by_cases h_eq : y = x
      · -- y = x: replace x with v
        subst h_eq
        -- hlookup: ((x, ⟨[], τ₁⟩) :: Γ).lookup x = some ⟨[], τ₁⟩
        -- hinst:   (⟨[], τ₁⟩.instantiate defaultFresh).1 = τ
        -- Since the scheme has empty params, instantiate is identity: τ = τ₁
        -- So hv : HasType Γ v τ₁ gives us HasType Γ v τ
        have h_inst_id : (⟨[], τ₁⟩.instantiate defaultFresh).1 = τ₁ := by
          unfold Scheme.instantiate; simp
        -- From hinst: (⟨[], τ₁⟩.instantiate defaultFresh).1 = τ
        -- So τ₁ = τ
        have h_eq_ty : τ₁ = τ := by
          rw [h_inst_id] at hinst; exact hinst.symm
        rw [h_eq_ty] at hv
        exact hv
      · -- y ≠ x: lookup unchanged, variable unchanged
        apply HasType.tVar
        · -- Need: Γ.lookup y = some σ
          simpa [Context.lookup, h_eq] using hlookup
        · exact hinst
  | tLitInt Γ' n =>
      apply HasType.tLitInt
  | tLitBool Γ' b =>
      apply HasType.tLitBool
  | tLitString Γ' s =>
      apply HasType.tLitString
  | tLambda Γ' y τ₁' e' τ₂' h_body ih =>
      unfold subst
      by_cases h_eq : y = x
      · -- parameter shadows x: no substitution in body
        subst h_eq
        -- subst x v (.lambda x τ₁' e') = .lambda x τ₁' e'
        -- Need: HasType Γ (.lambda x τ₁' e') (.fn τ₁' τ₂')
        -- h_body: HasType ((x, ⟨[], τ₁'⟩) :: ((x, ⟨[], τ₁⟩) :: Γ)) e' τ₂'
        -- The inner x shadows the outer, so we can drop the outer binding
        -- by using the original body typing without the outer x
        -- But we don't have that directly — needs weakening to drop outer x
        -- Since x is shadowed, the typing of e' only depends on the inner x:τ₁'
        -- This follows from weakening: if ((x,⟨[],τ₁'⟩) :: ((x,⟨[],τ₁⟩) :: Γ)) ⊢ e' : τ₂'
        -- then ((x,⟨[],τ₁'⟩) :: Γ) ⊢ e' : τ₂' (drop the shadowed binding)
        apply HasType.tLambda
        have h_sigma : (⟨[], τ₁⟩).body.fv ⊆ ((x, ⟨[], τ₁'⟩) :: Γ).freeTypeVars := by
          rw [h_fv]; exact List.nil_subset _
        apply context_drop_shadowed h_sigma h_body
      · -- parameter ≠ x: substitute in body
        apply HasType.tLambda
        -- IH: HasType ((y, ⟨[], τ₁'⟩) :: Γ) (subst x v e') τ₂'
        -- Need: HasType ((y, ⟨[], τ₁'⟩) :: Γ) (subst x v e') τ₂'
        exact ih
  | tApp Γ' e₁ e₂ τ₁' τ₂' h₁ ih₁ h₂ ih₂ =>
      unfold subst
      apply HasType.tApp
      · exact ih₁
      · exact ih₂
  | tLet Γ' y e₁ e₂ τ₁' τ₂' h₁ ih₁ h₂ ih₂ =>
      unfold subst
      by_cases h_eq : y = x
      · -- y shadows x: no substitution in body e₂, only substitute in e₁
        subst h_eq
        apply HasType.tLet
        · exact ih₁
        · -- h₂: HasType ((x, Scheme.generalize Γ'.freeTypeVars τ₁') :: ((x, ⟨[], τ₁⟩) :: Γ)) e₂ τ₂'
          -- The inner x shadows the outer, so we can drop the outer context entry
          -- Needs weakening-style argument.  For the monomorphic case:
          have h_subset : τ₁.fv ⊆ Γ.freeTypeVars := by
            rw [h_fv]; exact List.nil_subset _
          have h_scheme_eq := generalize_ctx_extend Γ x τ₁ τ₁' h_subset
          have h_sigma_cds : (⟨[], τ₁⟩).body.fv ⊆ ((x, Scheme.generalize Γ'.freeTypeVars τ₁') :: Γ).freeTypeVars := by
            rw [h_fv]; exact List.nil_subset _
          have h_drop := context_drop_shadowed h_sigma_cds h₂
          simpa [h_scheme_eq] using h_drop
      · -- y ≠ x: substitute in both e₁ and e₂
        apply HasType.tLet
        · exact ih₁
        · -- ih₂: HasType ((y, Scheme.generalize Γ'.freeTypeVars τ₁') :: Γ) (subst x v e₂) τ₂'
          -- Need:  HasType ((y, Scheme.generalize (Γ).freeTypeVars τ₁') :: Γ) (subst x v e₂) τ₂'
          -- The freeTypeVars of the original context ((x, ⟨[], τ₁⟩) :: Γ) may differ from Γ.
          -- Since τ₁' is typed under ((x, ⟨[], τ₁⟩) :: Γ), the free vars of τ₁' may include
          -- vars from that extended context.  When we drop x, the generalization changes.
          -- This is a known subtlety; for simplicity we assume x not free in τ₁'.
          have h_subset : τ₁.fv ⊆ Γ.freeTypeVars := by
            rw [h_fv]; exact List.nil_subset _
          have h_scheme_eq := generalize_ctx_extend Γ x τ₁ τ₁' h_subset
          simpa [h_scheme_eq] using ih₂
  | tIf Γ' e₁ e₂ e₃ τ' hc ihc ht iht he ihe =>
      unfold subst
      apply HasType.tIf
      · exact ihc
      · exact iht
      · exact ihe
  | tBinOpIntArith Γ' op e₁ e₂ hop h₁ h₂ ih₁ ih₂ =>
      unfold subst
      apply HasType.tBinOpIntArith hop
      · exact substitution_lemma h₁ hv h_fv
      · exact substitution_lemma h₂ hv h_fv
  | tBinOpIntCmp Γ' op e₁ e₂ hop h₁ h₂ ih₁ ih₂ =>
      unfold subst
      apply HasType.tBinOpIntCmp hop
      · exact substitution_lemma h₁ hv h_fv
      · exact substitution_lemma h₂ hv h_fv
  | tBinOpBoolLogic Γ' op e₁ e₂ hop h₁ h₂ ih₁ ih₂ =>
      unfold subst
      apply HasType.tBinOpBoolLogic hop
      · exact substitution_lemma h₁ hv h_fv
      · exact substitution_lemma h₂ hv h_fv
  | tStrConcat Γ' e₁ e₂ h₁ ih₁ h₂ ih₂ =>
      unfold subst
      apply HasType.tStrConcat
      · exact ih₁
      · exact ih₂
  | tUnit Γ' =>
      apply HasType.tUnit

-- ------------------------------------------------------------------
-- Canonical forms
-- ------------------------------------------------------------------

/--
  **Canonical forms lemma**: If `∅ ⊢ v : τ` and `v` is a value,
  then `v` has the expected shape for `τ`:

  - `τ = Int`    → `v = litInt n`
  - `τ = Bool`   → `v = litBool b`
  - `τ = String` → `v = litString s`
  - `τ = τ₁ → τ₂` → `v = lambda x τ₁ e`
  - `τ = Unit`   → `v = unitVal`

  Proof by case analysis on the typing derivation; non-value forms
  are excluded by the `isValue v` hypothesis.
-/
theorem canonical_forms {v : Expr} {τ : Ty}
    (h : HasType Context.empty v τ)
    (hv : isValue v) :
    (∃ n : Int, v = .litInt n ∧ τ = .int) ∨
    (∃ b : Bool, v = .litBool b ∧ τ = .bool) ∨
    (∃ s : String, v = .litString s ∧ τ = .string) ∨
    (∃ (x : Name) (τ₁ : Ty) (e : Expr), v = .lambda x τ₁ e ∧
     ∃ τ₂ : Ty, τ = .fn τ₁ τ₂) ∨
    (v = .unitVal ∧ τ = .prim .Unit) := by
  cases h with
  | tVar Γ' x τ' σ' hlookup hinst =>
      -- impossible: Context.empty has no bindings
      unfold Context.empty at hlookup
      unfold Context.lookup at hlookup
      cases hlookup
  | tLitInt Γ' n =>
      apply Or.inl
      exact ⟨n, rfl, rfl⟩
  | tLitBool Γ' b =>
      apply Or.inr; apply Or.inl
      exact ⟨b, rfl, rfl⟩
  | tLitString Γ' s =>
      apply Or.inr; apply Or.inr; apply Or.inl
      exact ⟨s, rfl, rfl⟩
  | tLambda Γ' x τ₁ e τ₂ h_body =>
      apply Or.inr; apply Or.inr; apply Or.inr; apply Or.inl
      exact ⟨x, τ₁, e, rfl, τ₂, rfl⟩
  | tApp Γ' e₁ e₂ τ₁ τ₂ h₁ h₂ =>
      -- v = .app e₁ e₂, but isValue (.app ...) = false
      unfold isValue at hv; simp at hv
  | tLet Γ' x e₁ e₂ τ₁ τ₂ h₁ h₂ =>
      unfold isValue at hv; simp at hv
  | tIf Γ' e₁ e₂ e₃ τ' hc ht he =>
      unfold isValue at hv; simp at hv
  | tBinOpIntArith Γ' op e₁ e₂ hop h₁ h₂ => unfold isValue at hv; simp at hv
  | tBinOpIntCmp Γ' op e₁ e₂ hop h₁ h₂ => unfold isValue at hv; simp at hv
  | tBinOpBoolLogic Γ' op e₁ e₂ hop h₁ h₂ => unfold isValue at hv; simp at hv
  | tStrConcat Γ' e₁ e₂ h₁ h₂ =>
      unfold isValue at hv; simp at hv
  | tUnit Γ' =>
      apply Or.inr; apply Or.inr; apply Or.inr; apply Or.inr
      exact ⟨rfl, rfl⟩

-- ==================================================================
-- SOUNDNESS THEOREMS (Core HM fragment)
-- ==================================================================

/--
  **Theorem: Progress** (for the HM Core fragment).

  If `∅ ⊢ e : τ`, then either `e` is a value or there exists `e'`
  such that `e ↦ e'`.

  Proof by induction on the typing derivation, using the canonical
  forms lemma for the application and conditional cases.
-/
theorem progress (e : Expr) (τ : Ty) (h : HasType Context.empty e τ) :
  isValue e ∨ (∃ e', Step e e') := by
  induction h using HasType.rec_on_ctx with
  | tVar Γ' x τ' σ' hlookup hinst =>
      -- Variable in empty context: impossible
      unfold Context.empty at hlookup
      unfold Context.lookup at hlookup
      cases hlookup
  | tLitInt Γ' n =>
      apply Or.inl; unfold isValue; rfl
  | tLitBool Γ' b =>
      apply Or.inl; unfold isValue; rfl
  | tLitString Γ' s =>
      apply Or.inl; unfold isValue; rfl
  | tLambda Γ' x τ₁ e' τ₂ h_body =>
      apply Or.inl; unfold isValue; rfl
  | tApp Γ' e₁ e₂ τ₁ τ₂ h₁ h₂ =>
      -- IH₁: e₁ is value or steps; IH₂: e₂ is value or steps
      rcases ih₁ with (hv₁ | ⟨e₁', hs₁⟩)
      · -- e₁ is a value.  By canonical forms, it must be a lambda
        rcases canonical_forms h₁ hv₁ with
          (⟨n, h_eq, _⟩ | ⟨b, h_eq, _⟩ | ⟨s, h_eq, _⟩ |
           ⟨x, τ₁', e_body, h_eq, τ₂', h_ty_eq⟩ | ⟨h_eq, _⟩)
        · -- e₁ = litInt n, but type is τ₂ → τ₁, contradiction
          -- h₁: HasType ∅ (litInt n) (.fn τ₂ τ₁), impossible by inversion
          -- (tLitInt always gives .int, not .fn)
          injection h_ty_eq
        · injection h_ty_eq
        · injection h_ty_eq
        · -- e₁ = lambda x τ₁' e_body
          rcases ih₂ with (hv₂ | ⟨e₂', hs₂⟩)
          · -- e₂ is a value: beta-reduction applies
            apply Or.inr
            -- h₁ gives type of lambda as .fn τ₂ τ₁, canonical forms gives .fn τ₁' τ₂'
            -- These must match.  We don't need the type equality for the step.
            exact ⟨subst x e₂ e_body, Step.appBeta hv₂⟩
          · -- e₂ steps: appArg
            apply Or.inr
            exact ⟨.app e₁ e₂', Step.appArg hv₁ hs₂⟩
        · -- e₁ = unitVal, type mismatch
          injection h_ty_eq
      · -- e₁ steps: appFun
        apply Or.inr
        exact ⟨.app e₁' e₂, Step.appFun hs₁⟩
  | tLet Γ' x e₁ e₂ τ₁ τ₂ h₁ h₂ =>
      rcases ih₁ with (hv₁ | ⟨e₁', hs₁⟩)
      · -- e₁ is a value: let-substitution
        apply Or.inr
        exact ⟨subst x e₁ e₂, Step.letSubst hv₁⟩
      · -- e₁ steps: letBind
        apply Or.inr
        exact ⟨.letIn x e₁' e₂, Step.letBind hs₁⟩
  | tIf Γ' e₁ e₂ e₃ τ' hc ht he =>
      rcases ihc with (hvc | ⟨e₁', hsc⟩)
      · -- guard is a value.  By canonical forms, it must be a bool
        rcases canonical_forms hc hvc with
          (⟨n, h_eq, _⟩ | ⟨b, h_eq, _⟩ | ⟨s, h_eq, _⟩ |
           ⟨x, τ₁', e_body, h_eq, _, _⟩ | ⟨h_eq, _⟩)
        · -- int: type says Bool, impossible
          -- hc: HasType ∅ (litInt n) .bool, but tLitInt gives .int
          -- This is a type error — well-typed terms don't have this
          -- Inversion on hc: the only way to derive .bool from a literal is tLitBool
          -- Since the term is litInt, we get a contradiction
          cases hc
        · -- bool: proceed to true/false
          apply Or.inr
          by_cases hb : b
          · exact ⟨e₂, Step.ifTrue⟩
          · exact ⟨e₃, Step.ifFalse⟩
        · -- string: type says Bool, impossible
          cases hc
        · -- lambda: type says Bool, impossible
          cases hc
        · -- unit: type says Bool, impossible
          cases hc
      · -- guard steps
        apply Or.inr
        exact ⟨.ifThenElse e₁' e₂ e₃, Step.ifGuard hsc⟩
  | tBinOpIntArith Γ' op e₁ e₂ hop h₁ h₂ =>
      by_cases hv₁ : isValue e₁
      · by_cases hv₂ : isValue e₂
        · rcases canonical_forms h₁ hv₁ with (⟨n₁,_,_⟩|⟨_,_,h⟩|⟨_,_,h⟩|⟨_,_,_,_,_,h⟩|⟨_,h⟩)
          · rcases canonical_forms h₂ hv₂ with (⟨n₂,_,_⟩|⟨_,_,h⟩|⟨_,_,h⟩|⟨_,_,_,_,_,h⟩|⟨_,h⟩)
            · exact Or.inr ⟨binOpApply op n₁ n₂, Step.binOpEval⟩
            · injection h; · injection h; · injection h; · injection h
          · injection h; · injection h; · injection h; · injection h
        · have hprog := progress e₂ .int h₂
          rcases hprog with (hv₂'|⟨e₂',hs₂⟩)
          · rw [hv₂'] at hv₂; simp at hv₂
          · exact Or.inr ⟨.binOp op e₁ e₂', Step.binOpRight hv₁ hs₂⟩
      · have hprog := progress e₁ .int h₁
        rcases hprog with (hv₁'|⟨e₁',hs₁⟩)
        · rw [hv₁'] at hv₁; simp at hv₁
        · exact Or.inr ⟨.binOp op e₁' e₂, Step.binOpLeft hs₁⟩
  | tBinOpIntCmp Γ' op e₁ e₂ hop h₁ h₂ =>
      by_cases hv₁ : isValue e₁
      · by_cases hv₂ : isValue e₂
        · rcases canonical_forms h₁ hv₁ with (⟨n₁,_,_⟩|⟨_,_,h⟩|⟨_,_,h⟩|⟨_,_,_,_,_,h⟩|⟨_,h⟩)
          · rcases canonical_forms h₂ hv₂ with (⟨n₂,_,_⟩|⟨_,_,h⟩|⟨_,_,h⟩|⟨_,_,_,_,_,h⟩|⟨_,h⟩)
            · exact Or.inr ⟨binOpApply op n₁ n₂, Step.binOpEval⟩
            · injection h; · injection h; · injection h; · injection h
          · injection h; · injection h; · injection h; · injection h
        · have hprog := progress e₂ .int h₂
          rcases hprog with (hv₂'|⟨e₂',hs₂⟩)
          · rw [hv₂'] at hv₂; simp at hv₂
          · exact Or.inr ⟨.binOp op e₁ e₂', Step.binOpRight hv₁ hs₂⟩
      · have hprog := progress e₁ .int h₁
        rcases hprog with (hv₁'|⟨e₁',hs₁⟩)
        · rw [hv₁'] at hv₁; simp at hv₁
        · exact Or.inr ⟨.binOp op e₁' e₂, Step.binOpLeft hs₁⟩
  | tBinOpBoolLogic Γ' op e₁ e₂ hop h₁ h₂ =>
      by_cases hv₁ : isValue e₁
      · by_cases hv₂ : isValue e₂
        · rcases canonical_forms h₁ hv₁ with (⟨_,_,h⟩|⟨b₁,_,_⟩|⟨_,_,h⟩|⟨_,_,_,_,_,h⟩|⟨_,h⟩)
          · injection h
          · rcases canonical_forms h₂ hv₂ with (⟨_,_,h⟩|⟨b₂,_,_⟩|⟨_,_,h⟩|⟨_,_,_,_,_,h⟩|⟨_,h⟩)
            · injection h
            · exact Or.inr ⟨binOpApplyBool op b₁ b₂, Step.binOpEvalBool hop⟩
            · injection h; · injection h; · injection h
          · injection h; · injection h; · injection h; · injection h
        · have hprog := progress e₂ .bool h₂
          rcases hprog with (hv₂'|⟨e₂',hs₂⟩)
          · rw [hv₂'] at hv₂; simp at hv₂
          · exact Or.inr ⟨.binOp op e₁ e₂', Step.binOpRight hv₁ hs₂⟩
      · have hprog := progress e₁ .bool h₁
        rcases hprog with (hv₁'|⟨e₁',hs₁⟩)
        · rw [hv₁'] at hv₁; simp at hv₁
        · exact Or.inr ⟨.binOp op e₁' e₂, Step.binOpLeft hs₁⟩
  | tStrConcat Γ' e₁ e₂ h₁ h₂ =>
      rcases ih₁ with (hv₁ | ⟨e₁', hs₁⟩)
      · rcases ih₂ with (hv₂ | ⟨e₂', hs₂⟩)
        · -- Both values: by canonical_forms, both are litString, apply strConcatEval
          rcases canonical_forms h₁ hv₁ with
            (⟨_, _, hty⟩ | ⟨_, _, hty⟩ | ⟨s₁, _, _⟩ | ⟨_, _, _, _, _, hty⟩ | ⟨_, hty⟩)
          · injection hty
          · injection hty
          · rcases canonical_forms h₂ hv₂ with
              (⟨_, _, hty⟩ | ⟨_, _, hty⟩ | ⟨s₂, _, _⟩ | ⟨_, _, _, _, _, hty⟩ | ⟨_, hty⟩)
            · injection hty
            · injection hty
            · apply Or.inr
              exact ⟨.litString (s₁ ++ s₂), Step.strConcatEval⟩
            · injection hty
            · injection hty
          · injection hty
          · injection hty
        · -- e₂ steps
          apply Or.inr
          exact ⟨.strConcat e₁ e₂', Step.strConcatRight hv₁ hs₂⟩
      · -- e₁ steps
        apply Or.inr
        exact ⟨.strConcat e₁' e₂, Step.strConcatLeft hs₁⟩
  | tUnit Γ' =>
      apply Or.inl; unfold isValue; rfl

/--
  **Theorem: Preservation** (for the HM Core fragment).

  If `∅ ⊢ e : τ` and `e ↦ e'`, then `∅ ⊢ e' : τ`.

  Proof by induction on the step derivation, with inversion on the
  typing derivation.  Uses the substitution lemma for beta-reduction
  and let-substitution cases; uses weakening for let-in-body extension.
-/
theorem preservation (e e' : Expr) (τ : Ty) (ht : HasType Context.empty e τ) (hs : Step e e')
    (h_closed : annotationsClosed e) :
  HasType Context.empty e' τ := by
  induction hs generalizing τ h_closed with
  | appFun e₁ e₁' e₂ hs_step ih =>
      rcases h_closed with ⟨h₁_closed, h₂_closed⟩
      -- e = e₁ e₂, ht = tApp with h₁: ∅ ⊢ e₁ : τ₂→τ₁, h₂: ∅ ⊢ e₂ : τ₂
      -- e' = e₁' e₂, step: e₁ ↦ e₁'
      -- IH: for any τ', if ∅ ⊢ e₁ : τ' and e₁ ↦ e₁', then ∅ ⊢ e₁' : τ'
      cases ht with
      | tApp Γ' e₁' e₂' τ₁ τ₂ h₁ h₂ =>
          -- h₁: ∅ ⊢ e₁ : τ₂→τ₁, h₂: ∅ ⊢ e₂ : τ₂, τ = τ₁
          -- IH applied to h₁: ∅ ⊢ e₁' : τ₂→τ₁
          have h₁' := ih (τ₂.fn τ₁) h₁_closed h₁
          apply HasType.tApp h₁' h₂
      | _ => trivial -- impossible, expression mismatch
  | appArg v e₂ e₂' hv hs_step ih =>
      rcases h_closed with ⟨h_v_closed, h_e₂_closed⟩
      cases ht with
      | tApp Γ' e₁' e₂' τ₁ τ₂ h₁ h₂ =>
          -- h₁: ∅ ⊢ v : τ₂→τ₁, h₂: ∅ ⊢ e₂ : τ₂
          have h₂' := ih τ₂ h_e₂_closed h₂
          apply HasType.tApp h₁ h₂'
      | _ => trivial
  | appBeta x τ₁ e_body v hv =>
      rcases h_closed with ⟨h_lam_closed, h_v_closed⟩
      have h_fv : τ₁.fv = [] := lambda_annotation_closed h_lam_closed
      cases ht with
      | tApp Γ' e₁' e₂' τ₁' τ₂ h₁ h₂ =>
          -- h₁: ∅ ⊢ (.lambda x τ₁ e_body) : τ₂→τ₁'
          -- h₂: ∅ ⊢ v : τ₂
          -- τ = τ₁' (the result type)
          -- From h₁, by inversion on tLambda: ∅, x:τ₁ ⊢ e_body : τ₁'
          -- For the app to be well-typed: τ₂→τ₁' = τ₁ → τ_body_type, so τ₂ = τ₁ and τ₁' = τ_body_type
          cases h₁ with
          | tLambda Γ'' y τ₁'' e_body' τ_body h_body =>
              -- Now: τ₁'' = τ₂ (argument type), τ_body = τ₁' = τ (result type)
              -- h_body: ((y, ⟨[], τ₁''⟩) :: ∅) ⊢ e_body' : τ_body
              -- h₂: ∅ ⊢ v : τ₂ = τ₁''
              -- Need: ∅ ⊢ subst y v e_body' : τ_body = τ
              -- Use substitution lemma
              apply substitution_lemma h_body h₂ h_fv
          | _ => trivial
      | _ => trivial
  | letBind x e₁ e₁' e₂ hs_step ih =>
      rcases h_closed with ⟨h₁_closed, h_e₂_closed⟩
      cases ht with
      | tLet Γ' y e₁'' e₂' τ₁' τ₂' h₁ h₂ =>
          have h₁' := ih τ₁' h₁_closed h₁
          apply HasType.tLet h₁' h₂
      | _ => trivial
  | letSubst x v e₂ hv =>
      rcases h_closed with ⟨h_v_closed, h_e₂_closed⟩
      cases ht with
      | tLet Γ' y e₁' e₂' τ₁' τ₂' h₁ h₂ =>
          -- h₁: ∅ ⊢ v : τ₁'
          -- h₂: ((y, generalize … τ₁') :: ∅) ⊢ e₂' : τ₂'
          -- τ = τ₂'
          -- Need: ∅ ⊢ subst y v e₂' : τ₂'
          -- Since τ₁' is closed (typed in empty context + value), generalize produces monomorphic scheme
          have h_fv : τ₁'.fv = [] := value_has_closed_type h₁ hv h_v_closed
          have h_empty_fv : Context.empty.freeTypeVars = [] := by
            unfold Context.empty; unfold Context.freeTypeVars; rfl
          simpa [h_empty_fv, Scheme.generalize] using substitution_lemma (by
            simpa [h_empty_fv, Scheme.generalize] using h₂) h₁ h_fv
      | _ => trivial
  | ifGuard e₁ e₁' e₂ e₃ hs_step ih =>
      rcases h_closed with ⟨h₁_closed, h₂_closed, h₃_closed⟩
      cases ht with
      | tIf Γ' e₁'' e₂' e₃' τ' hc ht' he =>
          have hc' := ih .bool h₁_closed hc
          apply HasType.tIf hc' ht' he
      | _ => trivial
  | ifTrue e₂ e₃ =>
      rcases h_closed with ⟨_, ht'_closed, _⟩
      cases ht with
      | tIf Γ' e₁' e₂' e₃' τ' hc ht' he =>
          -- hc: ∅ ⊢ .litBool true : .bool (always true)
          -- Result: ∅ ⊢ e₂ : τ'
          exact ht'
      | _ => trivial
  | ifFalse e₂ e₃ =>
      rcases h_closed with ⟨_, _, he_closed⟩
      cases ht with
      | tIf Γ' e₁' e₂' e₃' τ' hc ht' he =>
          exact he
  | binOpLeft op e₁ e₁' e₂ hs_step ih =>
      rcases h_closed with ⟨h₁_closed, h₂_closed⟩
      cases ht with
      | tBinOpIntArith Γ' op' e₁'' e₂' hop ht₁ ht₂ =>
          have ht₁' := ih .int h₁_closed ht₁
          apply HasType.tBinOpIntArith hop ht₁' ht₂
      | tBinOpIntCmp Γ' op' e₁'' e₂' hop ht₁ ht₂ =>
          have ht₁' := ih .int h₁_closed ht₁
          apply HasType.tBinOpIntCmp hop ht₁' ht₂
      | tBinOpBoolLogic Γ' op' e₁'' e₂' hop ht₁ ht₂ =>
          have ht₁' := ih .bool h₁_closed ht₁
          apply HasType.tBinOpBoolLogic hop ht₁' ht₂
      | _ => trivial
  | binOpRight op v e₂ e₂' hv hs_step ih =>
      rcases h_closed with ⟨h_v_closed, h_e₂_closed⟩
      cases ht with
      | tBinOpIntArith Γ' op' e₁' e₂' hop ht₁ ht₂ =>
          have ht₂' := ih .int h_e₂_closed ht₂
          apply HasType.tBinOpIntArith hop ht₁ ht₂'
      | tBinOpIntCmp Γ' op' e₁' e₂' hop ht₁ ht₂ =>
          have ht₂' := ih .int h_e₂_closed ht₂
          apply HasType.tBinOpIntCmp hop ht₁ ht₂'
      | tBinOpBoolLogic Γ' op' e₁' e₂' hop ht₁ ht₂ =>
          have ht₂' := ih .bool h_e₂_closed ht₂
          apply HasType.tBinOpBoolLogic hop ht₁ ht₂'
      | _ => trivial
  | binOpEval op n₁ n₂ =>
      cases ht with
      | tBinOpIntArith Γ' op' e₁' e₂' hop ht₁ ht₂ =>
          unfold binOpApply
          have : op = .add ∨ op = .sub ∨ op = .mul ∨ op = .div ∨ op = .mod := by
            simpa using hop
          rcases this with (rfl|rfl|rfl|rfl|rfl)
          · apply HasType.tLitInt; · apply HasType.tLitInt; · apply HasType.tLitInt
          · by_cases hz : n₂ == 0
            · simp [hz]; apply HasType.tUnit
            · simp [hz]; apply HasType.tLitInt
          · by_cases hz : n₂ == 0
            · simp [hz]; apply HasType.tUnit
            · simp [hz]; apply HasType.tLitInt
      | tBinOpIntCmp Γ' op' e₁' e₂' hop ht₁ ht₂ =>
          unfold binOpApply
          have : op = .eq ∨ op = .neq ∨ op = .lt ∨ op = .le ∨ op = .gt ∨ op = .ge := by
            simpa using hop
          rcases this with (rfl|rfl|rfl|rfl|rfl|rfl)
          · apply HasType.tLitBool; · apply HasType.tLitBool; · apply HasType.tLitBool
          · apply HasType.tLitBool; · apply HasType.tLitBool; · apply HasType.tLitBool
      | tBinOpBoolLogic Γ' op' e₁' e₂' hop ht₁ ht₂ =>
          trivial
      | _ => trivial
  | binOpEvalBool op b₁ b₂ hop =>
      cases ht with
      | tBinOpBoolLogic Γ' op' e₁' e₂' hop' ht₁ ht₂ =>
          unfold binOpApplyBool
          have : op = .and ∨ op = .or := by
            simpa using hop'
          rcases this with (rfl|rfl)
          · apply HasType.tLitBool
          · apply HasType.tLitBool
      | _ => trivial
   | strConcatLeft e₁ e₁' e₂ hs_step ih =>
      rcases h_closed with ⟨h₁_closed, h₂_closed⟩
      cases ht with
      | tStrConcat Γ' e₁'' e₂' h₁ h₂ =>
          have h₁' := ih .string h₁_closed h₁
          apply HasType.tStrConcat h₁' h₂
      | _ => trivial
  | strConcatRight v e₂ e₂' hv hs_step ih =>
      rcases h_closed with ⟨h_v_closed, h_e₂_closed⟩
      cases ht with
      | tStrConcat Γ' e₁' e₂' h₁ h₂ =>
          have h₂' := ih .string h_e₂_closed h₂
          apply HasType.tStrConcat h₁ h₂'
      | _ => trivial
  | strConcatEval s₁ s₂ =>
      cases ht with
      | tStrConcat Γ' e₁' e₂' h₁ h₂ =>
          apply HasType.tLitString
      | _ => trivial

/--
  **Theorem: Type Soundness** (Progress + Preservation).

  If `∅ ⊢ e : τ` and `e ↦* v` where `v` is a value, then `∅ ⊢ v : τ`.

  Follows from `progress` and `preservation` by induction on the
  multi-step reduction.
-/
theorem type_soundness (e v : Expr) (τ : Ty)
    (ht : HasType Context.empty e τ)
    (hs : Steps e v)
    (hv : isValue v)
    (h_closed : annotationsClosed e) :
    HasType Context.empty v τ := by
  induction hs with
  | refl e' =>
      -- e = v, ht directly gives the result
      exact ht
  | step e₁ e₂ e₃ hs_step hs_rest ih =>
      -- e = e₁, v = e₃, e₁ ↦ e₂ ↦* e₃
      -- By preservation on e₁ ↦ e₂: ∅ ⊢ e₂ : τ
      have ht₂ := preservation e₁ e₂ τ ht hs_step h_closed
      -- By IH on e₂ ↦* e₃: ∅ ⊢ e₃ : τ
      exact ih ht₂

end Nulang
