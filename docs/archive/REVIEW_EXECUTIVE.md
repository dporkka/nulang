# Nulang 50-Year Architecture Review — Executive Summary

> **Status:** Architecture review for the Nulang project, synthesized from a multi-layer audit of the current codebase and design documents.  
> **Scope:** Language core, compiler frontend, runtime, distributed systems, AI integration, developer experience, and long-term governance.  
> **Date:** 2026-07-06

---

## 1. Executive Summary

Nulang is an ambitious alpha-stage language that fuses Erlang-style actors, Pony-inspired reference capabilities, Koka-style algebraic effects, a register VM, and a Cranelift JIT. The current codebase (~33 kLOC, 590+ tests) demonstrates that the straight-line pipeline works for small programs: lex → parse → Hindley-Milner typecheck → effect check → capability check → AST-to-bytecode compile → VM/JIT execute (`src/main.rs:172-251`). That is a real achievement.

However, **breadth has been prioritized over depth and architectural coherence**. The system is a stack of individually plausible subsystems glued directly to the AST. There is no intermediate representation, no real optimizer, no parser error recovery, no module system, and no separation between "what the user wrote" and "what the runtime executes". The type system is a partial HM implementation bolted to an ad-hoc effect-row checker and an approximate capability analyzer. Many features advertised in the README (AI agent DSL, deep Python integration, distributed actor migration) are removed, stubbed, or exist only as design documents.

**Core thesis for 50-year relevance:** Languages that survive are not the ones with the most features; they are the ones with the cleanest *semantic core* and the strongest *extension model*. Nulang’s best long-term bet is to become a **semantic platform** around actors + effects + capabilities, with multiple surface syntaxes (handwritten code, natural-language intent, AI-generated plans, JSON API, voice, visual blocks) converging on a single typed intermediate representation.

The current foundation does not support that convergence because the handwritten parser owns the AST and the compiler lowers it directly to bytecode. Before declaring v1.0, Nulang needs a compiler frontend rewrite around a small, explicit IR stack (Intent IR → AST → HIR → MIR), a real effect/capability solver, a workspace split, and the removal or postponement of half of the current feature checklist.

---

## 2. Critical Weaknesses in the Current Design

The following weaknesses are grounded in specific files and line numbers.

### 2.1 No Intermediate Representation — the AST is the only IR

`Compiler::compile_module` in `src/compiler.rs:188-202` lowers `ast::AstModule` directly to `bytecode::CodeModule`. There is no high-level IR (HIR), no mid-level IR (MIR), and no SSA form. This means:

- Optimizations cannot be expressed as passes over a stable IR.
- The LSP (`src/lsp/mod.rs`) is forced to use regex heuristics because there is no resolved, typed tree to query.
- A natural-language or AI frontend cannot target Nulang semantics without re-implementing the parser’s concrete syntax.
- Every new surface construct (e.g. `for`, `pipe`, `handle`) must be compiled by adding a special case in `compile_expr` (`src/compiler.rs:265-312`).

### 2.2 Parser is brittle and has no error recovery

`Parser::parse_module` in `src/parser.rs:75-108` parses declarations sequentially and aborts on the first error. When a declaration fails, it falls back to parsing an expression and wrapping it in `__main`, which means a typo in one declaration can silently change the rest of the file into top-level expression mode. There is no synchronization, no "skip to next top-level keyword" recovery, and no diagnostic accumulation.

`Parser::expect` (`src/parser.rs:1110-1120`) constructs errors by formatting the debug representation of the expected token. `NuError` is a single-error enum (`src/types.rs:457-505`), and the pipeline uses `?` everywhere (`src/main.rs:174-231`). The LSP therefore cannot provide diagnostics because the compiler stops at the first failure.

### 2.3 AST mixes concrete syntax, types, and capabilities

`Expr` in `src/ast.rs:38-208` is a flat enum containing everything from `Pipe`, `CapAnnotate`, and `TypeAnnotate` to `Send`, `Ask`, `Migrate`, `Spawn`, and `Emit`. Surface-level conveniences such as the pipe operator (`|>`) are carried all the way to the compiler instead of being desugared. The AST also stores `Type` and `EffectRow` values directly inside nodes, so type-system data structures are entangled with syntax.

