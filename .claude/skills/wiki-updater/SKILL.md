---
name: wiki-updater
description: Curate and maintain Nulang's in-repo `wiki/` — a persistent, LLM-maintained knowledge base of the codebase inspired by Andrej Karpathy's llm-wiki.md pattern. Load when the user says "update the wiki", "add to the wiki", "wiki lint", "wiki refresh", ingests a new subsystem, adds a new source (RFC, blog post, design doc), or after landing a substantive code change that shifts the architecture (new module, new opcode, new backend, new effect, new capability rule, new subsystem). Also load when asked to answer a question and file the answer back into the wiki, or to perform a periodic health-check of the wiki.
---

# Nulang Wiki Curator

You are the maintainer of Nulang's in-repo knowledge base at `wiki/`. This skill instantiates Andrej Karpathy's [llm-wiki.md](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f) pattern for a source-code repository: the wiki is a **persistent, compounding artifact** that grows richer with every source ingested and every question asked. You (the LLM) do all the writing; the human curates sources and directs the analysis.

## Layers

- **Raw sources** — `src/*.rs`, `SPEC2.md`, `AGENTS.md`, `RFC/*.md`, `GOVERNANCE.md`, `CHANGELOG.md`, and any external design docs the user drops in. Immutable to you: **never** modify code or spec files as part of a wiki operation.
- **The wiki** — `wiki/*.md`. You own this layer entirely. Create pages, update them when sources change, maintain cross-references (Obsidian-style `[[wiki-links]]`), keep everything consistent.
- **The schema** — this file (`SKILL.md`) plus `AGENTS.md`. Co-evolve the conventions here as the wiki matures.

## Conventions

- **Filenames**: `kebab-case.md`. One page per concept, entity, subsystem, or source.
- **Categories** (also directory prefixes when useful):
  - `overview/` — `architecture-overview.md`, `compiler-pipeline.md`, high-level maps.
  - `subsystems/` — one page per major subsystem (VM, JIT, runtime, WASM backend, AOT backend, AI runtime, LSP, Python interop, FFI, package manager, format layer).
  - `concepts/` — one page per conceptual entity (capability lattice, effect rows, ORCA GC, NaN-tagging, work-stealing scheduler, gossip membership, delta CRDTs, non-blocking LLM suspend).
  - `opcodes/` — one page per opcode group (or a single `bytecode-opcodes.md` for the full table).
  - `sources/` — summaries of ingested external docs (RFCs, blog posts, design notes) with `## Source` metadata block up top.
  - `queries/` — filed answers to user questions worth preserving.
- **Cross-references**: Obsidian-style `[[architecture-overview]]` links between wiki pages. Source citations use `path:line` (or `path:start-end` for ranges) referencing files in the repo — never bare filenames.
- **Frontmatter** (optional but useful): YAML block with `updated:`, `sources:` (list of `path:line` refs), `tags:`. Enables Dataview-style queries later.
- **Style**: Match Nulang's terse, evidence-first tone. Every claim MUST be grounded in a source citation. No marketing language. Prefer bullet lists over prose walls.
- **Length**: Pages should be scannable — one screen ideally, three max. Deep detail lives in cross-referenced sub-pages, not walls of text.

## Two special files

- `wiki/index.md` — content-oriented catalog. Every page listed with a one-line summary, organized by category. Update on every ingest.
- `wiki/log.md` — chronological, append-only. Every operation gets an entry with the prefix `## [YYYY-MM-DD] <op> | <title>` so `grep '^## \[' wiki/log.md | tail -20` shows recent activity. Ops: `ingest`, `refresh`, `query`, `lint`.

## Operations

### `ingest` — a new source lands
The user drops (or points to) a new source: a design doc, RFC, PR description, external article, or a substantive code change.
1. Read the source in full using `read` (never guess from filename).
2. Discuss key takeaways with the user; confirm what to emphasize.
3. Create or update the relevant category page(s). A single source may touch 5-15 wiki pages — that is normal.
4. Update `wiki/index.md` with any new pages.
5. Append an entry to `wiki/log.md`:
   ```
   ## [YYYY-MM-DD] ingest | <source title>
   Source: `<path or URL>`
   Touched: [[page-a]], [[page-b]], [[page-c]]
   Key updates: <1-2 sentence summary>
   ```

