# Nulang Changelog

> This changelog is organized by **stability tier** (see `GOVERNANCE.md` §2),
> not by release. The tier determines what may change and how. The crate
> version in `Cargo.toml` is the implementation version; the
> **language version** (`[package.metadata] language-version`, and
> `LANGUAGE_VERSION` in `src/format/constants.rs`) is what this changelog
> tracks — it moves only on RFC-ratified change.

**Language version:** `1.0.0-frozen` (since 2026-07-19; RFCs 0001, 0002).

---

## Frozen tier

*Will never break. A change here is a new language and requires a new major
version + migration.*

### Language version 1.0.0-frozen — 2026-07-19

- **RFC 0001 — Format Stability.** Established versioned, frozen binary
  formats for durable artifacts and the wire protocol.
  - `.nbc` bytecode artifact format version 1 (magic `NLBC`, header with
    `format_version`, `language_version`, BLAKE3 `source_hash`). Codec:
    `CodeModule::to_nbc` / `from_nbc` in `src/format/nbc.rs`.
  - NUL0 wire protocol handshake version 1 (16-byte
    `{magic "NUL0", version u32, node_id u64}`). Unknown versions are refused,
    never reinterpreted. `src/runtime/network.rs`.
  - Value layout version 1 (`src/value_layout.rs`, i64-tagged).
  - Migration registry `src/format/migrate.rs` as the sole legal home for
    format upgrades. v1→v1 identity.
  - `FormatError` enum: `Truncated`, `BadMagic`, `UnsupportedVersion`,
    `IncompatibleLanguage`, `LengthMismatch`, `UnknownOpcode`, `BodyDecode`,
    `BadConstant`.
- **RFC 0002 — Frozen Core.** Defined Nulang Core, the minimal frozen subset:
  `fn`/`let`/`if`/`match`/closures, `Int`/`Bool`/`String`/`Unit`/`Nil`/
  `Vec`/`Map`/tuples/records/`enum`, HM inference over this subset, `IO.print`
  and `IO.read` only, `val` capability only. Every Core program valid today is
  valid in every future version.
- Stability contract published as `SPEC2.md` §"Format Stability" and
  `GOVERNANCE.md`.

## Stable tier

*Breaking changes require an accepted RFC and a deprecation cycle of at least
two major versions.*

### Unchanged at 1.0.0-frozen

The following are classified Stable as of 1.0.0-frozen. They have not changed
in this version; they are recorded here to establish their tier.

- The full HM type system and inference rules (`src/typechecker.rs`).
- The effect-row system: closed/open rows, regions (`src/effect_checker.rs`).
- The capability lattice (`iso`/`trn`/`ref`/`val`/`box`/`tag`/`lineariso`)
  and subtyping (`src/effect_checker.rs`).
- The actor surface: `spawn`, `send`, `receive`, supervision
  (`src/runtime/`, `src/vm.rs`).
- CRDT operations and merge semantics (`src/runtime/crdt.rs`,
  `src/runtime/crdt_reg.rs`).

### Added since 1.0.0-frozen — 2026-07-29

- **RFC 0005/0007 — `entity` keyword and event sourcing.** `entity` desugars
  to `persistent actor` with `event_sourced` default state model. `events`
  and `apply` blocks for typed event declarations and automatic state
  mutation. `emit EventName(args)` type-checked against entity event
  declarations. `after ms => expr` standalone sugar. Entity events validated
  at compile time; unknown events produce type errors.
- **RFC 0008 — Migration contracts.** `version: N` and
  `migration from N to M { ... }` blocks parsed inside entity declarations.
  AST/HIR/bytecode metadata wired through pipeline. Migration state bodies
  and event-migration handlers are now type-checked.
- **RFC 0009 — Organization primitives.** `organization` keyword
  parsed and desugared to `entity` with durable defaults. `is_organization`
  flag tracked through AST → HIR → bytecode.
- **RFC 0003 Item 6 — Backend trait boundary.** `JitBackend`, `WasmBackend`,
  `CryptoProvider`, `ForeignInterop`, `HttpProvider` traits defined in
  `src/backends/mod.rs`. JIT and WASM wired behind traits.
- **MIR register spilling.** Functions with more locals than fit in the
  register file (238 usable registers) now spill excess locals into a
  frame-local `Vec<Value>` via `SpillLoad`/`SpillStore` opcodes (0xF5/0xF6).
  Fix (2026-07-24): replaced post-processing spill rewrite with inline
  SpillLoad/SpillStore emission during codegen, removing the 17-slot
  capacity limit entirely.  Round-robin temp register allocation (r12/r13/r14)
  prevents clobbering in multi-operand spilled reads.  Net -112 lines.
  Unblocks the self-hosting bootstrap compiler (RFC 0003 Item 3).
