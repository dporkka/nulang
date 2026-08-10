import Nulang.Types
import Nulang.SyntaxDB

/- Generalized context exchange at arbitrary depth.
   Swaps elements at positions k and k+1 in the context,
   adjusting de Bruijn indices accordingly. -/

namespace Nulang.DeepExchange

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

/-- Swap de Bruijn indices k and k+1. Used when context entries at k, k+1 are exchanged. -/
def swapAt (k : Nat) : Expr → Expr
  | .var idx =>
    if idx == k then .var (k+1)
    else if idx == k+1 then .var k
    else .var idx
  | .binop op e₁ e₂ => .binop op (swapAt k e₁) (swapAt k e₂)
  | .unop op e => .unop op (swapAt k e)
  | .letE e₁ e₂ => .letE (swapAt k e₁) (swapAt (k+1) e₂)
  | .ifE c t e => .ifE (swapAt k c) (swapAt k t) (swapAt k e)
  | .app f a => .app (swapAt k f) (swapAt k a)
  | .lam body => .lam (swapAt (k+1) body)
  | .returnE e => .returnE (swapAt k e)
  | .block es => .block (es.map (swapAt k))
  | e => e

/-- Swap context entries at positions k and k+1. -/
def swapCtxAt (k : Nat) (Γ : Ctx) : Ctx :=
  match Γ with
  | a::b::rest => if k == 0 then b::a::rest
                  else a :: swapCtxAt (k-1) (b::rest)
  | _ => Γ

/--
Generalized context exchange: swapping adjacent context entries at depth k
preserves typing under swapAt k renaming.
-/
theorem context_exchange_at (Γ : Ctx) (k : Nat) (e : Expr) (ρ : Ty)
  (h : hasType Γ e ρ) : hasType (swapCtxAt k Γ) (swapAt k e) ρ := by
  induction h generalizing k with
  | intLit Γ' n => simp [swapAt]; exact hasType.intLit (swapCtxAt k Γ') n
  | boolLit Γ' b => simp [swapAt]; exact hasType.boolLit (swapCtxAt k Γ') b
  | stringLit Γ' s => simp [swapAt]; exact hasType.stringLit (swapCtxAt k Γ') s
  | unitLit Γ' => simp [swapAt]; exact hasType.unitLit (swapCtxAt k Γ')
  | nilLit Γ' => simp [swapAt]; exact hasType.nilLit (swapCtxAt k Γ')
  | @var Γ' n ρ' h_lookup => sorry
  | binop Γ' op e₁ e₂ τ₁ τ₂ ρ' h₁ h₂ hop =>
      simp [swapAt]
      exact hasType.binop (swapCtxAt k Γ') op _ _ τ₁ τ₂ ρ'
        (context_exchange_at Γ' k e₁ τ₁ h₁) (context_exchange_at Γ' k e₂ τ₂ h₂) hop
  | unop Γ' op e τ₁ ρ' h hop =>
      simp [swapAt]
      exact hasType.unop (swapCtxAt k Γ') op _ τ₁ ρ' (context_exchange_at Γ' k e τ₁ h) hop
  | ifE Γ' c t e ρ' hc ht he =>
      simp [swapAt]
      exact hasType.ifE (swapCtxAt k Γ') _ _ _ ρ'
        (context_exchange_at Γ' k c .bool hc) (context_exchange_at Γ' k t ρ' ht) (context_exchange_at Γ' k e ρ' he)
  | app Γ' f a τ₁ τ₂ hf ha =>
      simp [swapAt]
      exact hasType.app (swapCtxAt k Γ') _ _ τ₁ τ₂
        (context_exchange_at Γ' k f (.fn τ₁ τ₂) hf) (context_exchange_at Γ' k a τ₁ ha)
  | returnE Γ' e ρ' h =>
      simp [swapAt]
      exact hasType.returnE (swapCtxAt k Γ') _ ρ' (context_exchange_at Γ' k e ρ' h)
  | block_cons Γ' e es ρ' hfirst hrest =>
      simp [swapAt]
      exact hasType.block_cons (swapCtxAt k Γ') _ _ ρ'
        (context_exchange_at Γ' k e ρ' hfirst) (context_exchange_at Γ' k (.block es) ρ' hrest)

end Nulang.DeepExchange
