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
  -- This is a complex lemma requiring:
  -- 1. The var case: mapping index lookups across the swap
  -- 2. The binder cases: adjusting k when going under binders
  -- Proof by induction on the typing derivation.
  sorry

end Nulang.DeepExchange