- **Self-hosting bootstrap: Stage 5 (closures with env capture).** The
  `bootstrap/compiler_core.nula` Pratt evaluator now supports `fn(x) => body`
  lambdas, function application `f(arg)`, and environment capture
  (`let a = 3 in (fn(x) => a + x)(5)` → 8).  Closure encoding: 30-bit flag
  with packed param-hash, body-start, and captured binding.  Out-of-band
  sentinel `1 << 40` distinguishes "no left operand" from value 0.
- **Formal semantics: all three Core theorems proved.** Progress, preservation,
  and type soundness for the Core expression language (RFC 0002) are now
  machine-checked in `spec/formal/types.lean` (0 sorries). The capability
  lattice proofs (`capabilities.lean`) remain proved; effect and combined
  judgments are definition-only. See `spec/formal/README.md` for scope.

- **Error handling syntax.** `catch expr => body`, `fail expr`
  (structured short-circuit return), and `T ! E` return-type syntax
  (`fn div(a: Int, b: Int) -> Int ! String`). Errors propagate through
  `?` operator — `expr?` is sugar for `catch expr => |e| fail e`.
  Desugaring, type inference, and codegen wired in `src/parser.rs`,
  `src/typechecker.rs`, `src/hir_lower.rs`, `src/mir_lower.rs`, and
  `src/mir_codegen.rs`.
- **Transport resilience.** `send remote` and `ask remote` keywords
  enforce network-sendable (`val`/`tag`) capability constraints at the
  call site. `ask remote actor behavior(args) timeout N` accepts an
  optional `timeout` clause for request-response with deadline semantics.
  Capability enforcement lives in `src/effect_checker.rs`; transport
  modifiers parsed in `src/parser.rs`.
- **RFC 0010 — 100-Year Language Architecture.** Documented design rationale
  for multi-century relevance. Deliverables implemented:
  - **LLM→Inference effect alias:** `perform LLM.ask(p)` and
    `perform Inference.ask(p)` are synonyms; both resolve to
    `Effect::Inference`. The `LLM.ask` surface is a deprecated alias
    (`src/effect_checker.rs`, `src/mir_lower.rs`, `src/runtime/mod.rs`,
    `src/stdlib.rs`).
  - **Keyword lifecycle governance:** `GOVERNANCE.md` §2a defines keyword
    introduction, reservation, deprecation, and removal rules.
  - **Keyword namespace cleanup:** Five formerly-reserved keywords
    (`where`, `priv`, `loop`, `node`, `subworkflow`) removed from the
    lexer and now lex as plain identifiers. `await` re-reserved (July
    2026) for future async/await support (`src/lexer.rs`).
  - **Keyword inventory documented** in `SPEC2.md` §Implementation Status
    and verified against the implementation.

- `ai-runtime` feature: the AI runtime — **pure types live in the `nulang-ai`
  workspace crate** (`crates/nulang-ai/`) with zero dependencies on the core
  language crate. The core crate (`src/ai/mod.rs`) provides a thin re-export
  facade behind `#[cfg(feature = "ai-runtime")]`. All AI effects dispatch
  through the generic `PerformAsync` opcode (`0xC6`) with `effect_op` strings
  (`"Inference.ask"`, `"Pipeline.run"`, etc.). The monolithic AI opcode range
  (0x9D–0xC5: `LlmAsk`, `PipelineNew`…`DebateRun`) has been removed.
  Runtime integration (`src/runtime/agent.rs`, `llm.rs`, `ai_registry.rs`)
  bridges the pure AI types to the actor model. Behind `--features ai-runtime`
  (enabled by default). `LLM.ask` is a deprecated alias for `Inference.ask`
  and emits a compiler warning (RFC 0010).
- `python` feature: PyO3 interop (`src/python/`). Behind `--features python`.
- `sqlite` feature: libsql/Turso persistence. Behind `--features sqlite`.
- `lsp` feature: the tower-lsp language server (`src/lsp/`). Behind
  `--features lsp`.
