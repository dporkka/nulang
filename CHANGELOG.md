# Nulang Changelog

> This changelog is organized by **stability tier** (see `GOVERNANCE.md` §2),
> not by release. The tier determines what may change and how. The crate
> version in `Cargo.toml` is the implementation version; the
> **language version** (`[package.metadata] language-version`, and
> `LANGUAGE_VERSION` in `src/format/constants.rs`) is what this changelog
> tracks — it moves only on RFC-ratified change.

**Language version:** `1.0.0-frozen` (since 2026-07-19; RFCs 0001, 0002).

---

## Frozen tier

*Will never break. A change here is a new language and requires a new major
version + migration.*

### Language version 1.0.0-frozen — 2026-07-19

- **RFC 0001 — Format Stability.** Established versioned, frozen binary
  formats for durable artifacts and the wire protocol.
  - `.nbc` bytecode artifact format version 1 (magic `NLBC`, header with
    `format_version`, `language_version`, BLAKE3 `source_hash`). Codec:
    `CodeModule::to_nbc` / `from_nbc` in `src/format/nbc.rs`.
  - NUL0 wire protocol handshake version 1 (16-byte
    `{magic "NUL0", version u32, node_id u64}`). Unknown versions are refused,
    never reinterpreted. `src/runtime/network.rs`.
  - Value layout version 1 (`src/value_layout.rs`, i64-tagged).
  - Migration registry `src/format/migrate.rs` as the sole legal home for
    format upgrades. v1→v1 identity.
  - `FormatError` enum: `Truncated`, `BadMagic`, `UnsupportedVersion`,
    `IncompatibleLanguage`, `LengthMismatch`, `UnknownOpcode`, `BodyDecode`,
    `BadConstant`.
- **RFC 0002 — Frozen Core.** Defined Nulang Core, the minimal frozen subset:
  `fn`/`let`/`if`/`match`/closures, `Int`/`Bool`/`String`/`Unit`/`Nil`/
  `Vec`/`Map`/tuples/records/`enum`, HM inference over this subset, `IO.print`
  and `IO.read` only, `val` capability only. Every Core program valid today is
  valid in every future version.
- Stability contract published as `SPEC2.md` §"Format Stability" and
  `GOVERNANCE.md`.

## Stable tier

*Breaking changes require an accepted RFC and a deprecation cycle of at least
two major versions.*

### Unchanged at 1.0.0-frozen

The following are classified Stable as of 1.0.0-frozen. They have not changed
in this version; they are recorded here to establish their tier.

- The full HM type system and inference rules (`src/typechecker.rs`).
- The effect-row system: closed/open rows, regions (`src/effect_checker.rs`).
- The capability lattice (`iso`/`trn`/`ref`/`val`/`box`/`tag`/`lineariso`)
  and subtyping (`src/effect_checker.rs`).
- The actor surface: `spawn`, `send`, `receive`, supervision
  (`src/runtime/`, `src/vm.rs`).
- CRDT operations and merge semantics (`src/runtime/crdt.rs`,
  `src/runtime/crdt_reg.rs`).

### Added since 1.0.0-frozen — 2026-07-29

- **RFC 0005/0007 — `entity` keyword and event sourcing.** `entity` desugars
  to `persistent actor` with `event_sourced` default state model. `events`
  and `apply` blocks for typed event declarations and automatic state
  mutation. `emit EventName(args)` type-checked against entity event
  declarations. `after ms => expr` standalone sugar. Entity events validated
  at compile time; unknown events produce type errors.
- **RFC 0008 — Migration contracts.** `version: N` and
  `migration from N to M { ... }` blocks parsed inside entity declarations.
  AST/HIR/bytecode metadata wired through pipeline. Migration state bodies
  and event-migration handlers are now type-checked.
- **RFC 0009 — Organization primitives.** `organization` keyword
  parsed and desugared to `entity` with durable defaults. `is_organization`
  flag tracked through AST → HIR → bytecode.
- **RFC 0003 Item 6 — Backend trait boundary.** `JitBackend`, `WasmBackend`,
  `CryptoProvider`, `ForeignInterop`, `HttpProvider` traits defined in
  `src/backends/mod.rs`. JIT and WASM wired behind traits.
