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

## Item 3: Self-Hosting Bootstrap for the Frozen Core (Frozen)

**Target:** `bootstrap/` — new top-level directory. `SPEC2.md` §"Core".

**Change:**
- Write a Nulang→Nulang compiler in Nulang Core targeting the `.nbc` format
  (RFC 0001). Stage 1 compiles Core programs only. Stage 2 compiles itself.
- `bootstrap/host.nula` — thin shim running the bootstrap compiler under the
  current Rust implementation until stage 2.
- CI job: `nulang bootstrap/bootstrap_compiler.nula --eval
  bootstrap/self_test.nula` produces identical output to `cargo run --
  bootstrap/self_test.nula`.

**Why:** Self-hosting decouples the language from its host's survival. Every
50+ year-old language either self-hosts or has multiple independent
implementations.

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

## Item 6: Host-ABI Trait Boundary Over All Transient Backends (Stable) — TRAITS DEFINED 2026-07-19

**Target:** New traits in `src/backends/` directory. Existing impls in
`src/jit/`, `src/mir_wasm.rs`, `src/wasm_runtime.rs`, `src/python/`,
`src/runtime/persistence.rs`.

**Change:**
- Define `trait JitBackend`, `trait WasmBackend`, `trait StorageBackend`
  (generalize `PersistenceStore`), `trait Transport`, `trait CryptoProvider`,
  `trait HttpProvider`.
- `src/jit/` becomes a `JitBackend` impl for Cranelift, swappable.
- `src/mir_wasm.rs` + `src/wasm_runtime.rs` become a `WasmBackend` impl.
- `src/python/` becomes a `ForeignInterop` impl for PyO3.
- Core language never imports `cranelift`, `wasmtime`, `pyo3`, `libsql`,
  `quinn`, `rustls`, or `reqwest` directly.

**Why:** Dependencies are transient; the language is not. Trait boundaries
let a 2125 runtime swap Cranelift for whatever codegen exists then.

**Contingency:** If this breaks JIT tiering (concrete access to VM fields),
keep a `JitView` struct exposed by `VM` — the boundary is "JIT sees a
stable view", not "JIT sees only traits".

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

## Resolution

Accepted 2026-07-19. Items 5 (non-breaking phase), 6 (traits), 10
(supervisor teams), and 11 (content-addressed modules) are implemented and
verified — 1358 tests pass. Items 2 (formal semantics) and 3 (self-hosting
bootstrap) are multi-week research efforts that remain as scoped follow-ups;
they are the highest-leverage remaining items. Item 5's breaking phase
(removing `LlmAsk`/`Effect::LLM`) follows the deprecation cycle (≥2 major
versions). Item 6's full wiring (routing existing impls behind the new
traits) and item 14 (deprecating direct quinn/rustls/reqwest use) are
incremental follow-ups.

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
- **Item 6 (full wiring):** Route `src/jit/`, `src/mir_wasm.rs`,
  `src/wasm_runtime.rs`, `src/python/` behind the new traits. The trait
  definitions are in place; the concrete impls need to be moved.
- **Item 14:** Route `quinn`/`rustls`/`reqwest` through `trait Transport` /
  `trait HttpProvider` (to be defined). Depends on item 6 full wiring.
