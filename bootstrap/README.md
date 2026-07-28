# Nulang Self-Hosting Bootstrap

> **Status:** Stage 10 — end-to-end hex → .nbc pipeline.
> **Target:** A Nulang→Nulang compiler written in Nulang Core (RFC 0002)
> that targets the `.nbc` format (RFC 0001).

## Architecture

```
source.nula
  → compiler_core.nula      (lexer + parser + evaluator in Core)
  → compile_hex.nula         (Core → hex bytecode emitter)
  → fixup_hex.py             (patch jump offsets + constant pool)
  → hex2nbc.py               (hex → .nbc binary)
  → source.nbc               (frozen bytecode artifact)
  → VM::run(nbc)
```

## Files

| File | Purpose |
|------|---------|
| `host.nula` | Host shim |
| `compiler_core.nula` | Lexer + Pratt parser + evaluator in Nulang Core |
| `compile_arith.nula` | Bytecode compiler for arithmetic (prints VM instructions) |
| `compile_hex.nula` | Hex-output bytecode compiler (u32 words as 8-char hex) |
| `fixup_hex.py` | Patch Jmp/JmpF/JmpT offsets and ConstU indices |
| `hex2nbc.py` | Convert hex text to .nbc binary |
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

# Hex bytecode compiler (piped through fixup):
echo "1 + 2 * 3" | nulang bootstrap/compile_hex.nula | python3 bootstrap/fixup_hex.py

# Full .nbc pipeline:
echo "1 + 2 * 3" | nulang bootstrap/compile_hex.nula | python3 bootstrap/fixup_hex.py | python3 bootstrap/hex2nbc.py > out.nbc
nulang out.nbc
# → 7

# Hex compiler self-test (when stdin is empty, compiles "1 < 2 and 2 < 3"):
nulang bootstrap/compile_hex.nula < /dev/null

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

### Stage 9 — Bytecode compiler (text) + hex output (2026-07-28)
- **compile_arith.nula:** single-pass Pratt compiler emits VM instructions as text.
- Supports integer literals, `+`, `-`, `*`, `/`, and parenthesized expressions.
- Register allocation: starts at r8, linear assignment per subexpression.
- **let bindings:** scoped variables via env (hash|reg), 4 slots. Variable refs emit Move.
- **if/then/else:** JmpF/Jmp with position-based labels (L0e/L0x).
- Outputs `Const0/1/2/M1/U`, `IAdd/ISub/IMul/IDiv`, `Move`, `ICmp*`, `JmpF` (short-circuit), `Jmp`, `JmpF`, `Jmp`, `Halt`.

### Stage 10 — Hex bytecode output + .nbc pipeline (2026-07-28)
- **compile_hex.nula:** emits u32 instruction words as 8-char hex (one per line).
- Adds `hex_digit` helper and `emit_hex` for hex formatting.
- Works around Nulang string-var concatenation bug using `""` prefix trick.
- **fixup_hex.py:** patches Jmp/JmpF/JmpT offsets and ConstU indices in a two-pass fixup.
- **hex2nbc.py:** converts corrected hex text to `.nbc` binary (NLBC magic, header, JSON metadata).
- **Bool/Int conversion:** comparisons return Bool-tagged values (bit 39); `and`/`or`/`if` convert Int→Bool via `ICmpEq+Not` when needed. `!=` lowered to `ICmpEq+Not`.
- Full pipeline: `compile_hex.nula | fixup_hex.py | hex2nbc.py > out.nbc`
- Outputs `Const0/1/2/M1/U`, `IAdd/ISub/IMul/IDiv`, `ICmp*`, `Not`, `Move`, `JmpF`, `JmpT`, `Jmp`, `Halt`.

## What remains

- HM type inference
- MIR lowering → `.nbc` codec
- Self-compilation (`compiler_core.nula` → `compiler_core.nbc`)
- Multi-binding closure capture (closures still limited to 1 captured variable)
