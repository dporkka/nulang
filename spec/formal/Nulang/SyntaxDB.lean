import Nulang.Types

/- De Bruijn index syntax with shift/subst, swapAt, and supporting lemmas.
   Non-nested Expr type (block_nil/block_cons) enables `induction` in proofs.
   swapAt_shift is the key lemma for weakening: swapAt at depth k after shift 1 k
   equals shift 1 (k+1) (needed when pushing weakening under binders). -/

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
  | returnE (e : Expr)
  | block_nil | block_cons (e : Expr) (es : Expr)
  deriving BEq, Repr, Inhabited

def shift (d : Int) (c : Nat) : Expr → Expr
  | .var idx => if idx ≥ c then .var (idx + d.toNat) else .var idx
  | .binop op e₁ e₂ => .binop op (shift d c e₁) (shift d c e₂)
  | .unop op e => .unop op (shift d c e)
  | .letE e₁ e₂ => .letE (shift d c e₁) (shift d (c+1) e₂)
  | .ifE ct te ee => .ifE (shift d c ct) (shift d c te) (shift d c ee)
  | .app f a => .app (shift d c f) (shift d c a)
  | .lam body => .lam (shift d (c+1) body)
  | .returnE e => .returnE (shift d c e)
  | .block_nil => .block_nil
  | .block_cons e es => .block_cons (shift d c e) (shift d c es)
  | e => e
termination_by structural e => e

def subst (s : Expr) (k : Nat) : Expr → Expr
  | .var idx => if idx == k then s else if idx > k then .var (idx-1) else .var idx
  | .binop op e₁ e₂ => .binop op (subst s k e₁) (subst s k e₂)
  | .unop op e => .unop op (subst s k e)
  | .letE e₁ e₂ => .letE (subst s k e₁) (subst (shift 1 0 s) (k+1) e₂)
  | .ifE ct te ee => .ifE (subst s k ct) (subst s k te) (subst s k ee)
  | .app f a => .app (subst s k f) (subst s k a)
  | .lam body => .lam (subst (shift 1 0 s) (k+1) body)
  | .returnE e => .returnE (subst s k e)
  | .block_nil => .block_nil
  | .block_cons e es => .block_cons (subst s k e) (subst s k es)
  | e => e
termination_by structural e => e

def subst0 (s e : Expr) : Expr := subst s 0 e

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
  | .block_nil => .block_nil
  | .block_cons e es => .block_cons (swapAt k e) (swapAt k es)
  | e => e
termination_by structural e => e

def swap01 (e : Expr) : Expr := swapAt 0 e

theorem swapAt_shift_var (k idx : Nat) : swapAt k (shift 1 k (.var idx)) = shift 1 (k+1) (.var idx) := by
  simp [shift]
  by_cases h : idx ≥ k
  · simp [h]
    by_cases h_eq : idx = k
    · subst h_eq; simp [swapAt]
    · -- idx ≠ k
      by_cases hk1 : idx ≥ k+1
      · simp [hk1]
        have h_ne : idx + 1 ≠ k := by omega
        dsimp [swapAt]; simp [h_ne, h_eq]
      · omega
  · -- idx < k
    simp [h]
    -- Now: swapAt k (.var idx) = (if k+1 ≤ idx then .var (idx+1) else .var idx)
    have h_lt : ¬ (k+1 : Nat) ≤ idx := by omega
    rw [if_neg h_lt]
    -- Now: swapAt k (.var idx) = .var idx
    have h_ne_k : idx ≠ k := by omega
    have h_ne_k1 : idx ≠ k + 1 := by omega
    dsimp [swapAt]; simp [h_ne_k, h_ne_k1]

theorem swapAt_shift (k : Nat) (e : Expr) : swapAt k (shift 1 k e) = shift 1 (k+1) e := by
  induction e generalizing k with
  | var idx => exact swapAt_shift_var k idx
  | intLit _ => rfl
  | boolLit _ => rfl
  | stringLit _ => rfl
  | unitLit => rfl
  | nilLit => rfl
  | binop op e₁ e₂ ih₁ ih₂ => simp [swapAt, shift, ih₁, ih₂]
  | unop op e ih => simp [swapAt, shift, ih]
  | letE e₁ e₂ ih₁ ih₂ => simp [swapAt, shift, ih₁, ih₂ (k+1)]
  | ifE c t e ihc iht ihe => simp [swapAt, shift, ihc, iht, ihe]
  | app f a ihf iha => simp [swapAt, shift, ihf, iha]
  | lam body ih => simp [swapAt, shift, ih (k+1)]
  | returnE e ih => simp [swapAt, shift, ih]
  | block_nil => rfl
  | block_cons e es ih_e ih_es => simp [swapAt, shift, ih_e, ih_es]

end Nulang.SyntaxDB