- `ai-runtime` feature: the AI runtime (`crates/nulang-ai/` workspace crate,
  re-exported through `src/ai/`) — LLM providers (OpenAI, Ollama), pipelines,
  debates, supervisor teams, memory subsystems, and usage tracking. Behind
  `--features ai-runtime` (enabled by default). **Changed in 1.0.0-frozen:**
  all AI effects now dispatch through the generic `PerformAsync` opcode
  (`0xC6`) with `effect_op` strings (`"Inference.ask"`, `"Pipeline.run"`,
  etc.). The dedicated `LlmAsk` opcode and the `PipelineNew`…`DebateRun`
  opcode range (0x9D–0xC5) have been removed. AI types live in the
  `nulang-ai` crate with zero core dependencies; the core `ActorVmCallbacks`
  trait no longer carries AI-specific methods. The `LLM` effect redirects to
  `Provider.ask` under the hood.
- AOT native backend (`src/aot/`), JIT tiering (`src/jit/`).

- **Stdlib modules.** Standard library modules provide reusable generic
  data structures and operations: `stdlib::core` (base utilities),
  `stdlib::list` (map/filter/fold/reverse), `stdlib::string`
  (split/join/trim/replace), `stdlib::set` (add/remove/contains/union/
  intersect), `stdlib::map` (insert/get/remove/keys/values), and
  `stdlib::http` (get/post request builders). Modules live under
  `src/stdlib/` and are resolved via `NULANG_STDLIB`, the executable-
  relative path, or the dev-fallback `src/stdlib/`.
- **Typeclass declarations (Phase 4).** `class` and `impl`
  keyword support: `class Eq[T] { fn eq(self: T, other: T) -> Bool }`
  declares a typeclass with optional superclasses (`class Ord[T]: Eq`).
  `impl Eq Int { fn eq(self: Int, other: Int) = self == other }` registers
  a concrete instance. Class/instance tables in `TypeChecker`
  (`src/typechecker.rs`). Typechecker integration (dictionary-passing
  transform): method calls on concrete types (`1.eq(2)`) resolve through
  the instance table and type-check against the impl dictionary; missing
  instances (`"hi".eq("there")` with no `impl Eq String`) produce
  compile-time errors. HIR lowering for runtime dictionary construction
  is implemented: `Decl::Impl` lowers to `hir::Decl::Constant`, producing
  a module-level function that evaluates to a record of method closures.
  Field access routing through the dictionary at call sites is
  implemented: method calls on concrete types (`1.eq(1)`) lower to
  dict-constant calls, field accesses, and method invocations at the
  HIR level, producing correct runtime results. Full end-to-end
  verified with integration tests.
- **RFC 0003 — Content-addressed functions.** Proposal document
  (`RFC/0003-content-addressing.md`): defines a deterministic
  content-hash-based code identity scheme for distributed code
  deployment, cache invalidation, and reproducible builds across
  heterogeneous Nulang runtimes. Status: Draft. Content hashing
  infrastructure (BLAKE3 `source_hash` in `.nbc` artifacts) is
  available per RFC 0001; full code-identity registry and
  content-addressed deployment are not yet implemented.
- **`::` import resolution.** Module imports now support `::`-delimited
  paths: `import stdlib::set`, `import mypkg::utils::math`. The resolver
  (`src/resolver.rs`) maps `stdlib::*` prefixes to the standard library
  directory and general `::` paths to filesystem-relative module files.

### Added since 1.0.0-frozen — 2026-07-30
- **Triple-quoted strings and `\u{...}` escapes.** Triple-quoted multi-line strings (`\"\"\"...\"\"\"`) and `\u{...}` unicode escape sequences implemented. Triple-quoted strings support standard escapes; interpolation is unsupported. Surrogate and out-of-range code points are rejected with a `LexError`. Implementation: `src/lexer.rs`. (Stable)

- **`**` exponentiation operator.** Right-associative, precedence above `*`
  (Pratt level 13), tokenized as `Star2`. Wired through the full pipeline:
  lexer (`src/lexer.rs`), parser (Pratt `PREC_EXP`), typechecker, HIR
  lowering, and bytecode. `a ** b ** c` parses as `a ** (b ** c)`.
- **Structured error messages.** `NuError` enum in `src/types.rs` with
  per-variant `expected`/`found` fields, `ErrorCode` classification,
  automatic fix suggestions (`suggestion()`), and `format_rich()` for
  colorized multi-line diagnostics with source excerpts and carets.
  Constructor helpers (`type_mismatch`, `missing_effect`, etc.) produce
  rich errors with minimal boilerplate at each call site.
