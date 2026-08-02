# Nulang: Plan to Production-Ready, World-Class

> **Status:** Draft, unratified.
> **Author:** planning artifact, not a governance document.
> **Scope:** the sequence of work to move Nulang from `0.1.0` alpha to a
> language a serious team can run in production and that stands up to
> comparison with Erlang/OTP, Rust, Elixir, and Pony on the axes each of
> those languages already owns.
> **Relationship to governance:** this file is descriptive. Each Frozen or
> Stable-tier change it names still requires an RFC per `GOVERNANCE.md` §4.

---

## Thesis

Nulang today has a coherent design, unusually strong format-stability
discipline for its age (RFCs 0001/0002), and roughly 70% of a production
implementation. The remaining 30% is neither exotic nor speculative: it is
closing gaps between advertised and implemented behavior, generating
evidence for claims that are currently unverified, delivering the
self-hosting story the frozen-format promise depends on, and getting the
first external user. Nothing on the critical path is a research problem.

## Principles

1. **Truth-in-advertising precedes new features.** No new surface until
   every claim in `README.md`, `LAUNCH.md`, and `CHANGELOG.md` is either
   implemented or removed. A user hitting a `not yet supported` error on
   an advertised flag is worth −100 users.
2. **Evidence, not estimates.** Every performance claim ships with a
   criterion benchmark under CI regression tracking. Every correctness
   claim ships with a test, a proof, or a conformance case.
3. **Deprecate before delete.** Once past the current alpha, every removal
   goes through the two-major-version deprecation cycle in
   `GOVERNANCE.md` §6, even for Experimental-tier surfaces where possible.
4. **One implementation, then two.** Multi-implementation credibility is a
   Phase 4 goal, not a prerequisite. The single implementation must be
   trustworthy first.
5. **Small binaries stay small.** The `no-default-features` build is the
   longevity floor; every new dep is checked against it.
6. **The steward is a bottleneck.** Every phase specifies which work is
   delegable, and the plan prefers deep specialists (fuzzing, formal
   methods, package registry) over generalist contributors.
7. **Kill criteria are explicit.** Every phase has a "stop and re-plan"
   trigger. Sunk-cost is not a strategy for a 200-year language.

---

## Current state (verified 2026-08-01)

| Signal | Value |
|---|---|
| Crate version | `0.1.0` |
| Language version | `1.0.0-frozen` (RFCs 0001, 0002) |
| Rust source | ~89k lines, single crate + `nulang-ai` workspace crate |
| Tests | 1490+ (`cargo test`), 1541+ target per `RELEASE_CHECKLIST.md` |
| CI matrix | build/test/release/wasm/minimal/lint/lean/package-smoke |
| Direct deps | 65 |
| Transitive deps | 483 |
| Formal proofs (Lean 4) | Core type soundness proved; capabilities/effects definition-only |
| Conformance suite | 52 behavior cases + grammar cases |
| Bootstrap self-hosting | Stage 13; not yet self-compiling |
| Benchmarks | `benches/` uses criterion (7 files, 404 lines); no CI regression tracking |
| DST | `src/dst.rs` seed present (265 lines); not integrated into CI |
| Fuzzer | `src/fuzz.rs` present (412 lines); runs in `cargo test` |
| Shipped release binaries | None in repo evidence |
| External users | None known |

Unfinished implementation lines counted from `not yet implemented` /
`not yet supported` markers in `src/`:

- `src/vm.rs:4553-4630` — 7 interpreter opcodes trap (`ConstL`, `Pop`,
  `Switch`, `Alloc`, `TupleL`, `Unpack`, `Copy`).
- `src/aot/codegen.rs:797-1108` — ~15 MIR constructs unsupported in the
  AOT native backend (all effects, actors, spawn, send, ask, receive,
  FFI, state, capability check).
- `src/mir_wasm.rs:322`, `src/wasm_runtime.rs:149,187` — WASM handler
  emission is a nil-drop; `host_read` and `host_dispatch` are stubs.
- `src/fmt.rs:31` — formatter refuses files with `workflow`, `agent`,
  `let`, `class`, or `impl`.
- `src/typechecker.rs:274-284` — `opaque` nominal types are transparent.
- `Cargo.toml:38-40` — `simd-experimental` and `quic-experimental`
  features are documented as dead-code / panic-on-use.
