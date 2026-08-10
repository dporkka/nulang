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

Of the axes named in Scope above, the distributed actor runtime is where
the design has the most unclaimed advantage and the least proof. No
competing system ships CRDTs, capability-typed messages, and algebraic
effects as first-class actor-state primitives together; Erlang/OTP's
default distribution hits a 60-200 node full-mesh ceiling before requiring
a third-party topology library (Partisan), Akka/Pekko's split-brain
resolver and cluster sharding are mature but bolted onto a JVM actor model
with no CRDT or capability story, and Orleans' virtual actors get
automatic placement and distributed transactions but no CRDTs and no
capability-checked messaging. Nulang's distributed runtime today is a real
TCP/gossip mesh with genuine delta-state CRDT sync and full local
Erlang-style supervision — but it is unauthenticated by default, has no
split-brain handling, does not supervise or fail over across nodes, and
leaks CRDT tombstones forever. Phase 5 below treats closing that gap as
the primary differentiating ambition, not one item competing for attention
among Phases 1-4's broader production-readiness work.

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
| Direct deps | 72 |
| Transitive deps | 468 (was 504 before a 2026-08-02 libsql feature trim dropped the unused tonic/axum gRPC stack; was documented as 483, already stale before that) |
| Formal proofs (Lean 4) | Core type soundness NOT proved (`progress`/`preservation`/`type_soundness` all `sorry`, regressed 2026-07-26, undocumented until 2026-08-02); capability lattice genuinely proved (5/6 theorems); effects are vacuous `True` stubs, not proofs |
| Conformance suite | 52 behavior cases + grammar cases |
| Bootstrap self-hosting | Stage 13; not yet self-compiling |
| Benchmarks | `benches/` uses criterion (7 files, 404 lines); no CI regression tracking |
| DST | `src/dst.rs` seed present (265 lines); not integrated into CI |
| Fuzzer | `src/fuzz.rs` present (412 lines); runs in `cargo test` |
| Shipped release binaries | None in repo evidence |
| External users | None known |
| Distributed cluster ceiling | full-mesh heartbeats + TCP connections (O(N) per node, O(N²) cluster-wide); gossip membership payload capped at 256 entries — practical ceiling in the tens of nodes, the same class of limit Erlang's default distribution hits before requiring Partisan |
| Cluster transport security | plaintext, unauthenticated by default; `TlsConfig::SelfSigned` exists (`src/runtime/network.rs`) but zero call sites in the entire codebase ever construct one — `enable_distribution` is only ever called with `tls_config: None` (sole caller: `src/runtime/tests.rs:2961`) |

Unfinished implementation lines counted from `not yet implemented` /
`not yet supported` markers in `src/`:

- `src/vm.rs:4553-4630` — 7 interpreter opcodes trap (`ConstL`, `Pop`,
  `Switch`, `Alloc`, `TupleL`, `Unpack`, `Copy`).
- `src/aot/codegen.rs:797-1108` — ~15 MIR constructs unsupported in the
  AOT native backend (all effects, actors, spawn, send, ask, receive,
  FFI, state, capability check).
- `src/mir_wasm.rs:322`, `src/wasm_runtime.rs:149,187` — WASM handler
  emission is a nil-drop; `host_read` and `host_dispatch` are stubs.
- `src/fmt.rs` — formatter now covers every `Decl`/`Expr` construct
  (workflow, agent, class, impl, let-binding, given, effect, module, import,
  extern, database, crdt, state_machine, named handler, record type,
  spawn/handle/receive/emit/migrate/cap-annotate/type-annotate), round-trips
  idempotently; 9 unit tests (2026-08-09).
- `src/typechecker.rs:274-284` — `opaque` nominal types are transparent.
- `Cargo.toml` — `simd-experimental` feature removed (zero code references); `quic-experimental` removed 2026-08-05 (unwired, incompatible handshake, tokio runtime overhead).
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

