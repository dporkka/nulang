# Nulang Language Specification v2.0

## July 2026

---

# Forward

This document defines the Nulang programming language, version 2.0. It is intended as the authoritative reference for both language implementers and users, providing a complete and precise account of Nulang's syntax, semantics, type system, runtime model, and standard library.

Nulang 2.0 represents a significant architectural evolution from the 1.x series. Where the earlier specification treated AI agents, distributed computing, and persistence as separate subsystems accessed through domain-specific keywords (`agent`, `cluster`, `store`), version 2.0 unifies these concerns under a single, coherent abstraction: the actor. In Nulang 2.0, all concurrent and distributed computation is expressed through actors. AI capabilities are granted to actors through the capability system, not through a separate agent DSL. Durability is a property of actors, not a separate storage layer. Distribution is an emergent property of the actor runtime, not a bolt-on framework.

This unification yields a language with fewer primitives and greater compositional power. A programmer learns one abstraction—the actor with behaviors, state, and effects—and applies it uniformly from a single-threaded script to a globally distributed, durable workflow. AI agents are one composition of these primitives; they are not a separate language surface.

The specification is organized into five conceptual layers:

1. **The Language Layer** (Chapters 1–7) defines the core language: syntax, types, algebraic effects, capability-based security, expressions, and declarations. This layer is self-contained and can be implemented independently of any runtime.

2. **The Actor Runtime Layer** (Chapter 8) defines the actor model: how actors are declared, how they communicate via asynchronous message passing, how they manage state, and how they are supervised. This layer is the foundation upon which all higher layers are built.

3. **The Durable Execution Layer** (Chapter 9) extends the actor runtime with persistence. Persistent actors survive process restarts through automatic checkpointing, event journaling, deterministic replay, and snapshotting.

4. **The Distributed Platform Layer** (Chapter 12) extends the durable actor runtime across machine boundaries. Virtual actors are transparently activated on any cluster node (**Planned**). Messages are routed across the network. CRDT state converges automatically (**Planned** — the CRDT replication machinery is implemented and tested at the Rust level, but `state crdt` fields are not yet wired to it and behave as `durable`; see §9.10 and §12.5). Faults are contained and recovered.

5. **The AI Runtime Layer** (Chapter 11) provides language-integrated access to large language models, tool use, memory systems, and planning. AI capabilities are expressed through the same algebraic effect system used for IO and network effects, and are gated by the same capability-based security model.

Each chapter contains a detailed outline of all sections and subsections, followed by the prose and examples for those sections. Chapters 1 through 3 are fully written. Chapters 4 through 15 contain detailed outlines with section headings, descriptive bullet points, and at least one complete code example per chapter to illustrate the key concepts.

Unless otherwise noted, examples in sections describing *implemented* features are complete, syntactically valid Nulang programs or program fragments under the current compiler. Sections describing unimplemented features are marked **Planned**, and their examples are aspirational.

---

# Implementation Status (Current Alpha)

This document is the design target for Nulang 2.0. The implementation in this repository is an alpha (the v0.9 series) that realizes a substantial subset of the design. This section records, as of the current commit, what is implemented and what remains planned, so readers can distinguish descriptions of working behavior from aspirational ones. Sections that describe unimplemented surface are marked **Planned** inline.

> **Verification note (July 2026).** The syntax, keyword, and semantic claims in Chapters 1–12 and Appendices A–C were re-verified against the implementation in July 2026 — specifically `src/lexer.rs` (keyword inventory, literals, operators), `src/parser.rs` (grammar), `src/ast.rs` (AST shapes), `src/typechecker.rs` (inference, defaults), `src/effect_checker.rs` (effect rows, capability lattice, sendability), `src/vm.rs` (runtime effect dispatch, arithmetic), `src/hir_lower.rs` (pipe semantics, AI builtins), `src/main.rs` (CLI), and `src/fuzz.rs` (typechecker fuzzer). Chapters 13–15 and Appendix D describe planned surfaces and were only annotated as such, not verified line-by-line.

**Implemented and verified against the source tree:**