- `src/runtime/mod.rs:2773` — WASM component runtime path is a stub.

---

## Phase 0 — Truth-in-Advertising (weeks 0–4) — **COMPLETE 2026-08-01**

**Goal.** Every claim in the repo is either implemented, gated behind an
Experimental warning, or removed. No user hits an unimplemented path on
a documented flag.

**Actual outcome vs. original scoping below:** the single largest item
wasn't on this list. `--no-default-features` didn't compile at all —
`crate::ai::*` (LlmRequest, EpisodicMemory, SupervisorTeam, Pipeline,
ToolSchema, ...) was used unconditionally across `bytecode.rs`, `hir.rs`,
`hir_lower.rs`, and 6 `runtime/` files despite living behind
`#[cfg(feature = "ai-runtime")]`. This directly contradicted principle 5
("small binaries stay small") and the CI `minimal-build` job's own
stated purpose. Fixed by moving `ToolSchema` to a new core module
(`src/tool_schema.rs`, unconditional — it's core language surface per
RFC 0010 §C.2, not AI-specific) and gating every genuinely-AI function/
field/match-arm behind `ai-runtime` (~50 sites in `runtime/mod.rs` +
`runtime/actor.rs`, plus `agent.rs`/`ai_registry.rs`/`llm.rs`/
`supervisor_registry.rs` wholesale, plus 31 tests). `suspend_enabled`
moved off the AI-only `LlmState` onto `Runtime` directly — it wasn't
LLM-specific, core receive-wait suspension read it too. Verified: all
four feature configs (default/no-default/all-features/wasm-backend)
compile with zero warnings and pass their full test suites.