- **MIR register spilling.** Functions with more locals than fit in the
  register file (238 usable registers) now spill excess locals into a
  frame-local `Vec<Value>` via `SpillLoad`/`SpillStore` opcodes (0xF5/0xF6).
  Fix (2026-07-24): replaced post-processing spill rewrite with inline
  SpillLoad/SpillStore emission during codegen, removing the 17-slot
  capacity limit entirely.  Round-robin temp register allocation (r12/r13/r14)
  prevents clobbering in multi-operand spilled reads.  Net -112 lines.
  Unblocks the self-hosting bootstrap compiler (RFC 0003 Item 3).
- **Self-hosting bootstrap: Stage 5 (closures with env capture).** The
  `bootstrap/compiler_core.nula` Pratt evaluator now supports `fn(x) => body`
  lambdas, function application `f(arg)`, and environment capture
  (`let a = 3 in (fn(x) => a + x)(5)` → 8).  Closure encoding: 30-bit flag
  with packed param-hash, body-start, and captured binding.  Out-of-band
  sentinel `1 << 40` distinguishes "no left operand" from value 0.
- **Formal semantics: all three Core theorems proved.** Progress, preservation,
  and type soundness for the Core expression language (RFC 0002) are now
  machine-checked in `spec/formal/types.lean` (0 sorries). The capability
  lattice proofs (`capabilities.lean`) remain proved; effect and combined
  judgments are definition-only. See `spec/formal/README.md` for scope.

- **Error handling syntax.** `catch expr => body`, `fail expr`
  (structured short-circuit return), and `T ! E` return-type syntax
  (`fn div(a: Int, b: Int) -> Int ! String`). Errors propagate through
  `?` operator — `expr?` is sugar for `catch expr => |e| fail e`.
  Desugaring, type inference, and codegen wired in `src/parser.rs`,
  `src/typechecker.rs`, `src/hir_lower.rs`, `src/mir_lower.rs`, and
  `src/mir_codegen.rs`.
- **Transport resilience.** `send remote` and `ask remote` keywords
  enforce network-sendable (`val`/`tag`) capability constraints at the
  call site. `ask remote actor behavior(args) timeout N` accepts an
  optional `timeout` clause for request-response with deadline semantics.
  Capability enforcement lives in `src/effect_checker.rs`; transport
  modifiers parsed in `src/parser.rs`.
- **RFC 0010 — 100-Year Language Architecture.** Documented design rationale
  for multi-century relevance. Deliverables implemented:
  - **LLM→Inference effect alias:** `perform LLM.ask(p)` and
    `perform Inference.ask(p)` are synonyms; both resolve to
    `Effect::Inference`. The `LLM.ask` surface is a deprecated alias
    (`src/effect_checker.rs`, `src/mir_lower.rs`, `src/runtime/mod.rs`,
    `src/stdlib.rs`).
  - **Keyword lifecycle governance:** `GOVERNANCE.md` §2a defines keyword
    introduction, reservation, deprecation, and removal rules.
  - **Keyword namespace cleanup:** Six formerly-reserved keywords
    (`where`, `priv`, `loop`, `node`, `await`, `subworkflow`) removed
    from the lexer and now lex as plain identifiers (`src/lexer.rs`).
  - **Keyword inventory documented** in `SPEC2.md` §Implementation Status
    and verified against the implementation.

- `ai-runtime` feature: the AI runtime — **pure types live in the `nulang-ai`
  workspace crate** (`crates/nulang-ai/`) with zero dependencies on the core
  language crate. The core crate (`src/ai/mod.rs`) provides a thin re-export
  facade behind `#[cfg(feature = "ai-runtime")]`. All AI effects dispatch
  through the generic `PerformAsync` opcode (`0xC6`) with `effect_op` strings
  (`"Inference.ask"`, `"Pipeline.run"`, etc.). The monolithic AI opcode range
  (0x9D–0xC5: `LlmAsk`, `PipelineNew`…`DebateRun`) has been removed.
  Runtime integration (`src/runtime/agent.rs`, `llm.rs`, `ai_registry.rs`)
  bridges the pure AI types to the actor model. Behind `--features ai-runtime`
  (enabled by default). `LLM.ask` is a deprecated alias for `Inference.ask`
  and emits a compiler warning (RFC 0010).
