# Nulang Self-Hosting Bootstrap

> **Status:** Stage 1 scaffold — Core compiler skeleton exists; full
> self-hosting is a multi-session follow-up.
> **Target:** A Nulang→Nulang compiler written in Nulang Core (RFC 0002)
> that targets the `.nbc` format (RFC 0001).

## Architecture

```
source.nula
  → compiler_core.nula   (lexer + parser + typechecker + codegen in Core)
  → source.nbc            (frozen bytecode artifact)
  → VM::run(nbc)          (output = same as Rust-cih compiler)
```

Stage 1: `compiler_core.nula` compiles a trivial Core subset (Int literals
and binary `+`).  The Rust compiler is the host; `host.nula` is the
thin shim that will eventually chain: bootstrap compiler → `.nbc` → run.

Stage 2: `compiler_core.nula` compiles itself (full Core).

## Files

| File | Purpose |
|------|---------|
| `host.nula` | Host shim — invokes bootstrap compiler under Rust compiler |
| `compiler_core.nula` | Minimal lexer + parser + emitter in Nulang Core |
| `self_test.nula` | Core program that the bootstrap compiler must compile correctly |

## Running (today)

```bash
# Run the bootstrap compiler (hosted by Rust) on a Core program:
nulang bootstrap/host.nula -- bootstrap/self_test.nula

# Emit .nbc and verify round-trip:
nulang --emit-nbc --out bootstrap/self_test.nbc bootstrap/self_test.nula
nulang bootstrap/self_test.nbc
```

## What's implemented

- **lexer:** single-token scanning (`+`, `IntLit`, `EOF`)
- **parser:** `IntLit` and `IntLit + IntLit` expressions
- **emitter:** bytecode `ConstU` + `IAdd` + `Halt` + `RetVal`

## What remains (Stage 2)

- Multi-token lexer (identifiers, keywords, delimiters)
- Pratt parser for full Core expressions
- HM type inference (deferred to separate module)
- MIR lowering → `src/format/nbc.rs` codec
- Self-compilation (`compiler_core.nula` → `compiler_core.nbc`)
