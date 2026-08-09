# Nulang: Formal Verification

Mechanized metatheory for Nulang, formalized in the Rocq Prover (Coq). 
This covers a subset of Nulang: Hindley-Milner type system plus Algebraic Effects.

## What's proved (WIP)

We aim to prove type soundness via preservation + progress, determinism, and semantic equivalence.

### Included features
- Integer, Boolean, String, Unit literals
- Arithmetic operations
- Let bindings, branching
- Lambda / application
- Algebraic Effects (`perform`, `handle`)

## File structure

| File | Contents |
|------|----------|
| `Syntax.v` | Types, operators, expressions |
| `Typing.v` | Typing rules, contexts |
| `Semantics.v` | Small-step semantics |

## Building

Requires the Rocq Prover (>= 9.0). Install via:

```bash
brew install rocq-prover   # macOS
opam install rocq-prover   # or via opam
```

Then compile:
```bash
cd formal/
make
```