### 2.4 Spans are inconsistent and error locations are often lost

Every `Expr` variant carries a `Span`, but the semantic contexts that track variable usage do not. `TypeContext::consume` for linear variables uses `Span::default()` for its errors (`src/types.rs:394-418`). `TypeChecker::infer_letrec` unifies with `Span::default()` (`src/typechecker.rs:966`), and `infer_handle` unifies handler bodies with `Span::default()` (`src/typechecker.rs:1666`). `effect_checker.rs` has a hand-rolled `expr_span` helper (`src/effect_checker.rs:1147-1182`) precisely because spans are not stored uniformly.

### 2.5 Function types are single-parameter in the type system but multi-parameter at runtime

`Type::Function` stores a single `param: Box<Type>` (`src/types.rs:264-269`). Multiple parameters are represented as a `Type::Tuple` wrapped around the parameter (`src/typechecker.rs:484-488`, `src/typechecker.rs:1019-1034`). This creates an impedance mismatch with the runtime calling convention, which passes arguments in consecutive registers (`src/compiler.rs:510-524`).

### 2.6 The HM implementation is incomplete and uses a no-op substitution context

`apply_subst_to_ctx` in `src/typechecker.rs:99-106` is a placeholder that clones the context without applying substitutions. `TypeChecker::get_ctx_free_vars` (`src/typechecker.rs:1743-1750`) returns only `self.ctx_free_vars`, initialized once and never updated. Because `do_generalize` (`src/typechecker.rs:1728-1741`) relies on this set, the implementation can both over-generalize and under-generalize variables.

### 2.7 Effect rows are not properly unified

`effect_row_compatible` in `src/typechecker.rs:134-151` checks row compatibility by sorting debug string representations of effects and comparing sets. Open rows are handled with ad-hoc subset checks; row variables (`Region`) are never unified through the substitution machinery. `effects.rs::unify_rows` (`src/effects.rs:28-92`) silently adds missing effects rather than emitting real constraints.

### 2.8 Capability analysis is approximate and capabilities are erased at runtime

`CapabilityAnalyzer::infer_cap` in `src/effect_checker.rs:800-1091` computes capabilities by joining the capabilities of free variables captured by a lambda. It does not track aliases, assignments, or field mutations. `LinearIso` consumption is tracked in `TypeContext` by a string-based `consumed: HashSet<String>` (`src/types.rs:352-425`), without span information.

The bytecode ISA defines capability opcodes (`CapChk`, `CapUp`, `CapDown`, `CapSend` at `src/bytecode.rs:132-137`), but `AGENTS.md:33` says these are MVP no-ops. The capability system is compile-time linting with no runtime enforcement, and the lint itself is unsound for non-trivial programs.

### 2.9 There is no optimizer, and escape analysis is dead code

`src/escape_analysis.rs` is a 1,500-line bytecode-level escape analysis with tests, but `AGENTS.md:120` notes it is **dead code** — never imported by `compiler.rs`, `vm.rs`, or `jit/`. The compiler performs no constant folding, no dead-code elimination, no inlining, and no copy propagation. Performance work is concentrated in the JIT tier, which covers only ~30 opcodes and falls back to interpretation for the rest.

### 2.10 Single-crate monolith cannot scale to the intended feature set

`Cargo.toml` declares one crate (`nulang`) with 17 public modules (`src/lib.rs`). The intended feature set — AI SDK, workflow SDK, web framework, package manager, cloud platform, LSP — is documented in five multi-thousand-line design documents but lives in the same compilation unit as the runtime, GC, JIT, and Python bridge. This produces long compile times, tight coupling, and inability to version the language spec independently of the runtime.

### 2.11 Lexer and grammar are ASCII-only and informally specified

`Lexer::read_identifier` accepts only ASCII alphanumerics (`src/lexer.rs:152-153`), and the test suite explicitly asserts that Unicode identifiers are rejected (`src/lexer.rs:938-943`). The language has no published grammar; indentation and semicolon rules are handled by ad-hoc helpers scattered through the parser (`src/parser.rs:1147-1157`).

