# Nulang Formal Semantics

> Machine-checked formal specification of the Nulang type system,
> capability lattice, and algebraic effects in Lean 4.
>
> **Status:** The Core soundness chain — `progress`, `preservation`,
> `type_soundness` — is machine-checked (2026-08-14). The capability
> lattice laws (`join` assoc/comm/idem) and `cap_sendable`/
> `discharge_sendable` are proved. Two items remain open:
> `linear_at_most_once` (requires the split-context refinement of
> `HasTypeCap`, documented in `capabilities.lean`) and the effect-safety
> theorems (`effects.lean`), which are still vacuous `True` stubs, not
> proofs (deferred to the `combined.lean` handler-stack model).

## Purpose

Per [GOVERNANCE.md §7](../../GOVERNANCE.md#7-authoritative-artifacts), the
formal model is the authoritative definition of Nulang's semantics. Where
the formal model and prose specification (`SPEC2.md`) disagree, the formal
model takes precedence.

## Structure

Two layers are formalized:

| File | Content | Status |
|---|---|---|
| `Nulang/Types.lean` | Type language, `freeVars`, `Subst`, `occurs`, `mgu` | Formalized |
| `Nulang/Capabilities.lean` | Capability lattice, `subtype`, `join`, `isSendable` | Formalized |
| `Nulang/Effects.lean` | Effect rows, `subrow`, `union` | Formalized |
| `types.lean` | HM `HasType`, small-step semantics, `progress`/`preservation`/`type_soundness` | **Proved** |
| `capabilities.lean` | Capability lattice laws, `cap_sendable`, `discharge_sendable` | Proved (`linear_at_most_once` open) |
| `effects.lean` | Effect rows, `effect_safety`, `effect_safety_static` | `True` stubs (not proved) |

## Theorems

### Type Soundness — proved
```
Theorem type_soundness:
  ∅ ⊢ e : τ ∧ e ↦ v ⇒ ∅ ⊢ v : τ
```
A well-typed closed program either diverges or evaluates to a value of the
same type.  Proved via `progress` (well-typed closed terms are values or
can step) and `preservation` (stepping preserves type under a closed-annotation
invariant `annotationsClosed`, plus the closed-value `substitution_lemma`).
This is the fundamental correctness property of the type system.

### Capability Sendability — proved
```
Theorem cap_sendable:
  isSendable c = true → value tagged with c can cross actor boundaries
  without violating isolation
```
Values with `iso`, `val`, `tag`, `lineariso`, or `linear` capabilities
are safe to send between actors.

### Linear at-most-once — open conjecture
`linear_at_most_once` states that a `LinearIso` binding is consumed after its
single use.  This property is **not** a theorem of the single-context
`HasTypeCap` judgment (it is false there — see the counterexample in
`capabilities.lean`), because that judgment carries no *output* context.  It
requires the split-context refinement `Γ ⊢ e : τ / Γ'`, which is out of scope
for the current formalization and stated as a conjecture per the RFC 0003
Item 2 contingency.

### Effect Safety — stubbed (not proved)
```
Theorem effect_safety:
  A program with closed effect row {} cannot perform an unhandled effect
```
The intended statement: if a function's effect row is empty, every `perform`
in its body is statically handled — no runtime "unhandled effect" errors.
The current `effect_safety`/`effect_safety_static` bodies in `effects.lean`
are vacuous `True` stubs (`by trivial`), not proofs — the real proof requires
modeling the handler-stack push/pop dynamics, deferred to `combined.lean`.

## Build

```bash
cd spec/formal
lake build
```

## Build-graph note (2026-08-14)

`spec/formal/lakefile.lean` roots `#[Nulang, types, capabilities, effects]`.
From 89bd0d6 (2026-08-09) until 2026-08-14 the root list was `#[Nulang]`
only — the top-level `types.lean`/`capabilities.lean`/`effects.lean`
(the Core soundness formalization) were orphaned from `lake build`.
Proofs claimed for those files in commits fe610d8/dd3aafa were never
type-checked and have been reverted; the honest 9-`sorry` state of
2026-08-02 through 2026-08-14 was the 8 Core soundness sorries in
`types.lean` plus `linear_at_most_once` in `capabilities.lean`.  The
soundness chain was proved 2026-08-14, leaving `linear_at_most_once` as the
single remaining `sorry` (CI sorry-ratchet baseline is now 1).

## References

- `src/types.rs` — Rust implementation (oracle)
- `src/typechecker.rs` — Algorithm W implementation
- `src/effect_checker.rs` — Effect + capability checker
- `GOVERNANCE.md` §7 — Authoritative artifacts
- RFC 0003 Item 2 — Formal semantics scoping
