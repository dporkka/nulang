# RFC 0003: Content-Addressed Functions

- **Status:** Draft
- **Author:** Nulang Core Team
- **Date:** 2026-07-29
- **Stability Tier:** Experimental (proposed)

## Summary

Functions in Nulang are identified by a BLAKE3 content hash of their compiled
`.nbc` artifact — source, type signature, and bytecode combined. This enables
immutable deployments, deterministic replay across nodes, and version-pinned AI
agent workflows.

## Motivation

Nulang's distributed actor model and durable persistence (RFC 0002, durable
continuations) create a natural need for deterministic function identity:

1. **Distributed actors** must agree on which function version a behavior
   invokes. Name-based resolution (`behavior "handle"`) is ambiguous when
   multiple compiled versions of the same named function exist across nodes.

2. **Durable workflows** (v0.8) checkpoint their continuation. On recovery, the
   handler must resolve to byte-identical code. A content hash guarantees the
   same bytes produce the same behavior.

3. **AI agent pipelines** (v0.9) invoke LLM-generated code. Content addressing
   gives each generated function a stable, immutable identity — agents can
   cache results, share functions, and audit execution by hash.

4. **Package management** (package manager, `nula build`) needs verifiable
   artifact identity. A dependency locked to `sha256:abc123...` is
   tamper-evident.

Inspired by Unison's content-addressed code model, adapted for Nulang's
`.nbc` bytecode artifact format (RFC 0001 format stability).

## Design

### Hash Computation

```
content_hash = BLAKE3(
    source_hash   ||  // BLAKE3 of source text (already in .nbc, RFC 0001)
    type_sig      ||  // canonical type signature string
    bytecode      ||  // compiled .nbc bytecode bytes (excluding hash field)
)
```

`source_hash` is already computed during compilation (RFC 0001 §2.4). The
type signature is the canonical pretty-printed function type (e.g.
`fn(Int, Int) -> Int ! {IO}`). The bytecode is the serialized function body
from the `.nbc` artifact.

### Resolution

At the language level, `actor.behavior` and `agent.fetch_weather` resolve by
name as before. Content-addressed resolution is additive:

```nulang
// Name-based (existing): resolves to the latest compiled version.
let handler = Agent.ask("What is the weather?");

// Hash-based (new): resolves to a specific, immutable artifact.
let handler = resolve("sha256:abc123def456...").ask("What is the weather?");
```

The runtime dispatches by hash. A `ResolveByHash` effect looks up the `.nbc`
artifact in the local artifact store or fetches it from a configured registry.

### `.nbc` Format Extension

The `.nbc` artifact (RFC 0001) gains an optional `content_hash` field in its
header:

```
┌──────────────────────────────────────┐
│ magic: 4 bytes ("NLBC")              │
│ version: 2 bytes (u16, big-endian)   │
│ source_hash: 32 bytes (BLAKE3)       │
│ content_hash: 32 bytes (BLAKE3)  NEW │
│ flags: 2 bytes                        │
│ string_count: 2 bytes                 │
│ ...                                   │
└──────────────────────────────────────┘
```

`content_hash` is zero-filled for artifacts compiled without content addressing
(backward compatible). A non-zero `content_hash` signals that this artifact
supports hash-based resolution.

### Artifact Store

Nodes maintain a local artifact store (default: `~/.nulang/artifacts/`):

```
~/.nulang/artifacts/
  abc123def456...  →  artifact.nbc
  def789abc012...  →  artifact.nbc
```

Artifacts are stored by their `content_hash`. The runtime looks up functions by
hash before falling back to name-based resolution.

### Garbage Collection

Unreferenced artifacts are subject to GC after a configurable TTL (default: 30
days). References include:

- Actor behavior tables (which hash a behavior uses)
- Workflow checkpoints (durable continuations reference the hash)
- Package lockfiles (`Nulang.lock`)

A reference-counting or mark-and-sweep pass runs at node startup and
periodically (configurable interval).

## Migration Path

1. **Phase 1 (this RFC):** Compute and store `content_hash` in `.nbc`
   artifacts. Name-based resolution continues unchanged. No breaking changes.

2. **Phase 2:** Add `resolve(...)` and `ResolveByHash` effect. Nodes can opt
   into hash-based dispatch.

3. **Phase 3:** Package manager locks deps by hash. AI agents pin function
   versions by hash.

Name-based resolution is never removed — it remains the default for
interactive development. Hash-based resolution is an opt-in precision tool.

## Open Questions

1. **Hash chain for incremental updates:** Should we also store a
   `parent_hash` linking to the previous version? This would enable
   diff-based artifact distribution.

2. **Interaction with Nulang Core (bootstrap compiler):** The bootstrap
   compiler is itself a Nulang program. Its hash is self-referential.
   Bootstrapping requires a pre-built seed artifact.

3. **Artifact signing:** Content hashing guarantees integrity but not
   authorship. Should we add Ed25519 signatures for publisher identity?

4. **Registry protocol:** How do nodes discover and fetch artifacts by hash?
   A simple HTTP registry (like crates.io) or a distributed protocol (like
   IPFS)?

## References

- RFC 0001: Format Stability and Frozen Artifacts
- RFC 0002: Nulang Core (bootstrap compiler)
- Unison: A Content-Addressed Programming Language
  (https://www.unison-lang.org/)
- BLAKE3: https://github.com/BLAKE3-team/BLAKE3