### 2.12 Module and import system has no semantics

`parse_import` in `src/parser.rs:393-400` stores a string path and an empty `items` vector. `Decl::Import` is a no-op in the type checker (`src/typechecker.rs:561-573`). `Decl::Module` creates a nested namespace, but there is no name-resolution phase. Nulang currently has no real separate compilation, no package boundaries, and no enforced visibility.

### 2.13 Actor behavior typing is dynamic, not static

`TypeChecker::infer_send` (`src/typechecker.rs:1580-1609`) only requires that the receiver has an `Actor { ... }` type; it does not check that the behavior name exists or that argument types match. `Compiler::compile_send` (`src/compiler.rs:972-985`) resolves the behavior by string matching and falls back to an out-of-bounds sentinel. Remote sends use `behavior_id=0` as a placeholder (`AGENTS.md:122`).

### 2.14 Runtime is a single-threaded coordinator

The actor runtime is a `Runtime` god-object (`src/runtime/mod.rs:66`) that runs one `step_actor` at a time on the calling thread. The `Scheduler` uses Chase-Lev work-stealing deques (`src/runtime/scheduler.rs:84`) but the runtime never spawns worker threads; `Scheduler::run_worker` exists (`src/runtime/scheduler.rs:257`) but is unwired. This serializes all actor execution.

### 2.15 Distributed runtime is a proof of concept

The NUL0 wire protocol (`src/runtime/network.rs:17`) has no versioning, no encryption, no authentication, no message deduplication, and no backpressure beyond TCP. The outbound channel is bounded to 1024 messages and silently drops on overflow (`src/runtime/network.rs:700-703`). Remote sends carry `behavior_id = 0` resolved by name on the target (`src/runtime/distributed.rs:665`). Supervision is purely local; restart history is in-memory only (`src/runtime/supervisor.rs:120`).

### 2.16 Natural-language / AI convergence is not architecturally supported

The README still shows an `agent { ... }` DSL example (`README.md:125-141`) even though the DSL was removed in v0.7 (`README.md:43`). There is no Intent IR, no NL frontend, and no structured representation of "intent" separate from handwritten AST nodes. The current architecture cannot deliver on the "AI-native" goal without first solving the IR problem.

### Summary table of critical weaknesses

| Weakness | File evidence | Severity | Blocks v1.0? |
|---|---|---|---|
| AST is the only IR | `src/compiler.rs:188-202` | High | Yes |
| No parser error recovery | `src/parser.rs:75-108` | High | Yes |
| AST mixes syntax/types/caps | `src/ast.rs:38-208` | High | Yes |
| Lost spans in errors | `src/types.rs:394-418`, `src/typechecker.rs:966` | Medium | Yes |
| Single-parameter function types | `src/types.rs:264-269` | Medium | Yes |
| No-op substitution context | `src/typechecker.rs:99-106` | High | Yes |
| Broken effect-row unification | `src/typechecker.rs:134-151`, `src/effects.rs:47-68` | High | Yes |
| Capabilities erased / unsound | `src/bytecode.rs:132-137`, `src/effect_checker.rs:800-1091` | High | Yes |
| No optimizer; dead escape analysis | `src/escape_analysis.rs:1`, `AGENTS.md:120` | Medium | No |
| Single-crate monolith | `Cargo.toml:1-45` | Medium | No |
| ASCII-only lexer, no grammar | `src/lexer.rs:152-153` | Medium | Yes |
| No module/import semantics | `src/parser.rs:393-400` | High | Yes |
| Dynamic actor behavior typing | `src/typechecker.rs:1580-1609` | High | Yes |
| Single-threaded runtime | `src/runtime/mod.rs:337`, `src/runtime/scheduler.rs:257` | High | Yes |
| Distributed runtime not production-ready | `src/runtime/network.rs:700` | High | Yes |
| NL/AI convergence not architected | `README.md:125-141`, `README.md:43` | High | Yes |

---

## 3. Highest-Impact Architectural Improvements

These improvements are ranked by leverage on the 50-year goal, not by ease of implementation.

### 3.1 Introduce Intent IR / HIR / MIR between AST and bytecode

