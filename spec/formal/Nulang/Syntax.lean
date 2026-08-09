import Nulang.Types

namespace Nulang.Syntax

inductive BinOp where
  | add | sub | mul | div | mod
  | eq | neq | lt | le | gt | ge
  | and | or
  deriving BEq, Repr, Inhabited

inductive UnOp where
  | neg | not
  deriving BEq, Repr, Inhabited

inductive Pattern where
  | wildcard
  | intLit (n : Int)
  | boolLit (b : Bool)
  | stringLit (s : String)
  | varBind (x : String)
  deriving BEq, Repr, Inhabited

inductive Expr where
  | intLit (n : Int) | boolLit (b : Bool) | stringLit (s : String)
  | unitLit | nilLit
  | var (x : String)
  | binop (op : BinOp) (e₁ e₂ : Expr)
  | unop (op : UnOp) (e : Expr)
  | letE (x : String) (e₁ e₂ : Expr)
  | ifE (cond : Expr) (then_ : Expr) (else_ : Expr)
  | matchE (scrutinee : Expr) (arms : List (Pattern × Expr))
  | app (f : Expr) (arg : Expr)
  | lam (param : String) (body : Expr)
  | returnE (e : Expr)
  | block (es : List Expr)
  deriving BEq, Repr, Inhabited

def freeVarsPattern : Pattern → List String
  | .varBind x => [x]
  | _          => []

mutual
  def freeVarsExpr : Expr → List String
    | .var x         => [x]
    | .binop _ e₁ e₂ => freeVarsExpr e₁ ++ freeVarsExpr e₂
    | .unop _ e      => freeVarsExpr e
    | .letE x e₁ e₂  => freeVarsExpr e₁ ++ (freeVarsExpr e₂).erase x
    | .ifE c t e     => freeVarsExpr c ++ freeVarsExpr t ++ freeVarsExpr e
    | .matchE s arms => freeVarsExpr s ++ freeVarsExprArms arms
    | .app f a       => freeVarsExpr f ++ freeVarsExpr a
    | .lam x body    => (freeVarsExpr body).erase x
    | .returnE e     => freeVarsExpr e
    | .block es      => freeVarsExprList es
    | _              => []
  termination_by structural e => e

  def freeVarsExprArms : List (Pattern × Expr) → List String
    | [] => []
    | (p, e) :: arms =>
      let bound := freeVarsPattern p
      (freeVarsExpr e |>.filter (λ v => ! bound.elem v)) ++ freeVarsExprArms arms
  termination_by structural arms => arms

  def freeVarsExprList : List Expr → List String
    | [] => []
    | e :: es => freeVarsExpr e ++ freeVarsExprList es
  termination_by structural es => es
end

mutual
  def substExpr (v : Expr) (x : String) : Expr → Expr
    | .var y         => if y == x then v else .var y
    | .binop op e₁ e₂ => .binop op (substExpr v x e₁) (substExpr v x e₂)
    | .unop op e     => .unop op (substExpr v x e)
    | .letE y e₁ e₂  =>
      if y == x then .letE y (substExpr v x e₁) e₂
      else .letE y (substExpr v x e₁) (substExpr v x e₂)
    | .ifE c t e     => .ifE (substExpr v x c) (substExpr v x t) (substExpr v x e)
    | .matchE s arms => .matchE (substExpr v x s) (substExprArms v x arms)
    | .app f a       => .app (substExpr v x f) (substExpr v x a)
    | .lam y body    =>
      if y == x then .lam y body
      else .lam y (substExpr v x body)
    | .returnE e     => .returnE (substExpr v x e)
    | .block es      => .block (substExprList v x es)
    | e              => e
  termination_by structural e => e

  def substExprArms (v : Expr) (x : String) : List (Pattern × Expr) → List (Pattern × Expr)
    | [] => []
    | (p, e) :: arms => (p, substExpr v x e) :: substExprArms v x arms
  termination_by structural arms => arms

  def substExprList (v : Expr) (x : String) : List Expr → List Expr
    | [] => []
    | e :: es => substExpr v x e :: substExprList v x es
  termination_by structural es => es
end

end Nulang.Syntax
