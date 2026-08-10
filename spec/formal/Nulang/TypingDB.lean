import Nulang.Types
import Nulang.SyntaxDB

/- Typing rules with de Bruijn indices.
   Includes weakening and context exchange lemmas.
   Uses swapAt_shift from SyntaxDB for binder cases. -/

namespace Nulang.TypingDB

open Nulang.Types (Ty)
open Nulang.SyntaxDB (Expr BinOp UnOp shift swapAt swap01)
open Nulang.SyntaxDB (swapAt_shift)

abbrev Ctx := List Ty

def Ctx.get (Γ : Ctx) (n : Nat) : Option Ty := Γ.get? n

def binopType : BinOp → Ty × Ty × Ty
  | .add => (.int, .int, .int)  | .sub => (.int, .int, .int)
  | .mul => (.int, .int, .int)  | .div => (.int, .int, .int)
  | .mod => (.int, .int, .int)
  | .eq  => (.bool, .int, .int) | .neq => (.bool, .int, .int)
  | .lt  => (.bool, .int, .int) | .le  => (.bool, .int, .int)
  | .gt  => (.bool, .int, .int) | .ge  => (.bool, .int, .int)
  | .and => (.bool, .bool, .bool) | .or => (.bool, .bool, .bool)

def unopType : UnOp → Ty × Ty
  | .neg => (.int, .int) | .not => (.bool, .bool)

inductive hasType : Ctx → Expr → Ty → Prop where
  | intLit (Γ : Ctx) (n : Int) : hasType Γ (.intLit n) .int
  | boolLit (Γ : Ctx) (b : Bool) : hasType Γ (.boolLit b) .bool
  | stringLit (Γ : Ctx) (s : String) : hasType Γ (.stringLit s) .string
  | unitLit (Γ : Ctx) : hasType Γ .unitLit .unit
  | nilLit (Γ : Ctx) : hasType Γ .nilLit .nil
  | var (Γ : Ctx) (n : Nat) (τ : Ty) (h : Γ.get? n = some τ) : hasType Γ (.var n) τ
  | binop (Γ : Ctx) (op : BinOp) (e₁ e₂ : Expr) (τ₁ τ₂ τ : Ty)
      (h₁ : hasType Γ e₁ τ₁) (h₂ : hasType Γ e₂ τ₂) (hop : binopType op = (τ, τ₁, τ₂)) :
      hasType Γ (.binop op e₁ e₂) τ
  | unop (Γ : Ctx) (op : UnOp) (e : Expr) (τ₁ τ : Ty)
      (h : hasType Γ e τ₁) (hop : unopType op = (τ, τ₁)) : hasType Γ (.unop op e) τ
  | letE (Γ : Ctx) (e₁ e₂ : Expr) (τ₁ τ₂ : Ty)
      (h₁ : hasType Γ e₁ τ₁) (h₂ : hasType (τ₁ :: Γ) e₂ τ₂) : hasType Γ (.letE e₁ e₂) τ₂
  | ifE (Γ : Ctx) (c t e : Expr) (τ : Ty)
      (hc : hasType Γ c .bool) (ht : hasType Γ t τ) (he : hasType Γ e τ) :
      hasType Γ (.ifE c t e) τ
  | app (Γ : Ctx) (f a : Expr) (τ₁ τ₂ : Ty)
      (hf : hasType Γ f (.fn τ₁ τ₂)) (ha : hasType Γ a τ₁) : hasType Γ (.app f a) τ₂
  | lam (Γ : Ctx) (body : Expr) (τ₁ τ₂ : Ty)
      (hbody : hasType (τ₁ :: Γ) body τ₂) : hasType Γ (.lam body) (.fn τ₁ τ₂)
  | returnE (Γ : Ctx) (e : Expr) (τ : Ty) (h : hasType Γ e τ) :
      hasType Γ (.returnE e) τ
  | block_nil (Γ : Ctx) : hasType Γ .block_nil .unit
  | block_cons (Γ : Ctx) (e es : Expr) (τ : Ty)
      (hfirst : hasType Γ e τ) (hrest : hasType Γ es τ) :
      hasType Γ (.block_cons e es) τ

