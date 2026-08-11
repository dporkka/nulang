# AOT native backend — multi-block resuming effect handlers (scoping)

**Status:** Scoping / design note. Problem confirmed, implementation not started.
**Symptom:** `Error: AOT unsupported: multi-perform resuming handler with perform sites across multiple blocks is not yet supported by the native backend`
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
