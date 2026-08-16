# Spec Reconciliation Audit Trail

Reconciliation pass over the normative documents (`SPEC2.md`,
`spec/grammar.ebnf`, `docs/PITFALLS.md`, `README.md`) against the
reference implementation. Ground truth for every item below was
established by reading `src/parser.rs` (the reference parser per
`GOVERNANCE.md`/`spec/grammar.ebnf` §5), `src/lexer.rs`,
`src/typechecker.rs`, `src/effect_checker.rs`, `src/hir_lower.rs`, and
`src/vm.rs`. No Rust source and no `.nula` program logic was modified.

Branch: `fix/spec-reconciliation`.

## 1. `spec/grammar.ebnf` duplicate productions — CONFIRMED, fixed

- **Discrepancy:** `fn_decl` was defined twice with different
  return-type sigils (`-> type` and `: type`); `send_expr` was defined
  twice (`send expr to expr` and `send expr IDENT(...)`).
- **Ground truth:**
  - Return-type sigil is `->` (`TokenKind::Arrow`):
    `src/parser.rs:654` (`parse_function`).
  - Keyword send is `send [remote] actor behavior(args)` with mandatory
    parentheses: `src/parser.rs:3721-3735` (`parse_send_keyword`).
    There is no `send expr to expr` production anywhere in the parser.
  - Postfix send `actor ! behavior(args)` and async tell
    `actor <- behavior(args)` are Pratt postfix operators:
    `src/parser.rs:2466-2479`, `2483-2495`; both desugar to the same
    `Expr::Send`.
- **Change:** grammar now has one `fn_decl` production (with `->`) and
  one `send_expr` production (keyword form), with the postfix forms
  documented in a comment. `handle_expr` was also corrected (item 2).

## 2. `handle ... with` — CONFIRMED, fixed

- **Discrepancy:** SPEC2 §4.4 said "there is no `with` keyword";
  `examples/07_effects.nula` uses `handle ... with { ... }` throughout.
- **Ground truth:** `with` is optional after the handled body
  (`src/parser.rs:4182`, `consume_if(&TokenKind::With)`), and
  `handle body with name` references a prior `handler name { ... }`
  declaration (`src/parser.rs:4185-4195`, `2044-2085`).
- **Change:** SPEC2 §4.4 and `spec/grammar.ebnf` `handle_expr` now
  document the optional `with` (bare block, `with { ... }`, and
  `with name` forms).

## 3. Implemented-vs-Planned annotations — CONFIRMED, fixed

| Claim (old SPEC2) | Ground truth | Source |
|---|---|---|
| §2.6 "no `**` exponentiation operator" | `**` is implemented, right-assoc, above `*` | `src/lexer.rs:269` (`Star2`); `src/parser.rs:45,93` (`PREC_EXP` = 14) |
| §2.3 "no `var` keyword" | `var` is a keyword; mutable bindings parse | `src/lexer.rs:1185`; `src/parser.rs:2242`, `2951` |
| §2.3 "no `consume`/`recover`/`as` keyword" | all three are keywords | `src/lexer.rs:1266`, `1267`, `1197`; `src/parser.rs:3034` |
| §2.7 "`..`, `::`, `<-`, `?` not accepted anywhere" | all four parse: ranges, record update, import paths, async tell, try `?`, optional chaining `?.` | `src/parser.rs:83/5698`, `2974-3000`, `2208-2213`, `2483`, `2603`, `2567` |
| §3.5 record update `{ r .. f = v }` Planned | implemented | `src/ast.rs:152` (`Expr::RecordUpdate`), `src/parser.rs:2997` |
| §2.6.4 `consume`/`recover` rows "Planned" | both implemented (Experimental) | as above |

`enum`, `event`, `from`, `config`, `capability` remain non-keywords
(absent from the `src/lexer.rs` keyword table); §2.3 now says exactly
that. Also corrected the stale `PREC_EXP` level (13 → 14,
`src/parser.rs:45`) in the status section and added `**` to the §2.6.1
arithmetic table and the range-operator cross-reference in §2.5.

## 4. `recover` — CONFIRMED, resolved

