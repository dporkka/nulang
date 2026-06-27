# Project: Nulang Codebase Analysis

## Architecture
- Target repository: `/home/dporkka/dev/nulang`
- Core Modules: Compiler pipeline, typechecker, effect checker, escape analysis, VM/JIT runtime.
- Input files: `src/**/*.rs`, design files (`DESIGN_*.md`, `SPEC*.md`).
- Output: `codebase_analysis_report.md` at the project root.

## Milestones
| # | Name | Scope | Dependencies | Status | Conv ID |
|---|------|-------|-------------|--------|---------|
| 1 | Explore 1: Architecture & Specs | Analyze compiler, typechecker, effect checker, escape analysis, VM/JIT, spec alignment. | None | DONE | 40cd1abe-9693-46fa-88d4-11b2fc5aa725 |
| 2 | Explore 2: Performance | Identify at least 3 concrete areas for performance optimization with snippets. | None | DONE | fd28ab26-f29b-4a78-9d0c-e1fa9981af59 |
| 3 | Explore 3: Quality & Tests | Identify at least 5 areas for code quality/Rust idioms; test coverage gaps. | None | DONE | e787925c-c038-4923-9863-0c88625b9628 |
| 4 | Write Analysis Report | Synthesize Explorer findings, write codebase_analysis_report.md. | M1, M2, M3 | DONE | 7a3d2451-5c6d-4a60-8115-1cf0c54e130c |
| 5 | Verify & Run Tests | Run verify_report.py, review output correctness. | M4 | DONE | 523da1fa-7cf7-4820-95f9-883e172a0389 |

## Interface Contracts
- **Explorers** must output individual Markdown handoff files detailing files, functions, code snippets, and rationale in their respective directories.
- **Worker** must consume all Explorer reports and generate the unified report `codebase_analysis_report.md` complying exactly with `verify_report.py`.
- **Reviewer** must verify the report content and execute `verify_report.py` to ensure it exits with code 0.
