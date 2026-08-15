# Registry Seeding Plan

A package registry with zero packages is a dead end for new users. This plan
seeds the official registry (`nulang registry serve` / the future hosted
registry) with a first wave of packages extracted from — and compatible with —
the existing stdlib modules in `src/stdlib/`.

Goals: (1) exercise `nula publish` / `nula add` end-to-end against a real
registry, (2) give new users something worth installing on day one,
(3) establish naming and quality conventions before third-party packages
arrive.

## Conventions

- Names are lowercase, hyphenated, unscoped (`nula add json-ext`).
- `nula new <name> --template lib` for scaffolds; every package ships with
  tests runnable via `nula test` and docs via `nula doc`.
- Version all seed packages `0.1.0`; they are Experimental tier until they
  graduate through an RFC.

## The first packages

| # | Package | Scope | Based on | Est. LOC |
|---|---------|-------|----------|----------|
| 1 | `json` | JSON parse/encode as a standalone package (reference port proving stdlib extraction works) | `src/stdlib/json.nula` | ~400 |
| 2 | `json-ext` | JSON Schema validation + pretty-printing on top of `json` | new | ~300 |
| 3 | `http-client` | Ergonomic HTTP client wrapper (GET/POST helpers, headers, JSON bodies) over the `Http` effect | `src/stdlib/http.nula` (20 LOC stub today) | ~250 |
| 4 | `test-utils` | Assertions, fixtures, and test helpers beyond the built-in `Test` effect | `src/stdlib/test.nula` + `list_test.nula` patterns | ~200 |
| 5 | `datetime-ext` | Parsing/formatting/timezone helpers extending the stdlib datetime surface | `src/stdlib/datetime.nula` | ~250 |
| 6 | `collections-ext` | Ordered map, deque, priority queue — structures the stdlib `list`/`map`/`set` don't cover | new, follows `src/stdlib/list.nula` style | ~400 |
| 7 | `string-ext` | Slugify, truncate, pad, template interpolation | extends `src/stdlib/string.nula` | ~200 |
| 8 | `result-ext` | Combinators for `Option`/`Result` pipelines (map2, sequence, traverse) | `src/stdlib/option.nula` + `result.nula` | ~150 |

Packages 1–4 are the priority wave (they mirror what the docs and tutorial
already use); 5–8 follow once the publish flow is proven.

## How to publish each one

```bash
# 1. Scaffold
nulang nula new json-ext --template lib && cd json-ext

# 2. Write code + tests, then verify
nulang nula build
nulang nula test

# 3. Publish to a registry (local first, hosted when it exists)
nulang registry serve &                      # local registry on default port
nulang nula publish --registry http://localhost:<port> --token <token>
```

Then dogfood it from a fresh project:

```bash
nulang nula new consumer-app
cd consumer-app
nulang nula add json-ext
nulang nula run
```

## Acceptance criteria

- All 8 packages installable via `nula add` from the seeded registry.
- Each package's `nula test` passes on a clean machine after install.
- `nula doc` output for each package linked from the registry index/docs.
- Publish + consume round-trip covered by at least one CI or conformance job
  (candidate: extend `conformance/` or a nightly workflow).
