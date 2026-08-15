# RFC 0015: Error-Model Consolidation

- **Status:** Draft
- **Tier:** Stable (removes Stable-tier surface; touches Frozen-tier runtime
  semantics for integer division)
- **Author:** AI assistant
- **Created:** 2026-08-15
- **Language-version at effect:** 2.0 (deprecation warnings begin in v1.x)
- **Supersedes:** none
- **Depends on:** none (coordinates with SPEC2 Appendix E items 1 and 4)

## Summary

Nulang today has at least six overlapping ways to represent and propagate
failure. This RFC consolidates on one model: **`Result[T, E]` values with
`?` propagation as the base layer, algebraic effects as the checked-effects
layer, and actor supervision for faults** — and removes `catch` (both
forms), `fail`, and nil-on-error arithmetic. Integer division/modulo by
zero becomes a runtime fault; float division/modulo by zero follows IEEE
754 (`inf`/`NaN`).

## Motivation

The current surface gives programmers six answers to "how do I fail?":

1. **`Result` + `match`** — user-declared `Ok`/`Error` variants
   (SPEC2 §6.12; there is no prelude, programs declare the variants).
2. **`catch expr fallback` (prefix, bare)** — desugars in the parser to
   `match expr { Ok(x) => x, Error(_) => fallback }`
   (`src/parser.rs:4291-4308`, `parse_catch_prefix`).
3. **`catch expr { | pat => body, ... }` (prefix, block form)** —
   desugars to `match` with a synthesized `Ok` arm plus user arms
   (`src/parser.rs:4251-4290`). A postfix form `expr catch fallback`
   also exists (`src/parser.rs:2641`).
4. **`fail expr`** — is *not* an error mechanism at all: the parser
   desugars it directly to `Expr::Return` with the comment "fail is
   sugar for return; same semantics" (`src/parser.rs:3047-3055`). It
   does not wrap its argument in `Error`; any error semantics come from
   the caller's convention. (SPEC2's status section describes `fail
   Error(...)` as "structured short-circuit return" — accurate only
   because the programmer writes the `Error(...)` constructor by hand.)
5. **`T ! E` return types + `?`** — `! E` is parsed as an error-type
   annotation (`src/parser.rs:663-690`) and the typechecker wraps the
   declared return type as `Result[ret_type, error_type]`
   (`src/typechecker.rs:1199-1207`). `?` desugars to a `match` on
   `Ok`/`Error` with early `return Error(e)` (`src/parser.rs:2603-2637`);
   `?.` is nil-safe chaining (`src/parser.rs:2567-2600`). `throws` is an
   accepted alias for the second `!` (`src/parser.rs:669,676`).
6. **Effects-as-exceptions** — `perform`/`handle` with continuation
   capture and resume (`src/vm.rs` Perform/Resume/Unwind/Handle opcodes;
   SPEC2 §4.4), plus supervision (links, monitors, exit signals;
   SPEC2 Ch. 8) for actor faults.
7. **Nil-on-error arithmetic** — integer *and* float division/modulo by
   zero evaluate to `nil` (`src/vm.rs` `step_idiv:3655-3675`,
   `step_imod:3679-3698`), a value whose type (`Nil`) does not match the
   static result type (`Int`/`Float`) and which silently poisons
   downstream computation.