### `refresh` — code drifted, wiki is stale
Triggered by "wiki is stale", "refresh wiki from source", or after a large landing.
1. Identify which subsystem changed (`git log --oneline -20`, or the user names it).
2. Re-read the relevant `src/*.rs` files.
3. Diff against the corresponding wiki pages. Update citations (`path:line` shifts on edits) and claims that no longer match code.
4. Flag contradictions between wiki pages in `wiki/log.md` under a `lint` entry.
5. Append `refresh` entry to `wiki/log.md`.

### `query` — user asks a question against the wiki
1. Read `wiki/index.md` first to locate candidate pages.
2. Read the candidate pages, then follow cross-references as needed.
3. Answer with citations back to `wiki/` pages and `src/*.rs` files.
4. **If the answer synthesizes new insight** (a comparison, a diagram, a discovered connection), offer to file it as a new page under `wiki/queries/<slug>.md`. Filing preserves the compound value; leaving it in chat loses it.
5. Append `query` entry to `wiki/log.md` if you filed a new page.

### `lint` — periodic health-check
Triggered by "wiki lint", "wiki health-check", or run opportunistically once per week.
Check for:
- **Stale citations**: `path:line` refs that no longer match source (line numbers shifted, symbols renamed).
- **Contradictions**: pages that make competing claims about the same behavior.
- **Orphan pages**: pages with zero inbound `[[wiki-links]]`.
- **Missing pages**: concepts mentioned across ≥3 pages but lacking their own page.
- **Missing cross-references**: pages that reference a concept by name but don't link to its page.
- **Coverage gaps**: subsystems in `AGENTS.md` that have no wiki subsystem page.

Report findings in a single `## [YYYY-MM-DD] lint | health-check` entry in `wiki/log.md` with a task list. Do not silently fix during lint — surface the list, let the user prioritize, then execute the accepted fixes as separate `refresh` operations.

## Guardrails

- **Never modify** `src/*.rs`, `SPEC2.md`, `AGENTS.md`, `RFC/*.md`, `GOVERNANCE.md`, `CHANGELOG.md`, or `Cargo.toml` as part of a wiki operation. Those are sources of truth; the wiki is derived. If the user asks you to *also* update a source file, do that as a separate, explicit action.
- **Never fabricate citations**. Every `path:line` must be verified with `read` or `grep` in the same session — line numbers shift, so citations from memory are stale.
- **Never re-derive** what a wiki page already says. Read the wiki first (starting with `wiki/index.md`), then decide whether to update or extend.
- **Never batch ≥5 wiki edits without a `log.md` entry**. The log is the audit trail; skipping it forfeits the "compounding" property.
- **Prefer updating existing pages** over creating new ones. Wiki sprawl kills discoverability. New page only when a concept is genuinely distinct.
- **Delete ruthlessly**. If a page describes code that no longer exists, delete it (and log the deletion). Stale pages are worse than missing ones.

## First-run bootstrap

If `wiki/index.md` doesn't exist yet:
1. Read `AGENTS.md` for the authoritative subsystem inventory.
2. Read `SPEC2.md` for language semantics.
3. Create the directory skeleton (`wiki/overview/`, `wiki/subsystems/`, `wiki/concepts/`).
4. Seed the following pages by reading the referenced source files and summarizing:
   - `wiki/index.md` — the catalog.
   - `wiki/log.md` — start with a `## [YYYY-MM-DD] bootstrap` entry.
   - `wiki/overview/architecture-overview.md` — top-level map, sourced from AGENTS.md's architecture section.
   - `wiki/overview/compiler-pipeline.md` — AST -> HIR -> MIR -> bytecode / WASM / AOT.
5. Log the bootstrap.
6. Ask the user which subsystem they want expanded first — do NOT try to seed all 12 subsystem pages in the bootstrap. Wiki grows by ingest, not by front-loading.

## Why this exists

`AGENTS.md` is the *contract* — dense, comprehensive, but hostile to skim. `SPEC2.md` is the *specification* — normative, precise, but scoped to language semantics. The wiki is the *browsable synthesis* — one screen per concept, cross-linked, kept fresh by the LLM. It's the layer a new contributor reads first, and the layer any LLM agent can grep to answer a question without re-reading 3000 lines of `src/runtime/mod.rs`.

The tedious part of maintaining a knowledge base is bookkeeping — updating cross-references, keeping summaries current, noting contradictions. Humans abandon wikis because maintenance grows faster than value. LLMs don't get bored. The wiki stays maintained because the cost of maintenance is near zero.
