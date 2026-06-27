# Plan — Codebase Analysis & Improvement Report

This plan details the steps to compile and verify the comprehensive codebase analysis and improvement report for `nulang`.

## Phase 1: Planning and Setup
- [x] Create `BRIEFING.md`
- [ ] Create `plan.md`
- [ ] Create `progress.md`
- [ ] Create `PROJECT.md` (milestone decomposition)
- [ ] Start heartbeat cron

## Phase 2: Exploration & Analysis (Parallel Workers)
- [ ] Spawn Explorer 1 (`teamwork_preview_explorer`) to analyze:
  - Compiler pipeline, typechecker, effect checker, escape analysis, VM/JIT runtime.
  - Alignment of implementation with specifications (`DESIGN_AI_SDK.md`, `DESIGN_WEB_FRAMEWORK.md`, `SPEC.md`, `SPEC2.md`, etc.).
- [ ] Spawn Explorer 2 (`teamwork_preview_explorer`) to identify:
  - At least 3 concrete performance optimization areas (e.g., VM loop, JIT compilation, allocation patterns) with code snippets/paths.
- [ ] Spawn Explorer 3 (`teamwork_preview_explorer`) to identify:
  - At least 5 code quality and Rust idiom improvements with code snippets/paths.
  - Verification & test coverage gaps (unit, integration, stress tests).

## Phase 3: Synthesis & Report Writing
- [ ] Collect Explorer handoffs.
- [ ] Spawn Worker (`teamwork_preview_worker`) to:
  - Synthesize the findings into `/home/dporkka/dev/nulang/codebase_analysis_report.md`.
  - Format the report with the 5 required sections.
  - Include specific file names, functions/structs, and code snippets (before/after refactored versions).
  - Include checklists of actionable steps.

## Phase 4: Review and Verification
- [ ] Spawn Reviewer (`teamwork_preview_reviewer`) to:
  - Run the python verification script `verify_report.py`.
  - Verify that the report meets all the user's checklist items (5 sections, 3+ performance opts, 5+ code quality opts, valid paths, length).
  - Provide feedback if any validation fails.

## Phase 5: Handoff and Completion
- [ ] Ensure all verification tests pass.
- [ ] Write the final orchestrator handoff.
- [ ] Report completion to the user.