Also found and left unfixed (confirmed pre-existing on clean `HEAD`,
orthogonal to this phase's scope): `cargo clippy --all-targets -- -D
clippy::correctness` fails on a `clippy::approx_constant` hit in
`integration_tests/mod.rs:2278` and all 7 files under `benches/` fail
to compile under `--all-targets`. CI's lint job is red on `main`
independent of anything in this phase. Not fixed here — real but
unrelated to truth-in-advertising; needs its own pass.

**Deliverables.**

1. **Restrict advertised backends to what they support.** ✅ Revised
   decision after checking actual behavior: AOT already fails loudly
   with a specific "X is not yet supported in the native backend, use
   --backend bytecode" message per unsupported construct (verified —
   not a silent failure). The real gap was `--help` not saying so
   upfront. Fixed `--help` and the CLI doc comment to state native's
   (pure-functional only) and wasm's (IO.print/read only) scope before
   a user picks the flag, instead of adding a redundant runtime warning
   on top of an already-clear error.
2. **Interpreter opcode gap closure.** ✅ Confirmed all 7 opcodes
   (`ConstL`/`Pop`/`Switch`/`Alloc`/`TupleL`/`Unpack`/`Copy`) are
   unreachable from every current codegen path (zero matches across
   `mir_codegen`/`hir_lower`/`mir_lower`/`aot`/`mir_wasm`). Rewrote the
   error messages from "not yet implemented" (reads as a broken TODO)
   to state what each is reserved for and what already covers its use
   case. Added `test_no_codegen_path_emits_reserved_opcodes`
   (`integration_tests/mod.rs`) as CI-enforced proof, not just a
   comment claim.
3. **Formatter completeness.** ⏸️ Deferred, as flagged in the original
   scoping note. Genuinely multi-day: 5 distinct AST shapes need real
   formatting logic, not a quick fix. Tracked as its own follow-up.
4. **`opaque` nominal types.** 🔄 Revised after investigation: activating
   enforcement naively would have made opaque types permanently
   uninstantiable (verified: `mgu`'s current transparent unification is
   the *only* construction mechanism anywhere in the pipeline — no
   synthesized constructor, no cast operator). Real fix needs module-
   defining-scope tracking that doesn't exist in `TypeChecker` — new
   feature work, not a Phase 0 gap-closure. Also verified zero public
   docs (README/LAUNCH/CHANGELOG/SPEC2.md) claim opaque types work, so
   there was no truth-in-advertising violation to begin with. Tightened
   the code comment to state the zero-enforcement status unambiguously
   instead.
5. **Dead feature flags.** 🔄 Revised after investigation:
   `quic_transport.rs` is a complete, compiling `NetworkTransport` impl
   (TLS via rcgen, node-id handshake) — not the "empty stubs, panics in
   bind()" the stale Cargo.toml comment claimed. Deleting it would have
   destroyed working code to match a false comment. Real bug: the
   feature flag didn't gate its own `quinn` dependency (compiled into
   every build regardless). Fixed: `quinn` now `optional = true`, gated
   by `quic-experimental`; module doc states the honest unwired/
   untested caveat; stale Cargo.toml/CHANGELOG claims corrected.
   `simd-experimental` was genuinely 100% orphaned (zero code
   references anywhere) — deleted outright, zero risk. Also removed
   unused `multihash` dependency (never imported anywhere).
6. **README/LAUNCH audit.** ✅ Found and fixed real corruption: an
   orphaned table fragment (examples 15-17 with no header row) plus a
   dangling half-sentence, debris from an earlier edit. Fixed 3 stale
   "11 verified examples" mentions → 17 (actual count). Fixed stale
   "1490+ tests" → 1550+ (measured: 1554). Added the AOT backend's
   actual scope to Feature Highlights (previously unmentioned there).
   `examples/README.md` was itself missing rows for 16/17 — fixed.
   `LAUNCH.md` was already accurate; no changes needed.
7. **Release checklist enforcement.** ✅ `verify_implementation.py`
   claimed (via AGENTS.md) to run `cargo test` — it never did;
   `check_warnings()` only ran `cargo check --tests` (compiles, never
   executes). This is exactly why it never caught item 0 below. Added
   `run_tests()` (`cargo test --lib`), and extended `check_warnings()`
   to cover all three CI feature configs instead of just default.
0. **[Not originally scoped] `--no-default-features` didn't compile.**
   ✅ The dominant item this phase actually did — see "Actual outcome"
   above. `crate::ai::*` used unconditionally outside its feature gate
   across bytecode.rs/hir.rs/hir_lower.rs/6 runtime files. Fixed by
   moving `ToolSchema` to core (`src/tool_schema.rs`) and properly
   gating everything AI-specific (~50 sites + 4 whole modules + 31
   tests) behind `ai-runtime`, including moving `suspend_enabled` off
   the AI-only `LlmState` onto `Runtime` (core receive-wait needed it
   too — it was never actually LLM-specific).

**Acceptance — met.**
- `cargo test --lib` green on all 4 feature configs (default 1554,
  `--no-default-features` 1443, `--all-features` 1586, `wasm-backend`
  1586) — 0 failures throughout.
- `verify_implementation.py` exits 0 end-to-end (~43s), now actually
  running the test suite it claimed to run.
- `cargo check --tests` zero warnings on all 3 CI feature configs.
- `cargo fmt --check` clean throughout.
- README.md/examples/README.md/docs/GETTING_STARTED.md: every stale
  count and the corrupted fragment fixed; every remaining Feature
  Highlights bullet now matches verified behavior.

**Non-goals — held.** No new language features. No new backends
(AOT/WASM scope was clarified, not expanded). No performance work.

**Kill criteria.** If any bullet takes >2× its estimate, land the
downgrade (remove the surface) rather than the fix, and open an RFC for
the restoration. This phase must complete on schedule.

---

## Phase 1 — Correctness Floor (weeks 4–12)

**Current state (in progress, verified 2026-08-02):** ~1/8 deliverables
substantially done, 1/8 partially done, a real bug found and fixed along
the way.
- **[X] Bullet 1 (fuzzer maturation) — interp/JIT/AOT leg done.**
  `src/fuzz.rs` grew from panic-avoidance to real differential execution
  fuzzing (`differential_fuzz_one`): compiles a mutant, runs it
  interpreted, forces real JIT tier-up on the same VM instance, and
  compares against the AOT backend when it accepts the program. Building
  this surfaced and fixed three of its own false-positive bug classes —
  worth recording because they're exactly the kind of subtle harness bugs
  that make a "0 divergences" result meaningless if unaddressed:
  `Value::to_string_repr()` doesn't resolve pool-indexed or heap-pointer
  values, so raw comparison across independently-compiled backends is
  unsound (fixed via `is_safely_comparable` gating + reusing the VM's own
  `string_operand` resolver for the same-VM leg); `VM::step_count` is a
  lifetime counter that accumulates across repeated `run()` calls, so a
  step-limit-triggered safety abort trips at different cumulative counts
  on cold vs warm and must be compared by category, not exact text; and
  forcing JIT tier-up via a fixed repeat count is unbounded when a
  mutant's own body loops heavily, requiring a wall-clock warmup budget
  instead. `fuzz_differential_quick` (300 iter, default `cargo test`) and
  `fuzz_differential_extended` (30,000 iter, `#[ignore]`d) both currently
  pass with 0 divergences. **Not done:** WASM backend comparison leg;
  reaching the 10⁶/day CI-nightly or 4×10⁴/day per-PR scale (that needs a
  dedicated scheduled CI job, not a `cargo test` invocation — the seed for
  one exists in `fuzz_differential_extended` but the job itself isn't
  wired).
- **[~] Bullet 5 (conformance suite expansion) — 26 → 115 of 300 target.**
  Corrected from this doc's original "52" (that was a file count —
  `.nula`+`.json` pairs — not a case count; the actual starting case count
  was 26). Seven parallel agents each targeted one SPEC2.md area
  (capabilities, effect-handler resume, effect rows, actor
  messaging/supervision, CRDT merge laws, pattern matching/error handling,
  persistence/event sourcing), landing 87 new cases with every expected
  value captured from the real compiled binary. Two more cases added
  directly as regression coverage (see below). Still short of 300 —
  the remaining ~185 need the same treatment across whatever surfaces the
  existing batches didn't reach.
