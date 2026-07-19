# RFC NNNN: <Title>

- **Status:** Draft | Accepted | Rejected | Implemented | Superseded by NNNN
- **Tier:** Frozen | Stable | Experimental
- **Author:** <name>
- **Created:** YYYY-MM-DD
- **Resolved:** YYYY-MM-DD (on accept/reject)
- **Language-version at effect:** 1.x (Frozen-tier only)
- **Supersedes:** NNNN (if any)
- **Superseded by:** NNNN (if any)

## Summary

One paragraph describing the change.

## Motivation

Why is this change needed? What problem does it solve? What existing behavior
is broken or missing? Reference specific files, issues, or user pain.

## Design

The detailed design. Be concrete: name the files, signatures, byte layouts,
grammar productions, or semantics that change. This section must be detailed
enough that an implementer can execute it without making design decisions.

## Tier Classification

Which stability tier does this affect (Frozen / Stable / Experimental)? If
Frozen or Stable, what is the deprecation path for any removed surface? What
language version does this introduce?

## Backwards Compatibility

Does this break existing programs? If yes, how is the breakage survivable
(deprecation cycle, migration tool, automatic rewrite)? If a migration is
needed, where does it live (`src/format/migrate.rs` for formats; a dedicated
tool for syntax)?

## Alternatives Considered

Briefly enumerate the serious alternatives and why they were rejected.

## Open Questions

Non-blocking questions to resolve during discussion. Each should be answerable
without changing the design's load-bearing decisions.

## Resolution

(Filled in on accept/reject.) The decision, the rationale, and the language
version at which an accepted RFC took effect.
