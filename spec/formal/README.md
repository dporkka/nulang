# Nulang Formal Semantics

> **Status:** DRAFT — type definitions formalized; soundness proofs are open work.
> **Proof assistant:** Lean 4
> **Referenced implementation:** `src/typechecker.rs`, `src/effect_checker.rs`,
>   `src/types.rs`
> **Governance:** These formalizations are the **authoritative semantics.**
>   Where `SPEC2.md` (prose) and these files disagree, these files win.
>   See `GOVERNANCE.md` §7.

## What is formalized

| File | Coverage | Proof status |
|------|----------|-------------|
| `types.lean` | HM type system: `Type`, `Scheme`, `Substitution`, unification (`mgu`), generalization, instantiation | Soundness stated; proofs open |
| `capabilities.lean` | Capability lattice: `iso/trn/ref/val/box/tag/lineariso`, `join`, `is_subtype_of`, `is_sendable`, LinearIso consumption | Definitions complete; proofs open |
| `effects.lean` | Effect rows: `Closed/Open + Region`, handler dispatch, `Perform/Resume/Unwind` | Definitions complete; proofs open |

## What is NOT yet formalized

- The **combined** soundness of HM + capabilities + row effects
  (each subsystem is formalized separately; the interaction is a
  conjecture).
- LinearIso must-use (the constraint is at-most-once; exactly-once
  is a documented follow-up in AGENTS.md).
- Effect handler scoping and frame management (the runtime's
  `handler_stack` push/pop protocol).
- The numeric semantics of `Int`/`Float` (value-layout tag dispatch).

## How to build

```bash
cd spec/formal
lake build
```

Requires Lean 4 (`lean` and `lake` on PATH).

## Proof plan

1. **Theorem type_soundness** (`types.lean`):
   `∅ ⊢ e : τ ∧ e ↦ v ⇒ ∅ ⊢ v : τ` (progress + preservation).
   Standard HM; the obstacle is integrating capability annotations
   and effect rows into the typing judgment (see §2).

2. **Theorem cap_sendable** (`capabilities.lean`):
   If `Γ ⊢ e : τ @ cap` and `cap ≤ val`, then `e` crosses actor
   boundaries without violating isolation. Follows Pony's
   `is_sendable` proof.

3. **Theorem effect_safety** (`effects.lean`):
   `Δ ⊢ e : τ ! {σ₁,…,σₙ}` and `{σ₁,…,σₙ}` is closed ⇒ no
   unhandled effect can occur at runtime. Follows Koka's handler
   soundness.

Each theorem is stated in the corresponding Lean file as a `theorem`
with its hypotheses. The proofs are `sorry` pending completion —
the definitions are correct; the missing proofs are a documented
research task, not an unknown gap.
