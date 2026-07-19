/-
  Nulang capability lattice — Pony-inspired reference capabilities.

  Formalizes the eight-capability lattice from `src/types.rs` (Capability enum):
  LinearIso → Iso → Trn → Ref → Box → Tag, with Linear → Val → Box.
  Operations: `join`, `is_subtype_of`, `is_sendable`, `is_isolated`,
  `discharge_linear`.

  Theorem `cap_sendable` stated; proof open.
-/

namespace Nulang

-- ------------------------------------------------------------------
-- Capability lattice
-- ------------------------------------------------------------------

/--
  The eight capability constants.  Mirrors `Capability` in `src/types.rs`.
  The partial order is:
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
  Subtyping:
  lineariso <: iso <: trn <: ref <: box,
  linear <: val <: box,
  ref <: tag, val <: tag, box <: tag,
  lineariso <: linear (implicit — linear "promotes via Val"),
  iso <: val.
-/
inductive Cap where
| LinearIso | Linear | Iso | Trn | Ref | Val | Box | Tag
deriving BEq, Repr, Inhabited

namespace Cap

-- ------------------------------------------------------------------
-- Join (least upper bound)
-- ------------------------------------------------------------------

/--
  The join operation computes the least upper bound of two capabilities
  in the lattice.  Mirrors `Capability::join()` in `src/types.rs`.
  Join is commutative, associative, and idempotent (the lattice is
  a meet-semilattice through `≤` and a join-semilattice through `⊔`).
-/
def join (a b : Cap) : Cap :=
  match a, b with
  | .LinearIso, .LinearIso => .LinearIso
  | .LinearIso, .Iso       => .Iso
  | .Iso,       .LinearIso => .Iso
  | .LinearIso, .Trn       => .Trn
  | .Trn,       .LinearIso => .Trn
  | .LinearIso, .Ref       => .Ref
  | .Ref,       .LinearIso => .Ref
  | .LinearIso, .Val       => .Val
  | .Val,       .LinearIso => .Val
  | .LinearIso, .Box       => .Box
  | .Box,       .LinearIso => .Box
  | .LinearIso, .Tag       => .LinearIso
  | .Tag,       .LinearIso => .LinearIso
  | .Linear,    .Linear    => .Linear
  | .Linear,    .Val       => .Val
  | .Val,       .Linear    => .Val
  | .Linear,    .LinearIso => .Val
  | .LinearIso, .Linear    => .Val
  | .Linear,    .Iso       => .Val
  | .Iso,       .Linear    => .Val
  | .Linear,    .Trn       => .Val
  | .Trn,       .Linear    => .Val
  | .Linear,    .Ref       => .Box
  | .Ref,       .Linear    => .Box
  | .Linear,    .Box       => .Box
  | .Box,       .Linear    => .Box
  | .Linear,    .Tag       => .Linear
  | .Tag,       .Linear    => .Linear
  | .Iso,       .Iso       => .Iso
  | .Iso,       .Trn       => .Trn
  | .Trn,       .Iso       => .Trn
  | .Trn,       .Trn       => .Trn
  | .Iso,       .Ref       => .Ref
  | .Ref,       .Iso       => .Ref
  | .Trn,       .Ref       => .Ref
  | .Ref,       .Trn       => .Ref
  | .Ref,       .Ref       => .Ref
  | .Iso,       .Val       => .Val
  | .Val,       .Iso       => .Val
  | .Trn,       .Val       => .Val
  | .Val,       .Trn       => .Val
  | .Val,       .Val       => .Val
  | .Ref,       .Val       => .Box
  | .Val,       .Ref       => .Box
  | .Iso,       .Box       => .Box
  | .Box,       .Iso       => .Box
  | .Trn,       .Box       => .Box
  | .Box,       .Trn       => .Box
  | .Ref,       .Box       => .Box
  | .Box,       .Ref       => .Box
  | .Val,       .Box       => .Box
  | .Box,       .Val       => .Box
  | .Box,       .Box       => .Box
  | .Tag,       .Tag       => .Tag
  | .Tag,       c          => c
  | c,          .Tag       => c

