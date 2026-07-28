# Nulang Self-Hosting Bootstrap

> **Status:** Stage 9 — bytecode compiler for arithmetic, stdin REPL.
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
| `compile_arith.nula` | Bytecode compiler for arithmetic (prints VM instructions) |
| `self_test.nula` | Core conformance target (fib(10) = 55) |
| `spill_bug_repro.nula` | Minimal repro for spill temp clobbering bug (fixed) |

## Running

```bash
# Interactive evaluator (stdin):
echo "1 + 2 * 3" | nulang bootstrap/compiler_core.nula
# → 7

# Bytecode compiler (stdin):
echo "1 + 2 * 3" | nulang bootstrap/compile_arith.nula
# ; 1 + 2 * 3
#   Const1 r8
#   Const2 r9
#   ConstU r10 # 3
#   IMul r9 r10 r11
#   IAdd r8 r11 r10
#   Halt
# ; result in r10

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
- **Boolean operators:** `and` (prec 1), `or` (prec 0), `not` (prefix).
- **Boolean literals:** `true` → 1, `false` → 0.
- **Stdin REPL:** reads expression from stdin, evaluates, prints result.

### Stage 9 — Bytecode compiler: arithmetic, if/then/else, let bindings (2026-07-28)
- **compile_arith.nula:** single-pass Pratt compiler emits VM instructions as text.
- Supports integer literals, `+`, `-`, `*`, `/`, and parenthesized expressions.
- Register allocation: starts at r8, linear assignment per subexpression.
- **let bindings:** scoped variables via env (hash|reg), 4 slots. Variable refs emit Move.
- **if/then/else:** JmpF/Jmp with position-based labels (L0e/L0x).
- Outputs `Const0/1/2/M1/U`, `IAdd/ISub/IMul/IDiv`, `Move`, `ICmp*`, `JmpF` (short-circuit), `Jmp`, `Halt`.

## What remains

- HM type inference
- MIR lowering → `.nbc` codec
- Self-compilation (`compiler_core.nula` → `compiler_core.nbc`)
- Multi-binding closure capture (closures still limited to 1 captured variable)
