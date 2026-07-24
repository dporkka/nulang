# Nulang Formal Semantics

> **Status:** DRAFT — type definitions formalized; soundness proofs are open work.
> **Proof assistant:** Lean 4
> **Referenced implementation:** `src/typechecker.rs`, `src/effect_checker.rs`,
>   `src/types.rs`
> **Governance:** These formalizations are the **authoritative semantics.**
>   Where `SPEC2.md` (prose) and these files disagree, these files win.
>   See `GOVERNANCE.md` §7.


## What is formalized (updated 2026-07-23)

| File | Coverage | Proof status |
|------|----------|-------------|
| `types.lean` | HM type system + Core expression language + call-by-value small-step semantics. `HasType` (11 rules), `Step` (14 rules), progress/preservation/soundness stated. | Soundness stated; proofs open |
| `capabilities.lean` | Capability lattice + capability-annotated typing judgment `HasTypeCap` (10 rules including send/spawn). `CapContext`, linear consumption (`consumed`), `linear_at_most_once` theorem. | Definitions complete; proofs open |
| `effects.lean` | Effect rows + effect-annotated typing judgment `HasTypeEff` (11 rules including perform/handle). `EffExpr` (11 constructors), `HandlerStack` transitions, `effect_safety_static` theorem. | Definitions complete; proofs open |
| `combined.lean` | **Unified judgment** `Γ; Δ ⊢ e : τ @ cap ! r` — combines HM types, capabilities, and effect rows into a single 15-rule inductive. Includes send/spawn/perform/handle. `combined_soundness` conjecture stated. | Definitions complete; proof open |

## What is NOT yet formalized

- The **proofs** for all four soundness theorems (types, capabilities,
  effects, combined) — the definitions and theorem statements are
  complete; the proof terms are `sorry`.
- LinearIso must-use (the constraint is at-most-once; exactly-once
  is a documented follow-up in AGENTS.md).
- Effect handler scoping and frame management (the runtime's
  `handler_stack` push/pop protocol — the static model is defined;
  the dynamic model is future work).
- The numeric semantics of `Int`/`Float` (value-layout tag dispatch).
