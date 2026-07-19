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

## Experimental tier

*No stability promise. May change or be removed in any release. Behind a
feature flag or explicitly marked experimental.*

### Current experimental surface

- `wasm-backend` feature: the WASM compiler (`src/mir_wasm.rs`) and Wasmtime
  host runtime (`src/wasm_runtime.rs`). Behind `--features wasm-backend`.
- `python` feature: PyO3 interop (`src/python/`). Behind `--features python`.
- `sqlite` feature: libsql/Turso persistence. Behind `--features sqlite`.
- `lsp` feature: the tower-lsp language server (`src/lsp/`). Behind
  `--features lsp`.
- The AI runtime (`src/ai/`): LLM providers, pipelines, debates, supervisor
  teams, the `LlmAsk` opcode, and the `LLM` effect. **Deprecated since
  1.0.0-frozen:** `Effect::LLM` and `OpCode::LlmAsk` are deprecated in
  favor of `perform Provider.ask("llm", prompt)`, which references an
  eternal "provider" abstraction rather than a transient technology. The
  `LLM`/`LlmAsk` surface remains functional for the deprecation cycle (≥2
  major versions) and will be removed in 3.0 (RFC 0003, item 5 breaking
  phase). New code should use `Provider.ask`. The `Provider` effect is
  Stable-tier: it is the general, runtime-registered effect handler for any
  provider, with `"llm"` as the first registered name.
- AOT native backend (`src/aot/`), JIT tiering (`src/jit/`), QUIC transport
  (`src/runtime/quic_transport.rs`).

---

## Pre-1.0 (crate version 0.13.0-alpha.1 and earlier)

No stability promise. The 0.x series is the alpha development track. Language
version 1.0.0-frozen is the first version with a published stability contract;
everything before it is implicitly Experimental.