- **Discrepancy:** `docs/PITFALLS.md` (and the SPEC2 status section)
  said `recover { body }` wraps the result in `Ok`/`Error`; SPEC2
  §3.9.2/§6.13 said `recover` is not a keyword and reserved it for
  Pony-style capability recovery.
- **Ground truth:** `recover` IS a keyword (`src/lexer.rs:1267`) and
  parses as `Expr::Recover` (`src/parser.rs:4626-4632`). The
  typechecker infers the body's type unchanged
  (`src/typechecker.rs:1814`) — there is no `Ok`/`Error` wrapping
  anywhere in the pipeline — and HIR lowering is transparent
  (`src/hir_lower.rs:2062`). The single non-transparent check is in the
  capability checker: the body's result must be sendable, else
  "recover body must evaluate to a sendable value"
  (`src/effect_checker.rs:2023-2033`).
- **Resolution:** the single current meaning — an isolated scope with a
  sendable-result check, no wrapping, no capability upgrade — is
  documented in SPEC2 §3.9.2/§6.13 and PITFALLS. The Pony-style
  capability-upgrade semantics (constructing `iso`/`val` from
  restricted interiors) is explicitly marked reserved/future.

## 5. Effect set (`Net`/`Rand`) — CONFIRMED, fixed

- **Discrepancy:** Appendix C's intro listed `Net` and `Rand` among
  the recognized built-ins while §4.6 (corrected 2026-08-02) says those
  names have no runtime dispatch.
- **Ground truth:** `parse_effect_name` (`src/effect_checker.rs:73-97`)
  recognizes `IO`, `Net`, `Http`, `FS`, `Array`, `String`, `Test`,
  `Rand`, `Random`, `Time`, `Spawn`, `Send`, `Receive`, `Migrate`,
  `STM`, `Async`, `Inference`, `Cost`, `Event`, `FFI`, `DB`, `Python`,
  `Process`, `System`; `Net`→`Effect::Net` and `Rand`→`Effect::Rand`
  are compile-time aliases. The VM dispatches on the literal names
  `Http` (`src/vm.rs:926`) and `Random` (`src/vm.rs:1075`); there is no
  `Net`/`Rand` dispatch, so `perform Net.get(...)` fails with
  "Unhandled effect". `LLM` is handled as an `Inference` alias in
  `src/cir_lower.rs:58`.
- **Change:** Appendix C now lists the full recognized set, explains
  the recognition-vs-dispatch distinction, and directs users to
  `Http`/`Random`. §2.3's naming-convention example updated likewise.

## 6. Known design tensions — documented (no behavior change)

Added **SPEC2 Appendix E: Known Design Tensions Under Review for v2**,
listing with source citations:

1. `!` = prefix not / send `a!b` / effect row `!{IO}` / error type `!E`
   (`src/parser.rs` `prefix_precedence`, `:2466`, `:663`).
2. `&` = prefix borrow / bitwise AND (`src/parser.rs:42`) / the `&&`
   family.
3. Bitwise binds tighter than arithmetic: `1 + 2 & 3` = `1 + (2 & 3)`
   (Pratt table `src/parser.rs:30-46`; quirk already noted in §2.6).
4. Division by zero yields `nil` (`src/vm.rs` `step_idiv`,
   `:3655-3672`; §2.6.1), bypassing the `?`/error-value surface.

Each carries the recommendation that it be resolved by RFC before the
next frozen-tier bump.

## 7. CRDTs and multi-node distribution — CONFIRMED, fixed

- **Discrepancy:** README's feature list implied the 8 CRDT types ship
  with distribution, and its stability table listed "CRDT operations"
  as **Stable**; SPEC2's intro said "CRDT state converges
  automatically" unqualified. In fact `state crdt` behaves as `durable`
  (SPEC2 §9.10 — already accurately documented there) and `migrate` is
  a documented no-op (§12.4); the CRDT types are exercised only by
  Rust-level tests (`src/runtime/crdt.rs`, `CrdtManager`), with no
  `.nula`-level surface (§12.5).
