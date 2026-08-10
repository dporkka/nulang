import Nulang.Types
import Nulang.SyntaxDB

/- Context exchange lemma for de Bruijn weakening.
   Swaps the first two context entries and adjusts indices 0↔1. -/

namespace Nulang.TypingDB

open Nulang.Types (Ty)
open Nulang.SyntaxDB (Expr BinOp UnOp shift)
open Nulang.SyntaxDB (shift_compose)

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
  | block_nil (Γ : Ctx) : hasType Γ (.block []) .unit
  | block_cons (Γ : Ctx) (e : Expr) (es : List Expr) (τ : Ty)
      (hfirst : hasType Γ e τ) (hrest : hasType Γ (.block es) τ) :
      hasType Γ (.block (e :: es)) τ

/-- Swap indices 0 and 1 in an expression. Used for context exchange. -/
def swap01 : Expr → Expr
  | .var 0 => .var 1
  | .var 1 => .var 0
  | .var n => .var n
  | .binop op e₁ e₂ => .binop op (swap01 e₁) (swap01 e₂)
  | .unop op e => .unop op (swap01 e)
  | .letE e₁ e₂ => .letE (swap01 e₁) (swap01 e₂)
  | .ifE c t e => .ifE (swap01 c) (swap01 t) (swap01 e)
  | .app f a => .app (swap01 f) (swap01 a)
  | .lam body => .lam (swap01 body)
  | .returnE e => .returnE (swap01 e)
  | .block es => .block (es.map swap01)
  | e => e

/--
Context exchange: swapping the first two context entries preserves typing
under the swap01 renaming.
-/
theorem context_exchange (Γ : Ctx) (σ τ : Ty) (e : Expr) (ρ : Ty)
  (h : hasType (σ :: τ :: Γ) e ρ) : hasType (τ :: σ :: Γ) (swap01 e) ρ := by
  -- Proof by induction on the typing derivation.
  -- The only interesting case is var: indices 0↔1 must be swapped.
  -- All other cases just apply the IH.
  induction h with
  | intLit Γ' n => exact hasType.intLit (τ :: σ :: Γ') n
  | boolLit Γ' b => exact hasType.boolLit (τ :: σ :: Γ') b
  | stringLit Γ' s => exact hasType.stringLit (τ :: σ :: Γ') s
  | unitLit Γ' => exact hasType.unitLit (τ :: σ :: Γ')
  | nilLit Γ' => exact hasType.nilLit (τ :: σ :: Γ')
  | @var Γ' n ρ' h_lookup =>
      -- h_lookup: (σ::τ::Γ').get? n = some ρ'
      -- Need: (τ::σ::Γ').get? (swap01_idx n) = some ρ'
      -- where swap01_idx 0 = 1, swap01_idx 1 = 0, else n
      simp [swap01]
      -- The variable case is the key: we need to look up the right index
      -- in the swapped context.
      sorry
  | binop Γ' op e₁ e₂ τ₁ τ₂ ρ' h₁ h₂ hop =>
      simp [swap01]
      exact hasType.binop (τ :: σ :: Γ') op _ _ τ₁ τ₂ ρ'
        (context_exchange Γ' σ τ e₁ τ₁ h₁) (context_exchange Γ' σ τ e₂ τ₂ h₂) hop
  | unop Γ' op e τ₁ ρ' h hop =>
      simp [swap01]
      exact hasType.unop (τ :: σ :: Γ') op _ τ₁ ρ' (context_exchange Γ' σ τ e τ₁ h) hop
  | letE Γ' e₁ e₂ τ₁ τ₂ h₁ h₂ =>
      simp [swap01]
      have ih₁ := context_exchange Γ' σ τ e₁ τ₁ h₁
      -- h₂ : hasType (τ₁ :: σ :: τ :: Γ') e₂ τ₂
      -- Need: hasType (τ₁ :: τ :: σ :: Γ') (swap01 e₂) τ₂
      -- The context is (τ₁ :: σ :: τ :: Γ') = τ₁ :: (σ :: τ :: Γ')
      -- We need to apply context_exchange to the TAIL (σ::τ::Γ') within the extended context.
      -- This requires context_exchange to work under binders.
      sorry
  | ifE Γ' c t e ρ' hc ht he =>
      simp [swap01]
      exact hasType.ifE (τ :: σ :: Γ') _ _ _ ρ'
        (context_exchange Γ' σ τ c .bool hc) (context_exchange Γ' σ τ t ρ' ht) (context_exchange Γ' σ τ e ρ' he)
  | app Γ' f a τ₁ τ₂ hf ha =>
      simp [swap01]
      exact hasType.app (τ :: σ :: Γ') _ _ τ₁ τ₂
        (context_exchange Γ' σ τ f (.fn τ₁ τ₂) hf) (context_exchange Γ' σ τ a τ₁ ha)
  | @lam Γ' body τ₁ τ₂ hbody =>
      -- Same issue as letE: exchange under a binder.
      sorry
  | returnE Γ' e ρ' h =>
      simp [swap01]
      exact hasType.returnE (τ :: σ :: Γ') _ ρ' (context_exchange Γ' σ τ e ρ' h)
  | block_nil Γ' => exact hasType.block_nil (τ :: σ :: Γ')
  | block_cons Γ' e es ρ' hfirst hrest =>
      simp [swap01]
      exact hasType.block_cons (τ :: σ :: Γ') _ _ ρ'
        (context_exchange Γ' σ τ e ρ' hfirst) (context_exchange Γ' σ τ (.block es) ρ' hrest)

end Nulang.TypingDB
