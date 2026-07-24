# Nulang Self-Hosting Bootstrap

> **Status:** Stage 3 — identifiers, let bindings, variable references working.
> Stage 4 (lambdas/closures) blocked by MIR register limit (see below).
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

## Running

```bash
nulang bootstrap/compiler_core.nula
# Expected: 42, 7, 9, 43, 200
```

## What's implemented (Stage 3 — 2026-07-23)

- **Lexer:** character-at-a-time scanning via `perform String.charAt` /
  `String.length`. Recognises integers, identifiers, `let`, `in`, `fn`,
  `+`, `-`, `*`, `/`, `(`, `)`, whitespace.
- **Parser:** single-function Pratt parser (no forward references needed).
  Correct operator precedence and left-associativity.
- **Let bindings:** `let x = 42 in x + 1` → 43. 2-slot environment (e0, e1).
- **Variable references:** identifier hashing (hash*5, seed 0). "let"=3321.
- **Return-value encoding:** `(val << 32) | pos` packs value + position.

## Register limit status (resolved 2026-07-24)

Inline register spilling (commit 06b03c6) removed the capacity limit.

## Stage 4 blocker: compiler bug with large spilled functions

Lambda support requires adding ~40 lines to `parse_pratt`, which creates
enough spilled locals (~88) to trigger a correctness bug in the inline
spilling compiler (spill_bug_repro3.nula, 2026-07-24).  Functions with
>~50 spilled locals inside nested if/else branches produce wrong
arithmetic results (e.g. `1 + 2 * 3` returns 1 instead of 7).

Minimal reproduction at `/tmp/spill_bug_repro3.nula` (bisected threshold
at n=24 dummies per branch, ~54 spills).  Linear functions with the same
local count work correctly — the bug is specific to branched code.

Once this compiler bug is fixed, Stage 4 can proceed.

## What remains

- Lambda/closure support (Stage 4)
- HM type inference
- MIR lowering → `.nbc` codec
- Self-compilation (`compiler_core.nula` → `compiler_core.nbc`)

## Related RFCs implemented in this session

- RFC 0008: `migration` block parsing (parser + AST + HIR + ActorMeta)
- RFC 0009: `organization` keyword parsing (desugars to entity)
- RFC 0003 Item 6: `CryptoProvider`, `ForeignInterop` traits
- RFC 0003 Item 2: `combined.lean` unified typing judgment
