# AOT native backend — multi-block resuming effect handlers (scoping)

**Status:** Phase 1 IMPLEMENTED (`758ce4d`, 2026-08-11). General SSA phi fix
(prerequisite) IMPLEMENTED (`8d0c286`, 2026-08-11). Phase 2 (cross-block
continuation live-ins) IMPLEMENTED (`c51df8e`, 2026-08-11). The AOT backend is
now functionally complete for non-distributed programs (only remote
spawn/ask remain, genuinely out of scope).
**Symptom (before fix):** `Error: AOT unsupported: multi-perform resuming handler with perform sites across multiple blocks is not yet supported by the native backend`
**Source of truth:** `src/aot/codegen.rs` (compile_mir_function_body + helpers).

## Problem statement

The AOT native backend (`--backend native`, Cranelift) compiles *resuming*
effect handlers — a `perform E.op(args)` with a `| E.op(x) resume => body`
handler — as intra-function continuations. This works when a given resuming
handler body is performed from **one** MIR block (possibly multiple times in
that block). It is rejected when the same resuming handler body is performed
from **different** MIR blocks.

Confirmed failing pattern (realistic, not contrived):
```nula
effect Echo { run: Int -> Int }
fn f(cond: Bool) -> Int {
  handle {
    if cond then { perform Echo.run(1) } else { perform Echo.run(2) }
  } { | Echo.run(x) resume => x + 10 }
}
```
The two `Echo.run` sites live in the if/else branch blocks → different MIR
blocks → the guard at `codegen.rs` fires.

## Why this matters

This is the **last reachable correctness gap** in the AOT backend. Everything
else that rejects is either distribution (remote spawn/ask, genuinely out of
scope — no network transport), or dead defensive arms (`BinOp::Assign/Range/
Pipe`, `UnOp::Deref/Ref`, internal constants, `Resume`/`ReceiveCommit`
RValues no lowering emits). Conditional/branching `perform` of a resuming
handler is a natural Erlang/effect idiom (generators, `yield`, conditional
continuations), so closing this removes the backend's final functional
limitation and unblocks the "faster than Go" story for effect-heavy code.

## Current same-block model (how it works today)

All in `compile_mir_function_body` (`src/aot/codegen.rs`):

- `resuming_perform_count` — counts resuming performs per handler body.
- `multi_cont_bodies` — bodies invoked from ≥2 perform sites. Their
  `Terminator::Resume` must dispatch on a **continuation-index block param**.
- `handler_threaded_dsts` — per body, the destination registers of all its
  resuming performs in program order, **minus the last**. These are the
  "prior perform results" a later continuation may read.
- Per-block `cont_thread: Vec<u32>` — a **local** variable reset for every MIR
  block: the destination regs of prior resuming performs **in that block**.
- Each perform site:
  - creates a continuation block `cont` with params `[resume_value,
    prior_results...]` (1 + `cont_thread.len()`);
  - jumps to the handler body with `[effect_args, block_liveins, index,
    cont_thread_values...]` — the index is `handler_continuations[body].len()`
    before push.
- The handler body's `Terminator::Resume` (`compile_terminator_with_params`)
  dispatches on the index param: continuation `i` receives the resume value +
  `threaded[..i]` (the first `i` threaded prior results).

**The single-block invariant:** `handler_threaded_dsts` for a one-block body
equals that block's `cont_thread`, so the jump-arg count always matches the
handler body's threaded-slot count. `cont_thread` starts empty at each block
and grows as same-block performs are compiled, and the continuation-index +
threaded-slot positions line up exactly.

## Why multi-block fails

`cont_thread` is per-block, but `handler_threaded_dsts` is per-body (global).
For a body performed from two different blocks:
- the handler body's threaded-slot count = global prior count (from *all*
  blocks);
- a single block's perform site passes only its *own* block's `cont_thread`
  entries → jump-arg count ≠ handler param count → the guard rejects.

The guard (`sites.len() > 1` across distinct blocks) is the safety — it
rejects rather than mis-compile.

The deeper issue: cross-block prior perform results are not carried by the
CFG. The handler-return path (perform → handler body → Resume → continuation
block) is a codegen artifact, invisible to MIR `compute_liveins`, so a prior
perform result from block A is not a live-in of block B and cannot be passed
to block B's perform's jump.

## Design

### Key observation (makes the common case simple)

