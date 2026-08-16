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

# Round 2 — Grammar Conformance Reconciliation

Reconciliation pass over the round-1 and round-2 drift findings in
`conformance/grammar/README.md` (drift log) against `spec/grammar.ebnf`
and `SPEC2.md`. Ground truth was established by reading `src/parser.rs`
(`parse_if` :3513, `try_parse_param_capability` :5871, `parse_spawn`
:3839, `parse_actor` :969-1105, `parse_handle` :4424, `parse_catch_prefix`
:4494 / postfix catch :2884, `parse_receive`, `parse_effect_row` :5588,
fn-signature error-type/effect annotation :884-930, the Pratt table
:30-100) and `src/lexer.rs` (`read_number` :585, `keyword()` :1193).
No Rust source was modified.

Branch: `docs/spec-reconciliation-2`.

**Disposition counts (32 items): 17 spec-fixes, 7 parser-fixes
scheduled, 8 accepted leniencies.**

## Round-1 drift log items — dispositions

| # | Item | Disposition | Resolution |
|---|------|-------------|------------|
| R1.1 | `if` requires `then` | **Spec fix** | Precise rule: `then` is optional for an expression then-branch but **required** for a block then-branch (`src/parser.rs:3563-3577`). `grammar.ebnf` §2 `if_expr` and SPEC2 §6.6 updated. |
| R1.2 | Variant payloads take a single type | **Spec fix** | `grammar.ebnf` §2 `variant` now allows one payload type; multi-value constructors take a tuple payload (SPEC2 §3.5 already said this). |
| R1.3 | Capabilities on parameters are prefix (`val x: Int`) | **Spec fix** | This is the implemented, Pony-style syntax; the EBNF's postfix `x: Int @ val` production was removed. `grammar.ebnf` §2 `param`, SPEC2 §2.7/A.2 updated. |
| R1.4 | Type aliases require `type alias` or a non-variant `type Name =` body | **Spec fix** | Bare `alias Name = T` rejected (`src/parser.rs:2159`, :735 alias only under `type`). `grammar.ebnf` §2 `alias_decl`/`alias_body`, SPEC2 A.2 updated. |
| R1.5 | No character literals | **Spec fix** | `CHAR_LIT` removed from `grammar.ebnf`; SPEC2 §2.5.4/§3.6 already marked `Char` Planned. |
| R1.6 | No compound assignment (`+=`, `-=`) | **Spec fix** | Tokens lex but are rejected at parse time; documented in `grammar.ebnf` §2 `assign_expr`. |
| R1.7 | Bitwise/shift binds tighter than arithmetic | **Accepted** | Documented Pratt quirk (SPEC2 §2.6 + Appendix E item 3); changing it is a breaking change reserved for an RFC before the next freeze. `grammar.ebnf` §2 now encodes the real precedence levels. |
| R1.8 | Bitwise-or is `\|\|`; single `\|` reserved | **Spec fix** | `grammar.ebnf` §2 expression ladder now includes `<<`/`>>`/`&`/`^`/`\|\|`/`**` at the parser's levels (`src/parser.rs:30-46`). |

## Round-2 drift log items — dispositions

### Hard rejections (parser stricter than the old EBNF) — all spec fixes

