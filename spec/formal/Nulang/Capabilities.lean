/-
Nulang Formal Semantics: Reference Capability Lattice
======================================================

Formalizes the Pony-inspired capability lattice used for data-race
freedom and actor isolation.

The lattice (Hasse diagram):
```
      LinearIso
      /      \
    Iso     Linear
    / \      /
  Trn Val<--/
   |   |
  Ref Box
    \ /
    Tag
```

Subtyping is the partial order: c₁ <: c₂ iff c₁ is below c₂ in the
diagram (read bottom-up).  All capabilities are subtypes of Tag.
-/

/- ------------------------------------------------------------------
   Capability lattice
   ------------------------------------------------------------------ -/

/--
The eight reference capabilities.
- `lineariso` — unique ownership, linear-tracked (at-most-once consumption)
- `linear`    — immutable + linear-tracked + sendable
- `iso`       — unique ownership, sendable
- `trn`       — unique writer, recoverable to iso
- `ref`       — shared read/write
- `val`       — immutable shared, sendable
- `box`       — read-only (any cap except tag can be read as box)
- `tag`       — opaque identity only (no dereference)
-/
inductive Cap where
  | lineariso
  | linear
  | iso
  | trn
  | ref
  | val
  | box
  | tag
  deriving BEq, Repr, Inhabited

/- ------------------------------------------------------------------
   Subtyping Order (<:)
   ------------------------------------------------------------------ -/

/--
`subtype c₁ c₂` is true when `c₁ <: c₂` — i.e., c₁ is more restrictive
than c₂.  This is the partial order from the Hasse diagram above,
transitively closed.

Properties (to be proved):
- Reflexive:  c <: c
- Transitive: c₁ <: c₂ ∧ c₂ <: c₃ → c₁ <: c₃
- Tag is top: ∀c. c <: tag
-/
def subtype : Cap → Cap → Bool
  | c₁, c₂ =>
    -- Identity
    c₁ == c₂ ||
    -- Direct edges from Hasse diagram + transitive closure
    match (c₁, c₂) with
    | (.lineariso, .iso)    => true
    | (.lineariso, .linear) => true
    | (.iso,  .trn)  => true
    | (.iso,  .val)  => true
    | (.trn,  .ref)  => true
    | (.val,  .box)  => true
    | (.linear, .val) => true
    | (.ref,  .box)  => true
    | (.box,  .tag)  => true
    | (.ref,  .tag)  => true
    | (.val,  .tag)  => true
    | (.trn,  .box)  => true
    | (.iso,  .ref)  => true
    | (.iso,  .box)  => true
    | (.lineariso, .trn)  => true
    | (.lineariso, .ref)  => true
    | (.lineariso, .val)  => true
    | (.lineariso, .box)  => true
    | (.lineariso, .tag)  => true
    | (.linear, .box)     => true
    | (.linear, .tag)     => true
    | (.trn,  .tag)  => true
    | _, _ => false

/- ------------------------------------------------------------------
   Least Upper Bound (Join)
   ------------------------------------------------------------------ -/

/--
`join c₁ c₂` is the least upper bound (LUB) of c₁ and c₂.
Used by the type checker for branch merging.
-/
def join : Cap → Cap → Cap
  -- Bottom element: lineariso
  | .lineariso, c => c
  | c, .lineariso => c

  -- Compatible chains: return the less restrictive
  | .iso, .trn => .trn
  | .trn, .iso => .trn
  | .trn, .ref => .ref
  | .ref, .trn => .ref
  | .ref, .box => .box
  | .box, .ref => .box
  | .val, .box => .box
  | .box, .val => .box
  | .linear, .val => .val
  | .val, .linear => .val
  | .linear, .box => .box
  | .box, .linear => .box

  -- Incomparable branches: use box as safe upper bound
  | .iso, .val => .box
  | .val, .iso => .box
  | .trn, .val => .box
  | .val, .trn => .box
  | .ref, .val => .box
  | .val, .ref => .box
  | .linear, .iso => .box
  | .iso, .linear => .box

  -- Tag absorbs everything
  | _, .tag => .tag
  | .tag, _ => .tag

  -- Box absorbs most
  | _, .box => .box
  | .box, _ => .box

  -- Identity
  | c₁, c₂ => if c₁ == c₂ then c₁ else .tag

/- ------------------------------------------------------------------
   Sendability
   ------------------------------------------------------------------ -/

/--
A capability is *sendable* if values with that capability can cross
actor boundaries.  Only `iso`, `val`, `tag`, `lineariso`, and `linear`
are sendable.
-/
def isSendable : Cap → Bool
  | .iso       => true
  | .val       => true
  | .tag       => true
  | .lineariso => true
  | .linear    => true
  | _          => false

/- ------------------------------------------------------------------
   Properties (conjectures for machine proof)
   ------------------------------------------------------------------ -/

/-
Theorem cap_subtype_reflexive:
  ∀ (c : Cap), subtype c c = true

Theorem cap_subtype_transitive:
  ∀ (c₁ c₂ c₃ : Cap), subtype c₁ c₂ = true → subtype c₂ c₃ = true → subtype c₁ c₃ = true

Theorem cap_subtype_antisymmetric:
  ∀ (c₁ c₂ : Cap), subtype c₁ c₂ = true → subtype c₂ c₁ = true → c₁ = c₂

Theorem cap_tag_is_top:
  ∀ (c : Cap), subtype c tag = true

Theorem cap_join_upper_bound:
  ∀ (c₁ c₂ : Cap), subtype c₁ (join c₁ c₂) = true ∧ subtype c₂ (join c₁ c₂) = true

Theorem cap_join_least:
  ∀ (c₁ c₂ c₃ : Cap), subtype c₁ c₃ = true → subtype c₂ c₃ = true → subtype (join c₁ c₂) c₃ = true

Theorem cap_sendable_subtype:
  ∀ (c₁ c₂ : Cap), isSendable c₁ = true → subtype c₁ c₂ = true → isSendable c₂ = true

Theorem cap_sendable_val_tag:
  isSendable val = true ∧ isSendable tag = true ∧ isSendable iso = true
-/
