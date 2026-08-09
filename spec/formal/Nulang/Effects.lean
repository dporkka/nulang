/-
Nulang Formal Semantics: Row-Polymorphic Algebraic Effects
===========================================================

Formalizes the Koka-inspired row-polymorphic effect system.
-/

/- ------------------------------------------------------------------
   Effect names and rows
   ------------------------------------------------------------------ -/

/-- Built-in effect names (simplified from the full effect system). -/
inductive EffectName where
  | io
  | spawn
  | send
  | receive
  | timer
  | signal
  | inference
  | provider
  deriving BEq, Repr, Inhabited

/-- A region variable identifies an open row tail. -/
abbrev Region := Nat

/--
An effect row: a set of effects a computation may perform.
- `closed es`  — exactly the set `es` (no more)
- `open es r`  — at least the set `es`; `r` is a region variable
                 standing for "possibly more effects"
-/
inductive EffectRow where
  | closed (effects : List EffectName)
  | open   (effects : List EffectName) (region : Region)
  deriving BEq, Repr, Inhabited

/- ------------------------------------------------------------------
   Row Subsumption
   ------------------------------------------------------------------ -/

/--
`subrow r₁ r₂` is true when every effect in r₁ also appears in r₂.

Properties:
- Reflexive:  subrow r r
- Transitive: subrow r₁ r₂ → subrow r₂ r₃ → subrow r₁ r₃
- Empty is bottom: subrow (closed []) r
-/
def subrow : EffectRow → EffectRow → Bool
  | .closed es₁, .closed es₂ =>
    es₁.all (λ e₁ => es₂.elem e₁)
  | .open es₁ _, .closed es₂ =>
    es₁.all (λ e₁ => es₂.elem e₁)
  | .closed es₁, .open es₂ _ =>
    es₁.all (λ e₁ => es₂.elem e₁)
  | .open es₁ _, .open es₂ _ =>
    es₁.all (λ e₁ => es₂.elem e₁)

/- ------------------------------------------------------------------
   Row Union
   ------------------------------------------------------------------ -/

/--
`union fresh r₁ r₂` is the union of two effect rows with a fresh region
variable.  Used when sequencing computations: `e1; e2` has effect
`union fresh (eff(e1)) (eff(e2))`.
-/
def rowUnion (fresh : Region) : EffectRow → EffectRow → EffectRow
  | .closed es₁, .closed es₂ =>
    let combined := es₁ ++ es₂
    .closed (eraseDups combined)
  | .open es₁ _, .closed es₂ =>
    let combined := es₁ ++ es₂
    .open (eraseDups combined) fresh
  | .closed es₁, .open es₂ _ =>
    let combined := es₁ ++ es₂
    .open (eraseDups combined) fresh
  | .open es₁ _, .open es₂ _ =>
    let combined := es₁ ++ es₂
    .open (eraseDups combined) fresh

/-- Remove duplicates from a list (preserving order of first occurrence). -/
def eraseDups {α : Type} [BEq α] : List α → List α
  | [] => []
  | x :: xs =>
    let rest := eraseDups xs
    if rest.elem x then rest else x :: rest

/- ------------------------------------------------------------------
   Properties (conjectures for machine proof)
   ------------------------------------------------------------------ -/

/-
Theorem row_subrow_reflexive:
  ∀ (r : EffectRow), subrow r r = true

Theorem row_subrow_transitive:
  ∀ (r₁ r₂ r₃ : EffectRow),
    subrow r₁ r₂ = true → subrow r₂ r₃ = true → subrow r₁ r₃ = true

Theorem row_empty_is_bottom:
  ∀ (r : EffectRow), subrow (.closed []) r = true

Theorem row_union_upper_bound:
  ∀ (fresh : Region) (r₁ r₂ : EffectRow),
    subrow r₁ (rowUnion fresh r₁ r₂) = true ∧ subrow r₂ (rowUnion fresh r₁ r₂) = true
-/