- **Change:** README scopes CRDTs to the Rust embedder level and moves
  them from the Stable row to the Experimental row; SPEC2's overview
  sentences now carry explicit Planned/Experimental markers pointing at
  §9.10/§12.5. (§9.10, §12.4, §12.5 themselves were already accurate
  and were not changed.)

## Discrepancies checked and NOT found

- **`send expr to expr`** — not just absent from the grammar's intent;
  confirmed no such production exists in `src/parser.rs` (removed from
  the EBNF rather than "fixed to match").
- **Pony-style `recover` capability upgrade** — confirmed absent from
  typechecker, effect checker, and lowering (only the sendability check
  exists); documented as future, not current.
- **`Ok`/`Error` wrapping in `recover`** — confirmed absent
  (`src/typechecker.rs:1814` infers the body type unchanged). The
  PITFALLS/status-section claim was simply wrong and was corrected.

---

# Round 2 (branch `docs/spec-reconciliation-2`)

Reconciliation of the drift findings produced by grammar conformance rounds 1
and 2 (`conformance/grammar/README.md` drift log: 8 round-1 items pinned in
case comments; round 2 added 8 hard rejections + 16 parser leniencies).
Ground truth was re-verified against `src/parser.rs` and `src/lexer.rs` on
`main` (tip `b33b9e7`). No Rust source and no `.nula` program logic was
modified. Disposition classes: **spec-fix** (the spec was wrong; grammar.ebnf
and/or SPEC2 corrected to match the reference parser), **parser-fix** (known
parser deviation; fix scheduled — spec unchanged), **accepted** (intentional
leniency; documented, spec unchanged or annotated).

Summary: **17 spec-fixes, 6 parser-fixes, 9 accepted** (32 drift rows total).

## 8. Round-1 carryover (EBNF §1–§3 corrections applied in this pass)

These items were logged in round 1 (`conformance/grammar/README.md`) but the
EBNF corrections themselves land in this pass.

| # | Drift | Ground truth | Disposition |
|---|---|---|---|
| 8.1 | `if` requires `then` | `then` is optional before a bare expression but **required** before a `{ }` block branch (`src/parser.rs` `parse_if` :3563-3578) | **spec-fix** — grammar.ebnf §2 `if_expr` and SPEC2 §6.6 / Appendix A `conditional` corrected |
| 8.2 | Variant payloads take a single type | `parse_variants` (:5752) parses at most one `(type)` payload | **spec-fix** — grammar.ebnf `variant` |
| 8.3 | Parameter capabilities are prefix (`val x: Int`), not postfix (`x: Int @ val`) | `try_parse_param_capability` (:5871) consumes a capability keyword *before* the parameter name; no postfix `@ cap` parse exists | **spec-fix** — grammar.ebnf §2 `param = [ cap ] IDENT [ ":" type ]`; §3 postfix production removed. Matches ML-flavored design intent; prefix form is implemented reality (see also 9.1) |
| 8.4 | Type aliases require `type alias`; bare `alias Name = T` rejected | Appendix A already correct | **spec-fix** — grammar.ebnf `alias_decl` now `"type" "alias" ...` |
| 8.5 | No character literals | No `CharLit` token in `src/lexer.rs` | **spec-fix** — `CHAR_LIT` removed from grammar.ebnf §1 |
| 8.6 | No compound assignment (`+=`/`-=` rejected: "Not a binary operator") | Tokens exist (`src/lexer.rs` PlusAssign :141) and sit in the Pratt table, but `parse_expr_with_prec` rejects them at :3019 | **accepted** (no spec change) — grammar.ebnf already excludes compound assignment; annotated with a comment. The dead tokens are harmless; removal is a cleanup, not a language change |
| 8.7 | Bitwise/shift operators bind tighter than arithmetic (Pratt quirk) | `PREC_SHIFT`=10 … `PREC_BITOR`=13 sit above `PREC_TERM`/`PREC_FACTOR` (:39-44) | **spec-fix** — grammar.ebnf §2 expression chain now encodes the real precedence (`<<`/`>>`, `&`, `^`, `|||`, `**`, `..`); quirk remains listed in SPEC2 Appendix E item 3 for RFC resolution |
| 8.8 | Bitwise-or is `|||`; single `|` reserved for arm/pattern syntax | Infix table maps `Pipe3` to `PREC_BITOR` (:94); no `|` infix production | **spec-fix** — folded into the §2 expression chain above |

