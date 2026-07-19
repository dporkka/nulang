# Nulang Governance

> This document is the governance constitution for the Nulang programming
> language. It is itself a Frozen-tier artifact (see `CHANGELOG.md`): the
> process it defines may evolve only through the RFC mechanism it specifies.
>
> **Status:** Ratified 2026-07-19 alongside RFC 0001 (Format Stability) and
> RFC 0002 (Frozen Core). Language version 1.0-frozen.

## 1. Purpose

Nulang is engineered for a relevance horizon of 200+ years. No implementation,
author, or dependency lasts that long; the *process* must. This document
specifies how the language is changed, by whom, and under what stability
guarantees — so that a program written today continues to run decades from now
under conforming implementations that did not exist when it was written.

The lessons are drawn from languages that survived (C/ISO, Python/PEP, Rust/RFC,
Lisp/ANSI) and those that didn't (single-implementation languages that died
with their host). The recurring pattern: a published, versioned stability
contract and a process that makes breaking changes expensive and survivable.

## 2. Stability Tiers

Every public surface of Nulang is classified into one of three tiers. The tier
determines what changes are permitted and how. See `CHANGELOG.md` for the
current classification.

### Frozen
Will never break. A change to a Frozen surface is, by definition, a new
language and requires a new major version number and a migration path.

- `.nbc` bytecode format version 1 (`src/format/constants.rs`).
- NUL0 wire protocol version 1.
- Value layout version 1 (`src/value_layout.rs`).
- Nulang Core syntax and semantics (RFC 0002).
- The `IO`, `Spawn`, `Send`, `Receive` built-in effects and their semantics.

### Stable
Breaking changes require an accepted RFC and a deprecation cycle of at least
two major versions (the deprecated surface remains functional, emits a
warning, and is removed only in the version after next).

- The full HM type system and inference rules.
- The effect-row system (closed/open, regions).
- The capability lattice (`iso/trn/ref/val/box/tag/lineariso`) and subtyping.
- The actor surface (`spawn`, `send`, `receive`, supervision).
- CRDT operations and their merge semantics.

### Experimental
No stability promise. May change or be removed in any release. Lives behind a
feature flag (`wasm-backend`, `python`, `sqlite`, `lsp`) or is explicitly
marked experimental in `CHANGELOG.md`.

## 3. Roles

### Language Steward
- Final authority on RFC acceptance or rejection.
- Responsible for ensuring accepted RFCs are implemented and the
  `CHANGELOG.md` tiers are kept accurate.
- May delegate review but not the acceptance decision for Frozen-tier changes.

### RFC Authors
- Any contributor may author an RFC. The steward is the default author of
  Frozen-tier RFCs.

### Implementers
- May fix bugs and add Experimental-tier features without an RFC.
- Must not change Frozen or Stable surfaces without an accepted RFC.
- Must record every user-visible change in `CHANGELOG.md` under the correct
  tier.

## 4. The RFC Process

1. **Draft.** Copy `RFC/0000-template.md` to `RFC/NNNN-short-name.md` (next
   free number). Fill in every section.
2. **Discussion.** The RFC is discussed in the project's issue tracker. The
   steward resolves blocking questions; non-blocking questions are recorded
   in the RFC's "Open Questions" section.
3. **Accept or Reject.** The steward accepts or rejects. The decision and its
   rationale are recorded in the RFC's "Resolution" section. A rejected RFC
   is kept in `RFC/` with its resolution; it is not deleted.
4. **Implement.** The accepted RFC is implemented. The implementation must
   include tests and a `CHANGELOG.md` entry.
5. **Ratify.** Once implemented and verified, the RFC's status changes from
   "Accepted" to "Implemented". Frozen-tier RFCs additionally record the
   language version at which they took effect.

An accepted RFC is **immutable**: its text is never edited after acceptance.
Corrections require a new RFC that supersedes it (recorded in the superseding
RFC's header).

## 5. Versioning

- **Crate version** (`Cargo.toml` `version`): the implementation version. May
  rev freely (semver) for bug fixes, performance, and Experimental features.
- **Language version** (`Cargo.toml` `[package.metadata] language-version`,
  and the `LANGUAGE_VERSION` const in `src/format/constants.rs`): moves only
  on RFC-ratified change to Frozen or Stable surfaces. Recorded in every
  `.nbc` artifact. A runtime rejects an artifact whose language version
  exceeds its own (`FormatError::IncompatibleLanguage`).

## 6. Deprecation

A Stable-tier surface that is to be removed is first marked `#[deprecated]`
in the implementation and noted in `CHANGELOG.md` with the version that
deprecated it and the version that will remove it. The deprecated surface
remains functional for at least one major language version. Removal requires
an RFC and a major-version bump.

## 7. Authoritative Artifacts

The language is defined by the interaction of three artifacts, in priority
order:

1. `SPEC2.md` — the prose specification. Explanatory.
2. `spec/formal/` — the machine-checked formal semantics (RFC pending). Where
   the formal model and the prose disagree, the formal model is authoritative.
3. `conformance/` — the behavioral conformance suite. Where an implementation
   and the spec disagree, the conformance suite is the oracle.

A conforming implementation passes the conformance suite and respects the
frozen format versions. Multiple conforming implementations are not just
permitted but expected and encouraged.
