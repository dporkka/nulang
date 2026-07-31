<p align="center">
  <img src="docs/src/assets/logo.svg" width="120" alt="Nulang logo">
</p>
<h1 align="center">Nulang</h1>
<p align="center">
  An actor-based language with algebraic effects, capability-based types, and durable/distributed actors — built for software that outlasts a process.
</p>
<p align="center">
  <a href="https://nulang.org">Website</a> •
  <a href="https://nulang.cloud">Nulang Cloud</a> •
  <a href="https://github.com/dporkka/nulang">GitHub</a>
</p>
<p align="center">
  <a href="https://www.rust-lang.org"><img src="https://img.shields.io/badge/rust-2021%20Edition-orange.svg" alt="Rust 2021"></a>
  <a href="https://github.com/dporkka/nulang/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-Apache%202.0-blue.svg" alt="License Apache 2.0"></a>
  <a href="https://github.com/dporkka/nulang/actions"><img src="https://github.com/dporkka/nulang/workflows/CI/badge.svg" alt="CI"></a>
  <a href="https://codecov.io/github/dporkka/nulang"><img src="https://codecov.io/github/dporkka/nulang/graph/badge.svg" alt="Coverage"></a>
</p>

---

## What is Nulang?

Nulang is an actor-based programming language with algebraic effects and
capability-based types. It fuses Erlang-style fault-tolerant actors with a
Hindley-Milner type system, reference capabilities (`iso`/`trn`/`ref`/`val`/`box`/`tag`/`lineariso`),
and row-polymorphic algebraic effects. The compiler pipeline (AST → HIR → MIR)
targets a register-based bytecode VM with a Cranelift JIT, an ahead-of-time
native backend, and an optional WASM backend. The runtime is a multi-threaded
work-stealing executor with supervision trees, ORCA garbage collection,
location-transparent distribution, and durable persistence.

Nulang is in **alpha** — the compiler, VM, JIT, actor runtime, supervision,
distribution, persistence, and AI runtime are all implemented and backed by
1490+ tests.

---

## Quick Start

**Prerequisites:** Rust 1.93+, Linux or macOS (Windows planned).

```bash
git clone https://github.com/dporkka/nulang.git
cd nulang
cargo build --release
```

```bash
nulang hello.nula              # compile + run
nulang --check hello.nula      # type-check only
nulang --eval '40 + 2'         # evaluate inline code
nulang --repl                  # interactive REPL
```

A Nulang program:

```nulang
perform IO.print("Hello, Nulang!")

let name = "World"
perform IO.print("Hello, " + name + "!")
```

> Run `nulang examples/01_hello.nula`. See [`examples/`](examples/) for
> 11 verified programs covering actors, effects, pattern matching, records,
> loops, arrays, and more.

---

## Feature Highlights