## 9. Round-2 hard rejections (EBNF said legal; parser is stricter)

| # | Drift | Ground truth | Disposition |
|---|---|---|---|
| 9.1 | Postfix capability on params (`fn f(x: Int @ val)`) rejected | see 8.3 | **spec-fix** — postfix production removed from grammar.ebnf §3 (item 1) |
| 9.2 | Capabilities on `let` rejected (both `let x: Int @ val = 1` and `let val x = 1`) | `let_stmt` parsing has no capability path | **spec-fix** — the §3 `let_stmt` capability production removed (item 2). Capability-qualified *types* on `let` (`let x: &iso [Int] = ...`, SPEC2 §3.9) remain and are unrelated surface |
| 9.3 | `spawn [cap]` rejected (`spawn iso Counter {}`) | `parse_spawn` (:3839) has no capability parse; it does parse prefix `link`/`monitor`, `@target`, and positional args | **spec-fix** — grammar.ebnf §3 `spawn_expr` rewritten to the real form; `link`/`monitor` moved to prefix position. Capability-of-spawned-reference syntax reserved for a future RFC (item 3) |
| 9.4 | Open effect row requires comma before the bar: `{IO, | r}`; `{IO | r}` rejected; `{| r}` fine | `parse_effect_row` (:5588): after an effect name only `,` continues the loop | **spec-fix** — grammar.ebnf §2 `effect_row` + SPEC2 Appendix A.5 corrected (item 4). SPEC2 body text already used the comma form |
| 9.5 | `!` after a fn-signature return type is an error TYPE (`fn f() -> Int ! MyError`); inside a nested arrow type `!` introduces an effect row (`f: Int -> Int ! IO`) | `parse_function` :892-921 (`! {..}` row / `! E` or `throws E` error type, optional second `!`/`throws` row) vs `parse_type_arrow` :4933-4953 (`!` always a row) | **spec-fix** — grammar.ebnf §2 `fn_decl`/`fn_tail`/`arrow_type` + SPEC2 Appendix A.2 `fn_effect` (item 5). Tension with `!` overloading already tracked in SPEC2 Appendix E item 1 |
| 9.6 | `case` not accepted in `catch` arms (match arms accept it) | `parse_catch_prefix` (:4494): catch arms are bare or `|`-prefixed, with optional `if` guards | **spec-fix** — `catch_expr`/`catch_arm` productions added to grammar.ebnf §3 and SPEC2 Appendix A.3 (item 6). NOTE: all `catch`/`fail` forms are slated for **removal** under **DRAFT RFC 0015**; the productions describe the parser as it exists today. If RFC 0015 is accepted, this grammar surface is deleted rather than maintained — flagged here per the RFC-note policy, RFCs themselves not rewritten |
| 9.7 | Handle arms have no `if` guards (match/receive/catch arms do) | `parse_handle` (:4453-4484): arm params are **bare identifiers** (no types) and no guard parse exists | **spec-fix** — grammar.ebnf §3 `handle_arm` corrected (bare params, no guard) and SPEC2 Appendix A.3 annotated (item 7). Guards on handle arms are a reasonable future RFC; not scheduled |
| 9.8 | No `fn` members in actor bodies | `parse_actor` (:1003-1086) accepts only `state`, `behavior`, `initial`, `version`, `events`, `apply`, `migration` | **spec-fix** — grammar.ebnf §3 `actor_member` and SPEC2 Appendix A.6 corrected; the additional event-sourcing members are listed (item 8) |

## 10. Round-2 parser leniencies

### 10a. Parser bugs — fix scheduled (spec unchanged; parser to converge)

