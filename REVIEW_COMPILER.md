# Nulang 50-Year Architecture Review — Compiler, NL Frontend & AI Architecture

> **Status:** Architecture review for the Nulang compiler frontend, natural-language compilation pipeline, and AI integration.  
> **Date:** 2026-07-06

---

## 1. Updated Compiler Architecture

### 1.1 Current baseline

Nulang today compiles handwritten source through a straight-line pipeline:

```text
source &str
  -> Lexer::lex()              -> Vec<Token>            src/lexer.rs
  -> Parser::parse_module()    -> AstModule             src/parser.rs
  -> TypeChecker::check_module()-> Type                  src/typechecker.rs
  -> EffectChecker::infer_effects()-> EffectRow          src/effect_checker.rs
  -> CapabilityAnalyzer::infer_cap()-> Capability        src/effect_checker.rs
  -> Compiler::compile_module()-> CodeModule             src/compiler.rs
  -> VM::load_module() + run() -> Value                 src/vm.rs
```

This pipeline is deterministic, well tested (~590 tests), and reused by the REPL (`src/repl.rs:146-246`) and the LSP server (`src/lsp/mod.rs`). The pipeline is the single source of truth for executable semantics.

### 1.2 Target architecture

For 50-year relevance the compiler should accept programs from many surfaces and converge them on the same semantic pipeline:

| Frontend | Conversion step |
|----------|-----------------|
| Handwritten `.nula` | Lexer + Parser (`src/main.rs:174-185`) |
| Natural language | NL frontend → Intent IR → AST builder |
| Visual programming | Blocks → Intent IR → AST builder |
| JSON API | JSON → Intent IR → AST builder |
| Voice | Speech-to-text → NL frontend → Intent IR |
| IDE interactions | Code action → AST transform |

The key decision is that **Intent IR is the universal pre-AST representation**. It decouples input modality from language semantics.

### 1.3 AST → HIR → MIR → optimizer

Today the compiler lowers AST directly to bytecode in `src/compiler.rs:188-202`. The target architecture introduces explicit intermediate layers:

- **AST** (`src/ast.rs`) — concrete syntax tree with spans; preserves source layout and syntactic sugar.
- **HIR (High-level IR)** — resolved, typed AST. All names resolved, type schemes instantiated, effect rows explicit, capability annotations attached. This is where row-polymorphic effects and capability subtyping are materialized.
- **MIR (Mid-level IR)** — lower-level representation with explicit references, closures converted to environment-passing, actor behaviors flattened, linear `iso` consumption verified, pattern matches turned into decision trees. MIR is the natural place for escape analysis and lifetime reasoning if a future generational GC is introduced.
- **Optimizer** — passes including constant folding, inlining, dead-code elimination, guard stripping (`src/jit/typed_compiler.rs`), SIMD vectorization (`src/jit/simd_analyzer.rs`), and capability erasure.
- **Backends** — bytecode VM for portable interpretation, Cranelift JIT for fast native code, and LLVM AOT for release builds.

### 1.4 Validation gates

The following gates keep compilation deterministic regardless of whether the source is handwritten or AI-generated:

1. **Parser gate** — rejects malformed concrete syntax; emits spans for every error.
2. **Type gate** — Hindley-Milner unification with occurs check (`src/typechecker.rs`).
3. **Effect gate** — row-polymorphic effect compatibility (`src/effect_checker.rs`).
4. **Capability gate** — Pony-inspired capability lattice (`src/capabilities.rs`).
5. **Linearity gate** — `LinearIso` consumption tracking in `TypeContext` (`src/types.rs`).
6. **Compiler gate** — AST-to-bytecode lowering must be total and deterministic (`src/compiler.rs`).
7. **Test gate** — unit, integration, and stress tests must pass.
8. **Sandbox gate** — FFI calls, AI effects, and Python calls are capability-gated and audit-logged.

Because the same gates apply to all inputs, AI-generated code cannot introduce a weaker safety model.

### 1.5 Backend strategy: Cranelift + LLVM

Current state:

- Bytecode VM with 91 opcodes (`src/bytecode.rs:9-165`).
- Cranelift JIT that compiles a subset of those opcodes (`src/jit/compiler.rs:37-54`) with hot-counter tiering (`src/jit/mod.rs:55`, `292-324`).
- Typed compiler strips NaN-tag guards when types are known (`src/jit/typed_compiler.rs:68-128`).
- SIMD analyzer detects vectorizable array loops (`src/jit/simd_analyzer.rs:419`).

Recommendation: **keep Cranelift for the JIT tier, add LLVM as an optional AOT backend.**

- Cranelift’s fast compile times and simple API are ideal for dev/REPL and JIT-tiering hot loops.
- LLVM provides mature scalar/SIMD optimizations, multiple targets, and stable object-file output for release binaries, embedded deployment, and cross-compilation.
- Both backends consume the same MIR, so language semantics stay identical.

Dropping Cranelift would sacrifice the REPL experience and the proven JIT tiering path. Keeping only Cranelift would prevent release-grade AOT builds and exotic targets. A dual-backend strategy is the right long-term answer.

### 1.6 Migration from the current pipeline

Current pipeline:

```text
source → Lexer → Parser → AST → TypeChecker → EffectChecker → CapabilityAnalyzer → Compiler → Bytecode → VM/JIT
```

Target pipeline:

```text
source → Lexer → Parser → AST → HIR → Solver → Typed HIR → MIR → Optimize → {Bytecode, Cranelift} → Runtime
                  ↑
         Intent IR (from NL/AI)
```

To migrate incrementally:

1. Define HIR as a desugared subset of AST. Initially lower AST directly to HIR in one pass.
2. Move `TypeChecker`, `EffectChecker`, and `CapabilityAnalyzer` to operate on HIR. Keep the AST-to-HIR pass minimal at first.
3. Once HIR is stable, introduce MIR and move the bytecode compiler to consume MIR. The JIT can continue consuming bytecode temporarily.
4. Add optimizer passes on MIR.
5. Finally, add Intent IR and NL/AI frontends that lower to HIR.

---

## 2. Updated Natural-Language Compilation Pipeline

### 2.1 Pipeline stages

The NL frontend produces an **Intent IR** that is validated, clarified, planned, and then lowered to the existing AST. The stages are:

1. **Intent IR** — a typed, schema-defined representation of what the user wants: goals, constraints, examples, tests, invariants, performance requirements, and safety policies. It is not code; it is a specification graph.
2. **Intent validator** — checks the IR for schema conformance, security policy violations, unbounded loops, forbidden FFI calls, and capability/effect conflicts before any planning occurs.
3. **Clarification engine** — measures ambiguity (missing identifiers, contradictory constraints, underspecified types, unsafe capabilities) and emits targeted questions to the user or IDE.
4. **Architecture planner** — maps the validated intent onto coarse Nulang components: modules, actors, behaviors, effects, functions, and CRDTs. Output is an **architecture graph**.
5. **Semantic planner** — fills in algorithms, data structures, type signatures, effect rows, and capability annotations consistent with Nulang’s type system (`src/types.rs`).
6. **AST builder** — emits real `AstModule` / `Decl` / `Expr` nodes (`src/ast.rs`) with spans, then runs the existing parser/type/effect/cap pipeline.

### 2.2 Confidence, provenance, and approval

Every generated AST node carries **provenance metadata**:

- originating prompt/utterance ID,
- model provider and model version,
- generation parameters (temperature, top-p, seed),
- list of tool calls and retrieved examples,
- confidence score from the validator,
- human approval state (`auto-applied`, `pending`, `rejected`).

The approval flow is gate-based:

- **Low confidence** (< 0.7) or **high risk** (FFI, `iso` sends, distributed spawn, capability downgrades) → mandatory user review in the IDE.
- **Medium confidence** (0.7–0.95) → diff view shown; one-click accept/reject.
- **High confidence** (> 0.95) and all deterministic checks pass → auto-applied, but fully auditable.

Determinism is preserved because the AI never emits bytecode or runtime values directly; it only produces an AST, which is then compiled by the same deterministic compiler used for handwritten code.

### 2.3 Concrete integration points

