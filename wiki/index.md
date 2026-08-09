# Nulang Wiki — Index

The browsable, LLM-maintained knowledge base for Nulang. See [[log]] for the change history and the [[wiki-updater skill|../.claude/skills/wiki-updater/SKILL.md]] for how this wiki is curated.

**Authoritative sources** (never edited by wiki ops): `AGENTS.md` (architecture contract), `SPEC2.md` (language spec), `RFC/*.md` (proposals), `GOVERNANCE.md` (stability tiers), `CHANGELOG.md`, `src/*.rs` (code).

---

## Overview

- [[overview/architecture-overview]] — top-level map of Nulang's subsystems and how they compose.
- [[overview/compiler-pipeline]] — AST → HIR → MIR → bytecode / WASM / AOT native.

## Subsystems

_(to be seeded on ingest — one page per major subsystem. Candidates: bytecode-vm, jit, actor-runtime, distribution, wasm-backend, aot-backend, ai-runtime, lsp, python-interop, ffi, package-manager, format-layer.)_

## Concepts

_(to be seeded on ingest — one page per conceptual entity. Candidates: capability-lattice, effect-rows, orca-gc, nan-tagging, work-stealing-scheduler, gossip-membership, delta-crdts, non-blocking-llm-suspend, selective-receive.)_

## Sources

_(summaries of ingested external docs — RFCs, blog posts, design notes. Empty at bootstrap.)_

## Queries

- [[queries/performance-assessment]] — assessment of the 28-proposal performance catalog plus beyond-catalog techniques (threaded dispatch, OSR, NUMA, cache-line padding, non-temporal stores, Auto-SoA) against the current tree; 14/28 shipped, 5 actionable gaps ranked.

---

## How to grow this wiki

Ask Claude Code to invoke the `wiki-updater` skill:

- **"Ingest RFC-0007 into the wiki"** — reads the RFC, updates relevant pages, logs it.
- **"Refresh the actor-runtime wiki page from source"** — re-reads `src/runtime/`, updates the subsystem page, fixes stale `path:line` citations.
- **"Lint the wiki"** — health-check: stale citations, orphan pages, contradictions, coverage gaps.
- **"How does non-blocking LLM.ask work? File the answer."** — answers the question and files it under `wiki/queries/`.

The wiki grows by ingest, not by front-loading. Every operation appends an entry to [[log]].
