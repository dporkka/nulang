# Nulang Formal Semantics

> Machine-checked formal specification of the Nulang type system,
> capability lattice, and algebraic effects in Lean 4.
>
> **Status:** Bootstrap phase — type language, substitution, unification,
> capability lattice, and effect rows are formalized. Soundness proofs
> are stated as conjectures pending machine verification.
>
> **Proof status (2026-08-14):** the FULL Core type-soundness chain is
> PROVED in `types.lean` — zero `sorry`s: `weakening` (corrected
> statement), `progress`, `closed_type_under_closed_context`,
> `value_has_closed_type`, `substitution_lemma` (corrected statement),
> `preservation`, and `type_soundness` (sorry count 9 → 1; CI ratchet
> baseline updated). The single remaining `sorry` is
> `linear_at_most_once` in `capabilities.lean`, which needs the
> context-splitting semantics — a modeling gap, not a proof gap.

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

## Regression note (2026-08-02, extended 2026-08-14)

`types.lean`'s headline theorems (`progress`, `preservation`,
`type_soundness`) were silently regressed to `sorry` by a Lean 4.16.0
compatibility-fix commit (`ac9ef5d`, 2026-07-26); no downstream doc was
updated until 2026-08-02. A CI sorry-count ratchet
(`.github/workflows/ci.yml`, baseline 5) prevents silent recurrence.

**Root cause, now fully repaired (2026-08-14):** the naive head-form
weakening `HasType Γ e τ → HasType ((x,σ)::Γ) e τ` is FALSE for open
schemes σ — the `tLet` case generalizes over the larger context, and
the `tVar` case finds the new head binding, so the derivation does not
lift. The correct formulation appends a CLOSED binding at the TAIL
(`HasType (Γ ++ [(x, ⟨[], τ₀⟩)]) e τ` with `τ₀.fv = []`): head-first
lookup never sees it and the let-generalization is unchanged.
`weakening_append_closed` + the corrected `weakening` are proved.

The same statement-repair class applied to the rest of the chain, now
all proved:
  * `substitution_lemma` — the naive statement (arbitrary Γ, `v`
    typed in Γ) is FALSE: the recursion's contexts grow with `e`'s
    binders and lifting `v` into them is capture-prone.  The proved
    form requires `v` typed in the EMPTY context, `Γ` free of type
    variables with monomorphic observable schemes (`Context.Mono`),
    and closed annotations — every condition preservation satisfies.
    The λ case permutes the two head bindings (`HasType_permute`,
    valid because the substituted scheme is closed); the same-name
    cases drop the shadowed binding (`drop_shadowed_closed`).
  * `lift_from_empty` — a closed typing lifts to any type-closed
    context (the derivation never uses `tVar`, and the
    let-generalizations agree).
  * `preservation` / `type_soundness` — the standard induction on
    `Step` / `Steps`, with `step_preserves_closed` +
    `annotationsClosed_subst` keeping the closedness invariant.
  * `binOpApply`'s div/mod-by-zero cases returned `.unitVal`, which
    BREAKS preservation (unit ≠ int); corrected to Lean's total Int
    division (x / 0 = 0), keeping the result an Int literal.
The remaining `sorry` (`linear_at_most_once`, capabilities.lean)
needs the context-splitting semantics — a modeling extension, not a
proof repair.

## References

- `src/types.rs` — Rust implementation (oracle)
- `src/typechecker.rs` — Algorithm W implementation
- `src/effect_checker.rs` — Effect + capability checker
- `GOVERNANCE.md` §7 — Authoritative artifacts
- RFC 0003 Item 2 — Formal semantics scoping
