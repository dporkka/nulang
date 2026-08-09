# Nulang Formal Semantics

> Machine-checked formal specification of the Nulang type system,
> capability lattice, and algebraic effects in Lean 4.
>
> **Status:** Bootstrap phase — type language, substitution, unification,
> capability lattice, and effect rows are formalized. Soundness proofs
> are stated as conjectures pending machine verification.

## Purpose

Per [GOVERNANCE.md §7](../../GOVERNANCE.md#7-authoritative-artifacts), the
formal model is the authoritative definition of Nulang's semantics. Where
the formal model and prose specification (`SPEC2.md`) disagree, the formal
model takes precedence.

## Structure

| File | Content | Status |
|---|---|---|
| `Nulang/Types.lean` | Type language, `freeVars`, `Subst`, `occurs`, `mgu` | Formalized |
| `Nulang/Capabilities.lean` | Capability lattice, `subtype`, `join`, `isSendable` | Formalized |
| `Nulang/Effects.lean` | Effect rows, `subrow`, `union` | Formalized |
| `Nulang/Soundness.lean` | Type soundness theorem + proof | Conjecture |
| `Nulang/CapSafety.lean` | Capability safety theorem + proof | Conjecture |
| `Nulang/EffectSafety.lean` | Effect safety theorem + proof | Conjecture |

## Theorems (Stated, Pending Proof)

### Type Soundness
```
Theorem type_soundness:
  ∅ ⊢ e : τ ∧ e ↦ v ⇒ ∅ ⊢ v : τ
```
A well-typed closed program either diverges or evaluates to a value of the
same type.  This is the fundamental correctness property of the type system.

### Capability Sendability
```
Theorem cap_sendable:
  isSendable c = true → value tagged with c can cross actor boundaries
  without violating isolation
```
Values with `iso`, `val`, `tag`, `lineariso`, or `linear` capabilities
are safe to send between actors.

### Effect Safety
```
Theorem effect_safety:
  A program with closed effect row {} cannot perform an unhandled effect
```
If a function's effect row is empty, every `perform` in its body is
statically handled — no runtime "unhandled effect" errors.

## Build

```bash
cd spec/formal
lake build
```

## References

- `src/types.rs` — Rust implementation (oracle)
- `src/typechecker.rs` — Algorithm W implementation
- `src/effect_checker.rs` — Effect + capability checker
- `GOVERNANCE.md` §7 — Authoritative artifacts
- RFC 0003 Item 2 — Formal semantics scoping
