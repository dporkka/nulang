# Nulang Self-Hosting Bootstrap

> **Status:** Stage 8 — stdin REPL, comparisons, booleans, if/then/else, 4-slot env.
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
# Interactive: evaluate an expression from stdin
echo "1 + 2 * 3" | nulang bootstrap/compiler_core.nula
# → 7

# Self-test (when stdin is empty):
nulang bootstrap/compiler_core.nula < /dev/null
# Expected: 42, 7, 9, 43
```

## What's implemented

### Stage 3 — Arithmetic + let bindings (2026-07-23)
- **Lexer:** character-at-a-time scanning via `perform String.charAt` / `String.length`.
- **Parser:** single-function Pratt parser with correct precedence and left-associativity.
- **Let bindings:** `let x = 42 in x + 1` → 43. 2-slot environment (e0, e1).

### Stage 5 — Closures with environment capture (2026-07-24)
- **Lambdas:** `fn(x) => x + 1` — parsed inline in the Pratt prefix handler.
- **Function application:** `f(arg)` — handled as a postfix operator with highest precedence.
- **Environment capture:** `let a = 3 in (fn(x) => a + x)(5)` → 8.
- **Currying:** `let add = fn(a) => fn(b) => a + b in add(3)(4)` → 7.

### Stage 6 — 4-slot environment (2026-07-28)
- **Environment:** expanded from 2 slots to 4 slots (e0..e3), supporting 4 nested `let` bindings.
- **Lookup:** recursive `env_lookup` searches most-recent slot first for correct shadowing.

### Stage 7 — if/then/else (2026-07-28)
- **Conditional:** `if <cond> then <then> else <else>` — parsed in the Pratt prefix handler.
- Non-zero condition values are truthy; zero is falsy.

### Stage 8 — Comparisons + booleans + stdin (2026-07-28)
- **Comparisons:** `==`, `!=`, `<`, `>`, `<=`, `>=` — all return 1 (true) or 0 (false).
  Precedence 3 (between `and`/`or` and arithmetic).
- **Boolean operators:** `and` (prec 1), `or` (prec 0) — return 1 or 0.
- **Boolean literals:** `true` → 1, `false` → 0.
- **Prefix `not`:** `not x` → 1 if x is 0, else 0.
- **Precedence levels:** `or`(0) < `and`(1) < comparisons(3) < `+`/`-`(4) < `*`/`/`(5).
- **Stdin REPL:** reads expression from stdin and evaluates it. Falls back to self-tests on empty input.

## What remains

- HM type inference
- MIR lowering → `.nbc` codec
- Self-compilation (`compiler_core.nula` → `compiler_core.nbc`)
- Multi-binding closure capture (closures still limited to 1 captured variable)
