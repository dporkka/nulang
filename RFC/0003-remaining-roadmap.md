# RFC 0003: Remaining Longevity Roadmap Items

- **Status:** Accepted (partially implemented — see per-item status)
- **Tier:** varies (see individual items)
- **Author:** David Porkka
- **Created:** 2026-07-19
- **Resolved:** 2026-07-19 (Accepted)
- **Language-version at effect:** 1.0.0-frozen (items 5, 6, 10, 11 took effect)
- **Supersedes:** none
- **Superseded by:** Per-item RFCs TBD

## Summary

Documents the remaining items from the Nulang 200-Year Longevity Roadmap that
were scoped but not implemented in the initial execution pass. Each item below
is a distinct, self-contained RFC-to-be with concrete file targets and the
change to make. This RFC serves as the scoping artifact so the remaining work
is tracked, not forgotten.

## Item 2: Formal Semantics and Soundness Proofs (Frozen)

**Target:** `spec/formal/` — new top-level directory (language artifact).

**Change:**
- `spec/formal/types.lean` — formalize `Type`, `Scheme`, `Substitution`, `mgu`
  with occurs check, `generalize`/`instantiate`. State and prove **Theorem
  type_soundness**: `∅ ⊢ e : τ ∧ e ↦ v ⇒ ∅ ⊢ v : τ`.
- `spec/formal/capabilities.lean` — formalize the capability lattice,
  `join`, `is_subtype_of`, `is_sendable`, LinearIso at-most-once consumption.
  Prove **Theorem cap_sendable**: a `val`/`tag` value can cross actor
  boundaries without violating isolation.
- `spec/formal/effects.lean` — formalize `EffectRow` (Closed/Open + Region),
  handler dispatch, `Perform`/`Resume`/`Unwind`. Prove **Theorem
  effect_safety**: a program with closed effect row `{}` cannot perform an
  unhandled effect.
- `spec/formal/Makefile` and CI job running `lake build` on every PR touching
  `src/typechecker.rs`, `src/effect_checker.rs`, `src/types.rs`.

**Why:** The combined type/effect/capability system has no machine-checked
model. Prose is insufficient for a language that wants to survive 200 years.

**Contingency:** If the full combined system is too hard, formalize the
components separately (HM, capabilities follow Pony, row effects follow
Koka) and state the combination as a conjecture with a documented proof
plan. Do not skip the artifact entirely.

## Item 3: Self-Hosting Bootstrap for the Frozen Core (Frozen) — STAGE 1 SEEDED 2026-08-09

**Target:** `bootstrap/` — new top-level directory. `SPEC2.md` §"Core".

**Change:**
- Write a Nulang→Nulang compiler in Nulang Core targeting the `.nbc` format
  (RFC 0001). Stage 1 compiles Core programs only. Stage 2 compiles itself.
- `bootstrap/host.nula` — thin shim running the bootstrap compiler under the
  current Rust implementation until stage 2.
- CI job: `nulang bootstrap/bootstrap_compiler.nula --eval
  bootstrap/self_test.nula` produces identical output to `cargo run --
  bootstrap/self_test.nula`.

**Status (2026-08-09):** Stage 1 working end-to-end; Stage 2 parser seed.
- `bootstrap/README.md` — strategy document (updated to actual state).
- `bootstrap/compiler_core.nula` — Core Pratt parser + type-checker
  (evaluates a Core expression subset; stdin-driven).
- `bootstrap/compile_hex.nula` — Core → hex bytecode emitter (arithmetic,
  comparisons, booleans, if/else, let, closures, application, effects).
- `bootstrap/fixup_hex.py` — patches jump offsets, constant-pool indices,
  and closure frame indices in the emitted hex.
- `bootstrap/hex2nbc.py` — converts patched hex to a runnable `.nbc` binary.
- `bootstrap/self_test.nula` — minimal Core test program (fib(10) = 55).
- `bootstrap/verify.sh` — 11 checks: self_test eval, compiler_core eval,
  host shim, Rust `.nbc` round-trip, single-expression self-hosting
  pipeline (arithmetic, `let`, `if`, `not`, closure application, **fib
  recursion** = 55), and **multi-fn Stage 2** (desugar_fns.py → compile_hex
  → fixup → hex2nbc → VM). The pipeline checks are the Stage 1→2 bridge:
  a Nulang Core program compiles Core source to `.nbc` with no Rust
  compiler in the loop.
