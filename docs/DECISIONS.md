# Decision Records

Architecture/design decision records for Nulang. Each entry is immutable
once accepted; reversals get a new DR that supersedes the old one.

---

## DR-001: Hybrid Rendering — No Custom Renderer

**Status:** accepted

**Decision.** Nulang's UI story is hybrid rendering per platform, with no
custom renderer:

- **Web:** render to the DOM.
- **Mobile:** render to native platform controls.
- **Desktop:** render into the system WebView (Tauri-style embedding).

**Rationale.** A custom renderer (own text/layout/compositing/GPU stack)
is a multi-year, multi-team project that duplicates what platform vendors
already optimize, and it permanently lags on accessibility, input methods,
internationalization, and platform look-and-feel. Using each platform's
strongest built-in surface keeps the language kernel small (see the Core
Admission Rule in CONTRIBUTING.md) and lets rendering evolve as a set of
capability packages rather than a frozen core feature.

**Consequences.** Cross-platform UI code targets a common component
abstraction that lowers to DOM / native controls / WebView depending on
target. Pixel-identical rendering across platforms is explicitly a
non-goal; behavioral parity is the goal.

---

## DR-002: Backend Strategy — Cranelift-Only Until Evidence Says Otherwise

**Status:** accepted with gates

**Decision.** The compiler uses **Cranelift only** for both JIT and AOT
compilation. An LLVM (or other heavyweight) release backend is revisited
for `nulang build --release` only when **both** gates are met:

1. **(a)** The differential fuzzer is backend-parameterized, so two
   backends can be checked for semantic equivalence against each other.
2. **(b)** Measured workload evidence shows the Cranelift machine-layer
   ceiling matters for real Nulang workloads (not synthetic microbenchmarks
   alone).

Until then, a second backend is unjustified complexity: two code generators
to keep semantically identical, two sets of bugs, and a release pipeline
whose gains are unmeasured.

**What Cranelift omits** (the known machine-layer ceiling we are accepting):

- **Microarchitecture-specific instruction scheduling** — Cranelift does
  not model per-CPU pipelines; hot loops may leave single-digit-percent
  cycles on the table on specific parts.
- **Greedy/advanced register allocation** — its fast regalloc trades
  allocation quality for compile speed; no graph-coloring or
  live-range-splitting tier tuned for long AOT compiles.
- **Addressing-mode synthesis** — less aggressive folding of complex
  address computations into single instructions.
- **Auto-vectorization** — no automatic SIMDization of scalar loops; SIMD
  must come from explicit intrinsics or library code.
- **PGO-guided layout** — no profile-driven function ordering, branch
  layout, or inlining hints baked into the AOT artifact.

**Consequences.** `nulang build` (JIT and AOT) targets Cranelift in all
modes. If the gates are met, an LLVM backend is scoped to
`nulang build --release` only; Cranelift remains the JIT backend
regardless, since compile latency dominates there.

---

## DR-003: Execution-Placement Inference — Only Pure, Capability-Free Code Moves Automatically

**Status:** accepted

**Decision.** The compiler may automatically move a computation to a
different execution location (another actor, node, edge region, or cloud
placement) **only if** that computation is:

1. **pure** — no effects in its inferred effect row (Chapter 4), and
2. **capability-free** — it requires no authority capabilities
   (Chapter 5) at its new location.

Everything else — any computation with effects, capability requirements,
or placement-sensitive behavior — requires an **explicit placement
annotation** from the programmer, which the compiler then **verifies**
against the inferred effect row and capability requirements.

**Rationale.** Inference that changes observable semantics is a
miscompilation, not an optimization. Placement is observable: moving a
computation changes its latency profile, its failure modes (a remote call
can partition; a local one cannot), and the capabilities available at the
destination. A compiler that silently relocates effectful or
capability-bearing code is changing what the program means. Pure,
capability-free computations have no location-dependent observable
behavior, so moving them is a true optimization with no semantic change.

**Consequences.** Placement annotations are part of the checked program
surface; the compiler rejects annotations inconsistent with inferred
effects/capabilities. Auto-placement of pure code is always safe to apply
and safe to revert.

---

## DR-004: Memory Strategy — Bump Arenas Integrate With ORCA, Not Replace It

**Status:** accepted

**Decision.** Memory management is two cooperating mechanisms:

- **ORCA per-actor garbage collection** remains the collector of record
  for cross-message data and all live actor state.
- **Message-scoped bump arenas** are used for allocations provably
  non-escaping under the `iso` reference capability during processing of a
  single message; the whole arena is freed when the message completes.

**Rationale.** Most allocations during message handling are transient —
they die when the message completes. For allocations the compiler can
prove non-escaping (iso capability + escape analysis), a bump arena turns
allocation into pointer-bump and reclamation into a single pointer reset,
with no GC tracing at all. But anything that escapes — into actor state,
into another message, into another actor — needs real ownership and
collection, which is exactly what ORCA provides. Replacing ORCA with
arenas would either leak (escape analysis is conservative) or require
invasive lifetime annotations; integrating arenas *with* ORCA gets the
fast path where it's provable and keeps the general collector where it
isn't.

**Consequences.** The arena is an allocation-region optimization layered
on top of ORCA's ownership discipline: an allocation is arena-eligible
only when non-escape is proven, and any failed proof falls back to normal
ORCA-managed allocation. No language-surface change; this is an
implementation strategy pinned here because it constrains runtime design.
