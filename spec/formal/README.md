# Nulang Formal Semantics

> **Status:** `types.lean`'s three headline theorems (`progress`,
> `preservation`, `type_soundness`) are `sorry`-stubbed as of 2026-08-02 —
> see the regression note below. `capabilities.lean`'s lattice theorems are
> genuinely proved. `effects.lean` and `combined.lean` are definition-only.
> The implementation fuzzer (`src/fuzz.rs`) is the primary correctness
> mechanism.
> **Referenced implementation:** `src/typechecker.rs`

## Governance

**`SPEC2.md` (prose + inference rules) is the authoritative language
specification.** The Lean files in this directory model the Core fragment
and provide machine-checked proofs of type soundness for that fragment.
Where `SPEC2.md` and these files disagree on the Core fragment, these
files win for the Core fragment only. For capabilities, effects, and
distribution, `SPEC2.md` is authoritative and the Lean files are
descriptive sketches.

## Scope

| File | Coverage | Proof status |
|------|----------|-------------|
| `types.lean` | HM type system + Core expression language + call-by-value small-step semantics. `HasType` (13 rules), `Step` (14 rules). | **1/9 proved:** `canonical_forms` only. `weakening`, `substitution_lemma`, `progress`, `preservation`, `type_soundness`, `context_drop_shadowed`, `closed_type_under_closed_context`, `value_has_closed_type` are all `sorry` (regressed by `ac9ef5d`, 2026-07-26 — see below). |
| `capabilities.lean` | Capability lattice + capability-annotated typing judgment. | **5/6 proved:** `join_assoc`, `join_comm`, `join_idem`, `cap_sendable`, `discharge_sendable`. `linear_at_most_once` is `sorry` (needs context-splitting semantics not yet modeled — see the theorem's own doc comment). |
| `effects.lean` | Effect rows + effect-annotated typing judgment. | Definitions only; both theorems (`dispatch_type_preservation`, `effect_safety_static`) are vacuous `True` stubs, not proofs. |
| `combined.lean` | Unified judgment combining HM types, capabilities, and effect rows. | Definitions only (soundness conjecture stated, proof open). |

## Regression note (2026-08-02)

`types.lean` briefly achieved 0 sorries in commit `2740cfc` (2026-07-25),
using a custom induction principle (`HasType.rec_on_ctx`) to work around a
known weakening-proof subtlety: naive structural induction on the typing
derivation produces an induction hypothesis with the wrong context order
in the `tLambda`/`tLet` cases (`(x,σ)::(x₁,τ1)::Γ` from the IH vs. the
`(x₁,τ1)::(x,σ)::Γ` the goal needs), which are not interchangeable under
name shadowing when `x = x₁`. The very next commit (`ac9ef5d`,
2026-07-26) dropped that recursor because it broke under a Lean 4.16.0
upgrade, and reverted 9 theorem statements to `:= by sorry` to keep
`lake build` green — its own commit message honestly discloses "12 sorry
warnings". No downstream doc (this file, `SPEC2.md`, `CHANGELOG.md`,
`PLAN.md`) was updated to match until this correction. Re-proving
`weakening` requires either a context-splitting formulation (prove
insertion at an arbitrary position, specialize to position 0) or an
explicit freshness side condition; see `types.lean:463-466`.

## What is intentionally NOT formalized

The following are verified through other means (fuzzer, integration tests):

- **Capability soundness** — the capability lattice is enforced at
  compile time; runtime correctness is tested via `src/fuzz.rs`
- **Effect safety** — effect rows are checked statically; runtime
  handler dispatch is tested via integration tests
- **Distribution** — the wire protocol and actor model are tested via
  chaos/stress tests (`src/stress_tests.rs`)
- **LinearIso must-use** — at-most-once use is enforced by the
  capability analyzer; exactly-once is future work
- **Numeric semantics** — value-layout tag dispatch is tested via
  the bytecode VM test suite

## Known divergences from the implementation

| Item | Formal model | Implementation (`src/`) | Impact |
|------|-------------|------------------------|--------|
| Type representation | `Ty` uses de Bruijn-style vars; `Prim` includes `Unit`, `Nil` | `Type` uses nominal `TypeVar`; `PrimitiveType` includes `Float`, `String` | Low — Core fragment is sound for the modeled subset |
| BinOp representation | 3 separate typing rules (`tBinOpIntArith`, `tBinOpIntCmp`, `tBinOpBoolLogic`) | Single `tBinOp` with helper predicate | Low — logically equivalent |
| Effect checking | Static effect rows in `types.lean` | Runtime `handler_stack` in `vm.rs` | Medium — the formal model captures the static contract; the runtime model is not formalized |
| String concatenation | Typing rule `tStrConcat` | Built-in `strConcat` in bytecode | Low — straightforward typing rule |

## CI

The Lean files are checked in CI via `lake build` (`.github/workflows/ci.yml`).
A break in `lake build` signals a semantics-affecting change to the
Core fragment.

## Running locally

```bash
# Install Lean 4
curl https://raw.githubusercontent.com/leanprover/elan/master/elan-init.sh -sSf | sh

# Build the formal specs
cd spec/formal
lake build
```