- **Algebraic effects** — `perform Effect.op(args)` / `handle body with { | Effect.op(x) => ... }` with resume semantics. Effect dependencies are explicit in function signatures via `!` rows.
- **Capability-based types** — `iso`, `trn`, `ref`, `val`, `box`, `tag`, and `lineariso` guarantee memory safety and data-race freedom. Checked at compile time; erased at runtime.
- **Hindley-Milner type inference** — full Algorithm W with row-polymorphic records, variant types, and algebraic effect rows.
- **Actors** — `spawn`, `send`/`!`, `ask`, selective `receive` with `after` timeout, links, monitors, supervision trees, process groups, and actor priority scheduling.
- **Entities & workflows** — `entity` declarations (durable-first, event-sourced by default). `workflow` declarations with steps, timers, signals, and saga compensation that survive restarts.
- **`let` and `var`** — immutable and mutable bindings. Records with `{ field: value }` syntax and `{ base .. field = new_val }` update syntax. Pattern matching with guards, alias patterns, and recursive sub-patterns. `**` exponentiation. Multi-line `"""..."""` strings with `\u{...}` unicode escapes. Pipe operator `|>`.
- **Error handling** — `catch expr fallback` (prefix or postfix), `fail Error(...)` for structured short-circuit return, `T ! E` return types, `?` unwrap.
- **FS file I/O** — `perform FS.read(path)`, `perform FS.write(path, content)`, `perform FS.append(path, content)`, `perform FS.exists(path)`.
- **Package manager** — `nula new/init/build/run/test/add/remove/list/clean/doc`. See [below](#package-manager).
- **Test runner** — `nula test` discovers `.nula` files under `tests/`; uses the `Test` effect (`perform Test.assert_eq(a, b)`, `perform Test.assert(cond, msg)`, etc.).
- **LSP server** — `nulang --lsp` with diagnostics, hover, goto-definition, references, rename, completion, inlay hints, formatting, signature help, and semantic tokens.
- **REPL** — `nulang --repl` with `:help <topic>`, `:type <expr>`, `:load <file>`, tab completion, and automatic multi-line input.
- **AI runtime** — `agent` declarations, LLM providers (OpenAI, Ollama), episodic/semantic/procedural memory, pipelines, debates, and supervisor teams. Gated behind the `ai-runtime` feature flag. *Experimental.*
- **Distribution** — location-transparent `send`/`ask` over TCP (NUL0 wire protocol), gossip membership, 8 CRDT types. *Experimental.*
- **WASM backend** — MIR→WASM compilation via `--backend wasm|wasm-run|wasm-aot`, Wasmtime host runtime with guard pages and SIMD. Gated behind the `wasm-backend` feature flag. *Experimental.*

---

## Documentation

| Document | Description |
|----------|-------------|
| [`docs/GETTING_STARTED.md`](docs/GETTING_STARTED.md) | Installation, values, effects, actors, pattern matching — with runnable code snippets |
| [`docs/PITFALLS.md`](docs/PITFALLS.md) | Common mistakes: `::` vs `.`, `let` vs `var`, `perform` keyword, `catch`/`fail`, record syntax, and more |
| [`examples/`](examples/) + [`README`](examples/README.md) | 11 verified, self-contained example programs |
| [`SPEC2.md`](SPEC2.md) | Language specification: syntax, semantics, type system, runtime, format stability contract |
| [`CHANGELOG.md`](CHANGELOG.md) | Changelog organized by stability tier (Frozen / Stable / Experimental) |
| [`GOVERNANCE.md`](GOVERNANCE.md) | Stability tiers, RFC process, and language versioning |
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | Implementation architecture and module map |
| [`RFC/`](RFC/) | RFC proposals (format stability, frozen core, deprecation cycles, roadmap) |

---

## Package Manager

Nulang ships with `nula`, a package manager invoked as `nulang nula <subcommand>`:

```bash
nulang nula new my-app       # scaffold a new package
nulang nula init             # initialize a package in the current directory
nulang nula build            # resolve dependencies + type-check
nulang nula run              # build and run the entry point
nulang nula test             # discover and run tests/ directory
nulang nula add <name>       # add a dependency (--path, --git, --version)
nulang nula remove <name>    # remove a dependency
nulang nula list             # list locked dependencies
nulang nula clean            # remove build artifacts
nulang nula doc              # generate Markdown API docs
```

---

## Project Status & Stability

Nulang is **alpha software**. The language version is `1.0.0-frozen`
(RFC 0001/0002). Every public surface is classified into one of three tiers
(see [`GOVERNANCE.md`](GOVERNANCE.md) for the full definitions):

| Tier | Scope |
|------|-------|
| **Frozen** | Never breaks — `.nbc` bytecode format, NUL0 wire protocol, value layout, Nulang Core, and the `IO`/`Spawn`/`Send`/`Receive` built-in effects. |
| **Stable** | HM type system, effect rows, capability lattice, actor surface, CRDT operations. Breaking changes require an RFC and a deprecation cycle. |
| **Experimental** | Everything else — feature flags (`wasm-backend`, `python`, `sqlite`, `lsp`, `ai-runtime`) and items marked Experimental in [`CHANGELOG.md`](CHANGELOG.md). |

1490+ tests pass with `cargo test`. Add `--features wasm-backend` for the
WASM backend test suite.

---

## Nulang Cloud

**[Nulang Cloud](https://www.nulang.cloud)** is an optional managed platform
for running Nulang actors in production — auto-scaling, zero cold start,
managed durability, and location-transparent messaging across regions.

The language and runtime in this repository are **Apache-2.0** and fully
self-hostable. No lock-in.

---

## Docker

A multi-stage Docker image is available. Build and run:

```bash
docker build -t nulang .
docker run --rm nulang --eval 'perform IO.print("Hello from Docker!")'
```

The image is ~50 MB and contains only the `nulang` binary and its runtime
dependencies.

## License

Nulang is licensed under the [Apache License, Version 2.0](https://github.com/dporkka/nulang/blob/main/LICENSE).

Copyright 2026 © David Porkka