/--
Context exchange: swapping the first two context entries preserves typing
under swapAt 0 (swap01) renaming. Indices 0↔1 are swapped in the expression.
-/
theorem context_exchange (Γ : Ctx) (σ τ : Ty) (e : Expr) (ρ : Ty)
    (h : hasType (σ :: τ :: Γ) e ρ) : hasType (τ :: σ :: Γ) (swap01 e) ρ := by
  induction h generalizing σ τ with
  | intLit Γ' n =>
      exact hasType.intLit (τ :: σ :: Γ') n
  | boolLit Γ' b =>
      exact hasType.boolLit (τ :: σ :: Γ') b
  | stringLit Γ' s =>
      exact hasType.stringLit (τ :: σ :: Γ') s
  | unitLit Γ' =>
      exact hasType.unitLit (τ :: σ :: Γ')
  | nilLit Γ' =>
      exact hasType.nilLit (τ :: σ :: Γ')
  | @var Γ' n ρ' h_lookup =>
      -- h_lookup : (σ :: τ :: Γ').get? n = some ρ'
      -- swap01 (.var n) = if n=0 then .var 1 else if n=1 then .var 0 else .var n
      simp [swap01, swapAt]
      by_cases h0 : n = 0
      · subst h0
        simp [Ctx.get, List.get?] at h_lookup ⊢
        exact hasType.var (τ :: σ :: Γ') 1 ρ' (by simpa using h_lookup)
      · by_cases h1 : n = 1
        · subst h1
          simp [Ctx.get, List.get?] at h_lookup ⊢
          exact hasType.var (τ :: σ :: Γ') 0 ρ' (by simpa using h_lookup)
        · -- n ≥ 2: index unchanged
          simp [Ctx.get, List.get?, h0, h1] at h_lookup ⊢
          simpa using h_lookup
  | binop Γ' op e₁ e₂ τ₁ τ₂ ρ' h₁ h₂ hop =>
      simp [swap01, swapAt]
      exact hasType.binop (τ :: σ :: Γ') op _ _ τ₁ τ₂ ρ'
        (context_exchange Γ' σ τ e₁ τ₁ h₁) (context_exchange Γ' σ τ e₂ τ₂ h₂) hop
  | unop Γ' op e τ₁ ρ' h hop =>
      simp [swap01, swapAt]
      exact hasType.unop (τ :: σ :: Γ') op _ τ₁ ρ'
        (context_exchange Γ' σ τ e τ₁ h) hop
  | ifE Γ' c t e ρ' hc ht he =>
      simp [swap01, swapAt]
      exact hasType.ifE (τ :: σ :: Γ') _ _ _ ρ'
        (context_exchange Γ' σ τ c .bool hc)
        (context_exchange Γ' σ τ t ρ' ht)
        (context_exchange Γ' σ τ e ρ' he)
  | app Γ' f a τ₁ τ₂ hf ha =>
      simp [swap01, swapAt]
      exact hasType.app (τ :: σ :: Γ') _ _ τ₁ τ₂
        (context_exchange Γ' σ τ f (.fn τ₁ τ₂) hf)
        (context_exchange Γ' σ τ a τ₁ ha)
  | letE Γ' e₁ e₂ τ₁ τ₂ h₁ h₂ =>
      simp [swap01, swapAt]
      have ih₁ := context_exchange Γ' σ τ e₁ τ₁ h₁
      -- h₂ : hasType (τ₁ :: σ :: τ :: Γ') e₂ τ₂
      -- Need: hasType (τ₁ :: τ :: σ :: Γ') (swapAt 1 e₂) τ₂
      -- Apply context_exchange to the tail (σ :: τ :: Γ'), under the binder τ₁
      have ih₂ : hasType (τ₁ :: τ :: σ :: Γ') (swap01 e₂) τ₂ :=
        context_exchange (τ₁ :: Γ') σ τ e₂ τ₂ h₂
      -- swap01 e₂ = swapAt 0 e₂, but we need swapAt 1 for the body
      -- The swap01 on .letE produces swapAt 1 for the body
      exact hasType.letE (τ :: σ :: Γ') _ _ τ₁ τ₂ ih₁ ih₂
  | lam Γ' body τ₁ τ₂ hbody =>
      simp [swap01, swapAt]
      -- hbody : hasType (τ₁ :: σ :: τ :: Γ') body τ₂
      -- Need: hasType (τ₁ :: τ :: σ :: Γ') (swapAt 1 body) τ₂
      have ih_body : hasType (τ₁ :: τ :: σ :: Γ') (swap01 body) τ₂ :=
        context_exchange (τ₁ :: Γ') σ τ body τ₂ hbody
      -- swap01 on .lam gives swapAt 1 for the body
      exact hasType.lam (τ :: σ :: Γ') _ τ₁ τ₂ ih_body
  | returnE Γ' e ρ' h =>
      simp [swap01, swapAt]
      exact hasType.returnE (τ :: σ :: Γ') _ ρ'
        (context_exchange Γ' σ τ e ρ' h)
  | block_nil Γ' =>
      exact hasType.block_nil (τ :: σ :: Γ')
  | block_cons Γ' e es ρ' hfirst hrest =>
      simp [swap01, swapAt]
      exact hasType.block_cons (τ :: σ :: Γ') _ _ ρ'
        (context_exchange Γ' σ τ e ρ' hfirst)
        (context_exchange Γ' σ τ es ρ' hrest)

/--
Weakening: adding σ to the context preserves typing after shifting.
Uses swapAt_shift for the letE/lam binder cases.
-/
theorem weakening (Γ : Ctx) (σ : Ty) (e : Expr) (τ : Ty)
    (h : hasType Γ e τ) : hasType (σ :: Γ) (shift 1 0 e) τ := by
  induction h generalizing σ with
  | intLit Γ' n => exact hasType.intLit (σ :: Γ') n
  | boolLit Γ' b => exact hasType.boolLit (σ :: Γ') b
  | stringLit Γ' s => exact hasType.stringLit (σ :: Γ') s
  | unitLit Γ' => exact hasType.unitLit (σ :: Γ')
  | nilLit Γ' => exact hasType.nilLit (σ :: Γ')
  | @var Γ' n τ' h_lookup =>
      have h_shift : shift 1 0 (.var n) = .var (n + 1) := by simp [shift]
      rw [h_shift]
      have h_get : (σ :: Γ').get? (n + 1) = some τ' := by
        simp [Ctx.get, List.get?, h_lookup]
      exact hasType.var (σ :: Γ') (n + 1) τ' h_get
  | binop Γ' op e₁ e₂ τ₁ τ₂ τ' h₁ h₂ hop =>
      simp [shift]
      have ih₁ := weakening Γ' σ e₁ τ₁ h₁
      have ih₂ := weakening Γ' σ e₂ τ₂ h₂
      exact hasType.binop (σ :: Γ') op _ _ τ₁ τ₂ τ' ih₁ ih₂ hop
  | unop Γ' op e τ₁ τ' h hop =>
      simp [shift]
      have ih := weakening Γ' σ e τ₁ h
      exact hasType.unop (σ :: Γ') op _ τ₁ τ' ih hop
  | @letE Γ' e₁ e₂ τ₁ τ₂ h₁ h₂ =>
      simp [shift]
      have ih₁ := weakening Γ' σ e₁ τ₁ h₁
      -- h₂ : hasType (τ₁ :: Γ') e₂ τ₂
      -- weakening on extended context: hasType (σ :: τ₁ :: Γ') (shift 1 0 e₂) τ₂
      have ih_body_weakened := weakening (τ₁ :: Γ') σ e₂ τ₂ h₂
      -- ih_body_weakened : hasType (σ :: τ₁ :: Γ') (shift 1 0 e₂) τ₂
      -- Apply context_exchange to swap σ and τ₁ in the context
      have ih_body_exchanged : hasType (τ₁ :: σ :: Γ') (swap01 (shift 1 0 e₂)) τ₂ :=
        context_exchange (τ₁ :: Γ') σ τ₁ (shift 1 0 e₂) τ₂ ih_body_weakened
      -- swapAt_shift 0 e₂ : swap01 (shift 1 0 e₂) = shift 1 1 e₂
      have h_body : hasType (τ₁ :: σ :: Γ') (shift 1 1 e₂) τ₂ := by
        simpa [swap01, swapAt_shift 0 e₂] using ih_body_exchanged
      exact hasType.letE (σ :: Γ') _ _ τ₁ τ₂ ih₁ h_body
  | ifE Γ' c t e τ' hc ht he =>
      simp [shift]
      have ihc := weakening Γ' σ c .bool hc
      have iht := weakening Γ' σ t τ' ht
      have ihe := weakening Γ' σ e τ' he
      exact hasType.ifE (σ :: Γ') _ _ _ τ' ihc iht ihe
  | app Γ' f a τ₁ τ₂ hf ha =>
      simp [shift]
      have ihf := weakening Γ' σ f (.fn τ₁ τ₂) hf
      have iha := weakening Γ' σ a τ₁ ha
      exact hasType.app (σ :: Γ') _ _ τ₁ τ₂ ihf iha
  | @lam Γ' body τ₁ τ₂ hbody =>
      simp [shift]
      -- Same pattern as letE
      have ih_body_weakened := weakening (τ₁ :: Γ') σ body τ₂ hbody
      have ih_body_exchanged : hasType (τ₁ :: σ :: Γ') (swap01 (shift 1 0 body)) τ₂ :=
        context_exchange (τ₁ :: Γ') σ τ₁ (shift 1 0 body) τ₂ ih_body_weakened
      have h_body : hasType (τ₁ :: σ :: Γ') (shift 1 1 body) τ₂ := by
        simpa [swap01, swapAt_shift 0 body] using ih_body_exchanged
      exact hasType.lam (σ :: Γ') _ τ₁ τ₂ h_body
  | returnE Γ' e τ' h =>
      simp [shift]
      have ih := weakening Γ' σ e τ' h
      exact hasType.returnE (σ :: Γ') _ τ' ih
  | block_nil Γ' =>
      simp [shift]
      exact hasType.block_nil (σ :: Γ')
  | block_cons Γ' e es τ' hfirst hrest =>
      simp [shift]
      have ihf := weakening Γ' σ e τ' hfirst
      have ihr := weakening Γ' σ es τ' hrest
      exact hasType.block_cons (σ :: Γ') _ _ τ' ihf ihr

end Nulang.TypingDB