- **Real bug found and fixed, not from a PLAN.md bullet but squarely
  "correctness floor":** writing actor conformance cases surfaced that
  concatenating a top-level `ask` result (via `Int.to_string` or similar)
  silently produced `nil`. Root cause: `RuntimeVmCallbacks` (attached
  whenever a program declares any actor) allocated new heap strings via
  `Runtime.vm.allocate_string` — but `Runtime.vm` is a separate,
  lazily-created VM instance that only runs actor bytecode; its heap is
  not reachable from `main()`'s own top-level VM. Worse, `alloc`/
  `drop_ref`/`retain_ref` returned `None`/no-op whenever there was no
  *current actor* — which is always true at the top level — so this
  wasn't just an `ask`-result bug: **any string concatenation or other
  heap allocation in `main()`'s own code failed once a program used any
  actor construct at all**, confirmed against a real shipped example
  (`examples/supervisor_tree.nula` printed literal `nil` for two lines).
  Fixed by giving `Runtime` a dedicated `main_heap`/`main_gc` fallback
  (commit `7215088`) and adding two permanent regression cases
  (`actor_18`/`actor_19`, commit `ef1f451`) that the pre-fix binary would
  have failed. Full suite (1555 default, 1511 no-default-features), all
  115 conformance cases, and `wasm-backend` feature check all verified
  green after the fix.
- **[ ] Bullets 2, 3, 4, 6, 7, 8 — not started this session.** Bullet 3
  (benchmark harness) has a prerequisite already done: `benches/*.rs` was
  fixed to actually compile and run (they didn't before — see
  `8636b01`), but CI wiring, a `benchmarks/` results directory, and
  regression-threshold gating remain. Bullets 2/4 (DST, chaos suite) need
  `src/dst.rs` wired into the real actor runtime — currently a standalone,
  unwired module. Bullets 6/7/8 untouched.

**Goal.** The language does what it says, provably, on the paths users
actually take. This is what makes 0.1.0 → 0.2.0 justifiable.

**Deliverables.**

1. **Fuzzer maturation.** Grow `src/fuzz.rs` from panic-avoidance to
   differential fuzzing: compile a mutant, interpret vs JIT vs
   (surviving) AOT/WASM backends, assert identical observable results
   or identical errors. Target: 10⁶ iterations/day in CI nightly,
   4×10⁴/day in per-PR CI. Any divergence is a bug.
