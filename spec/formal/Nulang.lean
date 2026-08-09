/-
Nulang Formal Semantics
=======================

Machine-checked formalization of the Nulang type system, capability
lattice, and algebraic effects in Lean 4.

This is the authoritative formal specification per GOVERNANCE.md §7:
where the formal model and prose disagree, the formal model is
authoritative.

Structure:
- `Types.lean`        — Type language, substitution, unification (HM Algorithm W)
- `Capabilities.lean` — Reference capability lattice, subtyping, join, sendability
- `Effects.lean`      — Row-polymorphic algebraic effects, subsumption, union

Future:
- `Soundness.lean`    — Type soundness proof (∅ ⊢ e : τ ∧ e ↦ v ⇒ ∅ ⊢ v : τ)
- `CapSafety.lean`    — Capability safety proof (sendable values cross actor boundaries safely)
- `EffectSafety.lean` — Effect safety proof (closed rows ⇒ no unhandled effects)
-/

import Nulang.Types
import Nulang.Capabilities
import Nulang.Effects
