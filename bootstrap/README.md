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

Inline register spilling (commit 06b03c6) replaced the post-processing
spill rewrite with SpillLoad/SpillStore emission during codegen.  There
is no longer any capacity limit on spilled locals — functions of any
size compile correctly, limited only by the compiler frontend's stack
size (roughly 370 MIR locals with the default 8 MB stack).

The bootstrap parser's ~261 locals is now well within the supported range.

## What remains

- Lambda/closure support (Stage 4) — the Pratt parser needs `fn(x) => body`
  syntax and function-application evaluation
- HM type inference
- MIR lowering → `.nbc` codec
- Self-compilation (`compiler_core.nula` → `compiler_core.nbc`)

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