| # | Item | Disposition | Resolution |
|---|------|-------------|------------|
| 1 | Postfix `@cap` on params rejected | **Spec fix** | EBNF §3 `param += IDENT ":" type "@" cap` removed; prefix-only is normative (`src/parser.rs:5871`). |
| 2 | Capabilities on `let` rejected (both forms) | **Spec fix** | EBNF §3 capability `let_stmt` removed; `grammar.ebnf` §2 `let_stmt` documents the rejection. Capability-qualified let surface remains **Planned**. |
| 3 | `spawn [cap]` rejected | **Spec fix** | EBNF §3 `spawn_expr` rewritten to the real grammar (`[link\|monitor]`, `[@node]`, positional args XOR field-init block, trailing `as "name"`); no capability parsing exists in `parse_spawn` (:3839). Capability-qualified spawn remains **Planned**. SPEC2 §8.7 updated. |
| 4 | Open effect row requires comma before `\|` (`{IO, \| r}`) | **Spec fix** | EBNF §2 `effect_row` and SPEC2 A.5 now require the comma; `{IO \| r}` rejected, `{ \| r}` and `{r}` accepted (`parse_effect_row` :5588). |
| 5 | `! E` after a fn-signature return type is an error TYPE; in nested arrow types `!` starts an effect row | **Spec fix** | Positional split documented in EBNF `sig_suffix`/`error_type` vs `arrow_type` and SPEC2 A.2/A.5; already listed as design tension (SPEC2 Appendix E item 1). A future disambiguation is a breaking change for an RFC. |
| 6 | `case` not accepted in `catch` arms | **Spec fix** | EBNF §3 `catch_expr`/`catch_arm` added (guards and `\|` prefixes allowed, no `case`); SPEC2 §6.11 documents it. Consistent with RFC 0015 (Draft), which removes `catch` — see §"RFC notes" below. |
| 7 | No guards on `handle` arms | **Spec fix** | `handle_arm` admits bare parameter names and no guards (`parse_handle` :4424); SPEC2 §4.4 documents the asymmetry with match/catch/receive. |
| 8 | No `fn` members in actor bodies | **Spec fix** | EBNF §3 `actor_member` = `state` \| `behavior` \| `initial` \| `version` \| `events` \| `apply` \| `migration` (`parse_actor` :1003-1105); SPEC2 §8.1/A.6 updated. |

### Parser leniencies — dispositions

| # | Item | Disposition | Proposed resolution |
|---|------|-------------|---------------------|
| 9 | Unterminated block comments lex to EOF; nested `/* /* */` swallows the file | **Parser fix scheduled** | Lexer should error on unterminated `/*` at EOF. |
| 10 | `0o8`, `0b2`, bare `0b`, `1_` "lex as numbers" — in fact the lexer splits radix-prefixed/separated digit runs into `Int` + identifier (`0b1010` → `0` + `b1010`), which can then parse as adjacent expressions | **Parser fix scheduled** | Lex `0b`/`0o`/`0x` prefixes and `_` separators properly, or raise a lex error. SPEC2 §2.5.1 now documents the deviation; until fixed these forms are rejected-by-design. |
| 11 | `1.2.3` parses (member-access chaining on a float literal) | **Parser fix scheduled** | Reject a second `.` immediately following a float literal's fraction. |
| 12 | Case conventions unenforced: lowercase type/effect names (`type x = Int`, `perform rand.int()`), uppercase module segments accepted | **Accepted leniency** | The IDENT/UPPER_IDENT distinction remains normative in the EBNF for variant constructors; elsewhere it is a naming convention. A lint (not a parse error) is the intended enforcement. |
| 13 | `receive {}` and non-block `after` bodies parse | **Spec fix** | Adopted: EBNF §3 `receive_expr` admits an empty arm block and any expression `after` body (matches the standalone `after ms => expr` sugar). |
| 14 | `resume(1)` outside a handler arm parses as an ordinary expression | **Parser fix scheduled** | Known deviation: `resume` should be an error outside a handler arm (or unambiguous function-call syntax); today it fails only at runtime ("VM error", SPEC2 §4.4.2). |
| 15 | Trailing commas tolerated in call args, match arms, type params, tuple types | **Accepted leniency** | Documented as tolerated-not-normative in the EBNF header and reflected in the productions (`[ "," ]`). |
| 16 | `1 = 2` (assignment to a literal) parses | **Accepted leniency** | Lvalue checking is semantic, not syntactic; documented in EBNF `assign_expr`. |
| 17 | `#{...}` interpolation parses only the first expression, silently ignoring trailing tokens | **Parser fix scheduled** | Parser must consume the full expression and require the closing `}`. SPEC2 §2.5.3 documents the deviation. |
| 18 | `type X = A B` (missing `\|`) parses; second variant name not an error at parse time | **Parser fix scheduled** | Variant parser should require `\|` between constructors. |
| 19 | `let x = 1 let y = 2` parses (separator-less chaining) | **Accepted leniency** | Statement separators are effectively optional between `let` bindings; harmless in expression-position `let` chains. |
| 20 | `g() catch { }` (empty catch block) parses | **Accepted leniency** | Harmless (desugars to a match that re-raises); `catch` itself is slated for removal under RFC 0015 (Draft). Documented in SPEC2 §6.11. |
| 21 | `fn(x: Int) -> { x }` (lambda `->` with no type) parses, while `fn f() -> { 1 }` is rejected | **Spec fix** | Adopted: EBNF §2 `lambda` allows `->` without a type; declarations still require one. SPEC2 A.3 updated. |
| 22 | `const = 1` (missing name) parses | **Parser fix scheduled** | `parse_const` should require an identifier. Noted in EBNF `const_decl`. |
| 23 | `fn Foo() {}` — uppercase function names accepted | **Accepted leniency** | Same convention-level case leniency as item 12. |
| 24 | Dot-imports (`import a.b`) produce a dedicated diagnostic ("use `::`") | **Accepted** | Intentional helpful error; EBNF `import_decl` documents the `::` requirement. No change. |

