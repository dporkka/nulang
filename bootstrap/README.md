# Nulang Bootstrap Compiler

> Self-hosting path for Nulang: a Nulang→`.nbc` compiler written in Nulang Core
> (RFC 0002). Decouples the language from the Rust host's survival.

## Strategy (3-stage bootstrap)

### Stage 0 (Current — Rust host)
The existing Rust compiler (`nulang`) parses, type-checks, and compiles Nulang
Core programs. It serves as the **host** for Stages 1–2.

### Stage 1 (In Progress — Nulang Core emitter)
`emitter.nula` — a Nulang Core program that takes a simplified AST
representation (instruction sequence + metadata) and emits structured JSON.

The Rust host (`src/bootstrap_host.rs` or `bootstrap/host.rs`) converts the
JSON output to `.nbc` binary format (RFC 0001).

This stage proves that Nulang Core can produce the `.nbc` format. The emitter
does not parse Nulang syntax yet — it consumes a pre-lowered representation.

### Stage 2 (Planned — Self-compiling)
A full Nulang Core parser + type-checker + code generator, written in Nulang
Core, that compiles itself. Stage 2 compiles Stage 2 source → `.nbc`, and the
output (run on the Rust host) compiles the same source → identical `.nbc`.

### Stage 3 (Planned — Independence)
The Stage 2 `.nbc` is run on a minimal Core VM (pure interpreter, no JIT, no
WASM, no AI, no Python — just the frozen Core opcodes). That VM can be ported
to new hardware without the Rust toolchain, achieving full host independence.

## Directory Structure

```
bootstrap/
├── README.md           # This file
├── emitter.nula        # Stage 1: JSON emitter for .nbc generation
├── host.rs             # Rust host: runs emitter, converts JSON → .nbc
└── self_test.nula      # Minimal Nulang Core test program
```

## Build & Test

```bash
# Stage 1: Run the emitter (produces JSON on stdout)
cargo run -- bootstrap/emitter.nula

# Run the bootstrap self-test
cargo test -- bootstrap_host
```

## Core Constraints

The emitter uses ONLY Nulang Core (RFC 0002):
- Expressions: `let`, `if`/`else`, `match`, function application, arithmetic
- Types: `Int`, `Bool`, `String`, `Unit`, `Nil`, `Vec<T>`, `Map<K,V>`, records
- Declarations: `fn`, `const`
- Effects: `IO.print` and `IO.read` only
- Capabilities: `val` only (implicit)

## References

- RFC 0001: Format Stability (`.nbc` format)
- RFC 0002: Frozen Core
- RFC 0003: Remaining Longevity Roadmap (Item 3)
- `src/format/nbc.rs`: `.nbc` encoder/decoder
- `src/bytecode.rs`: Instruction encoding
