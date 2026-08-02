---
updated: 2026-08-02
sources:
  - AGENTS.md
  - src/lexer.rs
  - src/parser.rs
  - src/typechecker.rs
  - src/effect_checker.rs
  - src/hir_lower.rs
  - src/mir_lower.rs
  - src/mir_codegen.rs
tags: [overview, compiler]
---

# Compiler Pipeline

The Nulang compiler is **MIR-exclusive**: the legacy AST-direct compiler (`src/compiler.rs`) was removed. Every backend consumes MIR.

## Stages

```
source &str
  ├─ Lexer::lex()                        → Vec<Token>          src/lexer.rs
  ├─ Parser::parse_module()              → AstModule           src/parser.rs
  ├─ TypeChecker::check_module()         → Type                src/typechecker.rs   (HM Algorithm W)
  ├─ EffectChecker::check_module()       → ()                  src/effect_checker.rs
  ├─ CapabilityAnalyzer::infer_cap()     → Capability          src/effect_checker.rs
  ├─ hir_lower::lower_module()           → HIR Module          src/hir_lower.rs
  └─ mir_lower::lower_module()           → MIR Module          src/mir_lower.rs
```

From MIR, one of three backends runs:

### Bytecode backend (default)

```
MIR Module
  ├─ mir_codegen::compile_mir()          → CodeModule          src/mir_codegen.rs
  └─ VM::load_module() + VM::run()       → Value                src/vm.rs
```

Register-based VM with 256 registers per frame, i64-tagged `Value` (see `src/value_layout.rs` for the canonical NaN-tag constants), 135 opcodes across 18 groups. Cranelift JIT tiering triggers at `HOT_THRESHOLD=1000` per PC — see [[../subsystems/jit]] _(pending)_.

### AOT native backend (`--backend native`)

```
MIR Module
  └─ aot::codegen::compile_module()      → native object code   src/aot/codegen.rs
```

MIR → Cranelift CLIF → native. Uses compile-time type metadata (`src/type_metadata.rs`) for unboxed operations. Shares typed-code generation techniques with the JIT.

### WASM backend (`--backend wasm|wasm-run|wasm-aot`, requires `--features wasm-backend`)

```
MIR Module
  ├─ WasmBackend::compile()              → Vec<u8> (.wasm)      src/mir_wasm.rs
  ├─ WasmRuntime::new() + run()          → ()                    src/wasm_runtime.rs
  └─ (optional) aot_compile()            → .cwasm                src/wasm_runtime.rs
```

WASM module uses i64-tagged values (not NaN-boxed) to avoid WASM NaN canonicalization. Wasmtime host is configured with guard pages (4 GiB reservation, 128 MiB guard), Cranelift speed opts (inlining), and SIMD.

## `--check` mode

Stops after `CapabilityAnalyzer` — no MIR lowering, no compile, no run. Used by the LSP and by `nula build` to validate without producing artifacts.

## Frontend cross-cutting concerns

- **Spans**: threaded into nearly every `Expr`/`Decl` variant and every compile-time error. `NuError` (see `src/types.rs`) formats spanned errors as `<Kind> at <line>:<col>: <msg>`.
- **Error model**: first error aborts. `EffectChecker`/`CapabilityAnalyzer` accumulate `diagnostics: Vec<String>` instead of failing fast.
- **Type system**: HM Algorithm W with `Substitution = Vec<(TypeVar, Type)>`, `mgu` + occurs check, `generalize`/`instantiate` over `Type::Scheme`. Row-polymorphic records via the reserved `".."` pseudo-field. Row-polymorphic effects. See [[../concepts/effect-rows]] _(pending)_.
- **Capabilities**: Pony-inspired lattice with at-most-once `LinearIso` consumption tracked in `CapabilityAnalyzer`. See [[../concepts/capability-lattice]] _(pending)_.
- **`__main`**: synthetic function wrapping a top-level expression (parser + HIR lowering).

## What to read next

- Type system detail: [[../concepts/effect-rows]], [[../concepts/capability-lattice]] _(both pending)_.
- Backend detail: [[../subsystems/bytecode-vm]], [[../subsystems/jit]], [[../subsystems/wasm-backend]], [[../subsystems/aot-backend]] _(all pending)_.
- Architecture contract: `AGENTS.md` (Architecture & Data Flow section).

## Source citations

- Pipeline stages: `AGENTS.md` (Architecture & Data Flow section).
- MIR-exclusive claim: `AGENTS.md` — "The compiler pipeline is MIR-exclusive (AST → HIR → MIR → bytecode). The legacy AST compiler (`src/compiler.rs`) has been removed."
- Backend selection: `src/main.rs`.
- `--check` semantics: `src/main.rs` (`check_source` vs `run_source`).
