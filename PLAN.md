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

## Phase 0 — Truth-in-Advertising (weeks 0–4)

**Goal.** Every claim in the repo is either implemented, gated behind an
Experimental warning, or removed. No user hits an unimplemented path on
a documented flag.

**Deliverables.**

1. **Restrict advertised backends to what they support.**
   - `--backend native` errors early with a single message listing what
     is supported (pure functional Core) and what is not (effects, actors,
     distribution). Alternatively, gate the flag behind
     `--features aot-experimental` and remove it from `--help` in default
     builds. Decision: gate + warn.
   - Same treatment for `--backend wasm-run` and `--backend wasm-aot`
     while `host_read`/`host_dispatch` remain stubs. Ship a runtime
     warning on first `perform` of a non-IO effect.
2. **Interpreter opcode gap closure.** For each of the 7 opcodes in
   `src/vm.rs:4553-4630`, either implement or prove-and-assert
   unreachable from every codegen path (`mir_codegen`, `hir_lower`,
   `bootstrap/`, `nula` package manager). Track in `verify_implementation.py`
   as a hard gate.
3. **Formatter completeness.** `src/fmt.rs` handles `workflow`, `agent`,
   `let`, `class`, `impl`. Success criterion: `cargo run -- --format`
   is idempotent on every file under `examples/` and every generated
   `nula new` template.
4. **`opaque` nominal types.** Either activate the check
   (`src/typechecker.rs:274-284`) or remove the keyword from the lexer
   and mark it reserved in `GOVERNANCE.md` §2a per the keyword lifecycle
   rules. Decision: activate; the keyword is already in Stable-adjacent
   documentation.
5. **Dead feature flags.** Delete `simd-experimental` and
   `quic-experimental` from `Cargo.toml`, or replace them with
   real implementations. Decision: delete now, re-introduce with the
   real code later. Net −2700 lines per `PERFORMANCE_ANALYSIS.md`.
6. **README/LAUNCH audit.** Rewrite feature-highlights so every bullet
   maps to a passing conformance case. No bullet without a citation.
7. **Release checklist enforcement.** Wire `RELEASE_CHECKLIST.md` into
   `verify_implementation.py` as a machine-checked pre-flight. A
   release is not tagged until every box is green.

**Acceptance.**
- `cargo test` green including 7 new opcode tests or 7 new "unreachable
  from codegen" static assertions.
- `verify_implementation.py` exits 0 with the new gates.
- `README.md` diff: every removed bullet or downgraded bullet has a
  linked issue for restoration.

**Non-goals.** New language features. New backends. Performance work.

**Delegable to.** One contributor per bullet 1/2/3/5; the audit (bullets
6, 7) is the steward's.

**Kill criteria.** If any bullet takes >2× its estimate, land the
downgrade (remove the surface) rather than the fix, and open an RFC for
the restoration. This phase must complete on schedule.

---

## Phase 1 — Correctness Floor (weeks 4–12)

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
   52 to ≥300 cases covering every Frozen and Stable surface — every
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
