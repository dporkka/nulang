# Grammar Conformance Corpus

This directory holds the authoritative test cases for Nulang's parser.
The reference grammar is `spec/grammar.ebnf`. The cases here (positive: must parse, negative: must reject) are executed by the `test_grammar_conformance` harness in `src/integration_tests.rs`.

Any syntax change via RFC must be accompanied by an update to the EBNF and new cases here.

## Corpus layout

- `positive_*.nula` — grouped positive cases; each case is introduced by a
  `// case NN:` comment. Every file must parse in full (cases may depend on
  declarations earlier in the same file — the parser resolves type, effect,
  and handler names at parse time).
- `negative_*.nula` / `negative2_*.nula` — one negative case per file; each
  must be rejected by the lexer or parser.

Round 1 added 260 positive + 128 negative cases. Round 2 (branch
`feat/grammar-conformance-2`) added 163 positive cases across
`positive_{capabilities,effects,actors,patterns,strings,syntax}_round2.nula`,
`positive_imports_aliases.nula`, and `positive_error_types.nula`, plus 92
`negative2_*` negative cases. All cases were validated against the real
parser (`src/lexer.rs` + `src/parser.rs`, `cargo build --no-default-features`)
before inclusion.

## Drift log

Known divergences between `spec/grammar.ebnf` and the reference parser.
Cases in this corpus match **parser reality**; each entry below is a
candidate EBNF update (or parser bug) to be resolved by RFC.

### Documented in round 1 (see `// case` comments for locations)

- `if` requires a `then` keyword (EBNF §2 `if_expr` has none).
- Variant payloads take a single type (EBNF allows `{ "," type }`).
- Capabilities on parameters are prefix (`val x: Int`); EBNF §3 gives the
  postfix form `x: Int @ val`.
- Type aliases require `type alias` (or a non-variant `type Name =` body);
  bare `alias Name = T` is rejected.
- No character literals (EBNF §1 defines `CHAR_LIT`).
- No compound assignment (`+=`, `-=` are rejected at parse time:
  "Not a binary operator: PlusAssign/MinusAssign").
- Bitwise/shift operators bind tighter than arithmetic (Pratt quirk).
- Bitwise-or is `|||`; single `|` is reserved for arm/pattern syntax.

### New in round 2

Hard rejections where the EBNF says the form is legal (parser is stricter):

1. **Postfix capability on params** — EBNF §3 `param += IDENT ":" type "@" cap`;
   the parser rejects `fn f(x: Int @ val)` (prefix-only).
   Negative: `negative2_postfix_cap_param.nula`.
2. **Capabilities on `let`** — EBNF §3 `let_stmt = "let" IDENT ":" type "@" cap ...`;
   the parser rejects both `let x: Int @ val = 1` and `let val x = 1`.
   Negatives: `negative2_postfix_cap_let.nula`, `negative2_prefix_cap_let.nula`.
3. **`spawn [cap]`** — EBNF §3 allows `spawn iso Counter {}`; the parser has
   no capability parsing in `parse_spawn` and rejects it.
   Negative: `negative2_spawn_cap.nula`.
4. **Open effect row separator** — `Int -> Int ! {IO | r}` is rejected; the
   parser requires a comma before the row bar: `{IO, | r}` (bare `{| r}` and
   `{r}` are also accepted). Positives use the comma form; negative:
   `negative2_open_row_nocomma.nula`.
5. **`!` after a fn-signature return type is an error TYPE, not an effect
   row** — `fn f() -> Int ! MyError` resolves `MyError` as a (parse-time
   resolved) type name, while inside a nested arrow type
   (`f: Int -> Int ! IO`) `!` introduces an effect row. Same surface token,
   two different grammars depending on position.
   See `positive_error_types.nula` (declared error types vs braced rows).
6. **`case` is not accepted in `catch` arms** — `g() catch { case E(x) => 0 }`
   is rejected, although `match` arms accept optional `case`. Catch arms are
   bare or `|`-prefixed. Negative: `negative2_catch_case_kw.nula`.
7. **Handle arms have no guards** — `| E.op() resume if c => 1` is rejected,
   while `match`, `receive`, and `catch` arms all accept `if` guards.
   Negative: `negative2_handle_guard.nula`.
8. **No `fn` members in actor bodies** — EBNF §3
   `actor_member = state_field | behavior | fn_decl`; the parser rejects
   `fn` inside `actor { }` (allowed members: `state`, `behavior`, `initial`,
   `version`, `events`, `apply`, `migration`).

Parser leniencies (accepted although a strict reading of the EBNF would
reject; positives/negatives here deliberately avoid relying on most of
these):

9. Unterminated block comments (`/* ...` to EOF) lex without error; a
   nested `/* /* */` comment then swallows the rest of the file.
10. Digit-set violations are accepted: `0o8`, `0b2`, bare `0b`, and a
    trailing underscore `1_` all lex as numbers.
11. `1.2.3` parses (member-access style chaining on a float literal).
12. Lowercase names are accepted where the EBNF requires `UPPER_IDENT`:
    `type x = Int`, `import Foo` (uppercase where IDENT expected), and
    `perform rand.int()` (lowercase effect qualifier) all parse.
13. `receive {}` (no arms) and `receive ... after 100 => 0` (non-block
    after-body; EBNF requires a block) parse.
14. `resume(1)` outside a handler arm parses as an ordinary expression.
15. Trailing commas are tolerated in call args, match arms, type params,
    and tuple types (`g(1, 2,)`, `(Int,)`, `type X[T,] = A`).
16. `1 = 2` (assignment to a literal) parses; lvalue checking is not
    syntactic.
17. Interpolation `#{...}` parses only the first expression and silently
    ignores trailing tokens (`"#{a b}"` parses as `a`); a `}` after the
    closing brace becomes literal text.
18. `type X = A B` (missing `|` between variants) parses; the second
    variant name is not an error at parse time.
19. `let x = 1 let y = 2` parses (expression-position `let` chaining with
    no separator).
20. `g() catch { }` (empty catch-arm block) parses.
21. `fn f() -> { 1 }` is rejected, but `fn(x: Int) -> { x }` in lambda
    position parses (the `->` in lambdas does not require a type).
22. `const = 1` (missing const name) parses.
23. `fn Foo() {}` — uppercase function names are accepted.
24. Dot-imports produce a dedicated diagnostic ("use `::` ... not `.`");
    negative: `negative2_import_dot.nula`.
