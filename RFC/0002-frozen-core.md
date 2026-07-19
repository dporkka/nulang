# RFC 0002: Frozen Core

- **Status:** Accepted
- **Tier:** Frozen
- **Author:** David Porkka
- **Created:** 2026-07-19
- **Resolved:** 2026-07-19 (Accepted)
- **Language-version at effect:** 1.0-frozen
- **Supersedes:** none
- **Superseded by:** none

## Summary

Defines **Nulang Core**, the minimal language subset that is frozen forever
and that a conforming implementation must support. Core is the 200-year
invariant kernel: the subset a self-hosting compiler targets, and the subset
every implementation — present or future — must provide. Surface outside Core
is Stable or Experimental and may evolve under the governance process.

## Motivation

A language that wants to outlive its implementation needs a kernel that does
not move. Lisp's `lambda`/`apply`, C's `int`/`+`/pointer arithmetic, FORTRAN's
`DO` loop — these cores are why those languages still run 50–70 years later.
Nulang currently has no documented minimal kernel; every feature is implicitly
the same tier, so any refactor risks breaking any program. This RFC draws the
line.

Core is also the target of the self-hosting bootstrap compiler (planned,
see RFC 0003): a Nulang→Nulang compiler written in Core that decouples the
language's existence from the Rust host's existence.

## Design

Nulang Core consists of:

**Expressions:** `let`, `if`/`else`, `match`, function application, binary and
unary operators over `Int`/`Bool`, string literals, `fn` closures, `return`.

**Types:** `Int` (i64 fast path), `Bool`, `String`, `Unit`, `Nil`; `Vec<T>`
and `Map<K,V>`; tuples; records (closed); `enum`; function types. HM type
inference over this subset (no row-polymorphic effects, no capabilities
beyond `val`).

**Declarations:** `fn` (top-level and local), `enum`, struct/record literals,
`const`.

**Effects:** `IO.print` and `IO.read` only. No `Spawn`, no `Send`, no
`Receive`, no `LLM`, no `Migrate`, no `STM`. A Core program is a pure
sequential computation with terminal IO.

**Capabilities:** every value is `val` (immutable, sendable). The capability
lattice is present but degenerate: no `iso`/`trn`/`lineariso` in Core source.

**Explicitly excluded from Core:** actors (`actor`, `spawn`, `send`,
`receive`), all effects except IO, all capabilities except `val`, AI
(`agent`, `workflow`, `LLM`, `Pipeline`, `Debate`), distribution (`cluster`,
`node`, `migrate`), persistence (`@persistent`, state models), FFI (`extern`),
Python interop, the JIT/WASM backends (Core targets the interpreter), and all
Experimental features.

**Core is frozen:** the syntax, typing rules, and evaluation semantics of the
above are Frozen-tier. A change to Core is a new language and requires a new
major version. Additive extensions to Core (e.g., a new pure type) require an
RFC and are still Frozen once added.

## Tier Classification

Frozen. Core defines part of the Frozen tier. The remainder of the Frozen
tier is the format versions (RFC 0001) and the `IO`/`Spawn`/`Send`/`Receive`
effect semantics — though the actor effects beyond IO are Stable, not Core.

## Backwards Compatibility

No change to existing programs. Core is a subset; every Core program is
already a valid Nulang program. This RFC classifies existing surface into
tiers; it does not remove or rename anything.

## Alternatives Considered

- **Larger Core (include actors):** rejected. Actors pull in scheduling,
  mailboxes, supervision, and (eventually) distribution — too much surface
  to freeze and too much coupled to implementation choices. Core should be
  the minimal kernel that is obviously correct and obviously stable.
- **Smaller Core (exclude `Vec`/`Map`):** rejected. A self-hosting compiler
  needs collections; excluding them would force the bootstrap compiler to
  implement its own, defeating the purpose of a small frozen target.
- **Core as a separate syntax:** rejected. Core is a subset of Nulang, not a
  different language. The same parser, typechecker, and (interpreter) VM
  serve both; Core programs are distinguished by what they do not use.

## Open Questions

- Whether `match` on `String` is in Core (needed for a self-hosting lexer).
  Provisionally yes; confirm in RFC 0003 (self-hosting bootstrap).
- Whether `Float` is in Core. Provisionally no (i64 integer arithmetic
  suffices for a compiler; float semantics are a Stable-tier concern with
  its own determinism contract — see SPEC2 §"Determinism Contract").

## Resolution

Accepted 2026-07-19. Core is defined as above and classified Frozen. The
self-hosting bootstrap compiler targeting Core is a separate effort (RFC 0003,
planned). Core took effect at language version 1.0-frozen: every Core program
valid today is valid in every future version.
