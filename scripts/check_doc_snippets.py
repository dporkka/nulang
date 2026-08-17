#!/usr/bin/env python3
"""check_doc_snippets.py — verify fenced ```nulang code blocks in docs.

Extracts every ```nulang fenced block from SPEC2.md, README.md, and
docs/**/*.md, writes each block to a temp file, and runs
`$NULANG_BIN --check <file>` on it. Prints one PASS/FAIL line per snippet
with its source file:line, and exits 1 if any snippet fails.

A block is SKIPPED (never silently — it is reported) when:
  - its opening fence is ```nulang,no-check   (explicit opt-out), or
  - it contains a line that is exactly `// ...` or `...` (illustrative
    ellipsis — the block is a fragment by declaration).

Usage:
    python3 scripts/check_doc_snippets.py [--parse-only] [FILE ...]

    --parse-only   Use the CLI's parse-only mode instead of `--check`
                   when the CLI supports one; the current CLI has none,
                   so this falls back to plain `--check`.
    FILE ...       Restrict scanning to these files (default: SPEC2.md,
                   README.md, docs/**/*.md).

Environment:
    NULANG_BIN     Path to the nulang binary
                   (default: target/debug/nulang).

Examples:
    NULANG_BIN=./target/release/nulang python3 scripts/check_doc_snippets.py
    python3 scripts/check_doc_snippets.py README.md
"""

import argparse
import glob
import os
import re
import subprocess
import sys
import tempfile

FENCE_RE = re.compile(r"^```(nulang(?:,no-check)?)\s*$")
ELLIPSIS_RE = re.compile(r"^(// \.\.\.|\.\.\.)$")


def default_files():
    files = ["SPEC2.md", "README.md"]
    files.extend(sorted(glob.glob("docs/**/*.md", recursive=True)))
    return [f for f in files if os.path.isfile(f)]


def extract_blocks(path):
    """Yield (lineno, code, skip_reason) for each nulang block in path."""
    blocks = []
    in_block = False
    no_check = False
    start = 0
    buf = []
    with open(path, encoding="utf-8") as fh:
        for i, line in enumerate(fh, start=1):
            stripped = line.rstrip("\n")
            if not in_block:
                m = FENCE_RE.match(stripped)
                if m:
                    in_block = True
                    no_check = m.group(1).endswith("no-check")
                    start = i + 1
                    buf = []
            elif stripped.startswith("```"):
                code = "\n".join(buf) + "\n"
                reason = None
                if no_check:
                    reason = "no-check fence"
                elif any(ELLIPSIS_RE.match(b.strip()) for b in buf):
                    reason = "ellipsis fragment"
                blocks.append((start, code, reason))
                in_block = False
            else:
                buf.append(stripped)
    return blocks


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--parse-only", action="store_true",
                    help="use the CLI's parse-only check mode if available")
    ap.add_argument("files", nargs="*", default=None)
    args = ap.parse_args()

    nulang = os.environ.get("NULANG_BIN", "target/debug/nulang")
    if not os.path.exists(nulang) and os.sep in nulang:
        print(f"error: nulang binary not found at {nulang!r}; "
              "set NULANG_BIN or build first (cargo build)", file=sys.stderr)
        return 2

    files = args.files if args.files else default_files()

    # The CLI's check mode is `nulang --check <FILE>` (type-check, don't
    # run); there is no parse-only mode in the current CLI, so --parse-only
    # probes `nulang --help` for one and otherwise falls back to --check.
    check_args = ["--check"]
    if args.parse_only:
        try:
            help_out = subprocess.run(
                [nulang, "--help"],
                capture_output=True, text=True, timeout=30).stdout
        except Exception:
            help_out = ""
        for flag in ("--parse-only", "--parse", "--no-typecheck"):
            if flag in help_out:
                check_args = [flag]
                break
        # else: no parse-only mode supported; --check is the fallback.

    npass = nfail = nskip = 0
    failures = []
    tmpdir = tempfile.mkdtemp(prefix="nulang-doc-snippet-")
    for path in files:
        for lineno, code, reason in extract_blocks(path):
            label = f"{path}:{lineno}"
            if reason:
                print(f"SKIP {label} ({reason})")
                nskip += 1
                continue
            snippet = os.path.join(tmpdir, f"snippet_{npass + nfail}.nula")
            with open(snippet, "w", encoding="utf-8") as fh:
                fh.write(code)
            try:
                proc = subprocess.run(
                    [nulang] + check_args + [snippet],
                    capture_output=True, text=True, timeout=120)
                ok = proc.returncode == 0
                err = (proc.stderr or proc.stdout).strip()
            except subprocess.TimeoutExpired:
                ok, err = False, "timed out after 120s"
            if ok:
                print(f"PASS {label}")
                npass += 1
            else:
                print(f"FAIL {label}")
                first = err.splitlines()[0] if err else "(no output)"
                print(f"     {first}")
                failures.append(label)
                nfail += 1

    print()
    print(f"{npass} passed, {nfail} failed, {nskip} skipped "
          f"across {len(files)} file(s)")
    if failures:
        print("failing snippets:", file=sys.stderr)
        for f in failures:
            print(f"  {f}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
