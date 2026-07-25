# Nulang Formal Semantics

> **Status:** Core fragment proved; capabilities/effects/combined are
> definition-only. The implementation fuzzer (`src/fuzz.rs`) is the
> primary correctness mechanism.
> **Proof assistant:** Lean 4 (≥ 4.15)
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
| `types.lean` | HM type system + Core expression language + call-by-value small-step semantics. `HasType` (13 rules), `Step` (14 rules). | **Proved:** progress, preservation, type soundness. 0 sorries. |
| `capabilities.lean` | Capability lattice + capability-annotated typing judgment. | Definitions only (no active proof development) |
| `effects.lean` | Effect rows + effect-annotated typing judgment. | Definitions only (no active proof development) |
| `combined.lean` | Unified judgment combining HM types, capabilities, and effect rows. | Definitions only (no active proof development) |

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