2. **Deterministic Simulation Testing.** Wire `src/dst.rs` into the
   actor runtime. Deliverables:
   - `Simulator` replaces `Scheduler`, `NetworkTransport`, and the
     wall clock with deterministic fakes.
   - Message reorder, network partition, node crash, GC-during-send,
     and CRDT-sync-race scenarios expressed as seed-driven tests.
   - CI job runs 10⁴ seeds per commit, fails on any invariant
     violation (deadlock, lost message under `AtMostOnce`, CRDT
     divergence, supervision-cascade failure).
   - Any bug found is captured as a permanent regression test with
     its exact seed.
3. **Benchmark harness with regression tracking.** Every criterion
   bench under `benches/` runs in CI on `main` and PRs; results are
   written to a `benchmarks/` directory in the repo (or an
   externally-hosted dashboard). Regressions >5% fail the PR unless
   annotated. Publishes measured numbers to replace the estimates in
   `PERFORMANCE_ANALYSIS.md`.
4. **Chaos suite for distribution.** Extends the DST harness with
   concrete cluster topologies: 3-node, 5-node, split-brain,
   asymmetric partition, rolling restart. Runs 10³ seeds per commit.
5. **Conformance suite expansion.** Grow `conformance/behavior/` from
   26 to ≥300 cases covering every Frozen and Stable surface — every
   built-in effect, every capability transition, every CRDT merge law,
   every supervisor restart strategy, every effect-handler resume
   shape. This is the executable spec that a second implementation
   would target.
6. **Doc-example verification.** `scripts/verify_doc_examples.sh` runs
   every code block in `docs/`, `README.md`, `SPEC2.md`, and every
   `///` doc comment. A doc block that doesn't compile+run fails CI.
7. **Structured error quality pass.** Every `NuError` variant carries
   `expected`/`found`/`suggestion` per the recent structured-errors
   work, verified by test. No error contains the phrase
   "not yet supported" — those become their own error variant with a
   documented workaround.
8. **Persistence recovery correctness.** DST-driven test: kill an
   event-sourced entity mid-journal, restart, assert state equals a
   from-scratch reconstruction. Repeat for every `StateModel`.

**Acceptance.**
- Differential fuzzing 0 divergences over 10⁶ seeds.
- DST 0 invariant violations over 10⁵ seeds spanning all cluster
  topologies.
- 300 conformance cases pass on the current runtime.
- Benchmark dashboard live; every claim in `PERFORMANCE_ANALYSIS.md`
  is either measured or removed.
- Version bump: 0.2.0 (crate), language version unchanged.

**Non-goals.** New language surface. Ecosystem work.

**Delegable to.** Fuzzing specialist (bullet 1). Distributed-systems
tester (bullets 2, 4). Docs contributor (bullets 5, 6). Steward retains
bullets 3, 7, 8.

**Kill criteria.** If differential fuzzing surfaces a Frozen-tier bug
(bytecode divergence, wire-format divergence, value-layout divergence),
freeze all new work and treat it as a Sev-1 until fixed. This is the
whole point of the Frozen tier.

---

## Phase 2 — Prove It Works (weeks 12–24)

**Goal.** The language withstands adversarial correctness review. The
runtime withstands adversarial operational review. Both hold up as
"actually production-grade" not "compiled and ran."

**Deliverables.**

1. **Formal semantics completion.** Prove the theorems that already have
   definitions in `spec/formal/`:
   - `capabilities.lean`: `cap_sendable` (only `val`/`tag` cross actor
     boundaries), `linear_iso_at_most_once`.
   - `effects.lean`: `effect_safety` (closed row `{}` cannot perform an
     unhandled effect), progress+preservation for handler dispatch.
   - `combined.lean`: type + capability + effect judgment soundness.
   - CI gate on `lake build` blocks any PR that touches
     `src/typechecker.rs`, `src/effect_checker.rs`, or `src/types.rs`
     without a corresponding Lean update or an explicit `@sorry_ok`
     annotation reviewed by the steward.
2. **LinearIso must-use enforcement.** Upgrade the at-most-once check
   in `CapabilityAnalyzer` (`src/effect_checker.rs`) to exactly-once
   with a proof. The Lean statement is the source of truth.
