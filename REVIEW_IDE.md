# Nulang 50-Year Architecture Review — Semantic IDE & Developer Experience

> **Status:** Architecture review for the ideal Nulang IDE, developer tooling, package manager, web-framework integration, and observability.  
> **Date:** 2026-07-06

---

## 1. Updated UX Flow for the Semantic IDE

### 1.1 What the current tooling gives us

Nulang already has a clean compiler pipeline, a REPL, and a minimal LSP server.

- The CLI entry in `src/main.rs:172` runs the full `run_source` pipeline: lex → parse → typecheck → effect check → capability analysis → compile → VM run.
- The REPL in `src/repl.rs:146` keeps a persistent `TypeContext`, accumulated declarations, and supports `:type`, `:ast`, and `:bytecode` introspection.
- The LSP in `src/lsp/mod.rs:29` only advertises inlay hints and regex-based completion (`src/lsp/mod.rs:117` and `src/lsp/mod.rs:130`). It is explicitly MVP: no hover, diagnostics, goto-definition, find-references, or refactoring.

The semantic IDE is the next step: turn the batch compiler into a long-lived, incremental, queryable compiler database that powers a live, explainable, AI-assisted editor.

### 1.2 IDE server architecture

The IDE should be a separate, persistent process (or thread pool) that owns the compiler database (`CompileDb`). The editor communicates over LSP, but the IDE also exposes a richer Nulang-native protocol for visual graphs, intent previews, and deployment simulation.

Key requirement: every compiler pass must be position-aware and recover from errors. The current `NuError` enum (`src/types.rs:523`) carries a `Span`; the IDE needs a *list* of such errors plus partial AST/typed-AST results so the UI never goes blank on a syntax mistake.

### 1.3 IDE surfaces and user flows

#### Architecture graph

A visual map of the module graph, actor types, effect handlers, and capability boundaries. Computed from `Decl::Function`, `Decl::Actor`, `Decl::EffectDecl` (`src/ast.rs:271`), and import edges.

*Interaction:* click an actor node to see its state model (`src/ast.rs:252`: Local / Durable / EventSourced / CRDT), behavior list, and supervision parent. Double-click a function to jump to source.

#### Actor graph

A live or static view of actor instances, links, monitors, and supervisors. The runtime already maintains `actors`, `supervisors`, links, and monitors (`src/runtime/mod.rs:66-95`); the IDE reads sanitized snapshots.

*Interaction:* hovering a link shows whether it is a link, monitor, or parent-child edge. Fault-injection buttons ("kill this actor") let a developer test supervision behavior without writing a test.

#### Workflow graph

Workflows are a special kind of persistent actor (`DESIGN_WORKFLOW_SDK.md:26`). The IDE renders them as deterministic state machines: steps, branches, timers, signals, sagas, and compensation paths. Each node maps to a `behavior_id` in the compiled `BehaviorTableEntry` (`src/bytecode.rs:339`).

*Interaction:* clicking a step shows its inferred effect row, retry policy, and the source AST span. A slider replays the event journal (`src/runtime/mod.rs:542`) for debugging.

#### Semantic diff

Diffs are computed on the typed AST, not on raw text. The IDE shows:

- Type signature changes (e.g. a function returning `Int` changed to `Float`).
- Effect row changes (a pure function now performs `IO`).
- Capability changes (a `ref` became `iso`).
- Actor state-model changes (a field moved from `Local` to `Durable`).

This requires every `Expr` and `Decl` variant to keep its span (`src/ast.rs:39-208`), and the compiler database to retain the previous typed AST.

#### Live diagnostics

Diagnostics should be produced by all static passes, not just the first failing one. Today `EffectChecker` and `CapabilityAnalyzer` accumulate `diagnostics: Vec<String>` (`src/effect_checker.rs:349` and `src/effect_checker.rs:785`), but the pipeline in `src/main.rs:172` aborts on the first `NuError`. The IDE must run each pass to completion and merge diagnostics by span.