- The AST builder must emit nodes that already exist in `src/ast.rs`, e.g. `Decl::Function`, `Expr::Let`, `Expr::Perform`, `Decl::Actor`. This guarantees that every AI-generated program is processed by `TypeChecker::check_module()` and `Compiler::compile_module()` exactly like handwritten code.
- The Intent IR schema should be versioned independently of the language grammar, so the same intent document can be retargeted across Nulang language versions.
- The clarification engine should reuse span information from the planner so user-facing questions point to specific source ranges in the IDE.

---

## 3. Updated AI Architecture

### 3.1 Where AI participates — and where it does not

AI is allowed only in two places:

1. **The NL frontend**, turning intent into AST.
2. **The runtime effect layer**, where `perform LLM.complete(...)` or `perform Python.call(...)` invoke models as ordinary Nulang effects.

AI is **never** allowed to:

- replace the lexer, parser, typechecker, effect checker, capability analyzer, or bytecode compiler;
- short-circuit validation gates;
- mutate runtime state (actor heap, mailbox, GC) without going through the normal VM opcodes;
- emit free-form code that is executed without first being checked by the compiler.

This mirrors the recent architectural correction that moved Python interop out of the VM value representation and into isolated native actors.

### 3.2 Interchangeable providers

A provider abstraction should expose a single trait:

```rust
trait LlmProvider {
    fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError>;
    fn stream(&self, request: LlmRequest) -> impl Stream<Item = TokenChunk>;
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError>;
    fn supports(&self, capability: ModelCapability) -> bool;
}
```

Implementations include:

- **Cloud:** OpenAI, Anthropic, Azure OpenAI, Google, custom OpenAI-compatible endpoints.
- **Local:** llama.cpp (GGUF), Ollama, vLLM, MLX.
- **Hybrid / edge:** run small models locally for latency-sensitive completion, large cloud models for architecture planning.

A `ModelRegistry` selects providers by required capability (tool use, vision, JSON mode, context window), cost per token, latency SLA, privacy classification, and user preference / fallback chain.

### 3.3 Local + cloud models

The compiler/IDE should treat local and cloud models as **functionally equivalent inputs to the same deterministic pipeline**:

- Local models are ideal for low-latency in-line completions, clarifications, and small refactorings. They also keep source code private.
- Cloud models are used for larger architecture planning where reasoning quality matters more than latency.
- A deterministic cache makes repeated identical prompts free regardless of provider.

Because the output of every model call is validated by the compiler, model quality differences surface as compile errors or test failures, not as runtime corruption.

### 3.4 Mandatory structured outputs

To preserve deterministic compilation, the AI frontend must use **structured output** for every code-related operation:

- Intent IR is emitted as JSON matching a fixed JSON Schema.
- The architecture planner emits a JSON architecture graph.
- The AST builder emits a serialized AST (S-expression or JSON) matching the parser’s expectations, not raw source text.

Free-form text is permitted only for user-facing explanations, clarification questions, documentation comments, and code-review comments.

When possible, constrained decoding / grammar-based sampling should be used so the model physically cannot emit malformed Intent IR or AST nodes.

### 3.5 Tool calling vs. free-form

Tool calling is the primary mode for code generation. Example tools exposed to the model:

- `generate_function(name, signature, tests)` → returns an `Expr::Lambda` AST fragment.
- `generate_actor(name, state, behaviors)` → returns a `Decl::Actor`.
- `generate_effect(name, operations)` → returns a `Decl::EffectDecl`.
- `refactor_inline_function(span)` → returns a transformed AST.
- `search_symbol(name)` → returns type/effect/capability info from the IDE index.
- `run_tests(filter)` → returns test results used in an iterative improvement loop.

Free-form generation is restricted to the **planning phase** where the model proposes a high-level design; the design is then converted to structured Intent IR before it ever reaches the compiler.

### 3.6 AI never bypasses compiler validation

All generated AST fragments are fed into:

1. Parser (if serialized as source) or direct AST validation,
2. `TypeChecker::check_module()` (`src/main.rs:194-198`),
3. `EffectChecker::infer_effects()` (`src/main.rs:206-214`),
4. `CapabilityAnalyzer::infer_cap()` (`src/main.rs:218-226`),
5. Compiler and VM execution,
6. Unit / integration / stress tests.

A generation that fails any gate is rejected with a structured error, and the model may retry with the error message as context. Failed generations are logged in the audit trail.

---

## 4. Long-Term Backend & Value Representation Questions

### 4.1 Is the NaN-boxed `Value` representation future-proof?

**Verdict: adequate for the next few years, not for 50.**

Current layout (`src/vm.rs:194-214`):

```text
high 16 bits = type tag
low 48 bits  = payload
```

Strengths:

- Unboxed floats and integers keep numeric code fast.
- 48 bits is enough for current x86_48 user-space pointers and string-pool IDs.

Weaknesses:

- **Tag space is finite.** The quiet-NaN space can only hold a handful more tags before colliding with real float patterns.
- **Pointer width ceiling.** 48-bit payloads will break on future 57-bit or 64-bit address spaces.
- **Duplicated constants.** The same tags are defined in `src/vm.rs`, `src/jit/typed_compiler.rs`, and `src/python/marshal.rs`.
- **No versioning.** There is no header bit to distinguish future layout revisions, making wire-format compatibility risky.

Recommendation:

- Create a single `value_layout` module that owns all tag constants and bit-manipulation helpers.
- Reserve two tag bits for future layout versioning.
- For new complex types, prefer heap-allocated objects pointed to by `TAG_PTR` rather than consuming more NaN tags.
- Long-term, evaluate a hybrid representation: NaN-boxing for numbers, tagged pointers for heap objects, with a compile-time flag to switch to a fully boxed model for debugging or future architectures.

### 4.2 What is missing for reproducible builds?

Several global mutable artifacts undermine reproducibility today:

- `HOT_COUNTERS` is a static `OnceLock<Mutex<HashMap<usize, u64>>>` (`src/jit/mod.rs:57`). JIT compilation timing depends on runtime execution history.
- `PYTHON_REGISTRY` is a static `OnceLock<Mutex<PythonRegistry>>` (`src/python/bridge.rs:143-154`). Python object IDs are allocated monotonically.
- FFI libraries are resolved/loaded at runtime depending on the host’s installed shared libraries.
- `Cargo.toml` pins Rust crate versions but does not pin the Rust toolchain, system linker, or external model weights.

Needed for hermetic builds:

1. **Toolchain lockfile** — Rust version, LLVM/Cranelift versions, linker, Python ABI.
2. **Deterministic JIT** — disable JIT in reproducible mode or compile everything AOT.
3. **No runtime global mutable IDs** in reproducible mode; use content-addressed handles.
4. **Hermetic FFI** — vendor shared libraries or declare exact hashes/URLs.
5. **Model lockfile** — pin model versions, prompts, and seeds for AI-assisted builds.
6. **Build info embedding** — record compiler flags, dependency hashes, and model lockfile hash in the produced binary.

---

## 5. Summary of Compiler & AI Recommendations

1. **Do not let AI generate bytecode or bypass validation.** The parser/typechecker/effect/capability pipeline is Nulang’s safety kernel and must remain deterministic and AI-free.
2. **Introduce Intent IR as the universal pre-AST representation** for NL, visual, JSON, voice, and IDE inputs.
3. **Split the compiler into AST → HIR → MIR → optimizer → backends.** Keep Cranelift JIT for dev, add LLVM AOT for production.
4. **Canonicalize and version the `Value` layout** to avoid NaN-tag exhaustion and pointer-width ceilings.
5. **Build a provider-agnostic AI layer** with structured output, deterministic cache, and full audit trail.
6. **Make the LSP semantic** by reusing the real typechecker and adding diagnostics, hover, goto-definition, and AI-assisted code actions.
7. **Invest in hermetic builds:** toolchain lockfile, deterministic JIT/FFI, content-addressed Python handles, and pinned model parameters.

These changes preserve Nulang’s existing strengths — a small, fast, type-safe actor runtime — while creating a durable foundation for AI-assisted, multi-modal programming.
