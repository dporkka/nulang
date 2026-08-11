# Wiki Log

Append-only chronological record of wiki operations. Every ingest, refresh, query, and lint gets an entry. Entries start with `## [YYYY-MM-DD] <op> | <title>` so `grep '^## \[' wiki/log.md | tail -20` yields recent activity.

Operations:
- `bootstrap` — initial scaffolding.
- `ingest` — a new source landed, wiki updated.
- `refresh` — code drifted, wiki brought in sync.
- `query` — a user question was filed back into the wiki.
- `lint` — health-check findings.

---

## [2026-08-11] refresh | AOT multi-block resuming — Phase 2 (continuation live-ins) landed

Implemented cross-block continuation live-in threading: a resuming perform's
continuation reads values produced in an earlier block (e.g. a mutable
accumulator set by one perform, read after a later perform) by threading them
through the handler's Resume dispatch. `continuation_live_ins` (backward
liveness over normal successors), `resuming_threading` (per-site extras +
uniform width), threaded slots allocated even for single-site bodies (fixing a
loop-carried regression in the single-continuation Resume path). Commit
`c51df8e`. The AOT backend is now functionally complete for non-distributed
programs.

Touched: [[queries/aot-multi-block-resuming]], `src/aot/codegen.rs`

---

## [2026-08-11] refresh | AOT multi-block resuming — general SSA phi fix

While scoping Phase 2 (cross-block prior-result reads) found the
representative pattern hits a GENERAL AOT bug: a `var` assigned in one branch
and read after a merge failed with no effects involved. Fixed `compute_liveins`
to phi values live into a merge that are defined in a predecessor, plus fixed
the gen/kill terminator liveness (branch-cond wrongly live into a loop
back-edge). Commit `8d0c286`. Narrowed Phase 2 to *continuation live-in
threading* (still open, rare pattern).

Touched: [[queries/aot-multi-block-resuming]], `src/aot/codegen.rs`

---

## [2026-08-11] refresh | AOT multi-block resuming handler — Phase 1 landed

Implemented Phase 1 of the AOT multi-block resuming handler design: a
resuming effect handler performed from different MIR blocks now compiles
(was rejected). Uniform per-body threaded-slot width + `compute_liveins`
exclusion of handler-body blocks + a gen/kill liveness guard that rejects
genuine cross-block prior-result reads. Commit `758ce4d`. Tests:
exclusive-branch if/else, discarded-first-result, cross-read rejection.

Touched: [[queries/aot-multi-block-resuming]], `src/aot/codegen.rs`

---

## [2026-08-11] query | AOT multi-block resuming handler — scoping

Scoped the last reachable correctness gap in the AOT native backend: a
resuming effect handler (`| E.op(x) resume => ...`) performed from different
MIR blocks is rejected ("multi-perform resuming handler with perform sites
across multiple blocks is not yet supported"). Confirmed the failing pattern
(`if cond then perform E.run(1) else perform E.run(2)`) on `--backend native`.

Root-caused the same-block model: `cont_thread` is per-block while
`handler_threaded_dsts` is per-body (global), so cross-block perform sites
make the jump-arg count ≠ handler param count. Proposed a phased design:
Phase 1 = uniform threaded-slot width (max per-block, dummy-padded) covering
exclusive-branch patterns without cross-block reads; Phase 2 = thread prior
perform results through `compute_liveins` for loop-carried/accumulated
results. After this, the only remaining AOT rejects are distribution
(remote spawn/ask) and dead defensive arms.

Touched: [[queries/aot-multi-block-resuming]]


Implemented the "Immediate" tier of [[queries/performance-assessment]] and recorded an `## Implementation status` section on that page.

Touched: [[queries/performance-assessment]]
Key updates: Added `#[repr(align(64))]` to `Scheduler`/`SchedulerStatsInternal`/`Mailbox`; opt-in core pinning (`NULANG_PIN_CORES`, Linux `sched_setaffinity`) wired into the shard spawn; swapped the JIT's per-instruction hot-path maps to `rustc_hash::{FxHashMap,FxHashSet}`; added `VM::new_without_jit()` + an `interp` criterion bench group. Full token-threading deferred (risky core rewrite; hot-counter hashing captured the largest safe slice). Verified: 1650 lib tests pass, `interp` bench runs, gate default/no-default-features warning-clean.

## [2026-08-09] query | Performance proposal assessment

Filed the assessment of `PERFORMANCE_ANALYSIS.md`'s 28 proposals (plus beyond-catalog techniques: threaded dispatch, JIT OSR, NUMA awareness, cache-line padding, non-temporal stores, Auto-SoA) against the current tree.

Touched: [[queries/performance-assessment]]
Key updates: Ground-truth verified all claims via `read`/`grep` — 14/28 proposals shipped, 4 partial, 7 deferred; the 4 legacy explorer performance findings (Box'd frames, `.leak()` strings, RGA/MVRegister Vec churn) are all fixed in the current tree. Ranked 5 actionable gaps: interpreter dispatch throughput, rkyv wire serialization, cache-line/NUMA locality, DST harness, JIT OSR.

## [2026-08-02] bootstrap | wiki scaffolding

Established the wiki structure per the [[wiki-updater skill|../.claude/skills/wiki-updater/SKILL.md]], instantiating Andrej Karpathy's [llm-wiki.md](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f) pattern for the Nulang repository.

Created:
- [[index]] — content catalog.
- [[log]] — this file.
- [[overview/architecture-overview]] — top-level architecture map, sourced from `AGENTS.md`.
- [[overview/compiler-pipeline]] — compiler stages and backends, sourced from `AGENTS.md` and `src/`.

Not seeded (per skill's bootstrap rule: "wiki grows by ingest, not by front-loading"):
- Subsystem pages (`wiki/subsystems/`) — created on first ingest per subsystem.
- Concept pages (`wiki/concepts/`) — created on first ingest per concept.

Next: user directs which subsystem or concept to expand first via the `wiki-updater` skill.