In the common conditional pattern (`if cond then perform A else perform B`),
the branches are **exclusive** — neither continuation reads the *other*
branch's perform result. Cross-block prior results are simply not live into
the other branch. So the threading requirement per site is just:
- the resume value, plus
- the *same-block* prior results (already handled by `cont_thread`), plus
- the current block's live-ins that the continuation still needs (already
  accessible via `local_vals` in the jump-arg construction).

The failure is only a **mismatch in threaded-slot sizing/count**, not a
missing data dependency. Making the threaded-slot count uniform (max across
sites, not the global n−1) and padding per-site values to that width fixes the
common case without any cross-block data flow.

### Recommended approach (phased)

**Phase 1 — uniform threaded slots (covers exclusive-branch / no-cross-block-read patterns):**
1. Size the handler body's threaded slots to the **max** `cont_thread` length
   across all of its perform sites, not the global `n−1`. Implement by
   deriving a per-body max from the MIR (each block's performs counted
   separately), replacing the flat `handler_threaded_dsts`.
2. At each perform site, build the jump's threaded args to exactly that width:
   the site's same-block prior results (its `cont_thread`), then dummies (0)
   to the max.
3. The handler `Resume` already forwards `threaded[..i]` to continuation `i`;
   continuation `i` reads only the slots it actually uses. With uniform width,
   the arg counts align and unused slots are harmless.
4. **Remove the guard** for bodies whose cross-block prior results are never
   read across blocks. Determine this conservatively: if every perform site's
   dst is dead at every other site's block (no cross-block read), the uniform
   dummy-padding is sound.