Categories:

- Lex/parse errors.
- Type errors (unification failures, occurs check).
- Effect errors (disallowed effects escape a handler).
- Capability errors (`iso` consumed twice, non-sendable value sent across actors).
- Architecture warnings (effect-heavy function called from a pure context, actor without supervision).

#### Intent preview

When the user writes a natural-language intent (e.g. "add pagination to this controller"), the IDE shows a preview of the AST transformation before applying it. The preview includes:

- Inserted/updated functions.
- New effect requirements.
- Changed capability signatures.
- Estimated performance impact (e.g. a new database query).
- Whether the change violates existing supervision or workflow determinism rules.

The user accepts, rejects, or edits the preview; the editor then applies the structured edit, preserving formatting trivia.

#### Bidirectional editing

The editor maintains three layers in sync:

1. **Text layer** — the source file the user sees.
2. **Intent layer** — a higher-level semantic description of what each block does.
3. **AST layer** — the concrete typed AST.

If the user edits text, the AST and intent update. If the user edits intent, the AST and text update. If the user edits the AST structurally (e.g. via the architecture graph), text and intent update. Round-trip stability requires the parser to preserve whitespace/comments as trivia attached to spans.

#### Code generation

Code generation is not a black-box paste; it is a structured edit. The IDE exposes templates for:

- Actor scaffolding with chosen state model.
- Effect handler boilerplate.
- Web controller / channel / LiveView.
- Workflow step with retry and compensation.
- `@tool` wrapper from a function signature.

Each template is type-checked before insertion so the generated code is guaranteed to compile.

#### Explainability

Hovering any symbol shows the chain of reasoning that produced its type, effect row, and capability:

- For a variable: where it was bound and how it was generalized.
- For an effectful call: which handler will catch it, or why it escapes.
- For an actor message: the allowed message shapes and the state model of each field.

#### Architecture score

A single, inspectable score that rates the codebase on dimensions that matter for a 50-year-relevant distributed system. The IDE surfaces it per module, per actor, and globally, with drill-downs.

Dimensions:

| Dimension | What it measures | Source |
|-----------|------------------|--------|
| **Effect purity** | Fraction of functions with empty or fully-handled effect rows | `EffectChecker::infer_effects` |
| **Actor isolation** | Fraction of state fields using `Local`/`Durable`/`EventSourced`/`Crdt` correctly, no raw shared refs | `Decl::Actor`, `Capability::is_sendable` |
| **Supervision coverage** | Fraction of actors registered under a supervisor with a restart policy | `src/runtime/supervisor.rs` |
| **Capability safety** | Fraction of cross-actor sends where arguments are `iso`, `val`, or `tag` | `CapabilityAnalyzer` |
| **Module coupling** | Cycle-free module graph, fan-in/fan-out | Import declarations |
| **Distribution readiness** | Use of CRDTs vs mutable shared state across nodes | `Crdt` state model |
| **Determinism** | Workflows contain only activities/timers/signals, no raw `IO` | `DESIGN_WORKFLOW_SDK.md` |

Proposed formula:

```text
ArchitectureScore =
  0.25 * EffectPurity +
  0.20 * ActorIsolation +
  0.15 * SupervisionCoverage +
  0.15 * CapabilitySafety +
  0.10 * ModuleHealth +
  0.10 * DistributionReadiness +
  0.05 * WorkflowDeterminism
```

#### Performance estimates

The IDE uses the compiler database plus runtime telemetry to estimate:

- JIT hot-path likelihood (which loops will hit `HOT_THRESHOLD=1000` in `src/jit/mod.rs`).
- Actor mailbox pressure from a given send pattern.
- GC pressure from ORCA foreign-ref traffic (`src/runtime/mod.rs:265`).
- Network hops for distributed sends.

These estimates are shown inline or in the architecture graph.

#### Deployment preview