-- ------------------------------------------------------------------
-- Subtyping (partial order)
-- ------------------------------------------------------------------

/--
  `a ≤ b` iff the join of a and b is exactly b.
  Mirrors `Capability::is_subtype_of()`.
-/
def le (a b : Cap) : Bool := join a b == b

-- ------------------------------------------------------------------
-- Sendability for actor boundaries
-- ------------------------------------------------------------------

/--
  `a` is sendable iff values with capability `a` can be safely sent
  to another actor (the value is immutable and alias-tracked).
  Mirrors `Capability::is_sendable()` → `Linear | Val | Tag`.
-/
def is_sendable (a : Cap) : Bool :=
  match a with
  | .LinearIso | .Linear | .Val | .Tag => true
  | _ => false

/--
  `a` is isolated iff values with capability `a` can be sent AND
  provide full state isolation (unique ownership).
  Mirrors `Capability::is_isolated()` → `LinearIso | Linear | Iso | Val | Tag`.
-/
def is_isolated (a : Cap) : Bool :=
  match a with
  | .LinearIso | .Linear | .Iso | .Val | .Tag => true
  | _ => false

-- ------------------------------------------------------------------
-- Linear-to-iso promotion
-- ------------------------------------------------------------------

/--
  Discharge linear tracking: LinearIso → Iso, Linear → Val.
  Used when a linear value is consumed and the obligation is satisfied.
  Mirrors `Capability::discharge_linear()`.
-/
def discharge_linear (a : Cap) : Cap :=
  match a with
  | .LinearIso => .Iso
  | .Linear    => .Val
  | c          => c

-- ------------------------------------------------------------------
-- Lattice theorems (open proofs)
-- ------------------------------------------------------------------

/--
  **Theorem 1:** `join` is associative:
  `join (join a b) c == join a (join b c)` for all `a`, `b`, `c`.
-/
theorem join_assoc : ∀ (a b c : Cap), join (join a b) c = join a (join b c) := by
  intro a b c
  -- Case analysis on the 8³ = 512 combinations; can be discharged with
  -- `dec_trivial` once `Cap` is made `Decidable`.
  sorry

/--
  **Theorem 2:** `join` is commutative:
  `join a b == join b a` for all `a`, `b`.
-/
theorem join_comm : ∀ (a b : Cap), join a b = join b a := by
  intro a b
  sorry

/--
  **Theorem 3:** `join` is idempotent:
  `join a a = a` for all `a`.
-/
theorem join_idem : ∀ a : Cap, join a a = a := by
  intro a
  sorry

/--
  **Theorem: Sendable Capabilities are Safe for Actor Boundaries**
  If the runtime permits a value `v : τ @ cap` to cross an actor
  boundary, then `cap ≤ Val` (i.e., `is_sendable cap`).
  A value whose capability is `Iso`, `Trn`, or `Ref` must NOT cross
  an actor boundary — the capability lattice forbids it.
-/
theorem cap_sendable : ∀ (cap : Cap), is_sendable cap → le cap .Val := by
  intro cap h
  -- The proof reduces to checking each of the 8 capability cases.
  -- is_sendable matches LinearIso, Linear, Val, Tag.
  -- le LinearIso Val = true (by join table).
  -- le Linear Val = true (by join table).
  -- le Val Val = true (idempotence).
  -- le Tag Val = false — but is_sendable Tag is true and le Tag Val is false.
  -- This is a known divergence: `is_sendable` includes `Tag` because
  -- tagged pointers are safe to send (no dereference), but `le Tag Val`
  -- is false in the lattice. The actual guarantee is weaker:
  -- `is_tagged_or_val` is sufficient for sendability. The theorem
  -- statement needs refinement to match the implementation.
  sorry

/--
  **Theorem: Discharging linear tracking preserves sendability.**
  If `is_sendable cap`, then `is_sendable (discharge_linear cap)`.
-/
theorem discharge_sendable : ∀ (cap : Cap), is_sendable cap → is_sendable (discharge_linear cap) := by
  intro cap h
  sorry

end Cap

end Nulang