**What:** Define a small, typed, desugared HIR (name-resolved, no surface syntax) and an SSA-like MIR. Lower AST to HIR, run all semantic analysis on HIR, lower HIR to MIR, then optimize and codegen.  
**Difficulty:** Hard.  
**Maintenance cost:** Medium.  
**Why it matters:** This enables every other improvement: optimization, LSP, NL/AI frontends, separate compilation, and alternate backends.

### 3.2 Build a unified type / effect / capability constraint solver

**What:** Replace the separate HM substitution pass, ad-hoc effect-row compatibility check, and capability join analysis with one solver producing substitutions for type variables, row variables, and capability variables.  
**Difficulty:** Hard.  
**Maintenance cost:** Medium.  
**Why it matters:** The current effect and capability systems are unsound. A single solver is the only way to make the three subsystems coherent.

### 3.3 True M:N scheduler with actor affinity

**What:** Replace the single-threaded `Runtime::run_scheduler` with a pool of worker threads each running `Scheduler::run_worker`, while preserving actor-isolation invariants.  
**Difficulty:** Medium.  
**Maintenance cost:** Low.  
**Why it matters:** Today all actor execution is serialized on one thread. The Chase-Lev deque is already present but unused for true parallelism. Unlocking it is the single biggest throughput win.

### 3.4 Production-grade distributed messaging protocol

**What:** Replace the `behavior_id = 0` placeholder, silent drops, and unversioned NUL0 framing with a protocol carrying stable behavior IDs, delivery semantics, ACK/NACK, backpressure, and schema versioning.  
**Difficulty:** High.  
**Maintenance cost:** Medium.  
**Why it matters:** The current protocol loses messages silently when the outbound channel is full (`src/runtime/network.rs:700`), cannot evolve, and cannot express at-least-once vs at-most-once.

### 3.5 Durable workflow runtime

**What:** Build a workflow engine on top of persistent actors: durable timers, activity workers, saga compensation, and deterministic replay.  
**Difficulty:** Very High.  
**Maintenance cost:** High.  
**Why it matters:** Workflows are a major differentiator for a 50-year architecture because they turn "let it crash" into "resume exactly where you left off."

### 3.6 Split the repository into a Cargo workspace

**What:** Move the frontend, IR, type solver, VM/JIT, runtime, and LSP into separate crates with explicit, acyclic dependency edges.  
**Difficulty:** Medium.  
**Maintenance cost:** Low.  
**Why it matters:** Compile times, test isolation, and team scaling all improve. It also forces the architecture to be modular.

### 3.7 Replace the parser with a recoverable, error-accumulating frontend

**What:** Either adopt a parser generator (e.g. `lalrpop`, `chumsky`, `tree-sitter`) or rewrite the recursive-descent parser with diagnostic accumulation and synchronization points. Preserve a formal grammar document.  
**Difficulty:** Medium.  
**Maintenance cost:** Low to Medium.  
**Why it matters:** A language cannot ship with first-error abort. The LSP needs diagnostics, and users need useful error messages.

### 3.8 Add a real MIR-based optimizer

**What:** Replace dead `escape_analysis.rs` with MIR passes: constant folding, dead-code elimination, function inlining, common subexpression elimination, and escape analysis. Feed results to both the bytecode compiler and the JIT.  
**Difficulty:** Hard.  
**Maintenance cost:** Medium.  
**Why it matters:** Performance claims depend on JIT heroics; a solid optimizer makes the language consistently fast and the JIT simpler.

### 3.9 Observability stack

**What:** Add OpenTelemetry-compatible tracing, Prometheus-style metrics, and structured logging throughout the runtime.  
**Difficulty:** Low.  
**Maintenance cost:** Low.  
**Why it matters:** There is almost no observability today. Scheduler stats, resolver stats, and GC stats exist but are not exported.

### 3.10 Semantic IDE: incremental compiler database + real LSP

**What:** Replace regex-based inlay hints with a persistent, incremental compiler database (`CompileDb`) powering diagnostics, hover, goto-definition, find-references, refactoring, and AI-assisted code actions.  
**Difficulty:** High.  
**Maintenance cost:** Medium.  
**Why it matters:** The current LSP (`src/lsp/mod.rs`) is explicitly MVP and regex-based. A credible developer experience requires a semantic IDE.