- **Language correctness fixes** (all Stable, `src/`):
  - *Let-chain stack overflow:* long chains of consecutive `let` bindings are
    now flattened iteratively in the parser (sequential `let`-statement
    peeling) and HIR lowering (`lower_let_chain`), eliminating deep-recursion
    overflow on blocks with 40+ lets (`src/parser.rs`, `src/hir_lower.rs`).
  - *Spawn field-initializer overrides:* `spawn A { f = v }` now correctly
    overrides the actor's declared default for field `f`. Overrides are
    encoded in bytecode (`spawn_init_overrides` in `CodeModule`) and applied
    at VM spawn time, replacing any matching default (`src/vm.rs`,
    `src/mir_codegen.rs`, `src/bytecode.rs`). Backward-compatible: older
    `.nbc` artifacts missing the field deserialize with an empty vec via
    `serde(default)` (`src/format/nbc.rs`).
  - *Clearer immutable-binding error:* the type error for reassigning a
    `let` binding (`"cannot assign to immutable binding 'x'; mutable locals
    (var) are not yet supported. Use 'let x = <new value> in ...' to shadow
    the binding."`) now explains the constraint and suggests the shadowing
    workaround (`src/typechecker.rs`).
  - *Prefix `catch` syntax:* `catch expr fallback` is now accepted in
    addition to the postfix form `expr catch fallback`; desugars identically
    (`src/parser.rs`).
- **Package manager subcommands** (Experimental, `src/package/commands.rs`):
  `nula init` (scaffold a package with `Nulang.toml`, `src/main.nula`,
  `.gitignore`), `nula list` (print locked dependencies), `nula clean`
  (remove `.nbc` build artifacts), `nula add <name> [--path|--git|--version]`
  (add/update a dependency and re-resolve the lockfile), `nula remove <name>`
  (remove a dependency and update the lockfile), `nula run --watch` /
  `nula watch` (build, run, and re-run on source changes via mtime polling),
  and `nula doc [--open]` (generate Markdown API docs from doc comments and
  declarations).
- **REPL enhancements** (Experimental, `src/repl.rs`): `:help <topic>`
  (topics: syntax, types, actors, effects, commands), `:load <file>` (load
  and evaluate a `.nula` file), `:type <expr>` (show the inferred type
  without evaluating), tab completion (identifiers, keywords, REPL
  commands, stdlib modules), and automatic multi-line input when
  braces/parens/brackets are unclosed (prompt changes to `.... `).
- **New stdlib modules** (Experimental, `src/stdlib/`):
  - `result`: Result combinators (`unwrap`, `map`, `flat_map`). The `Result`
    type (`Ok(T) | Error(E)`) is defined in `stdlib::core` (auto-loaded).
  - `option`: Option combinators. The `Option` type (`Some(T) | None`) is
    defined in `stdlib::core`.
  - `datetime`: `DateTime` record type with calendar fields.
  - `math`: trigonometry (`sin`, `cos`, `tan`, `asin`, `acos`, `atan`,
    `atan2`), logarithms (`ln`, `log2`, `log10`), power/root (`pow`, `sqrt`),
    rounding (`ceil`, `floor`, `round`, `trunc`), constants (`PI`, `E`).
  - `fs`: wrapper functions around the `FS` built-in effect (see below).
  - `test`: assertion helpers powered by the `Test` built-in effect (see below).
- **`FS` filesystem effect** (Experimental). Built-in effect wired into the
  standalone VM: `perform FS.read(path) -> String`, `perform FS.write(path,
  content) -> Unit`, `perform FS.append(path, content) -> Unit`,
  `perform FS.exists(path) -> Bool`. Effect-aware type signatures (`!
  {FS}`) are enforced. Declared in `src/stdlib.rs`; wrapper functions in
  `src/stdlib/fs.nula`.
- **`Test` assertion effect + `nula test` runner** (Experimental).
  `perform Test.assert(cond, msg)`, `perform Test.assert_eq(a, b)`,
  `perform Test.assert_true(cond)`, and `fail_with(message)`. The test runner
  (`nula test [--filter <substr>]`) discovers `.nula` test files under the
  package's `tests/` directory, executes each, and reports pass/fail counts
  with optional name filtering (`src/stdlib/test.nula`,
  `src/package/commands.rs`).
- **LSP enhancements** (Experimental, `src/lsp/mod.rs`): `.` and `::`
  completion trigger characters for automatic invocation, field-access
  completion (on `self.` fields, record fields, and actor state),
  `textDocument/didSave` handler that re-checks the file on save, and
  completion items sorted by category (locals > functions > types > variants
  > keywords > effects) via `sort_text` prefixes.
- **Example programs.** 15 verified, runnable example programs under
  `examples/` with `examples/README.md`: from basic IO and arithmetic
  through functions, pattern matching, records, higher-order functions,
  algebraic effects, actors, loops, the pipe operator, arrays, JSON
  parsing, HTTP requests, Option/Result combinators, and range expressions.