3. **Backend-trait completion (RFC 0003 item 6 full wiring).** Route
   `src/jit/`, `src/mir_wasm.rs`, `src/wasm_runtime.rs`, and
   `src/python/` behind the traits already defined in `src/backends/`.
   Core language crate imports zero of `cranelift`, `wasmtime`,
   `pyo3`, `libsql`, `quinn`, `rustls`, `reqwest`. Enforced by
   `verify_implementation.py`.
4. **Runtime god-object completion (RFC 0003 item 10 full).** Extract
   `Scheduler`, `GcCoordinator`, `SupervisorTree`, `PersistenceLayer`,
   and `Cluster` from `src/runtime/mod.rs` into standalone structs
   owned by `Runtime`. Each behind its own trait. Enables independent
   evolution and independent test harnesses.
5. **Windows support.** Port build.rs (currently Fedora-specific
   Python symlink), test the mimalloc + Cranelift path, verify JIT
   symbol linking on MSVC. Add `windows-latest` to the CI matrix.
6. **Release binaries.** GitHub Releases workflow (`release.yml` is
   already scaffolded) produces:
   - `nulang-linux-x86_64`, `nulang-linux-aarch64`.
   - `nulang-macos-x86_64`, `nulang-macos-aarch64`.
   - `nulang-windows-x86_64`.
   Each binary passes the full conformance suite on its target
   platform. SHA-256 sums signed with a project key.
7. **Language server hardening.** Every LSP feature has integration
   tests via `tower-lsp`'s test harness. `cargo run -- --lsp` runs
   for 24 hours against a large `.nula` corpus without leaking
   memory (checked with `heaptrack`).
8. **Dep audit and reduction.** 483 transitive deps → target ≤300.
   Candidates for removal or replacement: `httparse` + `ureq`
   (unify), `libsql` (evaluate against a bytecode-only journal
   format), `rustyline`'s feature surface, `tracing-subscriber`
   heavy features. Every dep gets a "why we depend on this" line in
   `SPEC2.md` §Implementation Status.

**Acceptance.**
- All Frozen and Stable theorems proved in Lean, 0 sorries.
- Backend traits fully wired; core crate deps audit-clean.
- Windows CI green.
- Release v0.3.0 tagged with signed binaries for 5 targets.

**Non-goals.** Bootstrap self-hosting (Phase 3). Package registry
(Phase 3).

**Delegable to.** Formal methods contributor (bullets 1, 2). Rust
platform engineer (bullets 3, 4, 5). Release engineering (bullet 6).
LSP maintainer (bullet 7). Steward retains bullet 8.

**Kill criteria.** If the Lean proofs surface a soundness bug in the
current implementation, freeze the language version and issue a patch
release before continuing. If Windows support turns out to need >4
weeks (Cranelift/PyO3 quirks), split Windows into its own phase.

---

## Phase 3 — Longevity Foundation (weeks 24–52)

**Goal.** Nulang's 200-year story is defensible. The frozen formats have
a self-hosting compiler that emits them. There is a path to a second
implementation. Content-addressed dependencies actually work.

**Deliverables.**

1. **Bootstrap self-hosting.** Advance `bootstrap/compiler_core.nula`
   from Stage 13 to self-compilation. Milestones:
   - Stage 14: module-level parsing (multiple `fn` definitions).
   - Stage 15: multi-binding closure capture via `CapStore`/`CapLoad`.
   - Stage 16: HM inference sufficient for the compiler's own source.
   - Stage 17: type ascription syntax.
   - Stage 18: `compiler_core.nula` compiles itself; byte-identical
     output from stage-N+1 and stage-N+2 (fixpoint reached).
   - Verified in CI: `nulang bootstrap/compiler_core.nula < bootstrap/self.nula`
     produces `.nbc` byte-identical to `cargo run -- bootstrap/self.nula`.
2. **Package registry.** Minimum-viable, boring, static-file registry:
   - Host `.nbc` artifacts + `Nulang.toml` manifests on a git-backed
     store (GitHub Pages or Cloudflare R2).
   - Content-addressed by BLAKE3 (RFC 0003 item 11 already ships the
     lockfile hashing).
   - `nula publish`, `nula add <name>` (no path/git required).
   - Namespace ownership by TXT-record verification, transferrable.
   - Rate limits and moderation on the registry index only; content
     is immutable and CDN-cacheable.