**Current state (in progress, verified 2026-08-02):** 4/8 deliverables
done (fuzzer maturation, benchmark harness including regression
gating, doc-example verification extended), 3/8 substantially wired
this extended session (DST, persistence recovery, one real chaos-suite
test), bullet 5 at 239/300, bullet 7 partially addressed (phrase
cleanup, a unit-test verification layer for existing structured-error
fields, plus two arity-mismatch construction sites converted from
hollow to populated). 9 real runtime/tooling bugs found and fixed
(one, RFC 0008 migration contracts, was a false Stable-tier claim —
corrected in SPEC2.md), 12 more found and documented for follow-up
(including a real compiler SIGABRT on large functions and two
behavior-dispatch surprises), plus a dozen SPEC2.md/GOVERNANCE.md/
CHANGELOG.md truth-in-advertising corrections. 42 commits this
session.
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
- **[X] Bullet 3 (benchmark harness) — done, including regression
  gating.** `benches/*.rs` was fixed to actually compile and run (they
  didn't before — see `8636b01`). CI runs `cargo bench` on every push
  to `main`, collects results via `scripts/collect_bench_results.py`
  (verified against real criterion 0.5.x output, not just the
  documented schema), commits them to `benchmarks/`, and now also runs
  `scripts/check_bench_regression.py` against them. GitHub Actions'
  shared runners have commonly-cited 20-50%+ run-to-run noise on
  wall-clock benchmarks, so this deliberately isn't the `>5%` flat
  threshold this bullet originally called for (that would fail pushes
  on noise, not regressions, which is worse than no gate) — instead
  each benchmark's own median + 6×MAD across a rolling 10-commit
  window sets its threshold (floored at 20%), with a 3-prior-sample
  minimum before a benchmark is gated at all. Verified against
  synthetic history covering a genuine 2× regression (correctly
  flagged), a value within a noisy benchmark's normal historical
  spread (correctly not flagged), and both sparse-history cases
  (correctly skipped rather than false-positiving). See
  `benchmarks/README.md` for the full methodology.
- **[~] Bullet 5 (conformance suite expansion) — 26 → 239 of 300
  target.** Corrected from this doc's original "52" (that was a file
  count — `.nula`+`.json` pairs — not a case count; the actual starting
  case count was 26). Five waves of parallel agents:
  - Wave 1 (7 agents): capabilities, effect-handler resume, effect rows,
    actor messaging/supervision, CRDT merge laws, pattern
    matching/error handling, persistence/event sourcing → +87 cases.
  - Wave 2 (4 agents): built-in effects inventory, distributed-messaging
    single-node behavior, stdlib collections/string (the real, working
    subset — see the bug list below), workflow steps/conditionals/
    parallel/sagas → +43 cases, +2 direct regression cases.
  - Wave 3 (3 agents): JSON serialization (the real stdlib/json.nula
    API, quite different from SPEC2.md's description — corrected),
    HTTP client/server (proved client+server can't coexist in one
    process rather than asserting it; corrected SPEC2.md's API
    description too), concurrency primitives (scheduler FIFO/priority
    ordering, IEEE-754 float determinism including exact tie-break
    cases, and the real nil-collapse/epsilon-equality boundary
    semantics) → +17 cases.
  - Wave 4 (3 agents): generics (§7.8), imports/modules (§7.6-7.7),
    visibility (§7.9), and the Phase-4-experimental typeclass surface
    — zero prior coverage on any of these → +32 cases, plus 3 more
    targeted at the structured-error diagnostic fields (bullet 7).
    Surfaced a compiler crash on sibling same-named module functions
    (fixed) and a runtime crash on constrained generic typeclass
    dispatch (documented).
  - Wave 5 (3 agents): two Stable-tier claims with zero prior coverage
    (RFC 0008 migration contracts, RFC 0009 organization primitives),
    plus MIR register spilling and actor lifecycle edges → +28 cases.
    The most severe finding of this session: RFC 0008's "Stable" tag
    was false — migration contracts parse and shallow-typecheck but
    are functionally inert, no trigger mechanism exists anywhere in
    the runtime (corrected in SPEC2.md). Also found a real compiler
    SIGABRT (stack overflow) on ~286+-statement functions, and two
    behavior-dispatch surprises (unknown behavior names silently run
    behavior 0; same-named behaviors across different actor types can
    collide) — both documented, not fixed given the blast radius
    (`send_message` is called pervasively).
  Every value captured from the real compiled binary, never guessed.
  Still short of 300 — the remaining ~61 would need to cover
  progressively narrower surfaces (the AI runtime's `agent`/Pipeline/
  Supervisor/Debate declarations need a mock LLM provider the CLI
  binary doesn't expose, so that surface is appropriately tested at
  the Rust integration-test layer instead — not reachable by
  CLI-driven conformance cases at all, not a coverage gap in the same
  sense as the others).
- **Real bugs found and FIXED this session (not from a numbered
  bullet, but squarely "correctness floor" — every one of these was
  found by an agent whose actual assignment was writing conformance
  cases, verified independently before fixing, and pinned by a
  regression case or test):**
  1. **Top-level heap allocation silently failed for any actor-using
     program** (`7215088`, `ef1f451`). `RuntimeVmCallbacks` allocated
     new heap strings via `Runtime.vm.allocate_string` — a separate,
     lazily-created VM instance whose heap `main()`'s own top-level VM
     can't read back from — and `alloc`/`drop_ref`/`retain_ref`
     returned `None`/no-op whenever there was no *current actor*, which
     is always true at the top level. Confirmed against a real shipped
     example (`examples/supervisor_tree.nula` printed literal `nil`).
     Fixed with a dedicated `Runtime::main_heap`/`main_gc` fallback.
  2. **Importing two stdlib modules with a same-named function crashed
     the compiler** (`b3d6a2f`). `import stdlib::map` + `import
     stdlib::set` (both export `empty`/`contains`/`remove`/...) produced
     two same-named top-level functions with no collision check, which
     MIR's function-slot allocator can't handle — it failed deep in
     codegen with `internal: MIR function slot 0 left unfilled`. Fixed
     by detecting the collision at import-resolution time with a clear,
     actionable error instead.
  3. **Workflow-only programs never activated the actor runtime**
     (`2057900`). `main.rs`'s actor-detection matched `Decl::Actor`/
     `Decl::StateMachine` but not `Decl::Workflow`, so a program with
     only a `workflow` declaration ran on the stub-only standalone VM —
     every step silently never ran, no error. One-line fix.
  4. **LSP hover/autocomplete advertised effects that don't work**
     (`6d27037`). `STM`/`Async`/`Cost` shown with full example syntax
     despite zero implementation; `Net`/`Rand` didn't match their real
     names (`Http`/`Random`); `Spawn`/`Send`/`Receive`/`Migrate` shown
     as `perform`-able effects when they're actually keywords/opcodes
     with a parse error on that syntax. Removed the non-functional
     entries, corrected the misnamed ones.
  5. **Two sibling `module { }` blocks declaring a same-named function
     crashed the compiler** (`5e9430c`, extended session). Nested
     modules are purely a flattening/namespacing construct — `module
     Alpha { fn value() {..} } module Beta { fn value() {..} }` both
     land in the same flat `func_map`, and the second registration
     silently overwrote the first's slot mapping, leaving the
     first-reserved MIR function slot permanently unfilled: `internal:
     MIR function slot 0 left unfilled`. Same root-cause *shape* as bug
     2 above (silent name collision surfacing as an internal-error
     symptom far from the real cause) but a different code path
     (`mir_lower.rs`'s `reserve_decl`, not the resolver's import
     merging). Fixed with the same pattern: detect the collision where
     it happens, name it in the error.
  6. **Recovered actors silently lost per-field persistence-model
     tracking, breaking a second crash/recovery cycle** (`03ed058`,
     extended session). `Runtime::recover_actor` built a bare
     `Actor::new()` and never restored `Actor.state_models` (the
     `local`/`durable`/`event_sourced`/`crdt` map per field) — every
     field silently reverted to `local` after one recovery, meaning a
     *second* crash would have dropped `durable` fields from the
     snapshot entirely and stopped `event_sourced` fields from
     accumulating. Fixed by restoring `state_models` from the recovery
     module's `actor_metadata` alongside `bytecode_module`/
     `bytecode_offsets`. Verified with a new two-cycle recovery test.
- **Real bugs found and DOCUMENTED, not fixed this session (tracked
  for follow-up, all in `SPEC2.md` with full evidence):**
  1. `ask remote`/distributed `RAsk` returns the wrong value (the
     target's own actor reference) from a register-write mismatch
     between the local and remote `Ask` opcodes.
  2. `send remote`/`ask remote` silently drop their message single-node
     instead of using the local-delivery fallback that already exists
     and works for other distributed paths — just isn't wired to these.
  3. Saga compensation indexes by whole-module declaration order, not
     the workflow's own steps — another `actor` declared before the
     `workflow` silently shifts which step's compensation runs.
  4. Single-argument `perform Timer.sleep(ms)` suspends a step and
     never resumes it — a permanent hang, not an error. Only the
     two-argument durable form works.
  5. `event_sourced` field reconstruction during recovery is a bare
     count of persisted events, never running the field's `apply`
     handler against the event's args — correct for a plain counter,
     silently wrong for any field with a non-trivial `apply` handler
     (extended session; see bug 6 above's neighbor finding).
  6. A constrained generic function using a typeclass bound on a
     type-variable receiver type-checks but crashes at runtime ("Not a
     function: nil") — the dictionary-passing transform only resolves
     literal receivers (extended session, `typeclass_06`).
  7. Recursive generic ADTs cannot be constructed — SPEC2.md §7.8's own
     `Tree[T]` example fails to type-check its own constructor call
     (extended session, `generics_03`/`07`).
  8. Generic function type parameters are not skolemized in the
     function body, so an internally-inconsistent generic (e.g. a body
     that only ever produces `Int` for a declared `T`) is accepted at
     the declaration and only fails later at a mismatched call site
     (extended session, `generics_08`).
  Also corrected: `SPEC2.md` §4.6 (built-in effects table — see bug 4
  above), §12.4 (distributed message routing — see bugs 1-2 above, plus
  a separate correction: `monitor`/`link`/`exit` were undersold as
  "planned" when they're fully implemented and conformance-tested),
  Chapter 10 (workflow known-issues list — see bugs 3-4 above, plus a
  deprecation note per RFC 0004), Chapter 14 examples (stdlib argument
  order/naming mismatches found by the `StdlibCollectionsString` wave;
  Chapter 14 was already headed "— Planned" so this didn't need the
  same "Stable-tier false claim" severity of fix CRDT got).
- **[~] Bullet 6 (doc-example verification) — extended, not fully
  closed.** `scripts/verify_doc_examples.sh` only ever scanned the Astro
  docs site; now also scans `SPEC2.md`/`README.md`/
  `docs/GETTING_STARTED.md`/`docs/TUTORIAL.md`, gated behind
  `NULANG_DOC_VERIFY_INCLUDE_ROOT=1` rather than the default (and
  therefore CI-blocking) invocation — turning it on by default today
  would fail CI on 58 pre-existing SPEC2.md example fragments that
  reference prose context rather than being self-contained, a separate,
  larger gap (smarter fragment heuristic, or rewriting the examples)
  left for follow-up. Not done: the `///` doc-comment coverage this
  bullet also calls for (needs a Rust-source-aware extractor, not a
  markdown-fence scanner).
- **[~] Bullet 7 (structured error quality) — phrase cleanup done,
  "verified by test" now landed for the fields that already existed,
  the "every variant" structural ask still isn't.** Removed the "not
  yet supported" phrase from the two places it appeared in user-facing
  errors (`resolver.rs`'s new duplicate-import error, and 17
  uniformly-templated messages in `aot/codegen.rs`) — same actionable
  content, different wording. The fuller ask (every `NuError` variant
  carrying `expected`/`found`/`suggestion`, verified by test) is
  partially already in place: `ParseError`/`TypeError`/`EffectError`
  already carry rich fields (`expected`/`found`, `expected_type`/
  `found_type`/`similar_names`, `missing_effects`/`allowed_effects`)
  and their constructor helpers already populate them correctly at
  real call sites — verified this extended session (`type_mismatch`,
  `unbound_variable`, `missing_effects`, `parse_unexpected` each got a
  unit test asserting the field population, plus 3 conformance cases
  proving the same fields reach real compiled-binary stderr output,
  prior test only checked `is_err()`). Also converted two hollow
  construction sites (`typechecker.rs`'s function-call and
  emit-event arity-mismatch errors, both previously built via the
  hollow `type_error(msg, span)` helper despite having the exact
  counts on hand) to populate `expected_type`/`found_type` with
  correctly-pluralized descriptions ("2 arguments" / "1 argument") —
  2 more conformance cases plus a Rust integration test lock this in,
  and 3 pre-existing conformance cases whose stderr assertions
  depended on the old bare-number wording were updated after verifying
  the new wording against the real compiled binary. Not done:
  `LexError`/`FFIError`/`RuntimeError`/`VMError`/`PythonError`/
  `PackageError` still carry only `{msg, span}` — extending structured
  fields to these is lower-value (they're inherently dynamic/
  message-driven, without a natural "expected vs found" shape) but
  still unaddressed; no variant has a field literally named
  `suggestion` (the closest equivalents are `similar_names`/
  `explanation`, which already do the job under a different name); and
  the remaining `type_error`/`cap_error`/`effect_error`/`parse_error`
  hollow-helper call sites throughout the rest of the codebase are
  still hollow — this session fixed the two highest-traffic ones
  (general call arity, event arity), not an exhaustive sweep.
- **[~] Bullet 2 (DST) — single-node message-passing wired, cluster/timer
  determinism not started.** `src/dst.rs` was not even part of the
  compiled crate (`mod dst;` was missing from `src/lib.rs` — its 4
  tests had never run). Fixed that first, then added
  `Runtime::run_scheduler_deterministic`/`pick_ready_actor_deterministic`:
  actor selection driven by a seeded RNG over the sorted ready-set,
  reusing `step_actor` unchanged (same VM/GC/persistence machinery the
  production scheduler drives). Scope, by design: pure message-passing
  determinism only — does not drive the timer wheel, cross-shard
  messages, or LLM completions, all of which key off wall-clock reads.
  3 new tests verify same-seed same-sequence selection, real
  quiescence with correct final actor state, and step-limit-exceeded
  reporting for a run that hasn't settled.
- **[~] Bullet 4 (chaos suite) — one real test landed, not the full
  target.** `test_three_node_cluster_survives_hard_node_failure_and_rejoin`
  (`src/runtime/tests.rs`, extended session) drives 3 real `Runtime`
  instances over real loopback TCP: kills a node's transport hard (no
  graceful leave), confirms the survivors detect the failure via the
  real heartbeat-timeout/suspicion state machine, confirms they keep
  doing real cross-node work together (not just membership-table
  bookkeeping), then confirms a fresh node can rejoin. Investigated
  wiring `Runtime::install_virtual_clock`/`advance_time` to avoid the
  ~9s of real wall-clock waiting this needs, but found `ClusterState`
  keeps its own, separate `clock` field, unlinked from
  `Runtime.virtual_clock` — genuine determinism needs per-node
  `VirtualClock` instances kept in manual sync, left as follow-up.
  Not done: 5-node topologies, split-brain (mutually-invisible healthy
  sub-clusters, not one node dying), asymmetric partition, rolling
  restart of every node, and running any of this across many seeds in
  CI — the 10³-seeds-per-commit target needs the above determinism
  work first (a real-TCP, real-wall-clock test can't scale to that).
- **[~] Bullet 8 (persistence recovery correctness) — one real bug
  found and fixed, one real gap found and documented, not the full
  "repeat for every StateModel" sweep.** `Runtime::recover_actor` never
  restored `Actor.state_models` on the rebuilt actor, so every field
  silently reverted to `Local` after one recovery — a second crash
  would have dropped `durable` fields from the snapshot entirely.
  Fixed and verified with a new two-cycle recovery test. Separately,
  and NOT fixed: `event_sourced` field reconstruction during recovery
  is a bare count of persisted events, never running the field's
  `apply` handler against the event's args — correct for a plain
  counter, silently wrong for any field with a non-trivial `apply`
  handler (verified against the real compiled binary: an `apply`-driven
  counter reaches 9 with no crash, only 6 with a crash-and-recover
  between the same two messages). Root cause is architectural — apply
  handlers are inlined at each `emit` call site at compile time, no
  addressable bytecode unit recovery could re-invoke — tracked as
  follow-up, documented in SPEC2.md §9.6 and pinned by a regression
  test. `local`/`crdt` StateModels not exercised this session (`local`
  is a straightforward reset-on-restart check; `crdt` persistence is
  already documented elsewhere as not wired to the eight CRDT types).

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
3. **Benchmark harness with regression tracking.** ✅ **Delivered.** Every
   criterion bench under `benches/` runs in CI on `main` pushes (not
   PRs — a full run of every group is too slow to gate every PR on);
   results are written to `benchmarks/` in the repo. Regression
   threshold is not a flat percentage (GitHub Actions' shared runners
   show 20-50%+ run-to-run noise, which a flat 5% cutoff can't survive)
   — `scripts/check_bench_regression.py` computes each benchmark's own
   median + 6×MAD across a rolling 10-commit window, floored at 20%, and
   requires ≥3 prior samples before gating a benchmark at all. Publishes
   measured numbers to replace the estimates in `PERFORMANCE_ANALYSIS.md`.
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

**Current state (in progress, verified 2026-08-02):** 8/8 scoping areas
investigated this session; 3 concrete deliverables landed, 5 scoped and
deferred (all multi-day/infrastructure-gated, not something a single
session can responsibly rush). 5 commits this session.
- **Bullet 1 (formal semantics) — documentation corrected, real proof
  work still open.** Discovered `types.lean`'s three headline theorems
  (`progress`, `preservation`, `type_soundness`) were silently regressed
  to `sorry` by a Lean 4.16.0 compatibility-fix commit (`ac9ef5d`,
  2026-07-26) three weeks before this session — that commit's own
  message honestly disclosed "12 sorry warnings", but no downstream doc
  (`spec/formal/README.md`, `SPEC2.md`, `CHANGELOG.md`, `PLAN.md`) was
  ever updated to match; all four falsely claimed "0 sorries"/"proved"
  until corrected this session. Root cause identified and documented: a
  concrete variable-capture/context-ordering subtlety in the `weakening`
  lemma's naive-induction proof strategy (see
  `spec/formal/README.md`'s regression note). Added a CI sorry-count
  ratchet (`.github/workflows/ci.yml`) so this exact silent-regression
  pattern can't recur — previously CI only ran `lake build`, which
  passes even with sorries. Actually re-proving the theorems is
  specialist Lean work or a fresh independent implementation; not
  attempted this session — genuinely hard, not "follow-up" spin.
- **Bullet 2 (LinearIso must-use) — partially landed.** Exactly-once
  (must-use) is now enforced for `let`-bound linear values, with a
  transparent-rebind exemption (`let a = x` doesn't carry a second
  obligation) verified against all 6 existing lineariso conformance
  cases plus 8 new unit tests. Parameter-level must-use (a linear value
  already in scope, e.g. a function argument) remains open.
- **Bullet 3 (backend traits) — verified already done, not a gap.**
  `src/backends/mod.rs`'s own header claims every trait
  (`JitBackend`/`WasmBackend`/`Transport`/`CryptoProvider`/
  `HttpProvider`/`ForeignInterop`) is "Wired"; spot-verified `VM` does
  genuinely hold `Option<Box<dyn JitBackend>>` and construct it through
  the trait. No work needed here.
- **Bullet 6 (release binaries) — verified already mostly done.**
  `.github/workflows/release.yml` builds 4 targets (Linux x86_64/
  aarch64, macOS x86_64/aarch64), strips binaries, SHA256-checksums,
  and publishes to GitHub Releases on tag push; `v0.1.0` is tagged.
  Gaps: no cryptographic code signing (checksums only), and a 5th
  target (Windows) is blocked on Windows support itself (bullet 5).
- **Bullet 7 (LSP hardening) — assessed, partial.** 38 unit tests give
  decent coverage of individual feature logic (inlay hints, completion,
  hover, workspace symbols, diagnostics). No protocol-level
  (`tower-lsp` test-harness) integration tests and no 24-hour soak test
  against a large corpus — both remain open, not attempted (the soak
  test specifically needs wall-clock time no single session has).
- **Bullet 8 (dependency audit) — real, verified progress.** Found
  `libsql`'s `default-features` pulled in `replication`+`sync`, which
  drag in the entire `tonic`/`axum`/`tower-http` gRPC stack for
  embedded-replica sync — a feature nothing in this codebase calls
  (verified: only `Builder::new_local`/`new_remote` are used, both
  covered by the much lighter `remote`/`core`/`tls` features). Trimmed
  accordingly: 504 → 468 transitive deps (-36 crates, tonic and axum
  now fully absent from `Cargo.lock`; incidentally also dropped 3
  windows-* crates that were only pulled in by the gRPC stack, despite
  Windows not being a supported target). Target is still ≤300; the
  remaining ~168 are mostly legitimate (Cranelift, Wasmtime, PyO3,
  libsql-core, tokio, tower-lsp) or ordinary cross-ecosystem version
  skew (34 duplicate package names at different major versions pinned
  by unrelated upstream crates — not fixable without replacing those
  upstream deps entirely, a much larger and riskier undertaking for
  marginal benefit).
- **Bullets 4 (runtime god-object) and 5 (Windows support) — partially
  landed, remainder scoped and deferred.** `src/runtime/mod.rs` started
  this session at 6447 lines (a real god-object); three same-day
  extractions (VM callback bridges, agent tool-calling, LLM dispatch — see
  bullet 4 below) cut it to 4314 lines (-33%). The remaining ~4000-line
  core scheduling/actor-stepping block is a materially higher-risk,
  multi-day refactor entangled with the `unsafe` ORCA GC and cross-shard
  concurrency invariants AGENTS.md flags as "do not break" — deferred as a
  unit, not rushed. A same-day (2026-08-03) follow-up spot-check of
  `recover_actor` as a possible narrower first cut found it shares the same
  hot-path primitives plus an unenforced `current_actor` reentrancy
  assumption, confirming no lower-risk subset exists (full write-up under
  bullet 4). Windows support confirmed at effectively 0% (2 mentions of
  "windows" in all of `src/`) — needs a transport-layer port, path-handling
  audit, and a second CI runner at minimum; a multi-week effort, not
  started.

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
   **Partial progress (2026-08-02):** exactly-once is now enforced for
   `let`-bound linear values (`Expr::Let`'s must-use check, with a
   transparent-rebind exemption for bare `let a = x` aliases — 8 new
   tests). Still open: function/lambda parameter-level must-use (a
   linear value already bound in the *initial* context, e.g. a
   parameter, is not yet checked), and the Lean proof itself
   (`linear_at_most_once` in `capabilities.lean` is still `sorry` —
   the Rust-side implementation moved ahead of the formal statement).
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
   **Partial progress (2026-08-02):** `mod.rs` was 6447 lines at
   session start; three extractions (free functions taking `&Runtime`/
   `&mut Runtime`, following the pattern already established by
   `workflow.rs`/`exit.rs`/`distribution.rs`/`spawn.rs`/`agent.rs`,
   not yet the full standalone-struct-behind-a-trait vision this
   bullet describes) brought it to 4314 lines (-33%): VM callback
   bridges into `callbacks.rs` (-1528 lines, a verbatim cut of one
   contiguous, self-contained block), agent tool-calling into
   `agent.rs` (-284), and LLM dispatch/retry/suspend into `llm.rs`
   (-381, including the most delicate function moved so far —
   `resume_suspended_llm_step`'s raw-pointer VM-callback reinstallation,
   verified not to disturb the `vm_exec_begin`/`vm_exec_end`
   receive-wait-wake deferral invariant AGENTS.md documents). Each
   extraction verified via clean `cargo check` on both default and
   `--no-default-features`, full lib test suite (1576/1578, unchanged
   baseline), 239/239 conformance, `cargo fmt`, and clippy warning
   count parity (191, full workspace, before/after every commit).
   **Deliberately not attempted this session:** the remaining
   ~4000-line `impl Runtime` block is dominated by core scheduling/
   actor-stepping methods (`step_actor` at 399 lines, `recover_actor`,
   `ask_actor_sync_inner`) deeply entangled with the GC/concurrency
   invariants AGENTS.md flags as "do not break" (the reclamation
   protocol, `vm_execution_depth` tracking) — a materially higher risk
   profile than the AI/LLM subsystem extracted here, and better suited
   to a dedicated, fresh session than squeezed in at the end of a long
   one. The full trait-based structural decomposition this bullet
   originally envisions remains entirely open.
   **Confirmed: no lower-risk sub-target exists in the deferred block
   (2026-08-03).** `recover_actor` was the best candidate for a narrower
   first cut — it reads as cold-start/recovery code, not the scheduler hot
   path — but a full read shows it shares the exact same hot-path
   primitives and an unenforced invariant. Its workflow-resume branch calls
   `send_message_by_id` directly (`mod.rs:3643`); its journal-replay branch
   calls `run_bytecode_behavior` (`mod.rs:3667-3674`), the identical
   primitive `step_actor` and `flush_actor_mailbox` call. Its
   `current_actor` bracketing around that call (`mod.rs:3668,3670`:
   unconditional `Some(actor_id)` in, hard `None` out) matches
   `step_actor`'s own top-level-only pattern (`mod.rs:2148`) — not the
   save-`prev`/restore-`prev` pattern `flush_actor_mailbox` uses for the
   *same* `run_bytecode_behavior` call (`mod.rs:1504-1508`). Two call sites
   into one primitive, two different reentrancy assumptions, reconciled
   only by `recover_actor` always running before the recovered actor is
   enqueued — so `current_actor` happens to already be `None`. That holds
   by call-site discipline alone: nothing in the type system or the test
   suite enforces it, and none of `recover_actor`'s ~20 direct call sites
   (`stress_tests.rs`, `integration_tests/mod.rs`, `runtime/tests.rs`)
   exercise it from inside another actor's live context, unlike
   `step_actor`, implicitly covered by nearly every test that calls
   `run_scheduler()`. A mechanical extraction that normalized the two
   `current_actor` patterns to match — plausible cleanup, wrong in either
   direction — would land with zero failing tests. If this block is
   revisited: unify both call sites behind one audited helper (e.g.
   `with_current_actor(id, ...)`) that owns save/restore, before attempting
   any extraction, so there is one reentrancy contract instead of two
   undocumented ones. Until then, no subset of the remaining block is
   lower-risk than the whole; it stays deferred as a unit.
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
8. **Dep audit and reduction.** 468 transitive deps (down from 504; see
   2026-08-02 `libsql` feature trim below) → target ≤300.
   Candidates for removal or replacement: `httparse` + `ureq` (unify),
   `rustyline`'s feature surface, `tracing-subscriber` heavy features.
   `libsql` itself is now feature-trimmed to `core`+`remote`+`tls`
   (dropped `replication`/`sync`, -36 crates including all of tonic/
   axum) but its `core`/FFI/bindgen layer remains — full replacement
   with a bytecode-only journal format is still a candidate if further
   reduction is needed. Every dep gets a "why we depend on this" line
   in `SPEC2.md` §Implementation Status.

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
**Pulled forward into Phase 5** (Distributed Systems Excellence,
deliverable 13) — executed there, not gated on Phase 3's own timeline;
this bullet is satisfied by reference once Phase 5's CmRDT deliverable
lands.
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

## Phase 5 — Distributed Systems Excellence (parallel with Phases 1-3)

**Goal.** The distributed actor runtime — not just the single-node
language — withstands adversarial operational review. A cluster survives a
real network partition without silent data loss or a stuck split-brain;
the default transport is authenticated and encrypted; a node's death
triggers real recovery instead of silent orphaning; CRDT state doesn't
leak memory forever; an operator can point off-the-shelf tooling at a
running cluster. This is where Nulang's distributed-actor design either
becomes provably best-in-class or stays a credible-looking demo.

**Sequencing.** Groups are ordered by dependency, not by importance — all
are real gaps. Group A (partition tolerance) before Group C (scalable
membership): a split-brain resolver designed against a full membership
view needs revisiting once membership becomes partial-view. Group D
(cross-node fault tolerance) depends on Group A's failure-detection signal
already existing (it does, `cluster.rs` Failed-status detection) and needs
Group D's own link/monitor extension before failure notifications can
reach anything. Group E's tombstone GC (11) depends on Group C settling
membership bookkeeping. Group F's migration (14) depends on Group D's
cross-node link/monitor (8). Group B (transport security) and Group G
(observability) are independent of the others and can run in parallel with
anything.

**Deliverables.**

1. **Split-brain resolver.** Today: `ClusterState` has no quorum, no
   leader election, no majority logic anywhere (zero
   `quorum`/`leader`/`elect` hits in `src/runtime/`). Heartbeats and
   gossip are sent only to `Healthy`/`Joining` members (`cluster.rs:450`)
   — once two sides of a clean partition each mark the other `Failed`,
   neither redials the other, so the split does not self-heal; it requires
   an external rejoin. `tests.rs:3224-3228` explicitly disclaims
   split-brain/asymmetric-partition coverage. Build an Akka-SBR-style
   pluggable resolver operating on `ClusterState`'s existing membership
   view: `static-quorum` first (needs only a configured expected cluster
   size, no live count), then `keep-majority`/`keep-oldest` once
   deliverable 6 proves partial-view membership can still produce an
   accurate count. This is explicitly NOT a new consensus protocol — it
   reuses `PERFORMANCE_ANALYSIS.md` row 3.4's existing Raft deferral
   ("CRDTs cover 80% of distributed state needs"); do not relitigate that
   deferral here. Fix the non-self-healing bug alongside it: `Failed`
   peers need a minimal periodic probe (not full heartbeats) so the
   resolver has live data to decide with. New `ClusterAction` variant(s)
   for the down-and-remove decision. Cluster-membership behavior isn't in
   GOVERNANCE.md's current Frozen or Stable lists (similar to the CRDT
   surface's pre-RFC state per its erratum) — file an RFC anyway per the
   governance-discipline workstream's "every Frozen/Stable change is an
   RFC" plus the real operational blast radius of a mechanism that can
   autonomously shut down a node; this RFC can also be what first formally
   tiers cluster-membership behavior.
2. **DST-driven split-brain and asymmetric-partition test coverage.**
   ✅ **Landed 2026-08-03** as `src/runtime/cluster_sim.rs` (test-gated
   `SimCluster`, wired in `runtime/mod.rs`): N real `ClusterState`
   machines against a shared `VirtualClock` advanced in lockstep, with a
   directed cut-able message fabric (heartbeats, gossip, and probes —
   probes delivered as the heartbeat packets they are on the wire;
   deliveries to a downed node dropped, mirroring its shut-down
   transport). Five deterministic scenarios: clean 2/3 partition of a
   5-node cluster (minority downs itself at the 2 s Suspicious mark —
   the resolver counts only Healthy/Joining as reachable — majority
   survives and keeps the minority Failed after healing; downed nodes
   stay down, operator restart is the recovery path), asymmetric
   one-way partition (three phases: the 2-4 s asymmetry window where the
   silent node is down but still Healthy on the other side, mutual
   Failed at 10 s, heal-without-recovery), probe-based re-join of a
   healed 2/3 partition under quorum 2 (no downing; full mesh
   convergence via probes, no external rejoin), the documented
   2-node fail-closed caveat, and a seed-driven 50-partition invariant
   sweep (no node downs itself while it still sees quorum). This is the
   verification vehicle for deliverable 1 — the real-TCP chaos tests
   (`tests.rs`) stay as-is; the deterministic suite is what can
   eventually scale to many-seeds CI runs.
   **Deliverable 3 (three doc-vs-code bugs) was already fixed by the
   deliverable-1 work**: `pick_gossip_targets` is now OsRng-driven
   partial Fisher-Yates (not deterministic first-N), the dead
   `TcpTransport` `next_seq` `AtomicU64` is gone (sender-local counter
   only), and `NodeInfo`'s vestigial standalone `incarnation` field was
   removed — only the wire `NodeGossip.incarnation` remains.
3. **Fix three confirmed doc-vs-code / dead-code bugs found this session**
   (cheap, do first as warm-up, independent of 1-2): (a)
   `cluster.rs:460`'s comment "Gossip to a random subset of healthy nodes"
   and `cluster.rs:24-25`'s module doc both claim randomness;
   `pick_gossip_targets` (`cluster.rs:652-663`) actually does "simple
   deterministic selection: pick the first N" and says so in its own
   comment ("in a real deployment this would use
   `rand::seq::IteratorRandom`") — wire in real random selection
   (deterministic first-N systematically starves whichever members sort
   late in `HashMap` iteration order from gossip coverage); (b)
   `network.rs`'s `TcpTransport.next_seq` `AtomicU64` is incremented in
   `send()` but discarded (`let _ = seq;`, `network.rs:1472-1478`) while
   the sender thread keeps a second, actually-used local sequence counter
   (`network.rs:1742-1745`) — two divergent counters; delete the dead one;
   (c) `cluster.rs`'s standalone `incarnation` field
   (`cluster.rs:221-223`, bumped by `bump_incarnation` `:538-540`) is
   never transmitted on the wire — only the separate per-entry
   `_incarnation` metadata string is (the one `merge_membership`/AGENTS.md
   actually document and test) — delete the vestigial field.
4. **Wire up authenticated, encrypted transport, with plaintext as an
   explicit opt-out.** Today: `TlsConfig::SelfSigned` exists
   (`network.rs:64-132`) but is never constructed anywhere in the
   repository — confirmed by direct grep across
   `mod.rs`/`distribution.rs`/`tests.rs`/`main.rs`:
   `enable_distribution`'s `tls_config: Option<TlsConfig>` parameter is
   passed `None` at its one call site (`tests.rs:2961`) and is otherwise a
   Rust-embedder-only API with no CLI surface at all (confirmed by
   SPEC2.md §12.4's own callout: "called from nowhere in `main.rs`"). Even
   if constructed, the client installs a `NoVerification` verifier
   accepting any certificate (`network.rs:93-132`) — MITM-able — and node
   identity is a `DefaultHasher` hash of the node's own advertised listen
   address (`cluster.rs:68-83`) — spoofable, zero authentication. NUL0 is
   Frozen-tier (wire protocol v1); a handshake change to carry real
   authentication is a NUL0 v2 bump requiring an RFC and a
   version-negotiation path — the existing handshake already refuses
   unknown versions rather than reinterpreting them
   (`network.rs:296-323`), so v2 rollout is a clean refuse-old path, not a
   new mechanism. Concretely: (a) real certificate verification (pinned CA
   or operator-supplied pre-shared cluster cert — not auto-generated
   self-signed-trust-anyone), (b) node identity as a signed claim
   (cluster-issued token or cert-derived id) instead of a hash of
   self-reported address, (c) change `enable_distribution`'s shape so
   plaintext is an explicit named opt-out (e.g. a
   `TlsConfig::PlaintextInsecure` variant) rather than `Option::None`
   silently meaning "no security" — existing `None`-passing call sites
   migrate to the explicit variant, not a breaking signature removal. RFC
   required (Frozen-tier wire change), steward-authored or
   steward-reviewed per GOVERNANCE.md §3.
5. **Decide QUIC's fate — finish or remove, not permanent dead weight.**
   ✅ **Removed 2026-08-05.** Assessed for integration: requires a tokio
   runtime (separate from the main sync runtime), has an incompatible raw
   8-byte handshake with no NUL0 magic/version check, and zero test
   coverage. The TCP transport with MutualTLS (deliverable 4) already
   provides authenticated, encrypted transport. Removed
   `src/runtime/quic_transport.rs`, `quic-experimental` feature, and
   `quinn` dependency. `rcgen` is preserved (used by
   `TlsConfig::SelfSigned`). QUIC can be revisited when users ask for
   multiplexed transport; not needed for alpha.
6. **Partial-view membership beyond full-mesh.**
   ✅ **Landed 2026-08-03** — the heartbeat data plane is now
   O(active view) instead of O(every member), with the membership table
   and gossip unchanged (no wire change; views are local state, same
   Experimental tier as RFC 0011, which this work amends with §6):
   - **Active view (4) / passive view (20) / probation**: admission by
     incoming heartbeat (reciprocity evidence); the failure detector
     watches exactly the active view, so no member we do not heartbeat
     can be false-failed. A failed active member is repaired by
     promoting a Healthy passive to probation (heartbeated, not
     watched); first reply confirms, silence demotes (churn, not false
     failure), retry every 5 s.
   - **Bounded reply rule**: up to `REPLY_SLOTS` (4) replies per round
     to recent passive pingers (rotated) — a member whose view filled
     up still gets answered within the 2 s detection window (~80-node
     ceiling at these constants).
   - **Detector bumps incarnation on `Failed`** so the status
     propagates via gossip to non-watchers (invisible under full-mesh,
     fatal under partial view — this was a real gap found by the DST
     harness).
   - **Gossip liveness refresh** for passive live members
     (equal-incarnation re-broadcast refreshes `last_heartbeat`;
     watched members and Failed entries are never refreshed — the
     dead-peer protection regression-tested).
   - **Freshness-aware resolver view**: stale-status passives count as
     Suspicious in the view handed to the resolver, so an isolated
     node's frozen gossip cannot keep it above quorum; `static-quorum`
     stays correct under partial view. `keep-majority`/`keep-oldest`
     remain deferred (live-count accuracy not yet proven — unchanged
     from the RFC).
   - **Verification**: 4 new `SimCluster` scenarios (30-node bounded
     fanout, 10-node convergence, death with zero false failures +
     gossip failure propagation, heal/rejoin with view repair) + 6 unit
     tests for the view mechanics. The DST harness is what surfaced
     both real gaps (incarnation bump, stale-gossip quorum) — the
     plan's "verification vehicle" reasoning paid off.
7. **Node-death detection triggers real recovery, not silent orphaning.**
   ✅ **Parts (a)+(b) landed 2026-08-09** (`handle_node_failed` in
   `distribution.rs`, wired to `ClusterAction::NodeFailed`): (a) the dead
   node's `RemoteActorCache` entries are invalidated so sends fail fast
   instead of stale-resolving; (b) every local actor that had linked or
   monitored an actor on the failed node receives a
   `DOWN`-with-`noconnection` system message (new `ExitReason::NoConnection`,
   payload code 6) and the dead registry entries are dropped. The D8
   delivery half also landed: inbound `Packet::Link`/`Monitor` now register
   remote watchers and inbound `Packet::Down` delivers DOWN to local
   watchers (previously all three were silently dropped). Part (c) —
   supervisor-policy-driven re-spawn of durable actors on a healthy node —
   is **designed but not implemented**: **RFC 0014** specifies the
   confirmed-gone gate (new `Removed` membership state via positive
   `NodeGoodbye` or a majority-gated `removal_confirmation_timeout`
   promotion), a gossip-replicated durable-actor location directory with
   epoch-based two-live-copies resolution (self-demote), snapshot
   replication to a deterministic shadow node at `checkpoint_actor`, a new
   `RestartPolicy::RespawnOnNodeLoss` supervisor policy, and reuse of the
   existing `Packet::MigrateActor`/`receive_migrated_actor` transport.
   It requires the old-node-confirmed-gone gate that a bare
   failure-detection signal cannot provide; the safety rationale is
   documented in the `handle_node_failed` doc comment and RFC 0014 §1.
   **Original analysis (kept for provenance):** zero grep hits for
   `failover`/`rehome`/`migrat` logic across
   `distribution.rs`/`distributed.rs`. `ClusterState` already detects
   `Failed` nodes via a staged 2s/5s/60s timeout (`cluster.rs:381-433`) —
   that signal today goes nowhere except membership bookkeeping. When a
   node transitions to `Failed`: (a) invalidate that node's entries in
   every other node's `RemoteActorCache` so sends fail fast instead of
   stale-resolving: (b) fire link/monitor-equivalent
   `DOWN`-with-`noconnection` notifications to local actors that had
   linked/monitored an actor known to live on the failed node — this
   requires deliverable 8 to exist first, since today link/monitor tables
   have no remote-actor entries to notify; (c) for actors backed by
   durable state, enable an explicit supervisor-policy-driven re-spawn on
   a healthy node from the last durable snapshot. Do NOT implement silent
   automatic migration on node failure — without an explicit supervisor
   policy confirming the old node is actually gone (not just partitioned),
   automatic re-spawn risks two live copies of the same durable-id actor
   writing to the same store from two nodes. Model the safety gate on
   Kubernetes StatefulSet pod rescheduling requiring
   old-pod-confirmed-gone, not naive auto-failover.
8. **Cross-node link/monitor registration.** Prerequisite for 7(b).
   Confirmed: `link_actors`/monitor tables (`mod.rs:3735-3779`) and
   `Actor.parent` (`actor.rs:191`) only ever reference local `rt.actors`
   ids; remote actors exist only in `RemoteActorCache`, keyed `(node_id,
   actor_id)` (`distributed.rs:160-162`), never in `rt.actors`. Before
   implementing, the implementer must first check what `perform
   Actor.link`/`Actor.monitor` currently do when given an
   `ActorAddress::Remote` — this session's audit did not confirm whether
   it's a silent no-op, a resolve-then-fail, or an outright error; do not
   assume. Extend link/monitor tracking to record cross-node targets, and
   propagate link/`DOWN` notifications over the wire (a new `Packet`
   variant) both when a monitored/linked remote actor exits normally and
   when its home node is declared `Failed` (deliverable 7's signal). This
   extends the actor surface's supervision semantics, which GOVERNANCE.md
   lists as Stable-tier — file an RFC (purely additive: existing local
   link/monitor behavior is unchanged, so no deprecation cycle is
   triggered, only the RFC itself).
9. **Fix the nested-supervisor-restart bug.** Confirmed real, not
   speculative: `rebuild_child` (`supervisor.rs:301-384`) recreates a bare
   `Actor` on restart but never recreates the corresponding `Supervisor`
   struct in `rt.supervisors` — a supervisor that is itself supervised
   loses all supervision of its own children after one restart.
   Local-only, cheap, independent of the cross-node work — fix before or
   alongside deliverable 8, since cross-node supervision is worthless if
   local supervisor-of-supervisors restart is already broken.
10. **Fix the mass-restart rate-limit bug.** Confirmed:
   `restart_all`/`restart_from` (`supervisor.rs:478-534`, the
   `OneForAll`/`RestForOne` strategies) rebuild every sibling
   unconditionally without checking each sibling's own `should_restart` —
   only the triggering child's own rate limit is enforced, so a
   `OneForAll` group can restart-loop forever even though each individual
   child would have tripped its `MaxR`/`MaxT` limit alone. Bundle with
   deliverable 9 (same file, same investigation pass).
11. **Tombstone garbage collection for `ORSet`/`AWORSet`/`RGA`.**
   Confirmed: `removed` tombstone sets (`crdt.rs:454` ORSet, `crdt.rs:694`
   AWORSet) and RGA's tombstoned elements (`crdt_reg.rs:299-302`) grow
   unboundedly forever — a production-blocking memory leak and unbounded
   wire-payload growth for any long-running counter/set. Needs a
   causal-stability watermark: a tombstone is safe to drop once every
   known replica has observed it (classic CRDT GC via a stable
   vector-clock/Lamport-time cut across the member set). Depends on Group
   C settling — a partial-view membership makes "every replica has
   observed it" harder to compute; if deliverable 6 can't produce a
   reliable full-membership view for this purpose (even though it doesn't
   heartbeat everyone), the stability watermark may need its own
   lightweight full-membership gossip pass, separate from the heartbeat
   data-plane — decide this when deliverable 6 lands, don't block
   deliverable 11 on it indefinitely.
12. **Wire `state crdt` into real `.nula`-level syntax.** Already tracked
   by SPEC2.md §12.5 ("tracked as a real implementation gap... see RFC/
   for the tracking item once filed") — this deliverable formally files
   that RFC and executes it. Today `state crdt count: Int = 0` parses and
   runs but behaves identically to `state durable` — no type selector, no
   `Crdt.*` effect module, no merge-on-sync (confirmed, SPEC2.md §12.5).
   Ship: a concrete-CRDT-type selector in the `state crdt` declaration, an
   enforced operation set per type (e.g. a `GCounter`-typed field only
   accepts increment, not arbitrary assignment), and real
   merge-through-`CrdtManager` on sync. This is the single
   highest-leverage differentiator this investigation found: none of
   Erlang/OTP, Akka (CRDTs are a separate "Distributed Data" library, not
   core), or Orleans ship CRDTs as a first-class in-language
   state-declaration primitive. RFC required (additive Stable-tier
   surface, same reasoning as deliverable 8).
13. **Op-based CRDT replication (CmRDT).** Already fully scoped by
   `PERFORMANCE_ANALYSIS.md` §3.2 (its status note: "Op-based (CmRDT)
   replication was not implemented") and PLAN.md Phase 3 bullet 6 — no new
   investigation needed. This deliverable pulls that existing,
   already-scoped item forward into Phase 5's timeline instead of Phase
   3's (Step 4 below updates the Phase 3 bullet to point here). Do not
   re-scope it; execute it as already described there (`Packet::CrdtOp`
   alongside the existing `Packet::CrdtDeltaSync`).
14. **Real actor migration, not a no-op stub.** Confirmed:
   `OpCode::Migrate` records a `pending_migrations` entry
   (`vm.rs:4349-4360`) that nothing ever drains;
   `DistributedVmCallbacks::migrate` is `fn migrate(&mut self, _actor_id:
   u64, _target_node_id: u64) {}` — a literal empty body
   (`callbacks.rs:1526`). Implement as: (a) snapshot the actor's durable
   state through the existing `PersistenceStore` snapshot path, (b) spawn
   on the target node from that snapshot, reusing `recover_actor`'s
   restoration logic rather than forking a parallel copy of it, (c) update
   every node's `AddressResolver`/`RemoteActorCache` entries that pointed
   at the old location, (d) tombstone/forward the old location for
   in-flight messages during the handoff window. **Read PLAN.md's existing
   Phase 2 bullet 4 write-up on `recover_actor` before touching this**
   (the "Confirmed: no lower-risk sub-target exists" paragraph from this
   same session): `recover_actor` has an unconditional-`Some`/hard-`None`
   `current_actor` bracketing pattern around its `run_bytecode_behavior`
   call that differs from `flush_actor_mailbox`'s
   save-`prev`/restore-`prev` pattern for the identical primitive —
   migration's spawn-from-snapshot path becomes a *third* call site into
   this family and must not introduce a *fourth*, undocumented reentrancy
   convention. If migration needs to call into recovery machinery, match
   whichever of the two existing patterns is actually reentrancy-safe for
   migration's calling context, don't invent a third. Depends on
   deliverable 8 (a migrated actor's supervisors/linked peers need to
   already be reachable cross-node to react correctly to the
   identity/location change). Tier status of `migrate` itself is unclear
   from GOVERNANCE.md's current lists (not named, similar to CRDTs'
   pre-RFC state) — file an RFC anyway given the operational blast radius,
   same reasoning as deliverable 1.
15. **Cross-node durable-store replication: explicitly scoped down, not
   silently deferred.** Confirmed: `JsonFileStore`/`LibsqlStore` are
   purely per-node local disk; only CRDT state converges across nodes
   today. Full multi-node durable-store replication (Raft-backed or
   leader-based) is explicitly OUT of this phase — it is
   `PERFORMANCE_ANALYSIS.md` row 3.4's already-deferred Raft item; this
   plan does not relitigate that deferral (see deliverable 1's same note).
   What this phase DOES ship: make the existing *single-node* durability
   trustworthy before promising anything about surviving losing a whole
   node. Confirmed gaps: `LibsqlStore` has zero `PRAGMA` statements
   anywhere (no WAL, no `synchronous` setting); `JsonFileStore` fsyncs
   snapshots via temp-file-plus-rename (`persistence.rs:506-521`) but
   journal/workflow appends are unsynced
   (`persistence.rs:546-558,571-583`). Add `PRAGMA journal_mode=WAL` and
   an operator-configurable `PRAGMA synchronous` to `LibsqlStore`; fsync
   `JsonFileStore`'s journal/workflow appends with the same discipline its
   snapshot path already has (or, if a real reason not to surfaces during
   implementation, document it explicitly rather than leaving the
   asymmetry silent).
16. **Fix `LibsqlStore` silently dropping `crdt_snapshot` on save/load.**
   Confirmed bug, same file/subsystem as deliverable 15 — bundle together.
17. **Ship the distributed-actor-relevant slice of the
   metrics/tracing/debug story SPEC2.md §15.3-15.4 already speculatively
   designs — not the whole chapter.** Confirmed: zero
   opentelemetry/prometheus/metrics dependencies anywhere in the
   workspace; `tracing`/`tracing-subscriber` are wired only as a text
   stderr logger defaulting to `warn` (`main.rs:56-62`); real counters
   already exist but are never exported — `GcStats` (`gc.rs:87-129`),
   `SchedulerStats` (`scheduler.rs:29-65`), `ResolverStats`
   (`distributed.rs:315-327`), `mailbox_depths()`/`dlq_depth()`
   (`mod.rs:1841-1858`) — today's sole consumer is a one-shot end-of-run
   `--verbose` dump (`main.rs:1164-1183`). SPEC2.md §15.3/15.4 already
   fully designs `metrics.counter/histogram/gauge`, `trace.span`, `config
   trace { auto_trace_actor_messages }`, `debug.inspect(actor_id)` — do
   not redesign, implement against that existing spec, and scope to
   exactly: (a) an OpenTelemetry exporter over the counters that already
   exist (wiring existing data out, not inventing new instrumentation),
   (b) a trace-id field riding along in `Packet::ActorMessage`'s existing
   string-table wire mechanism (extend what already carries content
   cross-node by value, don't invent a second cross-node payload channel)
   so a span begun on the sending node continues on the receiving node,
   satisfying `auto_trace_actor_messages`, (c) `perform
   debug.inspect(actor_id)` as a new built-in effect returning `{ state,
   mailbox_size, behaviors, supervisor }`, directly reusing
   `mailbox_depths()` and existing actor accessors already in `mod.rs`.
   Explicitly out of scope for this phase:
   `debug.trace_messages`/`debug.snapshot`/deterministic replay debugging,
   any admin HTTP/CLI surface beyond what's needed to point an operator's
   existing Prometheus/Grafana/Jaeger at a running cluster, and every
   non-distributed part of SPEC2.md Chapter 15 (deployment manifests, the
   generic `config app` system, serverless targets) — those stay backlog.
18. **Visual actor topology dashboard.** Already scoped by
   `PERFORMANCE_ANALYSIS.md` row 6.4 ("DO as a side project, low-risk,
   high-fun, great for demos," 3 weeks, no dependencies) — pull it into
   Phase 5 as the demo-facing consumer of deliverable 17's metrics export.
   Do not build a bespoke dashboard backend: point an off-the-shelf
   Grafana at deliverable 17's OTel/Prometheus exporter and ship default
   panel JSON plus a `docker-compose` demo — this is cheaper and more
   credible than a custom UI, and was the original row's own "as a side
   project" framing.

**Acceptance.**

- Default new clusters require authenticated, encrypted transport;
  plaintext is an explicit, documented opt-out, never the silent default
  (deliverable 4).
- A 3-node and a 5-node DST/chaos scenario suite includes split-brain
  (mutually-invisible healthy sub-clusters) and asymmetric-partition
  cases; the cluster provably converges to one surviving side per the
  configured resolver strategy, never a stuck two-sided split
  (deliverables 1-2).
- A node killed mid-run triggers `DOWN` notifications to every local actor
  that had linked/monitored one of its actors, and a
  supervisor-policy-driven re-spawn from the last durable snapshot
  succeeds (deliverables 7-9).
- `ORSet`/`AWORSet`/`RGA` tombstones are garbage-collected once causally
  stable; a long-running soak test shows bounded, not unbounded, memory
  growth (deliverable 11).
- `state crdt` fields have real `.nula`-level syntax, a concrete-type
  selector, and merge-on-sync — SPEC2.md §12.5's "tracked as a real
  implementation gap" note is resolved, not just re-stated (deliverable
  12).
- A `migrate`-triggered move relocates a persistent actor to a healthy
  node; the old node's `AddressResolver` cache no longer resolves the old
  location afterward (deliverable 14).
- `LibsqlStore` uses WAL plus an explicit `synchronous` pragma and
  round-trips `crdt_snapshot` correctly; `JsonFileStore` fsyncs
  journal/workflow appends with the same discipline as its snapshot path
  (deliverables 15-16).
- A running cluster's `GcStats`/`SchedulerStats`/`ResolverStats`/mailbox
  depths are scrapeable by an off-the-shelf Prometheus/OTel collector with
  zero code beyond configuration; a trace begun on one node's actor send
  continues on the node that receives it (deliverable 17).

**Non-goals.** Native Raft/consensus-backed strongly-consistent
replication (`PERFORMANCE_ANALYSIS.md` row 3.4, still deferred — SBR-style
resolvers give partition safety without it; this phase does not relitigate
that call). Kernel-bypass networking (io_uring/RDMA, row 3.3, still
deferred). Content-addressable bytecode (row 3.5, still deferred). The
non-distributed-actor parts of SPEC2.md Chapter 15 (deployment manifests,
the generic `config app` configuration system, serverless deployment
targets) beyond the observability slice in deliverables 17-18. Automatic
silent actor rebalancing without an explicit supervisor policy
(deliverable 14 is operator/supervisor-triggered only, by design — see its
safety note).

**Delegable to.** Distributed-systems specialist (deliverables 1-10:
split-brain, transport security, membership scaling, cross-node fault
tolerance — the deepest and highest-risk work). CRDT/data-structures
specialist (deliverables 11-13). Storage/persistence engineer
(deliverables 15-16). Observability/SRE-tooling contributor (deliverables
17-18). Steward retains RFC authorship or review for every deliverable
that touches Frozen or Stable surface (4, 8, 12, and 1/14 by the more
cautious operational-blast-radius reasoning) per GOVERNANCE.md §3, and
retains deliverable 14 given its direct dependency on this session's
`recover_actor` findings.

**Kill criteria.** If the split-brain resolver (deliverable 1) cannot be
built on `static-quorum` alone because the cluster has no way to agree on
a configured expected size without agreement itself, stop and re-scope as
a Raft adoption instead of routing around it quietly — this would mean
`PERFORMANCE_ANALYSIS.md`'s Raft deferral was wrong, which is itself
significant enough to interrupt the phase and get steward sign-off, not
something to paper over. If NUL0 v2 (deliverable 4) surfaces a
wire-compatibility break that can't cleanly refuse old versions the way
v1's handshake already does (`network.rs:296-323`), treat it as a
Frozen-tier incident per GOVERNANCE.md and get steward sign-off before
shipping — same bar as any other Frozen-tier change. If partial-view
membership (deliverable 6) can't preserve the accuracy the split-brain
resolver's `keep-majority`/`keep-oldest` strategies need, ship deliverable
1 with `static-quorum` only and defer `keep-majority`/`keep-oldest` and
deliverable 6 together, rather than shipping a resolver that silently
miscounts.

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
- **Distributed-transport test discipline.** Every change to `network.rs`,
  `cluster.rs`, `distributed.rs`, or `distributed_context.rs` ships with a
  DST/chaos scenario covering the failure mode it touches (partition,
  split-brain, node death) before merge, once Phase 5 deliverable 2 lands
  the harness — this makes chaos coverage a standing gate, not a one-time
  sweep.

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
| Split-brain resolver false-positive downs a healthy majority | Medium | High | Default to the conservative `static-quorum` strategy; `keep-majority`/`keep-oldest` require explicit opt-in once membership-count accuracy is proven under Phase 5 deliverable 6 |
| NUL0 v2 transport-security bump breaks embedders relying on today's no-config plaintext `enable_distribution(addr, None)` | Medium | Medium | Plaintext stays available as an explicit, documented opt-out variant, never silently removed; existing `None` call sites migrate to an explicit insecure variant, not a breaking signature change |
| Partial-view membership (Phase 5 deliverable 6) undermines the split-brain resolver's member-count assumptions (deliverable 1) | Medium | Medium | Sequence deliverable 1 before deliverable 6; if 6 can't preserve count accuracy, ship 1 alone and defer 6 (see Phase 5 kill criteria) |

## Version + tier progression

| Version | Milestone | Trigger |
|---|---|---|
| 0.1.0 | current alpha | shipped |
| 0.2.0 | Phase 0+1 complete | truth-in-advertising + correctness floor |
| 0.3.0 | Phase 2 complete | proofs + Windows + release binaries |
| 1.1.0-stable | Phase 3 partial | bootstrap fixpoint + registry live |
| 2.0.0-frozen | Phase 3 complete | deprecation cycle graduations require major bump |
| 2.0.0-frozen (or sooner) | Phase 5 NUL0 v2 | authenticated/encrypted transport handshake is a Frozen-tier wire-format bump (deliverable 4) — an independent trigger from Phase 3's deprecation graduations; whichever lands first bumps the major language version, the other rides the same or a later major bump |

Phase 5 runs in parallel with Phases 1-3 and is not gated on their
completion; its Frozen/Stable-tier deliverables (1, 4, 8, 12, 14) each
need their own RFC per the governance-discipline workstream above,
independent of the phase's own internal sequencing.

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