- `python` feature: PyO3 interop (`src/python/`). Behind `--features python`.
- `sqlite` feature: libsql/Turso persistence. Behind `--features sqlite`.
- `lsp` feature: the tower-lsp language server (`src/lsp/`). Behind
  `--features lsp`.
- `ai-runtime` feature: the AI runtime (`crates/nulang-ai/` workspace crate,
  re-exported through `src/ai/`) — LLM providers (OpenAI, Ollama), pipelines,
  debates, supervisor teams, memory subsystems, and usage tracking. Behind
  `--features ai-runtime` (enabled by default). **Changed in 1.0.0-frozen:**
  all AI effects now dispatch through the generic `PerformAsync` opcode
  (`0xC6`) with `effect_op` strings (`"Inference.ask"`, `"Pipeline.run"`,
  etc.). The dedicated `LlmAsk` opcode and the `PipelineNew`…`DebateRun`
  opcode range (0x9D–0xC5) have been removed. AI types live in the
  `nulang-ai` crate with zero core dependencies; the core `ActorVmCallbacks`
  trait no longer carries AI-specific methods. The `LLM` effect redirects to
  `Provider.ask` under the hood.
- AOT native backend (`src/aot/`), JIT tiering (`src/jit/`), QUIC transport
  (`src/runtime/quic_transport.rs`).

- **Stdlib modules.** Standard library modules provide reusable generic
  data structures and operations: `stdlib::core` (base utilities),
  `stdlib::list` (map/filter/fold/reverse), `stdlib::string`
  (split/join/trim/replace), `stdlib::set` (add/remove/contains/union/
  intersect), `stdlib::map` (insert/get/remove/keys/values), and
  `stdlib::http` (get/post request builders). Modules live under
  `src/stdlib/` and are resolved via `NULANG_STDLIB`, the executable-
  relative path, or the dev-fallback `src/stdlib/`.
- **Typeclass declarations (Phase 4).** `class` and `impl`
  keyword support: `class Eq[T] { fn eq(self: T, other: T) -> Bool }`
  declares a typeclass with optional superclasses (`class Ord[T]: Eq`).
  `impl Eq Int { fn eq(self: Int, other: Int) = self == other }` registers
  a concrete instance. Class/instance tables in `TypeChecker`
  (`src/typechecker.rs`). Typechecker integration (dictionary-passing
  transform): method calls on concrete types (`1.eq(2)`) resolve through
  the instance table and type-check against the impl dictionary; missing
  instances (`"hi".eq("there")` with no `impl Eq String`) produce
  compile-time errors. HIR lowering for runtime dictionary construction
  is implemented: `Decl::Impl` lowers to `hir::Decl::Constant`, producing
  a module-level function that evaluates to a record of method closures.
  Field access routing through the dictionary at call sites is
  implemented: method calls on concrete types (`1.eq(1)`) lower to
  dict-constant calls, field accesses, and method invocations at the
  HIR level, producing correct runtime results. Full end-to-end
  verified with integration tests.
- **RFC 0003 — Content-addressed functions.** Proposal document
  (`RFC/0003-content-addressing.md`): defines a deterministic
  content-hash-based code identity scheme for distributed code
  deployment, cache invalidation, and reproducible builds across
  heterogeneous Nulang runtimes. Status: Draft. Content hashing
  infrastructure (BLAKE3 `source_hash` in `.nbc` artifacts) is
  available per RFC 0001; full code-identity registry and
  content-addressed deployment are not yet implemented.
- **`::` import resolution.** Module imports now support `::`-delimited
  paths: `import stdlib::set`, `import mypkg::utils::math`. The resolver
  (`src/resolver.rs`) maps `stdlib::*` prefixes to the standard library
  directory and general `::` paths to filesystem-relative module files.

---

## Pre-1.0 (crate version 0.13.0-alpha.1 and earlier)

No stability promise. The 0.x series is the alpha development track. Language
version 1.0.0-frozen is the first version with a published stability contract;
everything before it is implicitly Experimental.