- Fixed (2026-08-09): `false` keyword hash bug — `read_ident` returns the
  low-16 hash, but `false` (full hash 79251) was compared unmasked, so the
  `false` literal was never recognized (bare `false` → nil, `not false` →
  false). Correct constant is 13715. Verified: 20-expression oracle
  comparison against the Rust compiler, all matching.
- Verified (2026-08-09): multi-fn programs and recursion work through the
  pipeline end-to-end. Remaining blocker: the **3-argument `nperform` path**
  (`String.charAt`, 2 args) emits a corrupted effect-name constant because
  `compile_hex.nula`'s `comp` (178 locals) triggers the host compiler's MIR
  register-spill bug (`src/mir_codegen.rs`; see `spill_bug_repro.nula`). The
  existing repro passes; the 3-arg-inside-`comp` shape is a residual
  manifestation. `String.length` (1-arg) and `IO.print` work.
- Remaining: fix the residual register-spill manifestation so `String.charAt`
  compiles; Stage 2 self-compiling parser (compile `compiler_core.nula`
  through the pipeline and compare against the Rust compiler's output);
  Stage 3 minimal Core VM.


## Item 5: Decouple LLM (and All Transient Tech) from the Language Vocabulary (Stable) — NON-BREAKING PHASE IMPLEMENTED 2026-07-19

**Target:** `src/bytecode.rs`, `src/mir.rs`, `src/effect_checker.rs`,
`src/hir_lower.rs`, `src/lsp/mod.rs`, `src/ai/`.

**Change (non-breaking first phase — this RFC):**
- Add a `Provider` effect mechanism: a runtime-registered effect handler
  that the language dispatches to via the existing `ActorVmCallbacks` trait.
  Users write `perform Provider.ask("llm", prompt)`; the core language has
  no knowledge of "LLM".
- Mark `Effect::LLM` and `OpCode::LlmAsk` as deprecated in `CHANGELOG.md`.
  They remain functional for the deprecation cycle.

**Change (breaking second phase — separate RFC in 2 major versions):**
- Remove `OpCode::LlmAsk` from `src/bytecode.rs`; remove `RValue::LlmAsk`
  from `src/mir.rs`; remove `Effect::LLM` from `src/effect_checker.rs`.
- Remove `PipelineNew`, `PipelineStage`, `PipelineRun` opcodes likewise.
- Bytecode v1→v2 migration in `src/format/migrate.rs` rewrites `LlmAsk`
  opcodes to `Perform` + a `Provider` handler registration.

**Why:** The language's stable vocabulary must reference eternal concepts
(actor, message, type, effect, capability) not transient ones (LLM, pipeline,
debate).

## Item 6: Host-ABI Trait Boundary Over All Transient Backends (Stable) — FULLY WIRED 2026-08-09

**Target:** `src/backends/mod.rs` (trait definitions + factory functions),
`src/vm.rs`, `src/runtime/mod.rs`, `src/main.rs`.

**Change:**
- Define `trait JitBackend`, `trait WasmBackend`, `trait StorageBackend`
  (generalize `PersistenceStore`), `trait Transport`, `trait CryptoProvider`,
  `trait HttpProvider`, `trait ForeignInterop`, `trait TlsProvider`.
- `src/jit/` is a `JitBackend` impl for Cranelift; `vm.rs` accesses it
  exclusively through `Box<dyn JitBackend>` and `create_default_jit()`.
- `src/mir_wasm.rs` + `src/wasm_runtime.rs` are a `WasmBackend` impl;
  `main.rs` accesses them through `Box<dyn WasmBackend>`.
- `src/python/` is a `ForeignInterop` impl for PyO3; `Runtime` accesses it
  through `Option<Box<dyn ForeignInterop>>`.
- Core language never imports `cranelift`, `wasmtime`, `pyo3`, `libsql`,
  `quinn`, `rustls`, or `reqwest` directly. The sole exception is
  `src/backends/mod.rs` which imports concrete types for factory functions
  and `src/runtime/mod.rs` which constructs default impls at startup —
  both are composition roots, not core language code.

**Why:** Dependencies are transient; the language is not. Trait boundaries
let a 2125 runtime swap Cranelift for whatever codegen exists then.

**Contingency:** If this breaks JIT tiering (concrete access to VM fields),
keep a `JitView` struct exposed by `VM` — the boundary is "JIT sees a
stable view", not "JIT sees only traits". Not needed: JIT tiering works
through the trait boundary.

## Item 10: Break Up the Runtime God-Object (Hygiene) — SUPERVISOR TEAMS EXTRACTED 2026-07-19

**Target:** `src/runtime/mod.rs` (5911 lines).

**Change:**
- Extract `Scheduler`, `GcCoordinator`, `SupervisorTree`, `PersistenceLayer`,
  `Cluster` into separate structs owned by `Runtime` as fields, each behind
  its own trait. Partial factoring already exists (`distributed_context.rs`).

**Why:** Unblocks independent evolution of each subsystem on a 200-year horizon.

## Item 11: Content-Addressed Module System (Experimental) — IMPLEMENTED 2026-07-19

**Target:** `src/package/` — extend `resolver.rs` and `lockfile.rs`.

**Change:**
- `Nulang.lock` pins `{module_name → blake3(deps + source)}`.
- A module pinned in 2026 is bit-identically resolvable in 2226 if any
  conforming registry mirrors it.

**Why:** URLs and git repos are not durable artifact identifiers; content
hashes are. `blake3` is already a dep.

## Item 14: Deprecate Direct `quinn`/`rustls`/`reqwest` Use (Hygiene) — PENDING (depends on item 6 wiring)

**Target:** `src/runtime/network.rs`, `src/runtime/quic_transport.rs`.

**Change:**
- Route through Item 6's `trait Transport` / `trait HttpProvider`.
- Default impl uses quinn/rustls/reqwest today; a 2125 impl uses whatever
  then. The language never knows.

## Item 15: Distributed Trace Context Propagation (Stable) — IMPLEMENTED 2026-08-09

**Target:** `src/runtime/mailbox.rs`, `src/runtime/mod.rs`, `src/runtime/distributed.rs`,
`src/runtime/network.rs`.

**Change:**
- Add `trace_id: Option<String>` to `Message` struct — carries W3C traceparent
  across local send, cross-shard delivery, and network deserialization.
- Propagate `trace_id` from `Packet::ActorMessage` (already serialized on the
  wire, `SPEC2` §15.3) into `Message` in `parse_packet` — previously discarded.
- Add `trace_id` to `CrossShardMsg::DeliverMessage` so trace context survives
  shard-boundary hops.
- `send_message_by_id`, `deliver_cross_shard_message`, exit signals, and DOWN
  messages all carry `trace_id: None` by default; the OTel layer populates it
  when the `otel` feature is active.

**Why:** The `Packet::ActorMessage` wire format already serialized `trace_id`
but `parse_packet` dropped it with `..`. End-to-end trace context is the
foundation for observability — without it, every actor hop and node boundary
breaks the trace.

## Item 16: WASM Component Model WIT Interface Mapping (Experimental)

**Target:** `src/wasm_component_runtime.rs`, `src/wasm_types.rs`,
`src/effect_checker.rs`.

**Change:**
- Map Nulang's row-polymorphic algebraic effects onto WIT interfaces:
  each `perform Effect.op(args...)` becomes a WIT `import` function call
  in the compiled WASM component.
- The `wasm_component_runtime.rs` host provides WIT `export` functions that
  correspond to built-in effects (`IO`, `Timer`, `Signal`, `Provider`).
- Generate WIT world files from effect-row signatures so a compiled Nulang
  actor is a valid WASI 0.2+ component pluggable into any compliant host.

**Why:** WASM components are the emerging standard for language-agnostic,
capability-sandboxed modules. Mapping effect rows onto WIT makes Nulang
actors interoperable with the broader WASM ecosystem without FFI glue.

## Item 17: `.nbc` Library Distribution in nula Package Manager (Experimental)

**Target:** `src/package/` — extend `resolver.rs` and `lockfile.rs`.

**Change:**
- Support `.nbc` artifacts as library dependencies in `Nulang.toml`:
  `{ nbc = "path/to/lib.nbc" }` or registry-hosted `.nbc` packages.
- A library author publishes type-checked, compiled `.nbc` files; consumers
  link them without source distribution.
- Extend the `.nbc` format with an export table (symbol name → type signature
  + bytecode offset) so consumers can resolve library symbols at link time.
- The type checker validates consumer code against the library's export
  signatures (stored in the `.nbc` constant pool).

**Why:** Pre-compiled artifacts enable closed-source library distribution,
Accepted 2026-07-19. Items 5 (non-breaking phase), 6 (full trait wiring),
10 (supervisor teams), 11 (content-addressed modules), and 15 (trace context
propagation) are implemented and verified — 1647 tests pass. Items 2
(formal semantics) and 3 (self-hosting bootstrap) are multi-week research
efforts that remain as scoped follow-ups; they are the highest-leverage
remaining items. Item 5's breaking phase (removing `LlmAsk`/`Effect::LLM`)
follows the deprecation cycle (≥2 major versions). Item 14 (deprecating
direct quinn/rustls/reqwest use), item 16 (WASM WIT mapping), and item 17
(`.nbc` library distribution) are incremental follow-ups.

### Delivered this session

- **Item 5 (non-breaking):** `perform Provider.ask("llm", prompt)` is the
  new, eternal-vocabulary replacement for `perform LLM.ask(prompt)`. The
  `Provider` effect dispatches through the existing `Perform` opcode; the
  `"llm"` provider reuses the existing LLM client via MIR-level special-case
  lowering to `LlmAsk`. `Effect::LLM` and `OpCode::LlmAsk` are deprecated in
  `CHANGELOG.md`. 2 new tests pass.
- **Item 6 (traits):** `src/backends/mod.rs` defines `StorageBackend`,
  `JitBackend`, `WasmBackend`, and `Transport` traits. `StorageBackend` and
  `Transport` are blanket-impl'd over the existing `PersistenceStore` and
  `NetworkTransport`. 2 new tests pass.
- **Item 10 (extraction):** `src/runtime/supervisor_registry.rs` extracts
  the AI-runtime supervisor-team state (`supervisor_teams`,
  `next_supervisor_id`) into a `SupervisorTeamRegistry` struct. `Runtime`
  holds it as a field; methods delegate. 2 new tests pass; 401 runtime
  tests pass.
- **Item 11 (content-addressed):** `Nulang.lock` now carries a BLAKE3
  `content_hash` per pinned package, computed from `.nula` source files.
  2 new tests pass.
- **Item 15 (trace context):** `trace_id: Option<String>` added to
  `Message` struct; propagated end-to-end from `Packet::ActorMessage`
  wire format through `parse_packet`, `CrossShardMsg::DeliverMessage`,
  `deliver_cross_shard_message`, and all local `send_message_by_id` paths.
  The field was already serialized on the wire but discarded by `..` in
  `parse_packet`. 0 new tests (mechanical field addition to existing
  struct; 31 Message construction sites updated).

### Remaining as scoped follow-ups

- **Item 2 (formal semantics):** Multi-week Lean formalization of the
  type/effect/capability system. Starter artifact: the formalization target
  is `spec/formal/` (to be created). The Rust impl in `src/typechecker.rs`,
  `src/effect_checker.rs`, `src/types.rs` is the oracle to formalize
  against.
- **Item 3 (self-hosting):** Multi-week Nulang→Nulang bootstrap compiler
  targeting Core (RFC 0002) and `.nbc` (RFC 0001). Starter artifact: the
  bootstrap compiler lives in `bootstrap/` (to be created). Core is defined
  in RFC 0002; the `.nbc` format is defined in RFC 0001.
- **Item 5 (breaking phase):** Remove `OpCode::LlmAsk`, `RValue::LlmAsk`,
  `Effect::LLM` after the deprecation cycle. Requires bytecode v1→v2
  migration in `src/format/migrate.rs`.
- **Item 14:** Route `quinn`/`rustls`/`reqwest` through `trait Transport` /
  `trait HttpProvider` (to be defined). Item 6 trait wiring is complete;
  this is the final hygiene pass.
- **Item 16 (WASM WIT mapping):** Map Nulang effect rows onto WASI 0.2+
  WIT interfaces. `wasm_component_runtime.rs` already hosts WASM components;
  this item adds WIT world generation from effect-row signatures so compiled
  actors are pluggable into any WASM component host without glue code.
- **Item 17 (`.nbc` library dist):** Extend `nula` package manager with
  `.nbc` artifact dependencies. Library authors publish type-checked,
  compiled `.nbc` files; consumers link them without source. Requires an
  export table in the `.nbc` format (backward-compatible extension per
  RFC 0001).
