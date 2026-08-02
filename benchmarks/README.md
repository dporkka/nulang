# Benchmark Results

Machine-readable Criterion output from CI's `bench` job, one JSON file per
`main`-branch commit that ran it. Human-readable HTML reports (Criterion's
own `target/criterion/**/report/index.html`) are not checked in — they're
large and regenerate trivially from the raw estimates here.

## What's tracked

Each `<short-sha>.json` is the concatenated `estimates.json` output Criterion
writes per benchmark under `target/criterion/<bench>/<function>/`, one line
per benchmark, produced by `scripts/collect_bench_results.py` (see that
script for the exact schema: `{benchmark, mean_ns, mean_ns_lower,
mean_ns_upper}` per line).

## Status: informational only, not yet regression-gated

The CI `bench` job (`.github/workflows/ci.yml`) runs `cargo bench` on every
push to `main` and uploads results here plus as a build artifact. It does
**not** currently fail a PR on regression. PLAN.md Phase 1 bullet 3 calls
for a >5% regression gate; that's deliberately not wired yet because
GitHub Actions' shared runners have enough run-to-run noise (commonly
20-50%+ on wall-clock-sensitive benchmarks, from neighbor CPU/cache
contention) that a naive fixed threshold would be flaky — failing PRs for
noise, not real regressions, which erodes trust in the gate faster than
having no gate at all. Closing this properly needs either:

- A dedicated, reserved (non-shared) runner for benchmark jobs, or
- A statistical comparison against a rolling window of recent `main`
  results (not just the immediately preceding commit) with a
  noise-aware significance test, not a flat percentage cutoff.

Until one of those lands, treat this directory as "real, measured numbers
you can look at," not "an enforced regression gate." `PERFORMANCE_ANALYSIS.md`
should cite numbers from here, not estimates.
