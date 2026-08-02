#!/usr/bin/env python3
"""Collect Criterion benchmark results into one compact JSON file.

Walks `target/criterion/<group>/<function>/new/estimates.json` (Criterion's
per-benchmark output after a `cargo bench` run) and writes one JSON object
per line to the output file: `{benchmark, mean_ns, mean_ns_lower,
mean_ns_upper}`. `benchmark` is `<group>/<function>` (matching how
criterion's own `--bench-filter`/report paths name things).

Criterion's `new/estimates.json` schema (stable across the 0.5.x series):
    {
      "mean": {
        "confidence_interval": {"lower_bound": F, "upper_bound": F, ...},
        "point_estimate": F,
        "standard_error": F
      },
      "median": {...same shape...},
      ...
    }
`point_estimate`/`lower_bound`/`upper_bound` under "mean" are nanoseconds
per iteration (criterion's internal unit regardless of what the HTML
report displays scaled to).

Usage:
    collect_bench_results.py --criterion-dir target/criterion --out FILE
"""
import argparse
import json
import sys
from pathlib import Path


def collect(criterion_dir: Path) -> list[dict]:
    results = []
    # Layout: target/criterion/<group>/<function>/new/estimates.json
    # A group with no sub-function (single free-standing bench_function
    # call) instead lands directly at <group>/new/estimates.json — handle
    # both by searching for every "new/estimates.json" and deriving the
    # benchmark name from its parent chain up to (but excluding) "criterion".
    for estimates_path in sorted(criterion_dir.glob("**/new/estimates.json")):
        try:
            data = json.loads(estimates_path.read_text())
        except (json.JSONDecodeError, OSError) as e:
            print(f"warning: skipping unreadable {estimates_path}: {e}", file=sys.stderr)
            continue

        mean = data.get("mean", {})
        point = mean.get("point_estimate")
        if point is None:
            print(f"warning: no mean.point_estimate in {estimates_path}", file=sys.stderr)
            continue
        ci = mean.get("confidence_interval", {})

        # Parent of "new" is the function dir; its parent (relative to
        # criterion_dir) is the benchmark's full name path.
        rel = estimates_path.parent.parent.relative_to(criterion_dir)
        benchmark_name = str(rel).replace("\\", "/")  # posix-style on any OS

        results.append(
            {
                "benchmark": benchmark_name,
                "mean_ns": point,
                "mean_ns_lower": ci.get("lower_bound"),
                "mean_ns_upper": ci.get("upper_bound"),
            }
        )
    return results


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--criterion-dir", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()

    if not args.criterion_dir.is_dir():
        print(f"error: {args.criterion_dir} is not a directory", file=sys.stderr)
        return 1

    results = collect(args.criterion_dir)
    if not results:
        print(f"error: no benchmark estimates found under {args.criterion_dir}", file=sys.stderr)
        return 1

    args.out.parent.mkdir(parents=True, exist_ok=True)
    with args.out.open("w") as f:
        for r in results:
            f.write(json.dumps(r, sort_keys=True))
            f.write("\n")

    print(f"Wrote {len(results)} benchmark result(s) to {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
