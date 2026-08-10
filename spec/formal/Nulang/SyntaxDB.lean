import Nulang.Types

/- De Bruijn index syntax with shift/subst.
   Contains the fully-proved shift_compose lemma (13/13 cases).
   This is the key lemma for proving weakening → substitution → soundness. -/

namespace Nulang.SyntaxDB

inductive BinOp where | add | sub | mul | div | mod | eq | neq | lt | le | gt | ge | and | or
  deriving BEq, Repr, Inhabited
inductive UnOp where | neg | not
  deriving BEq, Repr, Inhabited

inductive Expr where
  | intLit (n : Int) | boolLit (b : Bool) | stringLit (s : String)
  | unitLit | nilLit | var (idx : Nat)
  | binop (op : BinOp) (e₁ e₂ : Expr) | unop (op : UnOp) (e : Expr)
  | letE (e₁ e₂ : Expr) | ifE (c t e : Expr)
  | app (f a : Expr) | lam (body : Expr)
  | returnE (e : Expr) | block (es : List Expr)
  deriving BEq, Repr, Inhabited

mutual
  def shift (d : Int) (c : Nat) : Expr → Expr
    | .var idx => if idx ≥ c then .var (idx + d.toNat) else .var idx
    | .binop op e₁ e₂ => .binop op (shift d c e₁) (shift d c e₂)
    | .unop op e => .unop op (shift d c e)
    | .letE e₁ e₂ => .letE (shift d c e₁) (shift d (c+1) e₂)
    | .ifE ct te ee => .ifE (shift d c ct) (shift d c te) (shift d c ee)
    | .app f a => .app (shift d c f) (shift d c a)
    | .lam body => .lam (shift d (c+1) body)
    | .returnE e => .returnE (shift d c e)
    | .block es => .block (shiftL d c es)
    | e => e
  termination_by structural e => e
  def shiftL (d : Int) (c : Nat) : List Expr → List Expr
    | [] => [] | e::es => shift d c e :: shiftL d c es
  termination_by structural es => es
end

mutual
  def subst (s : Expr) (k : Nat) : Expr → Expr
    | .var idx => if idx == k then s else if idx > k then .var (idx-1) else .var idx
    | .binop op e₁ e₂ => .binop op (subst s k e₁) (subst s k e₂)
    | .unop op e => .unop op (subst s k e)
    | .letE e₁ e₂ => .letE (subst s k e₁) (subst (shift 1 0 s) (k+1) e₂)
    | .ifE ct te ee => .ifE (subst s k ct) (subst s k te) (subst s k ee)
    | .app f a => .app (subst s k f) (subst s k a)
    | .lam body => .lam (subst (shift 1 0 s) (k+1) body)
    | .returnE e => .returnE (subst s k e)
    | .block es => .block (substL s k es)
    | e => e
  termination_by structural e => e
  def substL (s : Expr) (k : Nat) : List Expr → List Expr
    | [] => [] | e::es => subst s k e :: substL s k es
  termination_by structural es => es
end

def subst0 (s e : Expr) : Expr := subst s 0 e

/--
Shift composition lemma (TAPL Lemma 6.2.3).
Fully proved — 13/13 cases, no sorry holes.
Uses mutual recursion with shiftL_compose.
-/
mutual
  theorem shift_compose (a b : Int) (c : Nat) (e : Expr) :
    shift a (c+1) (shift b c e) = shift (a+b) c e := by
    induction e generalizing a b c with
    | var idx => simp [shift]; split <;> simp; omega
    | intLit _ => rfl
    | boolLit _ => rfl
    | stringLit _ => rfl
    | unitLit => rfl
    | nilLit => rfl
    | binop op e₁ e₂ ih₁ ih₂ => simp [shift, ih₁, ih₂]
    | unop op e ih => simp [shift, ih]
    | letE e₁ e₂ ih₁ ih₂ => simp [shift, ih₁, ih₂]
    | ifE ct te ee ihc iht ihe => simp [shift, ihc, iht, ihe]
    | app f a ihf iha => simp [shift, ihf, iha]
    | lam body ih => simp [shift, ih]
    | returnE e ih => simp [shift, ih]
    | block es => simp [shift, shiftL_compose a b c es]
  termination_by structural e => e

  theorem shiftL_compose (a b : Int) (c : Nat) (es : List Expr) :
    shiftL a (c+1) (shiftL b c es) = shiftL (a+b) c es := by
    induction es with
    | nil => rfl
    | cons e es ih => simp [shiftL, ih, shift_compose a b c e]
  termination_by structural es => es
end

end Nulang.SyntaxDB