- Triple-quoted multi-line strings (`\"\"\"...\"\"\"`) and `\u{...}` unicode escapes: standard escapes processed inside triple-quoted strings; interpolation not supported inside them; surrogate/out-of-range code points rejected with a `LexError` — `src/lexer.rs`. (Stable)
- The core expression language: literals (`Int`, `Float`, `String`, `Bool`, `Unit`, `Nil`), `let` / `let rec` bindings with `in`, `fn` lambdas, tuples, records, arrays, `if`/`then`/`else`, `match` (wildcard, variable, literal, tuple, record, variant, and `@` alias patterns), blocks, the pipe operator `|>`, and the operator set of Chapter 2.
- Top-level declarations: `fn` (with `[T]` type parameters, `->` return types, `!` effect rows, `: cap` capability annotations, and `@tool` annotations), `type` (alias, record, and variant forms), `effect`, `actor` / `persistent actor`, `entity`, `organization`, `agent`, `workflow`, `module`, `import`, and `extern` FFI blocks.
- Hindley-Milner type inference (Algorithm W) over tuples, records, variants, arrays, function types carrying effect rows and capabilities, and `&cap T` reference types.
- Algebraic effects: `perform Effect.op(args)`, `handle body { | Effect.op(x) => value }`, closed and open effect rows written `{IO, FS}` and `{IO, | row}`, enforced `!` annotations on `fn` and `behavior` bodies, and runtime handlers with resume semantics.
- Reference capabilities `iso`, `trn`, `ref`, `val`, `box`, `tag`, plus `lineariso` with exactly-once consumption tracking. Capabilities are checked at compile time and erased at runtime. Sendability (`lineariso`, `iso`, `val`, `tag`) is enforced for message arguments.
- Actors and entities: `actor`, `persistent actor`, `organization` (desugars to `entity`), and `entity` (desugars to `persistent actor` with `event_sourced` as the default state model); `spawn Actor { field = value }`, `spawn Actor {} as "name"` for stable identity, `send actor behavior(args)` and `actor ! behavior(args)`, `ask actor behavior(args)`, `receive { | Behavior(x) => expr }`, `self.field` state access, and the four state models (`local`, `durable`, `event_sourced`, `crdt`).
- Persistence for `persistent actor`s: durable snapshot/journal recovery and event-sourced replay, backed by in-memory, JSON-file, and SQLite stores.
- Workflows: `workflow Name { step name { body } compensate { expr } ... }` with `parallel { ... }` step groups, saga compensation in reverse order, `perform Signal.wait("name")`, and `perform Timer.sleep("name", ms)`, all durable across restarts.
- The AI runtime: `agent` declarations with model, system prompt, tools, episodic/semantic/procedural memory, and pricing; the generic `PerformAsync` opcode dispatches LLM, Pipeline, Supervisor, and Debate effects via `effect_op` strings (e.g. `"Inference.ask"`, `"Pipeline.run"`); agent behaviors (`ask`, `usage`, `store_fact`, `recall`); tool schemas generated from `@tool` functions; and the `Pipeline`, `Supervisor`, and `Debate` orchestration builtins. The pure AI types live in the `nulang-ai` workspace crate (`crates/nulang-ai/`); the core crate re-exports them behind the `ai-runtime` feature flag.
- A register-based bytecode VM with a Cranelift JIT tiering path; an OTP-style supervision runtime (restart strategies and policies, links, monitors, exit signals); a distributed runtime (TCP wire protocol, gossip membership, location-transparent addressing — Experimental; the eight CRDT types exist and are tested only at the Rust embedder level, with no `.nula`-level surface — see §9.10); a REPL; and an LSP server.
- Typeclass declarations: `class`/`impl` with dictionary-passing transform for method calls on concrete types (Phase 4, Experimental). See `CHANGELOG.md`. **Verified 2026-08-02, constrained-generic crash fixed 2026-08-13:** literal-receiver dispatch works end-to-end (minimal declarations, two-concrete-type dispatch, missing-impl rejection, superclass syntax all confirmed against the real binary). The canonical constrained-generic case — a typeclass bound on a type-variable receiver (`fn eq_check[T: Eq](a: T, b: T) -> Bool { a.eq(b) }`) — used to type-check and then **crash at runtime** ("Not a function: nil"): the dictionary transform only resolved literal receivers, not type-variable ones. Fixed at the HIR level (`DictKind::Param`); the call site now passes the concrete dictionary argument (`infer_type_arg` → `_impl_Eq_Int`). Pinned by `conformance/behavior/typeclass_06_constrained_generic_runtime_crash.nula` (now passes, exit 0).
- Generics (`fn f[T](...)`, `type T[A] = ...`, §7.8): basic generics — one or more independent type parameters, per-callsite type inference, return-only type parameters — work end-to-end. **Verified 2026-08-02, both gaps fixed 2026-08-13:** (1) recursive generic ADTs can now be constructed — §7.8's `type Tree[T] = Leaf | Node((Tree[T], T, Tree[T]))` type-checks its own constructor call, pinned for two independent recursive shapes (`generics_03` accept, `generics_07` accept); (2) declared type parameters are now skolemized inside the function body — a generic function that pins its type parameter to a concrete type via an internal literal (`fn fresh[T]() -> T { 0 - 1 }`) is rejected AT THE DEFINITION (rigid placeholder cannot unify with `Int`), not at a later mismatched call site (`generics_08` expects the type error, exit 4). See `conformance/behavior/generics_03/07/08_*.nula`.
- Standard-library modules: `stdlib::core`, `stdlib::list`, `stdlib::string`, `stdlib::set`, `stdlib::map`, `stdlib::http` — resolved via `NULANG_STDLIB` or `src/stdlib/` (Experimental). See `CHANGELOG.md`.
- Error handling syntax: `catch expr => body`, `fail expr` (structured short-circuit return), `T ! E` return types, `?` operator (Stable). See `CHANGELOG.md`.
- Transport resilience: `send remote`, `ask remote` with timeout clauses, capability enforcement at call sites (Stable). See `CHANGELOG.md`.
- `after ms => expr` standalone sugar, desugared to `receive {} after ms => expr` (Stable). See `CHANGELOG.md` §RFC 0005/0007.
- `entity` keyword and event sourcing (RFC 0005/0007): `events`/`apply`/`emit` blocks, compile-time event validation (Stable).
- Migration contracts (RFC 0008): `version: N`, `migration from N to M { ... }` blocks inside entities. **Correction (verified 2026-08-02): this is NOT Stable -- it is functionally inert, not just incomplete.** The syntax parses and shallow-typechecks, but no trigger mechanism exists anywhere in the runtime: MIR lowering discards migration bodies entirely (only the bare version number survives into `ActorMeta`), `Runtime::recover_actor` has no version comparison or migration dispatch, and the persistence layer's snapshot/journal formats carry no version field at all. Empirically: a migration body that mutates state never applies, one that performs `IO.print` never runs, RFC-mandated validations (version-gap and downgrade rejection) are unenforced, and the RFC's own example syntax fails to compile (event-migration handler parameters are never bound in scope). See `conformance/behavior/migration_*.nula` for the full evidence trail.
- Organization primitives (RFC 0009): `organization` keyword desugars to `entity` with durable defaults (Stable in its narrowest reading, verified 2026-08-02: the keyword-level desugar is real and the full entity body grammar works inside it, but "durable defaults" means persistent-actor-with-event-sourced-defaults, byte-identical to `entity` -- there is no separate durable-by-default state model. RFC 0009's own additional surface -- governance blocks, member/child-spawn syntax, contract blocks -- is still Draft/Planned per the RFC itself and correctly rejected at parse time, not silently accepted). See `conformance/behavior/org_*.nula`.
- MIR register spilling: `SpillLoad`/`SpillStore` opcodes for functions exceeding 238 usable registers. **Caveat (verified 2026-08-02): the spilling mechanism itself is correct** (checked at 1, 127, and 160 spilled locals, each against an independently computed expected value, not just "doesn't crash") **but is unreachable for the largest functions the claim is meant to cover** -- the compiler frontend itself aborts with a real stack overflow (SIGABRT) on functions with roughly 286+ flat `let` statements (measured by binary search: 285 compiles, 286 aborts), a recursion-depth limit unrelated to the register allocator. See `conformance/behavior/spill_*.nula`.
- Formal semantics: capability lattice proofs in `spec/formal/capabilities.lean` (5 of 6 theorems proved — lattice laws + `cap_sendable`/`discharge_sendable`; `linear_at_most_once` is the one remaining `sorry`, documented as requiring the split-context refinement of `HasTypeCap`). Core HM type soundness in `spec/formal/types.lean` is **proved** (2026-08-14): `progress`/`preservation`/`type_soundness` are all machine-checked. See `spec/formal/README.md`.
- `::` import resolution: module paths via `import stdlib::set`, `import mypkg::utils::math` (Experimental).
- Content-addressed lockfile: `Nulang.lock` carries BLAKE3 `content_hash` per pinned package (Experimental, RFC 0003 Item 11).
- Self-hosting bootstrap compiler: Stage 10 — end-to-end hex → .nbc pipeline (`bootstrap/compiler_core.nula`, `compile_hex.nula`) supporting lexing, Pratt parsing, evaluation, arithmetic, let bindings, closures, `if`/`else`, comparisons, and booleans in Nulang Core. Remaining: HM type inference, MIR lowering, self-compilation.
- Backend trait boundary (RFC 0003 Item 6): `JitBackend`, `WasmBackend`, `CryptoProvider`, `ForeignInterop`, `HttpProvider` traits in `src/backends/mod.rs` (Stable).
- Structured error messages: `NuError` enum with error codes (`ErrorCode`), `expected`/`found` fields per variant, automatic fix suggestions via `error_code()` and `suggestion()`, and `format_rich()` for colorized multi-line diagnostics with source excerpts and carets — `src/types.rs`. (Stable)
- `**` exponentiation operator: tokenized as `Star2`, right-associative, precedence above `*` (Pratt `PREC_EXP` level 14, `src/parser.rs:45`), wired through parser, typechecker, HIR lowering, and bytecode — `src/lexer.rs`, `src/parser.rs`. (Stable)
- `catch` prefix syntax: `catch expr fallback` in addition to postfix `expr catch fallback` — `src/parser.rs`. (Stable)
- Spawn field-initializer overrides: `spawn A { f = v }` now correctly overrides the actor's declared default for field `f`; overrides are encoded in bytecode and applied at VM spawn time — `src/vm.rs`, `src/mir_codegen.rs`, `src/bytecode.rs`. (Stable)
- Clearer "cannot assign to immutable binding" diagnostic: the type error for reassigning a `let` binding now explains that mutable locals are not yet supported and suggests shadowing — `src/typechecker.rs`. (Stable)
- Let-chain stack-overflow fix: long chains of consecutive `let` bindings are flattened iteratively in both the parser (sequential `let`-statement peeling) and HIR lowering (`lower_let_chain`), eliminating deep-recursion overflow on blocks with 40+ lets — `src/parser.rs`, `src/hir_lower.rs`. (Stable)
- Package manager subcommands: `nula init` (scaffold a package), `nula list` (locked dependencies), `nula clean` (remove build artifacts), `nula add <name> [--path|--git|--version]`, `nula remove <name>`, `nula run --watch` / `nula watch` (re-run on source changes), and `nula doc [--open]` (generate Markdown API docs) — `src/package/commands.rs`. (Experimental)
- REPL enhancements: `:help <topic>` (topics: syntax, types, actors, effects, commands), `:load <file>` (load and evaluate a `.nula` file), `:type <expr>` (show inferred type without evaluating), tab completion for identifiers and REPL commands, automatic multi-line input when braces/parens/brackets are unclosed — `src/repl.rs`. (Experimental)
- New stdlib modules: `result` (Result combinators: `unwrap`, `map`, `flat_map`), `option` (Option combinators), `datetime` (DateTime record type), `math` (trigonometry, logarithms, rounding), `fs` (FS-effect wrapper), and `test` (Test-effect assertion helpers: `assert`, `assert_eq`, `assert_true`, `fail_with`) — `src/stdlib/`. (Experimental)
- `FS` filesystem effect: `perform FS.read(path)` → `String`, `perform FS.write(path, content)` → `Unit`, `perform FS.append(path, content)` → `Unit`, `perform FS.exists(path)` → `Bool`; built-in effect wired into the standalone VM — `src/stdlib/fs.nula`, `src/stdlib.rs`, `src/vm.rs`. (Experimental)
- `Test` assertion effect + `nula test [--filter <substr>]` runner: `perform Test.assert(cond, msg)`, `perform Test.assert_eq(a, b)`, `perform Test.assert_true(cond)`; the runner discovers `.nula` test files under `tests/`, reports pass/fail counts, supports name filtering — `src/stdlib/test.nula`, `src/package/commands.rs`. (Experimental)
- LSP enhancements: `.` and `::` completion trigger characters, field-access completion (on `self.` fields, record fields, actor state), `textDocument/didSave` handler that re-checks the file on save, completion items sorted by category (locals > functions > types > variants > keywords > effects) — `src/lsp/mod.rs`. (Experimental)
- `var` bindings: mutable local variables via `var x = 0` declaration and `x = expr` reassignment; tracked separately from `let` in typechecker and codegen — `src/parser.rs`, `src/typechecker.rs`, `src/mir_codegen.rs`. (Experimental)
- Record-update syntax: `{ base .. field = value }` creates a new record with overridden fields; parsed with `PREC_RANGE` precedence, disambiguated from range-in-block by checking for `=` — `src/parser.rs`. (Experimental)
- Tuple `.0`/`.1` field access: numeric indices on tuples (`t.0`, `t.1`); chained access (`t.0.1`) on nested tuples without parenthesization — `src/parser.rs`, `src/hir_lower.rs`. (Stable)
- Range expressions: `a .. b` inclusive-exclusive range at `PREC_RANGE` precedence (below pipe, above logical-or); works in `for` loops (`for i in 0 .. 5 { … }`) and bare in blocks (`{ a .. b }`) — `src/parser.rs`. (Experimental)
- `String.from_char`: `perform String.from_char(code)` creates a single-character string from a Unicode code point; returns `nil` for invalid code points — `src/stdlib.rs`, `src/vm.rs`. (Stable)
- `String.+` fix for let-bound variables: `a + b` where both operands are `let`-bound string variables now correctly concatenates — `src/vm.rs`. (Stable)
- `else`-on-newline fix: an `else` keyword following a newline after `}` is accepted in `if`/`else` chains — `src/parser.rs`. (Stable)
- `let..in` scoping fix: block-level `let x = V in BODY` correctly scopes `x` to `BODY` only — `src/hir_lower.rs`. (Stable)
- `Http` builtin effect: `perform Http.get(url)` and `perform Http.post(url, body)` wired into the standalone VM via `ureq`; returns response body as `String` — `src/stdlib.rs`, `src/vm.rs`. (Experimental)
- `Array` builtin effect: `perform Array.length(arr)`, `perform Array.push(arr, elem)`, `perform Array.new(n, init)`, `perform Array.set(arr, idx, val)`, `perform Array.slice(arr, start, end)` — value semantics, all return new arrays — `src/stdlib.rs`, `src/vm.rs`. (Experimental)
- Numeric conversion primitives: `Int.to_float`, `Float.to_int` (truncates toward zero), `Float.to_string`, `String.to_int` (returns 0 for invalid), `String.to_float` (returns 0.0 for invalid) — `src/stdlib.rs`, `src/vm.rs`. (Experimental)
- JSON parse + stringify: pure-Nulang recursive-descent parser in `stdlib::json` with `parse` and `stringify` functions; uses `String.to_float`, `Float.to_string`, `String.from_char`, and `Array.*` primitives — `src/stdlib/json.nula`. (Experimental)
- All 13 stdlib modules functional: `core`, `list`, `string`, `set`, `map`, `test`, `fs`, `option`, `result`, `datetime`, `math`, `json`, `http` — all parse, import, and resolve with all VM primitives available — `src/stdlib/`. (Experimental)
- LSP code lenses, document links, enriched hover: `textDocument/codeLens` shows reference counts; `textDocument/documentLink` creates clickable import links; `textDocument/hover` includes doc comments, effects, and type signatures — `src/lsp/mod.rs`. (Experimental)
- LSP completion documentation: keyword and built-in effect completion items carry markdown documentation strings — `src/lsp/mod.rs`. (Experimental)
- 15 verified example programs under `examples/` with `examples/README.md` — from basic IO to JSON, HTTP, Option/Result, and ranges. (Experimental)
- `consume` / `recover` expressions: `consume x` marks a linear (`lineariso`) variable as consumed (reusing the existing at-most-once tracker); `recover { body }` is an isolated scope whose result must be sendable (checked in `src/effect_checker.rs`; the typechecker infers the body's type unchanged and lowering is transparent — it does **not** wrap the result in `Ok`/`Error`). See §3.9.2. Commit `e0cf432`. (Experimental)

**Planned (described in this specification, not implemented):**

- The WebAssembly compilation target (Chapter 13): WASM compilation exists behind the `wasm-backend` feature flag via `--backend wasm|wasm-run|wasm-aot`. WIT interface generation and WASI worlds are not yet implemented.
- Higher-kinded types, `Char` and `Decimal` primitives, character literals (Sections 2.4, 3.6).
- `<-` message syntax and indentation-based layout (Section 2.8).
- Authority capabilities (`capability` declarations on actors, delegation, revocation, auditing — Sections 1.5 and 5.3–5.6), `config` blocks, the `tool` declaration form inside actors, `virtual` actors, `select`, `await`, `await_human`, `sleep_until`, and `retry` blocks.
- The deployment manifest (`nulang.toml`), `nulang migrate`, and `nulang shell` (Chapter 15, Appendix D).

Five keywords formerly reserved (`where`, `priv`, `loop`, `node`,
`subworkflow`) have been removed from the lexer per RFC 0010 and now lex
as plain identifiers. `await` has been **re-reserved** (July 2026) as a
keyword for future async/await support; it is a reserved word that cannot
be used as an identifier. `link`, `monitor`, and `exit` are wired into
`spawn link/monitor` and `Actor.exit` syntax. `case` is accepted as an
optional match-arm prefix (but **not** in `catch` arms — catch arms are
bare or `|`-prefixed; see §6.11 and RFC 0015).

Where a section is marked **Planned**, its examples show the intended v2.0 syntax and may not parse under the current compiler.

---
# Format Stability

This chapter is the **stability contract** for Nulang's durable formats. It is
ratified by RFC 0001 (see `RFC/0001-format-stability.md`) and is part of the
Frozen tier (see `CHANGELOG.md`): the contracts below may not be weakened
without a new RFC and a major-version bump.

## FS.1 The Three Frozen Formats

Nulang has three versioned, frozen binary formats. Each carries a magic-bytes
identifier and a version number in every emitted artifact and on every wire
connection. A runtime that encounters a version it does not understand MUST
reject it with a named error; it MUST NOT reinterpret the bytes under a
different layout.

| Format | Magic | Version field | Source of truth |
| `.nbc` bytecode artifact | `NLBC` (4 bytes) | `format_version: u32` | `src/format/constants.rs` |
| NUL0 wire protocol | `NUL0` (4 bytes) | `version: u32` in handshake | `src/format/constants.rs` |
| Value layout (i64-tagged) | — | `VALUE_LAYOUT_VERSION: u32` | `src/value_layout.rs` |

The canonical constants live in `src/format/constants.rs`; no other module
defines format magic bytes or version numbers. Bumping any of these constants
is a breaking change requiring an RFC.

## FS.2 `.nbc` Byte Layout (Version 1)

A `.nbc` file is the durable, distributable encoding of a compiled module. All
integers are big-endian.

```text
offset  size           field
0       4              magic = b"NLBC"
4       4              format_version (u32)          — must be ≤ BYTECODE_MAX_VERSION
8       4              language_version (u32)        — recorded; checked against LANGUAGE_VERSION
12      32             source_hash (BLAKE3; 0x00..00 if unknown)
44      4              instr_count (u32)
48      4*instr_count  instructions (Instruction::encode() → u32)
48+4n   4              meta_len (u32)
52+4n   meta_len       metadata = JSON serialization of the module
```

The header is hand-rolled binary so a runtime can check magic + version in O(1)
without a serde dependency. The instruction stream is 4 bytes per instruction
(opcode `u8` + three `u8` operands, big-endian), so the format is coupled to
the frozen opcode values: an unknown opcode is rejected with
`FormatError::UnknownOpcode`, never reinterpreted. The metadata body is JSON,
universally parseable by any conforming runtime in any host language.

Codec: `CodeModule::to_nbc(source_hash) → Vec<u8>` and
`CodeModule::from_nbc(bytes) → Result<NbcArtifact, FormatError>` in
`src/format/nbc.rs`.

## FS.3 NUL0 Wire Handshake (Version 1)

Immediately after a TCP connection is established, both sides exchange a
16-byte handshake before any framed packets:

```text
[0..4]   magic "NUL0"   (WIRE_MAGIC)
[4..8]   version u32    (WIRE_VERSION, big-endian)
[8..16]  node_id u64    (big-endian)
```

A peer whose magic does not match or whose version does not equal
`WIRE_VERSION` is refused with an `io::Error` of kind `InvalidData`. The
negotiated version governs the packet framing for the lifetime of the
connection; packets themselves carry the existing `[len u32][magic "NUL0"]
[type u8][seq u64][payload]` framing unchanged.

## FS.4 Stability Contract

1. **A version, once assigned, is frozen.** The byte layout for version *N*
   never changes after publication. A "change" to a published version is a bug
   fix to a *deviation* from the published layout, not a layout change.

2. **Opcode values are never reused.** New opcodes are appended at the next
   free discriminant; an opcode removed in a major version leaves its value
   permanently retired. An artifact containing a retired opcode is rejected
   with `FormatError::UnknownOpcode`.

3. **Additive extensions only within a major version.** A new optional section
   may be appended to the metadata body; older runtimes ignore unknown JSON
   fields (serde default-deserializes them). A new required field or a changed
   field meaning requires a new format version and a migration.

4. **Migrations are pure, deterministic, append-only.** A `vN → v(N+1)`
   migration is registered once in `src/format/migrate.rs` and never modified
   thereafter. `migrate_nbc(bytes, target)` walks the chain so a runtime
   speaking v(N+1) loads a vN artifact by running the registered steps.

5. **Language version is distinct from crate version.** The crate
   (`Cargo.toml`) version may rev freely; the *language* version
   (`LANGUAGE_VERSION`, recorded in every `.nbc`) moves only on
   RFC-ratified change. An artifact whose `language_version` exceeds the
   runtime's is rejected with `FormatError::IncompatibleLanguage`.

## FS.5 Reference: `src/format/`

- `constants.rs` — `BYTECODE_MAGIC`, `BYTECODE_VERSION`, `BYTECODE_MAX_VERSION`,
  `WIRE_MAGIC`, `WIRE_VERSION`, `VALUE_LAYOUT_VERSION`, `LANGUAGE_VERSION`,
  `FormatError`.
- `nbc.rs` — `CodeModule::to_nbc` / `from_nbc`, `NbcArtifact`.
- `migrate.rs` — `peek_format_version`, `migrate_nbc` (the only legal home for
  format-version upgrades).

---


# Table of Contents

- Chapter 1: Introduction
- Chapter 2: Lexical Structure
- Chapter 3: Types
- Chapter 4: Effects
- Chapter 5: Capabilities
- Chapter 6: Expressions
- Chapter 7: Declarations
- Chapter 8: Actors
- Chapter 9: Persistent Actors
- Chapter 10: Workflows
- Chapter 11: AI Runtime
- Chapter 12: Distributed Runtime
- Chapter 13: WebAssembly Integration
- Chapter 14: Standard Library
- Chapter 15: Operational Model
- Appendix A: Grammar Reference
- Appendix B: Built-in Types Reference
- Appendix C: Effect Reference
- Appendix D: Migration Guide from v1 to v2

---

# Chapter 1: Introduction

## 1.1 What is Nulang?

Nulang is a durable computation language for long-lived, distributed, stateful software entities. It compiles to WebAssembly and runs on a purpose-built actor runtime that provides persistence, clustering, and effect-composed capabilities as first-class language features. AI agents, workflows, databases, and autonomous organizations are important applications of these primitives, not part of the language kernel.

Nulang occupies a distinctive position in the language design space. Like Erlang and Elixir, it is built on the actor model of concurrency, where independent computational entities communicate exclusively through asynchronous message passing. Like Rust, it employs a sophisticated type system with affine reference capabilities to guarantee memory safety and data-race freedom at compile time. Like Koka and Eff, it uses algebraic effects as the primary mechanism for defining, composing, and handling computational effects such as IO, exceptions, state mutation, storage, messaging, and time. AI model inference is one application of this mechanism, implemented through Cloud SDK libraries rather than as a language primitive. Like modern workflow orchestration systems, it provides durable execution semantics where long-running processes survive crashes and restarts automatically.

The synthesis of these features produces a language with four defining characteristics:

**Concurrency without locks.** Nulang actors do not share mutable memory. All communication is asynchronous and message-based, eliminating data races by construction. The type system enforces that mutable references cannot escape an actor's boundary.

**Effects as a unifying abstraction.** All side effects in Nulang—reading a file, making an HTTP request, querying a database, invoking an AI model, or sending a message—are expressed through a single mechanism: algebraic effects. An effect is declared, performed, and handled within a well-typed framework that makes effectful dependencies explicit in function signatures.

**Capability-based security.** Every reference in Nulang carries a capability that governs how it may be read, written, and shared. These capabilities form a lattice of authority that propagates through the program automatically. Combined with effect declarations, they provide a comprehensive security model: a function's type signature reveals exactly what it can do and what data it can access.

**Durable execution by default.** Any actor can be declared `persistent`, which enables automatic checkpointing, event journaling, and deterministic replay. Persistent actors form the building blocks of workflows—long-running compositions that survive crashes, support compensation, and orchestrate human-in-the-loop interactions.

## 1.1a Nulang Core (Frozen — RFC 0002)

Nulang **Core** is the minimal, frozen subset of the language. It is defined
in RFC 0002 and is the invariant kernel every conforming implementation must
support — including the self-hosting bootstrap compiler (`bootstrap/`). Core
consists of:

- **Expressions:** `fn`, `let`, `if`/`else`, `match`, closures, `return`.
- **Types:** `Int`, `Bool`, `String`, `Unit`, `Nil`; `Vec<T>`, `Map<K,V>`;
  tuples; records; `enum`. HM type inference over this subset.
- **Effects:** `IO.print` and `IO.read` only (terminal I/O).
- **Capabilities:** `val` only (immutable, sendable).

Core **excludes** actors, effects beyond IO, capabilities beyond `val`,
distribution, persistence, FFI, AI, JIT, WASM, and Python interop.
Every Core program is a valid Nulang program; every Core program valid
today is valid in every future language version.

The self-hosting bootstrap compiler (`bootstrap/compiler_core.nula`) is
written in Core and targets the `.nbc` format (RFC 0001). Stage 1 compiles
Core programs; Stage 2 compiles itself. The Rust implementation remains the
fast path; the bootstrap compiler is the longevity path.

**Note:** Core gained `String.charAt` and `String.length` (2026-07-23)
primitives via `perform String.length(s)` and `perform String.charAt(s, i)`, which unblocked the bootstrap compiler's lexer from iterating
source characters. See
`bootstrap/README.md`.


## 1.2 Design Philosophy

Nulang's design is guided by five principles that influence every aspect of the language, from syntax to runtime architecture.

**Composition over configuration.** Nulang prefers composable language primitives over framework-specific configuration. Distributed systems are built by composing actors, not by configuring cluster topologies. AI integration is achieved by performing effects, not by wiring model providers. Persistence is a keyword, not a deployment concern. This principle ensures that the full power of the language—types, effects, pattern matching, higher-order functions—is available at every layer of the system.

**Explicit is better than implicit.** Every effect a function can perform is visible in its type signature through effect rows. Every reference's sharing properties are visible through capability annotations. Every state model is declared explicitly. This explicitness makes programs easier to reason about, test, and audit. It also enables powerful program analyses: the compiler can verify that a function performs no network effects, that an actor's state is serializable, or that a workflow step is deterministic.

**Failure as a first-class concern.** Nulang treats failure as a normal condition to be handled, not an exceptional circumstance to be ignored. Actors are supervised by parent actors that define restart strategies. Messages that cannot be delivered are reported through links and monitors. Workflow steps that fail trigger compensation handlers. This supervision-oriented design, inherited from Erlang's "let it crash" philosophy, enables the construction of resilient systems that recover automatically from hardware failures, network partitions, and software bugs.

**Uniformity across scales.** The same actor abstraction works for a single-process application and a globally distributed cluster. A `persistent` actor on one node uses the same syntax and semantics as a `persistent` actor replicated across a hundred nodes. An effect performed in a unit test can be handled by a pure mock, while the same effect in production is handled by a system call. This uniformity reduces the conceptual surface area programmers must learn and enables code reuse across deployment scenarios.

**Composable capabilities.** External capabilities—whether an LLM, a vector index, a billing meter, or a remote API—are accessed through the algebraic effect system and governed by the capability security model. A function that invokes an external capability declares the corresponding effect in its type signature. An actor that uses AI does so by importing a Cloud SDK library (for example, `nlc.ai`) and holding any required capability. This composability keeps the language kernel small while allowing powerful, statically traceable integrations to evolve as libraries.

## 1.3 The Actor as Universal Abstraction

The actor is the fundamental unit of computation, concurrency, state, and distribution in Nulang. Every running program consists of a tree of actors, each with its own mailbox, behaviors, and optionally persistent state.

An actor is declared with the `actor` keyword, followed by a name, optional type parameters, and a body containing state declarations and behaviors. Inside behaviors, state is read and written through `self`. The parser accepts exactly seven member kinds in an actor body — `state`, `behavior`, `initial`, `version`, `events`, `apply`, and `migration` (`src/parser.rs` `parse_actor`); **`fn` declarations are not valid actor members**:

```nulang
actor Counter {
  state local count: Int = 0

  behavior increment(by: Int) {
    self.count = self.count + by
  }

  behavior get() {
    self.count
  }

  behavior reset() {
    self.count = 0
  }
}
```

Actors communicate exclusively through asynchronous messages. Sending a message is a non-blocking operation that places the message in the recipient's mailbox. The recipient processes messages sequentially, one at a time, guaranteeing that an actor's behavior handlers execute atomically with respect to each other. This single-threaded illusion within each actor eliminates the need for locks or other synchronization primitives.

Actors can be made persistent by adding the `persistent` keyword:

```nulang
type Result[T, E] = Ok(T) | Error(E)

persistent actor BankAccount {
  state durable balance: Int = 0

  behavior deposit(amount: Int) {
    self.balance = self.balance + amount
  }

  behavior withdraw(amount: Int) {
    if amount > self.balance then
      Error("Insufficient funds")
    else {
      self.balance = self.balance - amount
      Ok(unit)
    }
  }

  behavior get_balance() {
    self.balance
  }
}
```

(`Int` is used here because exact-decimal arithmetic — the `Decimal` type — is planned rather than implemented; see §3.2.4.)

The `persistent` keyword enables automatic checkpointing after each behavior invocation, ensuring that the actor's state survives process restarts. The `durable` state model (one of four available) guarantees that `balance` is written to persistent storage before the behavior returns.

Actors perform effects through the effect system, with their effect rows making authority explicit in the type signature. Inference (formerly "LLM") is itself an effect — `perform Inference.ask(...)` — wired to the agent runtime (Chapter 11). The legacy name `LLM` remains accepted as a deprecated alias:

```nulang
actor ChatBot {
  state local turns: Int = 0

  behavior ask(question: String) ! {Inference} {
    let answer = perform Inference.ask(question) in {
      self.turns = self.turns + 1
      answer
    }
  }
}
```

The `! {Inference}` row declares that this behavior may perform inference effects; the deprecated `{LLM}` row is also accepted. Performing an effect outside the declared row is a compile-time error. Authority capabilities (`capability llm`) that grant and revoke such authority per actor are planned — see §5.3.

## 1.4 State Models Overview

Every state variable in an actor has an associated *state model* that determines how it is stored, replicated, and recovered. Nulang provides four state models:

| Model | Persistence | Replication | Recovery | Use Case |
|-------|------------|-------------|----------|----------|
| `local` | None | None | Reset to initial value | Ephemeral caches, temporary buffers |
| `durable` | Snapshot + journal | None | Replay from journal | Single-node persistent state |
| `event_sourced` | Event journal | Event stream | Full event replay | Audit trails, temporal queries |
| `crdt` | Delta log | Concrete-type selector + merge-on-sync landed; `Crdt.*` effect module and operation-set enforcement open (see §9.10) | CRDT merge (on sync) | Shared distributed state |

The state model is declared alongside the variable:

```nulang
persistent actor ShoppingCart {
  state durable items_count: Int = 0
  state crdt    viewers: Int = 0
  state local   temp_discount: Int = 0

  behavior add_item(item_id: Int) {
    self.items_count = self.items_count + 1
  }

  behavior apply_discount(code: Int) {
    // Temporary, not persisted
    self.temp_discount = code
  }

  behavior track_viewer(node: Int) {
    // NOTE: `crdt` fields with a concrete-type selector route through
    // `CrdtManager` (merge-on-sync); see §9.10.
    self.viewers = self.viewers + 1
  }
}
```

The runtime enforces the semantics of `durable` (checkpointed and
journaled) and `event_sourced` (rebuilt by replaying emitted events, see
`emit`, §6.14). `crdt` fields with a concrete-type selector route through
`CrdtManager` (merge-on-sync); a `Crdt.*` effect module and operation-set
enforcement are not yet implemented — see §9.10/§12.5.

## 1.5 Capability Security Overview

Nulang employs two complementary capability systems: reference capabilities (which control how data can be aliased and shared) and authority capabilities (which control what effects an actor can perform).

Reference capabilities are part of the type system. Every reference has one of seven capabilities:

- `lineariso` — unique and linear, sendable, must be consumed exactly once
- `iso` — unique, sendable (no other references exist)
- `trn` — unique but locally writable (transitioning to `val`)
- `ref` — uniquely writable but not sendable
- `val` — immutable and sendable
- `box` — read-only view of `ref` or `val`
- `tag` — opaque identifier, not readable

These capabilities form a lattice under a subtyping relation (§3.9.1). The compiler uses them to guarantee that no data race can occur: a `lineariso`, `iso`, `val`, or `tag` reference can be sent to another actor because the sender cannot retain the ability to mutate the data. Reference capabilities are checked at compile time and erased before execution.

Authority capabilities — declared on actors to govern which effects they can perform — are planned (§5.3). The intended surface is:

```nulang
// fragment
// Planned — not yet implemented
capability llm      // Can perform LLM effects
capability http     // Can make HTTP requests
capability file     // Can access the file system
capability network  // Can open network connections
capability random   // Can access random number generation
capability time     // Can access the system clock
```

Authority capabilities are designed to be delegated from one actor to another and revoked at any time. This enables fine-grained security policies: an AI agent can be given `llm` and `http` capabilities, but not `file` or `network`.

## 1.6 Relationship to Other Languages

Nulang's design synthesizes ideas from several language families:

**From the actor languages (Erlang, Elixir, Pony, Akka):** The actor model as the fundamental concurrency primitive, supervision trees for fault tolerance, and the philosophy of isolated mutable state. Nulang differs in its static type system, algebraic effects, and unified treatment of persistence and distribution.

**From the effect languages (Koka, Eff, Flix):** Algebraic effects and handlers as the primary abstraction for computational effects, and effect rows in function types. Nulang extends this to include LLM calls as just another effect, and integrates effects with the actor model so that effect handlers can be actor-local.

**From the capability languages (Pony, Rust, Wyvern):** Reference capabilities for memory safety and data-race freedom. Nulang's capability system is most closely related to Pony's, but extends it with authority capabilities and distributed capability delegation.

**From the workflow languages (Temporal, Durable Functions):** Durable execution, deterministic replay, and saga compensation. Nulang embeds these concepts into the actor model rather than providing them as a separate framework.

**From the ML family (OCaml, Haskell, F#, Elm):** Hindley-Milner type inference, algebraic data types, pattern matching, and higher-order functions. Nulang's type system is closest to Elm's in its simplicity and inferability, extended with reference capabilities and effect rows.

---

# Chapter 2: Lexical Structure

## 2.1 Source Files and Encoding

Nulang source files conventionally use the `.nula` extension; the CLI accepts any path. A source file is a sequence of Unicode code points encoded in UTF-8. A leading UTF-8 Byte Order Mark is **not** currently stripped; source files should be saved without one.

A source file consists of a sequence of declarations: functions, type definitions, actor and agent definitions, effect definitions, workflow definitions, imports, and module-level expressions. There is no statement terminator; declarations and expressions are separated by newlines or semicolons. Blocks are delimited by braces, and newlines are tokens the parser uses to find expression boundaries — indentation itself is not significant (see Section 2.8).

A minimal Nulang program is a single module file that need not contain a `main` function. Each module-level expression is wrapped in a synthetic `__main` function and evaluated in order when the program starts, and any spawned actors continue running:

```nulang
// hello.nula: a minimal Nulang program
perform IO.print("Hello, World!")
```

If the module declares `fn main()`, that function is the entry point instead. Programs compile to bytecode for the Nulang register VM, which initializes the runtime, evaluates the entry function, and starts the actor scheduler. (Compilation to WebAssembly with a `__nulang_start` export is **Planned**; see Chapter 13.)

## 2.2 Comments

Nulang supports two comment styles:

**Line comments** begin with `//` and extend to the end of the line:

```nulang
// This is a line comment
let x = 42 in x  // Comments can also follow code on the same line
```

**Block comments** are delimited by `/*` and `*/`. Block comments may be nested, which allows commenting out code that itself contains block comments:

```nulang
/* This is a block comment.
   It can span multiple lines.
   /* And they can be nested. */
*/
```

Comments are treated as whitespace by the parser and have no semantic significance. They may appear between any two tokens. (An unterminated block comment currently consumes input to end-of-file rather than producing a lex error.)

**Documentation comments** are line comments beginning with `///`. They are preserved by the lexer as doc-comment tokens (ordinary comments are discarded) so tooling can associate them with the declaration that follows:

```nulang
/// Calculate the factorial of a non-negative integer.
/// Returns 1 for n = 0, and n * factorial(n - 1) otherwise.
fn factorial(n: Int) -> Int {
  if n == 0 then 1 else n * factorial(n - 1)
}
```

## 2.3 Keywords

The following identifiers are reserved as keywords in Nulang and may not be used as ordinary identifiers:

```
actor         durable       import        par           tag
agent         effect        in            parallel      then
alias         else          initial       perform       throws
and           emit          iso           persistent    tool
as            entity        let           pub           trn
ask           errdefer      linear        rec           true
await         event_sourced lineariso     receive       type
behavior      exit          link          recover       unit
box           extern        local         ref           until
break         fail          match         remote        using
case          false         migrate       resume        val
catch         fn            module        return        var
class         for           monitor       self          while
compensate    given         nil           send          with
consume       handle        not           spawn         workflow
crdt          handler       opaque        state
database      if            or            state_machine
defer         impl          organization  step
```

Keywords are case-sensitive and must be written in lowercase.

Notes on the inventory:

- `true`, `false`, `nil`, and `unit` are literal keywords, and `and`, `or`, `not` are keyword spellings of the `&&`, `||`, `!` operators.
- `entity` is a reserved keyword accepted by the grammar; it desugars to `persistent actor` with `event_sourced` as the default state model (see Chapter 8).
- `exit`, `link`, and `monitor` are used as operation names in `perform Actor.exit(...)` / `Actor.link(...)` / `Actor.monitor(...)`, and in `spawn link|monitor Actor { ... }` desugaring. `await` is reserved but unwired (re-reserved July 2026 for future async/await; see GOVERNANCE §2a). `where`, `priv`, `loop`, `node`, `subworkflow` were freed as identifiers per RFC 0010 §C.6.
- The capability words `iso`, `trn`, `ref`, `val`, `box`, `tag`, `lineariso`, `linear` are keywords usable anywhere a capability is parsed.
- `organization` is a reserved keyword accepted by the grammar; it desugars to `entity` with the same durable-first defaults (RFC 0009).
- `cap` (in the `expr :cap iso` annotation) and `to` (in `migrate a to node`) are contextual identifiers, not keywords.
- `var` (mutable binding), `consume`, `recover`, and `as` are keywords in the current lexer (`src/lexer.rs`). There is no `capability`, `enum`, `event`, `from`, or `config` keyword. Constructs earlier drafts associated with those words are either expressed differently (Chapters 5 and 7) or **Planned**.

## 2.4 Identifiers

An identifier begins with an ASCII letter (`a`–`z`, `A`–`Z`) or an underscore (`_`), followed by any number of ASCII letters, digits, or underscores. The current lexer is ASCII-only: non-ASCII letters (for example `α`) are rejected with a lex error. (Unicode identifiers are **Planned**.)

Identifiers beginning with an uppercase letter are lexed as *upper identifiers* and are used for type, variant-constructor, actor, and effect names. Both forms are otherwise ordinary identifiers.

Nulang uses the following naming conventions. They are conventions only — no style checker enforces them:

- **Types, variants, actors, and modules**: PascalCase (`String`, `Option`, `BankAccount`)
- **Functions and variables**: snake_case (`map`, `get_balance`, `process_request`)
- **Type variables in generics**: PascalCase, typically a single letter (`T`, `U`, `Elem`, `Key`)
- **Effect names**: PascalCase, short for the built-ins (`IO`, `Http`, `FS`, `Random`, `Time`, `Inference`; `Net` and `Rand` exist as compile-time aliases, §4.6/Appendix C)
- **Constants**: UPPER_SNAKE_CASE (`MAX_RETRIES`, `PI`)

Examples of valid identifiers:

```nulang
// fragment
name         _private     http2        x_y_z
Counter      Option       T            Elem
```

## 2.5 Literals

Nulang provides literals for the following types:

### 2.5.1 Integer Literals

Integer literals are sequences of decimal digits, or hexadecimal digits after a `0x` (or `0X`) prefix:

```nulang
42        // Decimal
0x2A      // Hexadecimal (= 42)
```

An integer literal has type `Int` (a 64-bit signed integer; see Section 3.2.2). A negative literal is written with unary negation (`-42`), which folds at compile time like any other unary expression. Octal (`0o52`), binary (`0b101010`), and underscore digit separators (`1_000_000`) are implemented (lexer extension; verified by `conformance/grammar/positive_syntax_round2.nula`). **Known lexer leniency (fix scheduled):** out-of-set digits (`0b2`, `0o8`), a bare `0b`, and a trailing underscore (`1_`) currently lex without error instead of being rejected.

### 2.5.2 Floating-Point Literals

Floating-point literals consist of an integer part, a decimal point, a fractional part, and optionally an exponent introduced by `e` or `E` with an optional sign:

```nulang
3.14159
2.99792458e8    // Scientific notation
1.0e-9          // Small numbers
```

A floating-point literal has type `Float` (IEEE 754 double precision). There are no `f32`/`f64` suffixes. A bare `1.` is not a float literal: a `.` is only consumed as a decimal point when followed by a digit, so `1..10` lexes as `1`, `..`, `10` (the range operator, §2.7).

### 2.5.3 String Literals

String literals are delimited by double quotes (`"`). They may contain any character except an unescaped double quote or backslash. The following escape sequences are recognized:

```nulang
"Hello, World!"
"Line 1\nLine 2"     // Newline
"Tab\tseparated"     // Tab
"Quote: \"hello\""   // Escaped quotes
"Backslash: \\"      // Escaped backslash
```

`\r` (carriage return) and `\0` (NUL) are also recognized. `\u{...}` Unicode escapes, triple-quoted multi-line strings, and `{expr}` string interpolation are **Planned**; none are accepted by the current lexer. (Template strings such as `"Research: {input}"` used with `Pipeline.stage` are plain string literals interpreted at runtime by the pipeline builtin — they are not language-level interpolation.)

### 2.5.4 Character Literals

**Planned.** There is no `Char` type and the lexer does not recognize single-quoted character literals.

### 2.5.5 Boolean Literals

The boolean literals are `true` and `false`, with type `Bool`.

### 2.5.6 Unit Literal

The unit literal is written `()` or with the keyword `unit`, both with type `Unit`. It represents the absence of a meaningful value and is the return type of operations that produce no data.

### 2.5.7 Nil Literal

The `nil` literal, with type `Nil`, represents the absence of a value (for example, a `receive` on an empty mailbox evaluates to `nil`).

## 2.6 Operators

The expression grammar is a Pratt parser with fourteen precedence levels (`src/parser.rs:30-46`). From loosest to tightest binding:

| Level | Operators | Associativity |
|-------|-----------|---------------|
| 1 | `=` | right |
| 2 | `\|>` | left |
| 3 | `..` | left |
| 4 | `\|\|` `or` | left |
| 5 | `&&` `and` | left |
| 6 | `==` `!=` | left |
| 7 | `<` `<=` `>` `>=` | left |
| 8 | `+` `-` | left |
| 9 | `*` `/` `%` | left |
| 10 | `<<` `>>` | left |
| 11 | `&` | left |
| 12 | `^` | left |
| 13 | `\|\|\|` | left |
| 14 | `**` | right |
| prefix | `!` `not` `-` `&` `*` | — |

The lexer recognizes `+=` and `-=` tokens, but the parser **rejects** them at parse time ("Not a binary operator: PlusAssign/MinusAssign") — there is no compound assignment. The level numbers above match the Pratt table in `src/parser.rs:30-46`.

Prefix operators bind at level 11. Postfix forms — function application `f(x)`, field access `x.f` / `x.0`, indexing `a[i]`, message send `a ! b(args)`, and annotations `e : T` / `e :cap c` — bind tighter than all binary operators.

Two quirks of the current grammar are worth noting:

- **Bitwise operators bind tighter than arithmetic.** `1 + 2 & 3` parses as `1 + (2 & 3)`. Use parentheses when mixing arithmetic and bitwise operators. (This ordering is inherited from the precedence table and may be revised before 2.0.)
- **Single `|` is not an infix operator.** It is reserved as the match-arm and variant separator, so bitwise OR is written `|||` (or the keyword `or` for booleans).

There is a `**` exponentiation operator (right-associative, binds tighter than `*`; `src/lexer.rs` `Star2`, `src/parser.rs` `PREC_EXP`). There is no `~` bitwise-not operator — the `~` token is lexed (`src/lexer.rs:1127`) but not accepted by the parser.

### 2.6.1 Arithmetic Operators

| Operator | Description | Example |
|----------|-------------|---------|
| `+` | Addition | `a + b` |
| `-` | Subtraction | `a - b` |
| `*` | Multiplication | `a * b` |
| `/` | Division | `a / b` |
| `%` | Remainder | `a % b` |
| `**` | Exponentiation (right-associative, binds tighter than `*`) | `a ** b` |
| `-` | Unary negation | `-x` |

Arithmetic operators are type-polymorphic through inference: both operands and the result share one type variable, so `+` works on `Int` or `Float` but the two operands must have the same type. Mixed-type arithmetic requires explicit conversion (conversion functions are **Planned** with the standard library). Division by zero evaluates to `nil`.

### 2.6.2 Comparison Operators

| Operator | Description |
|----------|-------------|
| `==` | Structural equality |
| `!=` | Structural inequality |
| `<` | Less than |
| `<=` | Less than or equal |
| `>` | Greater than |
| `>=` | Greater than or equal |

Comparison operators are left-associative like all binary operators, so `a < b < c` parses as `(a < b) < c`, which fails to type-check (`Bool` is not comparable to `c`). Write `(a < b) && (b < c)` instead.

### 2.6.3 Boolean Operators

| Operator | Description |
|----------|-------------|
| `&&` / `and` | Logical AND (short-circuiting) |
| `\|\|` / `or` | Logical OR (short-circuiting) |
| `!` / `not` | Logical NOT (unary prefix) |

The `&&` and `||` operators use short-circuit evaluation: the right operand is only evaluated if necessary.

### 2.6.4 Reference Capability Operators

| Operator | Description | Status |
|----------|-------------|--------|
| `&` | Create a reference (`&x`, capability `ref`) | Implemented |
| `*` | Dereference a reference (`*r`) | Implemented |
| `consume` | Consume a linear (`lineariso`) variable, marking it consumed in the at-most-once tracker | Implemented (Experimental) |
| `recover` | `recover { body }` — isolated scope whose result must be sendable (see §3.9.2) | Implemented (Experimental) |

Reference types and capabilities are discussed in detail in Chapter 5.

### 2.6.5 The Pipe Operator

The pipe operator `|>` passes the left operand as the **first** argument to the function on the right:

```nulang
// fragment
list |> map(f) |> filter(g)
// Equivalent to: filter(map(list, f), g)
```

The pipe operator has very low precedence (level 2, above only assignment) and is left-associative. It is described fully in Section 6.9.

## 2.7 Delimiters

Nulang uses the following delimiters:

| Delimiter | Usage |
|-----------|-------|
| `()` | Parentheses for grouping, tuples, and function arguments |
| `{}` | Braces for blocks, record literals, actor bodies, and effect rows |
| `[]` | Square brackets for array literals, indexing, and type parameters |
| `,` | Comma for separating elements |
| `:` | Colon for type annotations (`x : Int`) and capability annotations (`x :cap iso`) |
| `;` | Semicolon for separating expressions on the same line |
| `->` | Arrow for function types and `fn` bodies |
| `=>` | Fat arrow for match arms and handler clauses |
| `=` | Equals for bindings and assignment |
| `.` | Dot for field access (`r.name`) and tuple indexing (`t.0`) |
| `!` | Bang for message send (`a ! b(args)`) and effect annotations (`! {IO}`) |
| `\|` | Vertical bar introducing match arms, handler clauses, and variant constructors |
| `@` | At sign for annotations (`@tool(...)`) and pattern aliases (`n @ Some(x)`) |

The lexer also recognizes `..`, `::`, `<-`, and `?`, and the parser accepts all four today: `..` is the inclusive-exclusive range operator (`a..b`, Pratt `PREC_RANGE`, `src/parser.rs:83`) and the record-update separator (`{ base .. field = value }`, `src/parser.rs` `Expr::RecordUpdate`); `::` is the module-path separator in `import` declarations (`src/parser.rs` `parse_import`); `<-` is async tell, equivalent to `!` (`actor <- behavior(args)`, `src/parser.rs:2483`); and `?` is error propagation / try (`expr?` desugars to a `match` on `Ok`/`Error` with early `return`, `src/parser.rs:2603`) plus nil-safe optional chaining (`expr?.field`, `src/parser.rs:2567`).

## 2.8 Newlines, Semicolons, and Blocks

Block structure in the current grammar is explicit, not indentation-based:

- A **block** is a brace-delimited sequence of expressions, `{ e1; e2; …; en }`, whose value is the value of the last expression.
- **Newlines** are tokens. The parser skips newlines wherever an expression or declaration may continue, so expressions may span lines freely, and a newline (or a run of them) terminates an expression or declaration where one is complete.
- **Semicolons** separate expressions on the same line, exactly like newlines.

```nulang
let max = fn(a: Int, b: Int) {
  if a > b then a else b
} in
let m = max(3, 7) in
{ m; m + 1 }   // semicolons separate expressions; block value is m + 1
```

Indentation has no semantic significance; tabs and spaces are ordinary whitespace. (Indentation-sensitive layout, in the style of Haskell's offside rule, is **Planned** for a future revision.)

```nulang
// Example: an actor definition. Braces delimit the body; state fields
// and behaviors are separated by newlines.
actor WeatherService {
  state cache = 0

  behavior get_forecast(city: String) {
    match self.cache with {
      | 0 => self.cache
      | n => n
    }
  }
}
```

---

# Chapter 3: Types

## 3.1 Type System Overview

Nulang employs a static type system based on Hindley-Milner type inference with extensions for reference capabilities, effect rows, and generic programming. The type system has the following properties:

**Soundness.** Well-typed programs do not go wrong at runtime due to type errors. The type system prevents null pointer dereferences (through user-declared `Option`-style variants and the explicit `nil` value) and data races (through reference capabilities and the sendability check on messages).

**Complete inference.** The types of all expressions can be inferred automatically by the compiler (Hindley-Milner Algorithm W). Parameter type annotations are optional: unannotated parameters receive fresh type variables that unify with their uses. Annotations may be provided for documentation or to constrain inference, and they are required in `extern` FFI declarations.

**Parametric polymorphism.** Functions and types may be parameterized by type variables (`fn map[A, B](...)`, `type Pair[A, B] = ...`), enabling generic programming without runtime type checks. Let-bound values are generalized (let-polymorphism).

**Effect tracking.** Function types include an effect row that describes which computational effects the function may perform. This makes effectful dependencies explicit and enables local reasoning about code. Effect rows are inferred; a `! {Row}` annotation on a `fn` or `behavior` is enforced against the body's inferred row.

**Capability safety.** Reference types are qualified with capabilities that control how data can be read, written, and shared across actor boundaries. The capability system guarantees memory safety and data-race freedom and is checked entirely at compile time; capability annotations are erased before execution.

(Kinds and higher-kinded types are **Planned**; the current type system has a single kind `*` for ordinary types, and type constructors like `List` are not yet kind-checked.)

## 3.2 Primitive Types

Nulang provides the following primitive types:

### 3.2.1 Bool

The type `Bool` has two values: `true` and `false`. It supports the logical operators `&&`, `||`, and `!` (also spelled `and`, `or`, `not`).

```nulang
fn is_valid(x: Int) -> Bool {
  x > 0 && x < 100
}
```

### 3.2.2 Int

The type `Int` is a 64-bit signed integer (`i64`; range −9,223,372,036,854,775,808 to 9,223,372,036,854,775,807). It supports all arithmetic operators and the bitwise operations `&`, `^`, `|||`, `<<`, `>>`. (Single `|` is reserved for match arms — see Section 2.6.)

```nulang
fn double(x: Int) -> Int { x * 2 }
fn is_even(x: Int) -> Bool { x % 2 == 0 }
```

### 3.2.3 Float

The type `Float` represents IEEE 754 double-precision floating-point numbers (`f64`). (A single-precision `Float32` is **Planned**.)

```nulang
fn area(radius: Float) -> Float {
  3.14159 * radius * radius
}
```

### 3.2.4 Decimal

**Planned.** An arbitrary-precision `Decimal` type for financial calculations is not implemented. Use `Int` (scaled, e.g. cents) or `Float` today.

### 3.2.5 Char

**Planned.** There is no `Char` type; the lexer rejects single-quoted character literals.

### 3.2.6 Unit

The type `Unit` has a single value `()` (also written `unit`) and is used for functions and effects that return no meaningful value. It is analogous to `void` in C or `()` in Haskell.

### 3.2.7 Nil, Never, and Address

Three further primitive types complete the current set:

- `Nil` — the type of the `nil` literal (Section 2.5.7).
- `Never` — the empty type, used for computations that cannot produce a value.
- `Address` — the type of actor references (Section 8.10).

## 3.3 Product Types

Product types combine multiple values into a single value. Nulang provides two product type constructors: tuples and records.

### 3.3.1 Tuples

A tuple is an ordered collection of values of possibly different types. Tuple types are written with parentheses and commas; tuple values use the same syntax:

```nulang
let point: (Float, Float) = (3.0, 4.0) in point
let person: (String, Int, Bool) = ("Alice", 30, true) in person
```

Tuples are destructured with tuple patterns:

```nulang
let distance = fn(p: (Float, Float)) {
  match p with {
    | (x, y) => x * x + y * y
  }
} in distance((3.0, 4.0))
```

The empty tuple `()` is the same as the unit value. Single-element tuples `(a,)` are distinguished from parenthesized expressions by the trailing comma.

Tuple components are accessed by position using zero-based indexing: `point.0`, `point.1`.

### 3.3.2 Records

A record is a labeled product type, where each field has a name and a type. Record types are structural: two record types unify when they have the same field names with unifiable types, regardless of declaration order. Record literals and record types use a colon between the field name and its value or type:

```nulang
let person = { name: "Alice", age: 30, active: true } in
let greet = fn(p: { name: String, age: Int, active: Bool }) {
  p.name
} in
greet(person)
```

Record fields are accessed using dot notation: `person.name`, `person.age`. Record patterns destructure records with the same colon syntax: `{ name: n, age: a }`.

Structural update is implemented: `{ person .. age = 31 }` evaluates `person`, then produces a new record with the listed fields overridden (overrides use `=`; multiple overrides are comma-separated). Range expressions (`a..b`) are likewise implemented (`src/parser.rs` `Expr::RecordUpdate`, `BinOp::Range`). Records remain immutable values — an "update" expression constructs a new record; it does not mutate the base.

Record types can be named with a record type declaration or abbreviated with a type alias (Section 7.3):

```nulang
type Person = { name: String, age: Int, active: Bool }
type Point = { x: Float, y: Float }

fn translate(p: Point, dx: Float, dy: Float) -> Point {
  { x: p.x + dx, y: p.y + dy }
}
```

## 3.4 Sum Types

Sum types represent values that can be one of several alternatives. Nulang provides two sum type constructors: variants (tagged unions) and enums.

### 3.4.1 Variant Types

A variant type is defined with the `type` keyword and consists of a set of constructors, each optionally carrying a single payload type:

```nulang
type Option[T] =
  | Some(T)
  | None

type Result[T, E] =
  | Ok(T)
  | Error(E)

type Tree[T] =
  | Leaf
  | Node((Tree[T], T, Tree[T]))
```

The leading `|` on the first constructor is optional, and constructors may be written on one line (`type Color = Red | Green | Blue`). A constructor payload is a single parenthesized type; a constructor carrying several values takes a tuple payload, as `Node` shows. Record-style constructors with named fields (`Node { left: ..., ... }`) are **Planned**.

Nulang has no prelude: `Option` and `Result` are not built in. Programs declare the variants they need (as above) and then use the constructors as ordinary uppercase values. Variant constructors create values, and pattern matching destructures them:

```nulang
type Result[T, E] = Ok(T) | Error(E)

fn safe_divide(a: Float, b: Float) -> Result[Float, String] {
  if b == 0.0 then
    Error("Division by zero")
  else
    Ok(a / b)
}

fn describe(r: Result[Float, String]) -> String {
  match r with {
    | Ok(value) => "ok"
    | Error(msg) => msg
  }
}
```

> **Implementation status.** Declared variants work end-to-end: constructors create values, constructor names are first-class values (a payload constructor used as a value, such as `let f = Some in f(1)`, behaves as a one-argument function), and `match` destructures them with payload binding. At runtime a payload-less constructor is the bare tag string and a payload-carrying constructor is a record `{ ctor: <name>, payload: <value> }`; matching string-compares the tag. Nested constructor patterns match structurally — `Some(Some(x))` tests both tags and rejects an inner `None`. One limitation remains: tuple patterns do not check arity — a tuple-pattern arm tests only the positions it names, so `(a, b)` also matches a longer tuple (extra elements are ignored) and a position beyond the scrutinee's length binds nil (Section 6.7).

### 3.4.2 Enums

An enum is a variant type where no constructor carries data. Enums use the same concise syntax:

```nulang
type Color = Red | Green | Blue

type Status = Pending | Running | Completed | Failed
```

Enum constructors are pattern-matched like other variants:

```nulang
type Status = Pending | Running | Completed | Failed

fn status_message(s: Status) -> String {
  match s {
    | Pending   => "Waiting to start..."
    | Running   => "In progress..."
    | Completed => "Done!"
    | Failed    => "Something went wrong."
  }
}
```

## 3.5 Function Types

Function types describe the type of a function, including its parameter type, return type, effect row, and capability. The general form is:

```nulang
// fragment
A -> R ! {EffectRow} : cap
```

where `A` is the parameter type (a multi-argument function takes a tuple parameter `(A1, A2, ..., An)`), `R` is the return type, `! {EffectRow}` is the optional effect row, and `: cap` is the optional capability. Both annotations are omitted when the function is pure with the default `ref` capability:

```nulang
// Pure function: no effects
fn add(a: Int, b: Int) -> Int { a + b }

// Effectful function: the ! {IO} row is enforced — the body may perform
// IO effects and nothing else.
