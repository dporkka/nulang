# Nulang Self-Hosting Bootstrap

> **Status:** Stage 5 — closures with environment capture working.
> **Target:** A Nulang→Nulang compiler written in Nulang Core (RFC 0002)
> that targets the `.nbc` format (RFC 0001).

## Architecture

```
source.nula
  → compiler_core.nula   (lexer + parser + evaluator in Core)
  → source.nbc            (frozen bytecode artifact)
  → VM::run(nbc)
```

## Files

| File | Purpose |
|------|---------|
| `host.nula` | Host shim |
| `compiler_core.nula` | Lexer + Pratt parser + evaluator in Nulang Core |
| `self_test.nula` | Core conformance target (fib(10) = 55) |
| `spill_bug_repro.nula` | Minimal repro for spill temp clobbering bug (fixed) |

## Running

```bash
nulang bootstrap/compiler_core.nula
# Expected: 42, 7, 9, 43, 200, 6, 36, 11, 8, 7
```

## What's implemented

### Stage 3 — Arithmetic + let bindings (2026-07-23)
- **Lexer:** character-at-a-time scanning via `perform String.charAt` / `String.length`.
- **Parser:** single-function Pratt parser with correct precedence and left-associativity.
- **Let bindings:** `let x = 42 in x + 1` → 43. 2-slot environment (e0, e1).

### Stage 5 — Closures with environment capture (2026-07-24)
- **Lambdas:** `fn(x) => x + 1` — parsed inline in the Pratt prefix handler.
- **Function application:** `f(arg)` — handled as a postfix operator with highest precedence.
- **Environment capture:** `let a = 3 in (fn(x) => a + x)(5)` → 8. The closure captures the defining environment (up to 1 binding, stored in bits 8-15 of the 30-bit closure tag).
- **Currying:** `let add = fn(a) => fn(b) => a + b in add(3)(4)` → 7.
- **Closure encoding:** 30-bit flag `1 << 30` in the high word of the 32-bit value; low 16 bits are the source position. Packed fields: flag | (ph << 23) | (body_start << 16) | (cap_hash << 8) | cap_value.
- **Out-of-band sentinel:** `left == 1 << 40` replaces `left == 0` to distinguish "no left operand" from the valid expression result 0.

### Register spilling (2026-07-24)
- Inline spilling (commit 06b03c6): no capacity limit.
- Round-robin temp registers (commit db22c67): prevents clobbering in multi-operand spilled reads.

## What remains

- HM type inference
- MIR lowering → `.nbc` codec
- Self-compilation (`compiler_core.nula` → `compiler_core.nbc`)
- Multi-binding environment capture (currently limited to 1 captured binding)