3. **Second implementation seed.** The bootstrap compiler *is* the
   second implementation for the Core fragment. Beyond that, publish
   a Written Rules of Engagement for a second implementation: which
   parts of `SPEC2.md` are non-negotiable, which are hints, how a
   competing implementation registers as conforming (passes
   `conformance/`).
4. **RFC 0010 keyword audit follow-through.** Wire the remaining
   reserved keywords (`monitor`, `link`, `exit`, `await`) with RFCs
   for each, or remove them from the lexer. `await` is the one that
   matters if async/await is really the future direction.
5. **Escape analysis or region inference.** Reintroduce
   `src/escape_analysis.rs` (the earlier version was reverted, see
   `PERFORMANCE_ANALYSIS.md` row 2.4). Goal: statically prove
   stack-allocation for containers that never leave a function. Wire
   into the JIT tier so hot loops with local records/arrays never hit
   the heap. Measure via the Phase 1 bench dashboard.
6. **CRDT op-based replication (CmRDT).** Delta-state ships in 1.0.0;
   op-based is the missing complement per `PERFORMANCE_ANALYSIS.md`
   row 3.2. Ship `Packet::CrdtOp` alongside `CrdtDeltaSync`. Provides
   the lowest-bandwidth sync path.
7. **Deprecation cycle graduations.** Per `GOVERNANCE.md` §6, the
   deprecated surfaces from 1.0.0-frozen (LLM effect, `LlmAsk` opcode,
   `Pipeline`/`Supervisor`/`Debate` in-language modules) either move
   out of the language surface into `nulang-ai` stdlib or graduate
   to real removal. Requires bytecode v1→v2 migration in
   `src/format/migrate.rs`.

**Acceptance.**
- Bootstrap compiler passes its own byte-identity test.
- Registry live; 5 packages published by non-steward authors.
- Second-implementation ROE published.
- Language version bump: 2.0.0-frozen (if migrations were required)
  or 1.1.0-stable.

**Non-goals.** Wide adoption push (Phase 4). New backends. Perf work
outside escape analysis.

**Delegable to.** Language implementer for bootstrap (multi-month
effort by one specialist). Ops/infra for registry. Steward retains
bullets 3, 4, 7.

**Kill criteria.** If self-hosting surfaces a fundamental gap in Core
(e.g. it needs features currently outside Core), that gap is an RFC to
extend Core, gated by the steward, not a workaround in the bootstrap.
If it takes >6 months, ship what works and defer self-compilation to
Phase 4.

---

## Phase 4 — Ecosystem and Adoption (weeks 24–52+, parallel with Phase 3)

**Goal.** Nulang has users the maintainers don't personally know. It
has a killer application demonstrating a category it wins.

**Deliverables.**

1. **Reference application.** One production-quality application in
   `examples/` or a sibling repo that demonstrates what Nulang does
   better than anything else. Candidate: a distributed, durable,
   supervised AI-agent orchestrator (leverages `entity`, `workflow`,
   `Inference.ask`, supervision, CRDTs, persistence — all the parts
   no other language has together). Alternatives:
   - A distributed KV with per-key CRDT choice.
   - A fault-tolerant IoT ingester with location-transparent routing.
   Chosen application ships as a runnable demo, a blog post, and a
   `nula run` one-liner.
2. **Documentation completeness.**
   - `docs/TUTORIAL.md` verified end-to-end by CI.
   - `docs/PITFALLS.md` extended from lessons learned in Phases 0-2.
   - Book-length treatment (`docs/book/`) covering: type system,
     effects, capabilities, actor model, distribution, persistence,
     AI runtime, WASM, FFI. Deliverable: `mdBook` output published
     to `docs.nulang.org`.
   - Migration guides: "coming from Erlang", "coming from Rust",
     "coming from Elixir". Each with a translated non-trivial
     example.
3. **First external user.** Actively pursue one. This is a
   relationship-building task, not a technical task, and the steward
   owns it. Success looks like: an outside team runs Nulang in
   production (broadly defined — even internal tools count) and files
   at least one bug the steward didn't already know about.
4. **Community infrastructure.**
   - Discord/Zulip/Matrix (one, not three).
   - Weekly office hours for the first 3 months.
   - Public roadmap on GitHub Projects mirroring this file.
   - Contribution guide with the RFC process front-loaded.
   - Code of conduct.