- **`var` bindings** (Experimental). Mutable local variables via `var x = 0`
  (declaration) and `x = x + 1` (reassignment). `var` bindings are tracked
  separately from `let` in the typechecker and codegen, producing `Store`
  and `Load` bytecode ops for mutation — `src/parser.rs`,
  `src/typechecker.rs`, `src/mir_codegen.rs`.
- **Record-update syntax** (Experimental). `{ base .. field = value }`
  creates a new record with overridden fields. The `..` is parsed with
  `PREC_RANGE` precedence; the parser disambiguates record-update from
  range-in-block by checking for `=` after the right operand —
  `src/parser.rs`.
- **Tuple field access** (Stable). Numeric indices on tuples: `t.0`, `t.1`.
  Chained access (`t.0.1`) works directly on nested tuples without
  parenthesization — `src/parser.rs`, `src/hir_lower.rs`.
- **Range expressions** (Experimental). `a .. b` produces an inclusive-
  exclusive range at `PREC_RANGE` precedence (level 3, between pipe and
  logical-or). Ranges work in `for` loops (`for i in 0 .. 5 { … }`) and
  can appear bare in blocks (`{ a .. b }`) — `src/parser.rs`.
- **Language correctness fixes** (all Stable, `src/`):
  - *`else`-on-newline:* an `else` keyword following a newline after `}` is
    now accepted in `if`/`else` chains — `src/parser.rs`.
  - *`String.+` fix for variables:* `a + b` where both operands are
    `let`-bound string variables now correctly concatenates instead of
    returning `0` — `src/vm.rs`.
  - *`let..in` scoping fix:* block-level `let x = V in BODY` now correctly
    scopes `x` to `BODY` only, not to the remainder of the enclosing block
    — `src/hir_lower.rs`.
- **`String.from_char`** (Stable). `perform String.from_char(code)` creates
  a single-character string from a Unicode code point; returns `nil` for
  invalid code points (surrogates, out of range) — `src/stdlib.rs`,
  `src/vm.rs`.

- **`Http` builtin effect** (Experimental). `perform Http.get(url)` and
  `perform Http.post(url, body)` wired into the standalone VM via `ureq`.
  Returns the response body as a `String` on success, `nil` on error —
  `src/stdlib.rs`, `src/vm.rs`.
- **`Array` builtin effect** (Experimental). `perform Array.length(arr)`,
  `perform Array.push(arr, elem)`, `perform Array.new(n, init)`,
  `perform Array.set(arr, idx, val)`, and `perform Array.slice(arr, start, end)`
  wired into the standalone VM with value semantics (all return new arrays) —
  `src/stdlib.rs`, `src/vm.rs`.
- **Numeric conversion primitives** (Experimental). `Int.to_float`,
  `Float.to_int` (truncates toward zero), `Float.to_string`, `String.to_int`
  (returns 0 for invalid input), and `String.to_float` (returns 0.0 for
  invalid input) — `src/stdlib.rs`, `src/vm.rs`.

- **JSON parser** (Experimental). Pure-Nulang recursive-descent JSON parser
  in `stdlib::json`: `parse(json: String) -> JsonValue` handles all JSON
  value types with proper escape processing, and `stringify(value: JsonValue)
  -> String` produces valid JSON output. Uses `String.to_float`,
  `Float.to_string`, `String.from_char`, and `Array.*` primitives —
  `src/stdlib/json.nula`.
- **All 13 stdlib modules functional** (Experimental). `core`, `list`,
  `string`, `set`, `map`, `test`, `fs`, `option`, `result`, `datetime`,
  `math`, `json`, and `http` all parse, import, and resolve correctly with
  all VM primitives available — `src/stdlib/`.

- **LSP: code lenses, document links, enriched hover** (Experimental,
  `src/lsp/mod.rs`): `textDocument/codeLens` shows reference counts above
  function/actor declarations; `textDocument/documentLink` creates
  clickable links from `import` statements to resolved module files;
  `textDocument/hover` now includes doc comments (extracted from preceding
  `///` lines), effects, and formatted type signatures.
- **LSP: completion documentation** (Experimental, `src/lsp/mod.rs`):
  keyword and built-in effect completion items now carry markdown
  documentation strings with code examples in their `documentation` field.

---

## Pre-1.0 (crate version 0.13.0-alpha.1 and earlier)

No stability promise. The 0.x series is the alpha development track. Language
version 1.0.0-frozen is the first version with a published stability contract;
everything before it is implicitly Experimental.
