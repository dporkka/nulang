import Nulang.Types
import Nulang.Syntax

namespace Nulang.Typing

open Nulang.Types (Ty)
open Nulang.Syntax (Expr BinOp UnOp)

abbrev Ctx := List (String × Ty)

def Ctx.lookup (Γ : Ctx) (x : String) : Option Ty :=
  match Γ with
  | [] => none
  | (y, τ) :: Γ' => if y == x then some τ else Γ'.lookup x

def Ctx.extend (Γ : Ctx) (x : String) (τ : Ty) : Ctx :=
  (x, τ) :: Γ

def binopType : BinOp → Ty × Ty × Ty
  | .add => (.int, .int, .int)
  | .sub => (.int, .int, .int)
  | .mul => (.int, .int, .int)
  | .div => (.int, .int, .int)
  | .mod => (.int, .int, .int)
  | .eq  => (.bool, .int, .int)
  | .neq => (.bool, .int, .int)
  | .lt  => (.bool, .int, .int)
  | .le  => (.bool, .int, .int)
  | .gt  => (.bool, .int, .int)
  | .ge  => (.bool, .int, .int)
  | .and => (.bool, .bool, .bool)
  | .or  => (.bool, .bool, .bool)

def unopType : UnOp → Ty × Ty
  | .neg => (.int, .int)
  | .not => (.bool, .bool)

inductive hasType : Ctx → Expr → Ty → Prop where
  | intLit (Γ : Ctx) (n : Int) : hasType Γ (.intLit n) .int
  | boolLit (Γ : Ctx) (b : Bool) : hasType Γ (.boolLit b) .bool
  | stringLit (Γ : Ctx) (s : String) : hasType Γ (.stringLit s) .string
  | unitLit (Γ : Ctx) : hasType Γ .unitLit .unit
  | nilLit (Γ : Ctx) : hasType Γ .nilLit .nil
  | var (Γ : Ctx) (x : String) (τ : Ty) (h : Γ.lookup x = some τ) : hasType Γ (.var x) τ
  | binop (Γ : Ctx) (op : BinOp) (e₁ e₂ : Expr) (τ₁ τ₂ τ : Ty)
      (h₁ : hasType Γ e₁ τ₁) (h₂ : hasType Γ e₂ τ₂) (hop : binopType op = (τ, τ₁, τ₂)) :
      hasType Γ (.binop op e₁ e₂) τ
  | unop (Γ : Ctx) (op : UnOp) (e : Expr) (τ₁ τ : Ty)
      (h : hasType Γ e τ₁) (hop : unopType op = (τ, τ₁)) : hasType Γ (.unop op e) τ
  | letE (Γ : Ctx) (x : String) (e₁ e₂ : Expr) (τ₁ τ₂ : Ty)
      (h₁ : hasType Γ e₁ τ₁) (h₂ : hasType (Γ.extend x τ₁) e₂ τ₂) :
      hasType Γ (.letE x e₁ e₂) τ₂
  | ifE (Γ : Ctx) (c t e : Expr) (τ : Ty)
      (hc : hasType Γ c .bool) (ht : hasType Γ t τ) (he : hasType Γ e τ) :
      hasType Γ (.ifE c t e) τ
  | app (Γ : Ctx) (f a : Expr) (τ₁ τ₂ : Ty)
      (hf : hasType Γ f (.fn τ₁ τ₂)) (ha : hasType Γ a τ₁) :
      hasType Γ (.app f a) τ₂
  | lam (Γ : Ctx) (x : String) (body : Expr) (τ₁ τ₂ : Ty)
      (hbody : hasType (Γ.extend x τ₁) body τ₂) :
      hasType Γ (.lam x body) (.fn τ₁ τ₂)
  | returnE (Γ : Ctx) (e : Expr) (τ : Ty) (h : hasType Γ e τ) :
      hasType Γ (.returnE e) τ
  | block_nil (Γ : Ctx) : hasType Γ (.block []) .unit
  | block_cons (Γ : Ctx) (e : Expr) (es : List Expr) (τ : Ty)
      (hfirst : hasType Γ e τ) (hrest : hasType Γ (.block es) τ) :
      hasType Γ (.block (e :: es)) τ

end Nulang.Typing