From `cloud.nl` / `DESIGN_CLOUD.md` the IDE renders the regional deployment: actors, scaling policies, bindings, routes, and migrations. It can simulate traffic and show expected instance counts, latency, and cost before `nu cloud deploy`.

---

## 2. Highest-Impact IDE & Tooling Improvements

| # | Improvement | Goal | Difficulty | Maintenance Cost | Key Files |
|---|-------------|------|------------|------------------|-----------|
| 1 | Real semantic LSP | Hover, diagnostics, goto, refs, refactoring | 4 | 3 | `src/lsp/mod.rs`, `src/typechecker.rs`, `src/effect_checker.rs`, `src/compiler.rs` |
| 2 | Incremental compiler database (`CompileDb`) | Sub-100 ms IDE feedback for large codebases | 5 | 4 | `src/parser.rs`, `src/typechecker.rs`, `src/ast.rs` |
| 3 | Source maps + bytecode-to-AST mapping | Debugging, profiling, explainability, deployment preview | 3 | 2 | `src/compiler.rs`, `src/bytecode.rs:401`, `src/vm.rs` |
| 4 | Package manager implementation (`nu`) | Reproducible builds, versioning, migrations | 4 | 4 | new `src/nu/` or separate crate, `DESIGN_PACKAGE_MANAGER.md` |
| 5 | Web framework runtime bindings (`phoenix-nl`) | Channels, LiveView, HTTP routing on the actor runtime | 4 | 3 | new `src/web/`, `DESIGN_WEB_FRAMEWORK.md` |

**Rationale.** The semantic LSP is the user-visible gateway: without it the IDE cannot exist. The incremental compiler database is the enabling substrate; it is hard but pays off across every other feature. Source maps are cheap and immediately unlock debugging and explainability. The package manager and web framework are ecosystem prerequisites for adoption.

---

## 3. Compiler Interfaces Needed for a Full LSP

### 3.1 Hover

Hover needs, for any source position:

- The inferred `Type` from `TypeChecker::infer_expr` (`src/typechecker.rs:579`).
- The `EffectRow` from `EffectChecker::infer_effects` (`src/effect_checker.rs:364`).
- The `Capability` from `CapabilityAnalyzer::infer_cap` (`src/effect_checker.rs:797`).
- For actors: state model and behavior signatures (`src/bytecode.rs:348`).

Required change: preserve a typed AST where every `Expr` node carries its resolved type, effect row, and capability. Currently `TypeChecker::check_module` returns only the last declaration's type (`src/typechecker.rs:421`).

### 3.2 Diagnostics

The IDE needs *all* diagnostics, not the first error. The current pipeline uses `?` and aborts early (`src/main.rs:172`). A new `Diagnostic` struct (span, severity, message, related information) should be emitted by:

- The lexer with recovery tokens.
- The parser producing `Expr::Error` nodes.
- The typechecker returning unification failures without aborting.
- `EffectChecker::diagnostics` and `CapabilityAnalyzer::diagnostics` merged.

### 3.3 Goto-definition and find-references

These require a name-resolution index mapping each identifier use to its binding site. The compiler already builds `TypeContext` bindings and `Compiler::func_map` (`src/compiler.rs:163`). The IDE needs a cross-module `SymbolIndex` keyed by `Span` that tracks:

- Function declarations (`Decl::Function`).
- Actor declarations (`Decl::Actor`).
- Parameters, let bindings, pattern bindings.
- Effect operations (`Decl::EffectDecl`).
- Extern functions (`Decl::Extern`).

### 3.4 Refactoring

Refactorings are structured AST edits verified by re-typechecking:

- Rename symbol: update all uses in the `SymbolIndex`.
- Extract function: compute free variables, infer the new function's effect signature, and wrap the body.
- Change capability: e.g. promote `ref` to `iso`; the IDE verifies sendability with `Capability::is_sendable`.
- Convert to actor: wrap state in `Decl::Actor`, generate behavior stubs, update sends.