5. **VS Code extension published to marketplace.** The
   `.vscode/extension.js` scaffold exists; publish it. Same for a
   Zed extension and a Neovim plugin.
6. **`nula` template library.** `nula new --template <name>` currently
   supports 4 templates. Add: `--template distributed` (multi-node
   sample), `--template ai-agent` (agent + Inference), `--template
   web` (HTTP server + JSON), `--template cli` (already exists).
7. **Speaking + writing.** One conference talk (Strange Loop, Papers
   We Love, LambdaConf), one long-form technical post per quarter
   (JIT internals, capability system, DST harness, formal semantics).

**Acceptance.**
- One non-steward production user (with permission to name them).
- Reference app: 1000+ GitHub stars or equivalent traction signal.
- Book published; tutorial verified.
- 3+ merged PRs from non-steward contributors.

**Non-goals.** Chasing hype. Framework proliferation. Adding surface
to appear more-featured.

**Delegable to.** Docs writer (bullet 2). DevRel-shaped contributor
(bullets 4, 7). Steward owns bullets 1, 3, 5, 6 initially.

**Kill criteria.** If after Phase 3 the reference application has no
traction, re-evaluate the pitch. Some 200-year languages find their
niche late; that is fine, but requires honesty about the current pitch
not landing.

---

## Cross-cutting workstreams (continuous)

- **Governance discipline.** Every Frozen/Stable change is an RFC.
  Every RFC has a Lean update if the theorems touch. Every accepted
  RFC has a conformance case.
- **Security.** `cargo audit` runs in CI (already scheduled). Add
  fuzzing corpus artifacts to CI (persist failing seeds). Publish a
  security policy (`SECURITY.md`) with a disclosure email.
- **Dependency governance.** Every new direct dep requires a PR
  comment justifying it against the small-binary principle.
- **Docs stay live.** `scripts/verify_doc_examples.sh` runs on every
  PR; docs drift is a blocker, not a warning.

## Risk register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Steward bottleneck | High | High | Every phase names delegable work; hire/recruit specialists per phase |
| Formal proofs surface soundness bug in impl | Medium | Very High | Ship as Sev-1 patch; the Frozen tier is precisely for this |
| Bootstrap takes >6 months | Medium | Medium | Ship staged partial results; defer self-compile fixpoint if needed |
| WASM/AOT can't be finished usefully | Medium | Low | Truth-in-advertising phase already downgrades them |
| Package registry becomes moderation nightmare | Medium | Medium | Namespace verification via DNS; immutable content; no editorial voice |
| Cranelift API breakage on Rust upgrade | Low | Medium | Backend trait boundary (Phase 2 bullet 3) isolates the risk |
| No external user materializes | High | Very High | The reference application is the mitigation; if that doesn't land, re-evaluate pitch |
| Feature creep from AI-runtime enthusiasm | Medium | High | Language surface stays actor + effects + capabilities; AI stays in `nulang-ai` |

## Version + tier progression

| Version | Milestone | Trigger |
|---|---|---|
| 0.1.0 | current alpha | shipped |
| 0.2.0 | Phase 0+1 complete | truth-in-advertising + correctness floor |
| 0.3.0 | Phase 2 complete | proofs + Windows + release binaries |
| 1.1.0-stable | Phase 3 partial | bootstrap fixpoint + registry live |
| 2.0.0-frozen | Phase 3 complete | deprecation cycle graduations require major bump |

Language version moves only per `GOVERNANCE.md` §5. Crate version
revs freely.

---

## What this plan is not

- Not a wish list. Every item cites the file it modifies or the RFC it
  implements.
- Not a hiring plan. It scales to one steward + rotating specialists.
- Not a fundraise deck. The 200-year framing is a design constraint,
  not a valuation.
- Not immutable. Kill criteria and re-plan triggers are load-bearing.

## What this plan is

An honest sequence of the work between an alpha language with excellent
bones and a language a serious team would trust in production. The
sequencing is: stop lying, start proving, self-host, ship users. Every
phase before the last one is defensive work — the goal is that when the
first external user does show up, nothing they touch is a stub.