Consequences: `catch` and `?` disagree on what a failure *is* (both
match on `Ok`/`Error`, but `catch` swallows the error value in its bare
form while `?` propagates it); `fail` is keyword-squatting on a concept
it doesn't implement; and nil-on-error is invisible to the type system,
making it the only failure mode that cannot be detected statically or
handled explicitly. The design review (recommendation #4) directs
convergence on a single model.

## Design

### Target model (kept surface)

- **Base layer — `Result[T, E]` + `?`.** Recoverable, expected failures
  are values. `T ! E` remains as signature sugar for
  `Result[T, E]` (`src/typechecker.rs:1199-1207` already implements it
  this way). `?` is the sole propagation operator. `?.` stays (it is
  `nil` chaining, orthogonal to errors).
- **Checked-effects layer — effects-as-exceptions.** Where an operation
  is effectful anyway (IO, FS, Http), failure is an effect operation
  result handled by the enclosing handler, resumable per SPEC2 §4.4.2.
  No change.
- **Fault layer — supervision.** Programmer bugs and infrastructure
  failures (badarith, badmatch, dead node) crash the actor and are
  handled by supervisors, never by in-band values. No change to links,
  monitors, or exit signals.

### Removed surface

**`fail` is removed.** It is already literal sugar for `return`
(`src/parser.rs:3047-3055`), so migration is mechanical:

```nulang
// before (v1)
fn head(l: List[Int]) -> Int ! Error {
  if empty(l) { fail Error("empty list") }
  first(l)
}

// after (v2)
fn head(l: List[Int]) -> Int ! String {
  if empty(l) { return Error("empty list") }
  first(l)
}
```

**`catch` (all three forms) is removed.** Migration is to `match`,
which is exactly what the parser desugars to today:

```nulang
// before (v1): bare fallback
let port = catch parse_port(env) 8080

// after (v2): explicit match (what catch already means)
let port = match parse_port(env) {
  | Ok(p) => p
  | Error(_) => 8080
}

// before (v1): block form
catch read_config(path) {
  | Error(msg) => default_config(msg)
}

// after (v2)
match read_config(path) {
  | Ok(c) => c
  | Error(msg) => default_config(msg)
}
```

Because both `catch` and `fail` are pure parser desugars with no AST,
typechecker, or bytecode representation of their own, removal is a
parser-only change plus the diagnostic.

### Division/modulo by zero

- **`Int` division and modulo by zero raise a runtime fault** (VM
  `NuError`, propagated as actor failure to the supervisor — the same
  path as any other runtime trap), **not** `nil` and **not** a `Result`.
  Justification: changing `/` and `%` to return `Result[Int, E]` would
  re-type every arithmetic expression in every program (division is used
  in non-`Result`-returning functions pervasively) and would make
  `1 + 2 & 3`-style mixed expressions unwritable without `?`. Division
  by zero is a programmer bug, not an expected failure; Erlang's
  `badarith` precedent shows the fault layer absorbs it cleanly, and
  Nulang's supervision tree is precisely the mechanism for it. Callers
  that *expect* a zero divisor should check it explicitly or use a
  stdlib `Int.div_checked(a, b) -> Result[Int, String]` (added with the
  stdlib, SPEC2 Ch. 14).
- **`Float` division and modulo by zero follow IEEE 754**: `1.0/0.0` →
  `inf`, `-1.0/0.0` → `-inf`, `0.0/0.0` → `NaN`, `x % 0.0` → `NaN`.
  This matches hardware, costs nothing, and composes with the NaN
  canonicalization work proceeding on another branch
  (`src/vm.rs:1217-1389` NaN-boxed `Value` representation) — this RFC
  deliberately takes no position on NaN bit patterns, only on which
  IEEE result the operation produces.

## Tier Classification

- `catch` / `fail` removal: **Stable-tier** syntax removal — requires
  this RFC plus the deprecation cycle below.
- Integer div/mod-by-zero semantics: **Frozen-tier** runtime behavior
  (the VM is a frozen artifact). Per `GOVERNANCE.md` this is the class
  of change that normally requires a major-version bump; see the phased
  plan — the change takes effect at language version 2.0 exactly once.
- Float div/mod-by-zero: semantics clarification to IEEE, no tier
  impact (current nil behavior was never a specified guarantee beyond
  SPEC2 §2.6.1's one-liner, which §2.6.1 will be updated to reflect).

## Backwards Compatibility

Phased migration, acknowledging the project's current position
(pre-1.0-with-users: alpha, v0.9 series implementing a `1.0.0-frozen`
bytecode format — breaking source changes are cheaper now than they will
ever be again):

1. **v1.x (next release): deprecation warnings.** The parser continues
   to accept `catch` (both prefix forms and the postfix form) and
   `fail`, emitting an RFC 0010-style deprecation warning naming the
   replacement (`match` / `return`). SPEC2 and PITFALLS mark the
   constructs deprecated. A `nula fmt --migrate-rfc-0015` rewrite pass
   performs the mechanical rewrites above (the desugar targets are
   already known to the parser, so the tool is a syntax-level rewrite).
2. **v1.x (same release): div-by-zero warning mode.** Integer div/mod
   by zero still yields `nil` but logs a one-time-per-call-site
   deprecation warning in the VM; float div/mod switches to IEEE
   immediately (the old float-nil behavior has no defenders — it
   satisfies neither the static type nor IEEE).
3. **v2.0: errors.** `catch`/`fail` are parse errors with a "removed by
   RFC 0015, use `match`/`return`" message. Integer div/mod by zero
   raises the runtime fault. The Frozen-tier bump is taken here, once,
   as the single 2.0 breaking batch anticipated by SPEC2 Appendix E.

No bytecode-format migration is needed: neither `catch` nor `fail`
reaches bytecode (they desugar in the parser), and the div-by-zero
change is a VM behavior change to existing opcodes `IDiv`/`IMod`, not a
format change — so `src/format/migrate.rs` is untouched. `.nbc`
artifacts compiled under v1 remain loadable; their div-by-zero behavior
changes under the v2 runtime, which is the Frozen-tier break recorded
above.

## Alternatives Considered

- **Keep `catch` as the "ergonomic" layer over `match`.** Rejected:
  `catch` adds nothing over the `match` it desugars to, its bare form
  discards the error value (anti-pattern for diagnostics), and two
  spellings for one semantics is exactly the redundancy this RFC exists
  to remove.
- **Make `fail` a real error constructor** (`fail e` ≡ `return
  Error(e)`). Rejected: it special-cases one variant constructor in the
  language and conflicts with user-declared error types that are not
  named `Error`; `return` is honest and already works.
- **Div-by-zero returns `Result`.** Rejected for `Int`: re-types all
  arithmetic; expected-zero callers are better served by an explicit
  `div_checked`. (For `Float`, IEEE makes the question moot.)
- **Div-by-zero traps immediately with no v1.x warning phase.**
  Rejected for the Frozen-tier runtime change; accepted for float,
  where the change is a strict improvement with no migration cost.
- **Keep nil-on-error and type it** (`Int | Nil` unions). Rejected:
  introduces union types through the back door and makes every
  arithmetic result carry an implicit nil case — worse than the status
  quo in both directions.

## Open Questions

- Exact spelling of the stdlib checked-division API
  (`Int.div_checked` vs. `Checked.div`) — a stdlib naming question, not
  a language-semantics one.
- Should `%%`-style explicit-checked division operators be introduced
  for hot paths? (Leaning no; functions compose better than more
  operator sigils — see SPEC2 Appendix E item 2.)
- Interaction of the integer-division fault with WASM and native (AOT)
  backends: trap lowering per backend needs one sentence each in the
  implementation plan; semantics are backend-independent.

## Resolution

(Pending.)
