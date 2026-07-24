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

## Stage 4 blocker: MIR register limit

Adding lambda/closure support to the Pratt parser requires ~261 local
variables, exceeding the MIR register allocator's capacity.

MIR register spilling (`SpillLoad`/`SpillStore` opcodes 0xF5/0xF6) was
added (2026-07-23), allowing functions to exceed the 239-register limit.
However, the post-processing spill rewrite has a capacity of 17 spilled
locals (~256 total MIR locals) due to register wrapping ambiguity;
functions exceeding this get a clear error instead of silent corruption.

The bootstrap parser's ~261 locals is just above this limit.

Workarounds:
- Reduce local count: inline helper functions, merge branches
- Split parser across multiple top-level functions (requires forward
  references or mutual recursion — not currently supported in Core)
- Implement full inline spilling (removes the 17-slot capacity limit)

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