---

## 4. Features to Remove or Postpone Before v1.0

These features either do not work, are dead code, or add complexity the core cannot support yet.

1. **Python bytecode opcodes (`0x94-0x9B`).** Reserved but unused after the v0.12 audit moved Python to the native-actor boundary. Remove from the ISA; route Python through `perform Python.call(...)` only.
2. **`src/escape_analysis.rs` as it stands.** Dead code with no consumer. Delete or rewrite as a real MIR pass.
3. **Capability VM opcodes (`CapChk`, `CapUp`, `CapDown`, `CapSend`).** No-ops today. Remove; reintroduce only when capabilities have sound, enforced semantics.
4. **Agent DSL / `agent` declarations.** Removed in v0.7 but still referenced in docs. Ensure it is gone from docs, tests, and AST.
5. **Actor migration (`Migrate` expression and opcode).** Semantics undefined; compiler falls back to default behavior indices. Postpone until the distributed runtime is hardened.
6. **CRDT actor state model (`StateModel::Crdt`).** Wiring into actor state fields is incomplete and untested. Postpone until persistence and CRDT semantics are formalized.
7. **SIMD auto-vectorization in the JIT.** Impressive but premature for an alpha with only ~30 JIT-supported opcodes. Keep behind a feature flag.
8. **Regex-based LSP inlay hints.** Postpone shipping LSP features until HIR-based implementation exists.
9. **Dual-region generational heap + escape analysis pair.** Already reverted. Do not reintroduce without formal proof of safety.
10. **Manual CLI argument parser.** Replace `src/main.rs:44-83` with `clap` or similar.
11. **WASM component compilation.** Listed in `ROADMAP.md` and `ARCHITECTURE.md` as v1.0 but too risky to ship around a complex GC/JIT. Defer to v1.2+.
12. **Managed cloud / Nulang Cloud.** Defer to v1.5+; focus on self-hosted deployments for v1.0.
13. **Hot code reloading.** Interacts dangerously with durable state schemas. Defer.
14. **Web framework (`phoenix-nl`).** Provide only stdlib HTTP primitives in v1.0; ship the framework as a community package after v1.0.

---

## 5. Features Likely to Remain Valuable Over 50 Years

These choices align with long-lived language design principles and should be preserved and deepened.

1. **Actor model with location-transparent addressing.** Erlang/OTP has survived for decades because actors + message passing + failure isolation are robust abstractions.
2. **Per-actor heap + ORCA-style reference counting.** Shared-nothing heaps are the only realistic foundation for predictable latency in a distributed actor system.
3. **Register-based bytecode VM with NaN-boxed values.** Compact instructions, dense values, and a simple dispatch loop are durable implementation choices.
4. **Algebraic effects as a core abstraction.** Effects make side effects, AI tool calls, IO, and distribution explicit and testable.
5. **Capability-based reference permissions.** Even if the current implementation is weak, the *idea* of `iso/val/ref/box/tag` is a proven way to control sharing across actors.
6. **First-class functions and closures.** Fundamental for higher-order programming and effect handlers.
7. **Pattern matching and sum/record types.** The right data-modeling primitives for a typed functional language.
8. **Hindley-Milner type inference as the default.** Full inference is a strong usability bet.
9. **Effect-driven AI/tool integration.** Using effects (`perform LLM.generate(...)`, `perform Python.call(...)`) instead of a special DSL keeps AI as a first-class capability without polluting syntax.
10. **Distribution primitives (`spawn`, remote `send`, CRDTs, gossip).** The ambition to run actors across nodes with the same semantics as local actors is durable.

---

## 6. Closing Position

Nulang is a collection of good bets in search of an architectural center. The path to 50-year relevance is not to add more features, but to **refactor around a semantic core**: a clean IR stack, a unified type/effect/capability solver, a workspace split, a true M:N scheduler, a production distributed protocol, and the removal of dead or premature features. If that refactor is executed, the durable ideas in section 5 will have a stable foundation. If it is not, the codebase will become an increasingly fragile bag of demo features.