## RFC notes

- **RFC 0015 (Draft, error-model consolidation)** proposes removing `catch`
  entirely. Dispositions 6 and 20 document *current* `catch` surface and do
  not contradict the RFC; if 0015 is accepted, `catch_expr`/`catch_arm`
  leave `grammar.ebnf` §3 under its deprecation plan (RFC 0015 §"Migration").
  No RFC text was changed.
- **RFC 0002 (Frozen Core)** is unaffected: every Core-production change
  above is a *correction toward the parser* that round-1/round-2 conformance
  evidence shows was the implemented behavior at freeze time (prefix
  capabilities were already the only implemented form). No Core semantics
  changed.
- **Launch docs** (`docs/LAUNCH.md`, `docs/launch/`): spot-checked claims
  about capabilities, spawn, and effect rows match the reconciled grammar
  (prefix capabilities, `spawn Actor { ... }`); no contradictions found.

## Files changed in this round

- `spec/grammar.ebnf` — header conformance-suite note; lexical section
  (`CHAR_LIT` removed, radix-literal deviation note, interpolation);
  corrected keyword list from `src/lexer.rs`; §2 expression ladder with the
  real Pratt levels (`**`, `<<`/`>>`, `&`, `^`, `|||`, `..`, `|>`);
  `if_expr`, `variant`, `alias_decl`, `param`, `lambda`, `pattern` (or/alias/
  tuple/record/nil/unit), `sig_suffix`/`effect_row`/`error_type`; §3
  `handle_arm`, `catch_expr`, prefix-only capabilities, `actor_member`,
  `state_field`, `spawn_expr`, `receive_expr` guards/after; §5 leniency
  inventory. Validated: balanced `[]`/`{}`/`()`, no duplicate `=`
  nonterminal definitions (the round-1 duplicate-production class of bug).
- `SPEC2.md` — §2.5.1 radix-literal deviation, §2.5.3 `#{...}`
  interpolation implemented + first-expression deviation, §2.7 capability
  annotation row, §6.6 required-`then` rule, §6.11 catch paragraph, §6.12
  stale "no `with`" removed, §4.4 handle-arm guard asymmetry, §6.14
  receive guards/after/empty, §8.1 actor members, §8.7 spawn forms,
  Appendix A (A.2 parameters/function_definition/alias note; A.3
  conditional/lambda/handle/catch/actor_expr; A.4 or-patterns; A.5
  effect_row comma rule; A.6 actor_member/state/initial).
- `docs/SPEC_RECONCILIATION.md` — this section.