### 3.5 Intent preview

Intent preview is refactoring driven by an LLM or template. The compiler must accept an AST patch, re-run type/effect/cap checks, and return a structured diff plus any new diagnostics.

---

## 4. Natural-Language Programming and Bidirectional Editing

### 4.1 Text ↔ intent ↔ AST loop

Natural-language edits must never silently break the program. The IDE enforces:

1. Every generated AST change is type-checked.
2. Effect rows are preserved or explicitly widened.
3. Capability sendability is checked before inserting cross-actor code.
4. Workflow determinism rules are respected (no non-activity side effects inside a workflow body).
5. The user sees the diff in intent-IR form, not just raw text.

### 4.2 Concrete example

User selects a controller action and types: *"Cache this for 5 minutes using Redis."*

The IDE:

1. Parses the intent into an `IntentIR` node: `AddCache(policy: TTL(5m), backend: Redis)`.
2. Generates an AST patch: wrap the database query in a `Cache.get_or_set` call.
3. Type-checks the patch; the query's `EffectRow` gains `Cache` but not `IO` if the cache backend is abstracted.
4. Updates the architecture graph to show the new `Cache` dependency.
5. Pretty-prints the change back into the editor, preserving surrounding comments.

---

## 5. Package Manager: Missing Pieces

`DESIGN_PACKAGE_MANAGER.md` is thorough on the surface, but several concrete pieces are missing for a 50-year-relevant package ecosystem.

### 5.1 Reproducible builds

- **Lockfile content-addressed checksums**: the resolver must verify them on every build and reject any mismatch, including for path and git dependencies.
- **Vendoring / offline mode**: `nu --offline build` must work from a committed `vendor/` directory.
- **Deterministic resolver test corpus**: pathological dependency graphs (diamonds, conflicts, yanked versions) that the SAT-style resolver must solve identically on every platform.
- **Reproducible build environment**: record compiler version, OS, linker, and native-library versions in build artifact metadata.

### 5.2 Versioning

- **SemVer policy enforcement**: the registry must reject breaking changes in patch/minor versions by running API-diff against the previous published version.
- **Yanked packages**: a yank mechanism that leaves the package in the lockfile but warns/errors on new resolves.
- **Edition migration**: when `edition` changes, the package manager should offer an automated `nu migrate` that rewrites source constructs.

### 5.3 Migrations

Nulang's actors can be persistent. Migrations must handle:

- **State schema evolution**: mapping old `Durable`/`EventSourced` fields to new types with user-supplied conversion functions.
- **Journal replay compatibility**: a new actor version must still be able to replay old journal entries.
- **CRDT merge policy updates**: if a CRDT type changes, the merge function must be versioned.
- **Database migrations** for managed bindings (`DESIGN_CLOUD.md:496`).

### 5.4 Workspace and registry

- Workspace inheritance: implement `version.workspace = true` and `[workspace.dependencies]`.
- Private registry authentication, API tokens, and audit logging.
- Dependency audit: integrate with `cargo-audit`-style vulnerability scanning.

---

## 6. Web Framework Integration

`DESIGN_WEB_FRAMEWORK.md` describes `endpoint`, `controller`, `channel`, `liveview`, and templates. The runtime pieces needed to make this real are mostly present in `src/runtime/`; the framework layer must bind HTTP/WebSocket plumbing to actors.

### 6.1 Request lifecycle

```text
HTTP request -> Endpoint actor -> Router behavior -> Controller actor -> View / Template -> HTTP response
```

Each request is handled by a short-lived controller actor spawned by the endpoint. The controller performs effects such as `Database.query` and returns a response. Because Nulang effects are explicit, middleware plugs are just functions with an `EffectRow` that the router composes.

### 6.2 Channels

A `channel` declaration maps to an actor per WebSocket connection:

- `join/3` becomes a behavior that pattern-matches the topic.
- `handle_in/3` becomes a behavior that receives client messages.
- `handle_info/2` receives system messages (presence diffs, broadcasts).
- The socket's `assigns` are actor state fields.

Topic pub/sub can reuse `ProcessGroups` or a dedicated pub/sub actor backed by CRDTs for cross-node consistency.

### 6.3 LiveView

A `liveview` is a long-lived actor that holds UI state and renders a template on every state change:

```text
Browser -> HTTP GET -> Endpoint -> HTML + JS -> Browser
Browser -> WebSocket upgrade -> LiveView actor -> render diff -> Browser
Browser -> phx-click -> LiveView -> broadcast -> PubSub -> handle_info -> LiveView
```

The `schedule_interval` call in a LiveView maps directly to the `TimerWheel`. State changes trigger a render; the framework computes a minimal diff and pushes it over the WebSocket.

### 6.4 Integration requirements

- **Template compiler**: compile `@template` blocks to bytecode that produces a render tree or HTML string. Must preserve span information for IDE error reporting.
- **Route compiler**: compile route patterns to a decision tree; the IDE can visualize it.
- **Connection supervision**: channel/LiveView actors should be supervised so a crash does not tear down the endpoint.
- **Backpressure**: use the mailbox depth of the connection actor to slow down the client.

---

## 7. Developer Observability

### 7.1 Tracing

Nulang's actor model is ideal for distributed tracing. Every `send_message_by_id` (`src/runtime/mod.rs:215`) and `perform Effect.op` should optionally emit an OpenTelemetry span:

- Span name: actor type + behavior name.
- Attributes: actor id, node id, behavior id, effect name, capability.
- Parent context: propagated through message metadata.

The IDE can render a request trace across actors, nodes, and LLM calls.

### 7.2 Profiling

- **Reduction profiling**: `Runtime::step_actor` already increments `reduction_count`; aggregate reductions per actor/behavior to find hot actors.
- **JIT hotness**: the JIT hot counters can be exposed per program counter.
- **GC profiling**: `Runtime::gc_stats` returns ORCA object/ref counts.
- **Scheduler profiling**: `Runtime::scheduler_stats` reports queue depths and work stealing.

These should feed a unified profile view in the IDE: flame graphs per actor, JIT-tier annotations on source lines, and GC pressure heatmaps.

### 7.3 Debugger

A source-level debugger needs:

- **Source maps**: map bytecode PCs back to `Span`s in `src/ast.rs`. The `Instruction` struct currently has no debug field; add a `debug_pc` or keep a side table.
- **Breakpoints**: set at a `Span`; the compiler maps it to the first instruction whose source range contains it.
- **Stepping**: step over `Call`/`ClosureCall`, step into handlers, step across actor boundaries (message send).
- **Inspect state**: view actor state fields, mailbox queue, links, monitors, and effect handler stack.
- **Time-travel for workflows**: because workflows are event-sourced, the debugger can replay from any journal sequence.

---

## 8. Summary and Recommended Roadmap for IDE & Tooling

To make Nulang a 50-year-relevant platform, the tooling must be as intentional as the language itself. The immediate priorities are:

1. **Replace the MVP LSP with a real semantic server** built on a typed-AST query API.
2. **Add source maps** so bytecode, VM state, and runtime telemetry can be shown at the source level.
3. **Implement an incremental compiler database** (`CompileDb`) that powers diagnostics, hover, goto, refactoring, and intent preview.
4. **Build `nu` package manager** with reproducible lockfiles, state-schema migrations, and workspace support.
5. **Land the web framework runtime** (`phoenix-nl`) by compiling channels and LiveViews to supervised actors and binding HTTP to the actor runtime.
6. **Add OpenTelemetry tracing and a source-level debugger** to close the observability loop.

These changes do not alter Nulang's core design; they expose the rich information already present in the compiler and runtime through stable, queryable interfaces that the IDE, package manager, web framework, and cloud platform can share.