**Phase 2 — true cross-block result reads (loop-carried / accumulated prior results):**
Only needed when a later block's continuation reads an earlier *different*
block's perform result (e.g. `var acc = 0; handle { while c { acc =
perform E.go(acc) } }` where the next iteration reads the previous result).
Approach:
1. Extend `compute_liveins` with a synthetic "thread state": treat each
   body's prior-perform result as a value live into any successor block that
   (a) contains a later perform of the same body, or (b) is reached via the
   handler-return path. This makes prior results block live-ins.
2. Each perform site supplies the prior results available in its block
   (live-ins + same-block), the handler forwards them to the continuation,
   and the continuation reads them from its params. This generalizes Phase 1:
   uniform width = total prior performs, per-site availability = what's live
   into the block.
3. This is the harder half — it threads state through the existing
   `block_params`/`block_liveins` machinery and must handle merge points where
   two blocks with different prior-result states join.

**Recommendation:** land Phase 1 first (covers the realistic conditional-
perform cases, is a contained change to the threading bookkeeping), verify
with the fuzzer + targeted tests, then assess whether Phase 2 patterns are
common enough to justify the live-in machinery work.

### Implementation notes (Phase 1 landed, `758ce4d`)

- `handler_threaded_dsts` (flat per-body global list) replaced by
  `handler_threaded_width` — a per-body usize = max over blocks of
  (sites-in-block − 1). Each site supplies that many threaded values (real
  same-block priors, then dummies).
- Continuation blocks and the `Resume` dispatch now carry the FULL uniform
  threaded set to every continuation; each continuation binds only its
  same-block prior slots (excess params unused).
- `compute_liveins` excludes effect-handler body blocks entirely: they are
  reached by `perform` jumps (not normal merges), read only effect params
  (already block params) + dominance-scoped outer locals, and must not
  inherit a perform block's post-perform locals — most importantly the
  perform result dst, which flows BACK through the continuation, not into
  the handler.
- Cross-block prior READ (e.g. `if c { acc = perform E(1) } ; perform
  E(2); acc + ...`) is rejected by a new liveness guard
  (`cross_block_perform_read`, proper gen/kill backward dataflow): fires
  when a site's block is live-in with a value defined in another site block
  of the same body. Anything the guard misses still fails loudly in the
  CLIF verifier rather than mis-computing.
- Tests: exclusive-branch if/else, discarded-first-result (sequential sites
  that must compile), cross-block prior-read rejection.

### Phase 2 investigation — the general SSA phi bug (`8d0c286`)

Attempting Phase 2 (the `if c { acc = perform E(1) }; perform E(2); acc + b`
pattern) exposed that the representative case hits a GENERAL AOT bug, not an
effect-specific one: **a `var` assigned in ONE branch and read after a merge
fails even with no effects** (`var acc = 0; if cond { acc = 1 }; acc + 5`
→ CLIF verifier error; interp=11). Root cause: `compute_liveins` used only
the intersection of predecessor def-sets, so a local defined in a subset of
predecessors (with others carrying the flowed-through prior value) got no
block param.

Fixed in `8d0c286`:
- `compute_liveins` now also adds a block param for every local live into a
  >1-predecessor block AND defined in at least one predecessor (a value live
  into the merge is automatically live-out of every predecessor, so all preds
  supply it).
- `compute_live_ins` (gen/kill liveness) terminator operands were added to
  `gen` unconditionally even when defined earlier in the same block — wrongly
  marking a block's own branch-cond live into a loop back-edge predecessor.
  Terminator uses are now filtered by the block's kill set.
- Test: `test_aot_mutable_var_assigned_in_branch_read_after_join`.

This makes the NON-effect variant of the Phase 2 pattern work. The REMAINING
Phase 2 work is the effect-continuation half: even with `acc` a proper block
param of the later perform's block, the perform's continuation block (a
successor of the handler body) is NOT dominated by that block, so it still
can't read `acc` unless the value is threaded through the handler's Resume
dispatch into the continuation's params.

**Narrowed Phase 2 — implemented (`c51df8e`):**
1. `continuation_live_ins` computes, per resuming perform site, the register
   set live at the continuation entry via a backward liveness walk from each
   block's live-out — over NORMAL successors only (the handler body is not a
   real flow successor, so its effect params must not count as live into the
   perform block).
2. `resuming_threading` derives per-site "extras" (continuation live-ins minus
   the site's dst and same-block priors) and the per-body uniform width =
   max over sites of (priors + extras).
3. The handler body allocates threaded slots whenever any site has them
   (width > 0), not just for multi-cont bodies — a single-site loop
   continuation still needs them (this was a regression found and fixed: the
   single-continuation `Resume` path must forward the threaded set too, not
   just the resume value).
4. Each site packs [same-block priors, extras] padded to width; the
   continuation binds both; `Resume` forwards the full threaded set on both
   the single- and multi-continuation paths. `cross_block_perform_read` removed.

The general phi fix (8d0c286) is the prerequisite: it makes the cross-block
value a proper merge block param, so it is available at the site to thread.
Verified patterns: exclusive if/else (23), same-block multi (23/28),
discarded-first sequential (24), cross-block prior read (35), pre-perform
compute read cross-block (34), loop-carried resuming (3) — all match the
interpreter.

## Files / functions to touch

- `src/aot/codegen.rs`:
  - `handler_threaded_dsts` — change from flat global to per-body *max
    per-block width* (Phase 1) or keep and add per-site availability (Phase 2).
  - The perform-site jump-arg construction (~L1544-1660) — pad threaded args
    to uniform width.
  - The `Resume` dispatch (`compile_terminator_with_params` ~L404) — no change
    needed beyond the uniform width (it already forwards `threaded[..i]`).
  - The guard at ~L1190 — relax (Phase 1) or remove + handle live-ins (Phase 2).
  - `compute_liveins` (~L305) + `block_liveins` consumers (~L1249) — Phase 2.

## Test plan

- `test_aot_resuming_handler_multi_perform` (exists, same-block) must stay
  green — the refactor must not disturb the single-block path.
- New: `test_aot_resuming_handler_multi_block_if_else` — the confirmed
  `if cond then perform 1 else perform 2` pattern, both `cond=true/false`.
- New: conditional perform where the handle body reads both branch results
  into a sum (verifies continuations land correctly in both branches).
- New: a loop-body resuming perform (single block — should already work; guards
  the "don't over-reject" direction).
- Differential fuzzer: add an effect/conditional-perform seed so the fuzzer
  exercises multi-block resuming across interpreter/AOT/WASM.

## Risks / open questions

- **Cranelift block-param count** across the handler body: uniform width must
  stay consistent between the perform-site jumps and the handler's block
  params, or CLIF verification fails at finalize time. The existing same-block
  code already builds these arg lists by hand — extend carefully.
- **Phase 1 soundness:** must prove no cross-block prior-result read exists
  before dummy-padding. A conservative dead-dst analysis is the guard; if it
  ever over-rejects, Phase 2 is the fallback rather than a silent wrong value.
- **Resume dispatch chain** (`brif` fall-through blocks at ~L430) copies the
  threaded vector through intermediate blocks — verify the uniform width flows
  through that chain unchanged.
- **Interaction with `handler_continuations` ordering:** the continuation
  index is assigned in *compilation order* of blocks (`block_order`), not
  source order. For multi-block, the index must map to the same per-body
  global perform order the threaded slots assume — currently both use
  per-body program order, which must be preserved across blocks.
