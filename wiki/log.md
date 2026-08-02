# Wiki Log

Append-only chronological record of wiki operations. Every ingest, refresh, query, and lint gets an entry. Entries start with `## [YYYY-MM-DD] <op> | <title>` so `grep '^## \[' wiki/log.md | tail -20` yields recent activity.

Operations:
- `bootstrap` — initial scaffolding.
- `ingest` — a new source landed, wiki updated.
- `refresh` — code drifted, wiki brought in sync.
- `query` — a user question was filed back into the wiki.
- `lint` — health-check findings.

---

## [2026-08-02] bootstrap | wiki scaffolding

Established the wiki structure per the [[wiki-updater skill|../.claude/skills/wiki-updater/SKILL.md]], instantiating Andrej Karpathy's [llm-wiki.md](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f) pattern for the Nulang repository.

Created:
- [[index]] — content catalog.
- [[log]] — this file.
- [[overview/architecture-overview]] — top-level architecture map, sourced from `AGENTS.md`.
- [[overview/compiler-pipeline]] — compiler stages and backends, sourced from `AGENTS.md` and `src/`.

Not seeded (per skill's bootstrap rule: "wiki grows by ingest, not by front-loading"):
- Subsystem pages (`wiki/subsystems/`) — created on first ingest per subsystem.
- Concept pages (`wiki/concepts/`) — created on first ingest per concept.

Next: user directs which subsystem or concept to expand first via the `wiki-updater` skill.