| # | Leniency | Proposed resolution |
|---|---|---|
| 10.1 | Unterminated block comments (`/* ...` to EOF) lex without error; nested `/* /* */` swallows the rest of the file (item 9) | Lexer must error on unterminated block comment (standard ML-family behavior); add negative conformance case. Scheduled as a lexer bug fix |
| 10.2 | Digit-set violations accepted: `0o8`, `0b2`, bare `0b`, trailing underscore `1_` (item 10) | Lexer must validate digit sets per radix and reject a trailing separator. Scheduled as a lexer bug fix |
| 10.3 | `1.2.3` parses (member-access chaining on a float literal) (item 11) | Lexer must reject a second `.` immediately following a float literal's digits. Scheduled as a lexer bug fix |
| 10.4 | Interpolation `#{...}` parses only the first expression and silently ignores trailing tokens (`"#{a b}"` parses as `a`); a stray `}` becomes literal text (item 17) | Silent token dropping is a parser bug: reject trailing tokens before the closing `}`. Scheduled |
| 10.5 | `type X = A B` (missing `|` between variants) parses without error at parse time (item 18) | `parse_variants` must require `|` or end-of-body after each variant. Scheduled |
| 10.6 | `const = 1` (missing const name) parses (item 22) | `parse_const` must require an identifier. Scheduled |

### 10b. Accepted leniencies (documented; grammar remains the normative target)

| # | Leniency | Rationale |
|---|---|---|
| 10.7 | Case-insensitive identifier positions: lowercase type/effect names (`type x = Int`, `perform rand.int()`) and uppercase module names (`import Foo`) parse where the grammar distinguishes `IDENT`/`UPPER_IDENT` (item 12). Uppercase function names `fn Foo() {}` likewise (item 23) | The case convention is normative style, not a parser concern; enforced by convention/linting, not the grammar. grammar.ebnf §1 now says so explicitly |
| 10.8 | `receive {}` (no arms) and `receive ... after 100 => 0` (non-block after-body) parse (item 13) | Harmless degenerate forms; grammar.ebnf §3 `receive_expr` adjusted to the real, simpler shape (`{ receive_arm }` was already zero-or-more; after-body is `expr`). Classified spec-fix-adjacent but listed here as accepted behavior |
| 10.9 | `resume(1)` outside a handler arm parses as an ordinary call expression (item 14) | `resume` is contextual (only meaningful in handle arms); treating it as an identifier elsewhere is intentional and keeps it off the keyword list |
| 10.10 | Trailing commas tolerated in call args, match arms, type params, tuple types (item 15) | Intentional ergonomic leniency; grammar keeps the strict form as the normative target |
| 10.11 | `1 = 2` (assignment to a literal) parses; lvalue checking is not syntactic (item 16) | Standard phase separation (lvalue checks are semantic, as in Rust). Grammar annotates `=`-expressions as valid only in `let`/`const` as design intent |
| 10.12 | `let x = 1 let y = 2` parses (expression-position `let` chaining with no separator) (item 19) | Consequence of `let` being an expression whose body captures following expressions (ML-style `let ... in` desugar); accepted |
| 10.13 | `g() catch { }` (empty catch-arm block) parses (item 20) | Harmless degenerate form (desugars to a match with only the default `Ok` arm); accepted. Entire `catch` surface is slated for removal under DRAFT RFC 0015 anyway |
| 10.14 | Dot-imports produce a dedicated diagnostic ("use `::` ... not `.`") (item 24) | Not drift at all — intentional, helpful strictness; recorded here for completeness |

## 11. Round-2 notes on cross-document consistency

- **RFC 0015 (Draft, error-model consolidation)** proposes removing all
  `catch` forms and `fail`. Round-2 dispositions 9.6/10.13 document `catch` as
  it exists today per the reference parser; if RFC 0015 is accepted, those
  productions are deleted rather than maintained. No contradiction — but the
  RFC's migration timeline should reference the grammar productions added in
  9.6. RFCs were not rewritten.
- **grammar.ebnf §5** now states that accepted parser leniencies (§10b above)
  are deliberately NOT encoded in the grammar, so §2 ∪ §3 remains the
  normative target for future implementations.
- `cap` in grammar.ebnf §3 was also extended to the full parser set
  (`ref`, `linear` added; `src/parser.rs` `parse_capability` :5546) — a
  spec-fix folded into item 8.3's edit.
