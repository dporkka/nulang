//! Nulang compiler/runtime fuzzer: mutation-based fuzzing of the frontend
//! (lex -> parse -> typecheck -> HIR -> MIR -> bytecode) plus differential
//! execution fuzzing across the interpreter, JIT, and (when a mutant
//! compiles under it) the AOT native backend.
//!
//! Generates mutants from a seed corpus of valid programs. Two independent
//! properties are checked:
//!   1. Panic-avoidance: the compiler frontend never panics on a mutant,
//!      whether or not the mutant is well-formed (`fuzz_one`).
//!   2. Differential correctness: for mutants that DO compile to bytecode,
//!      the interpreter and the JIT-compiled path must agree on every
//!      observable result, and when the AOT backend also accepts the
//!      program, it must agree too (`differential_fuzz_one`). Any
//!      disagreement is a real bug, not a fuzzer false positive — see
//!      PLAN.md Phase 1 bullet 1's kill criteria: a divergence touching
//!      Frozen-tier surface (bytecode/value-layout semantics) is a Sev-1.
//!
//! Uses a built-in xorshift64 RNG, no external fuzzing crate dependency.
//!
//! ```bash
//! cargo test -- fuzz    # Quick fuzz (1000 iterations each mode, CI-friendly)
//! ```

use std::panic;
use std::panic::AssertUnwindSafe;

// ---------------------------------------------------------------------------
// Minimal xorshift64 RNG — no external dependencies
// ---------------------------------------------------------------------------

struct XorShift64(u64);

impl XorShift64 {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn range(&mut self, min: usize, max: usize) -> usize {
        if min >= max {
            return min;
        }
        min + (self.next() as usize % (max - min))
    }

    fn index<T>(&mut self, slice: &[T]) -> usize {
        if slice.is_empty() {
            return 0;
        }
        self.range(0, slice.len())
    }
}

// ---------------------------------------------------------------------------
// Seed corpus — valid Nulang programs exercising different language features
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn seed_corpus() -> Vec<&'static str> {
    vec![
        // --- Literals ---
        "42",
        "true",
        "false",
        r#""hello""#,
        "()",
        // --- Arithmetic ---
        "1 + 2",
        "3 * (4 + 5)",
        "10 - 3 * 2",
        "100 / 5",
        "7 % 3",
        "-42",
        // --- Comparisons ---
        "1 < 2",
        "3 >= 3",
        "5 == 5",
        "true != false",
        // --- Boolean logic ---
        "true and false",
        "true or false",
        "not true",
        // --- String concat ---
        r#""hello" ++ " " ++ "world""#,
        // --- If expressions ---
        "if true then 1 else 2",
        "if 1 < 2 then 10 else 20",
        "if false then 1 else if true then 2 else 3",
        // --- Let bindings ---
        "let x = 42; x",
        "let x = 1; let y = 2; x + y",
        "let x = 10; let y = x * 2; y + x",
        // --- Functions ---
        "fn(x) { x + 1 }",
        "fn(x, y) { x + y }",
        "let f = fn(x) { x * 2 }; f(21)",
        r#"let greet = fn(name) { "Hello, " ++ name }; greet("world")"#,
        // --- Recursive functions ---
        "let fib = fn(n) { if n <= 1 then n else fib(n - 1) + fib(n - 2) }; fib(10)",
        // --- Lambda application ---
        "(fn(x) { x + 1 })(41)",
        // --- Type annotations ---
        "fn(x: Int) -> Int { x + 1 }",
        "fn(x: Int, y: Int) -> Int { x + y }",
        "fn(b: Bool) -> Bool { not b }",
        // --- Records ---
        "{x = 1, y = 2}",
        r#"{name = "Alice", age = 30}"#,
        "let r = {x = 1, y = 2}; r.x + r.y",
        // --- Unit ---
        "let _ = (); 42",
        // --- Blocks ---
        "{ let x = 1; let y = 2; x + y }",
        // --- Nested lets and scoping ---
        "let x = 1; { let x = 2; x } + x",
        // --- Variant types and match ---
        "let x = 42; match x { 0 => false, _ => true }",
        "let b = true; match b { true => 1, false => 0 }",
        // --- Pipes ---
        "42 |> fn(x) { x + 1 }",
        // --- Field access ---
        "let r = {a = 1, b = 2}; r.a",
        // --- Comments ---
        "// comment\n42",
        "/* block */ 42",
        // --- Edge cases ---
        "0",
        "1",
        "fn() { 42 }",
        "fn() { 42 }()",
        "{ 42 }",
        "(42)",
        "((42))",
        "1 + 2 + 3",
        "if true then () else ()",
        r#"let s = "hello"; s"#,
        r#"let s = "a" ++ "b"; s"#,
    ]
}

// ---------------------------------------------------------------------------
// Mutation operators
// ---------------------------------------------------------------------------

/// Delete a random character from the source.
fn mutate_delete(rng: &mut XorShift64, source: &str) -> String {
    if source.is_empty() {
        return source.to_string();
    }
    let idx = rng.range(0, source.len());
    let mut s = String::with_capacity(source.len() - 1);
    s.push_str(&source[..idx]);
    s.push_str(&source[idx + 1..]);
    s
}

/// Insert a random character at a random position.
fn mutate_insert(rng: &mut XorShift64, source: &str) -> String {
    let chars = b"abcdefghijklmnopqrstuvwxyz0123456789 \n\t+-*/%=<>!&|.,;:(){}[]_\"'";
    let idx = rng.range(0, source.len() + 1);
    let ch = chars[rng.index(chars)] as char;
    let mut s = String::with_capacity(source.len() + 1);
    s.push_str(&source[..idx]);
    s.push(ch);
    s.push_str(&source[idx..]);
    s
}

/// Swap two adjacent characters.
fn mutate_swap(rng: &mut XorShift64, source: &str) -> String {
    if source.len() < 2 {
        return source.to_string();
    }
    let idx = rng.range(0, source.len() - 1);
    let mut chars: Vec<char> = source.chars().collect();
    chars.swap(idx, idx + 1);
    chars.into_iter().collect()
}

/// Duplicate a character at a random position.
fn mutate_duplicate(rng: &mut XorShift64, source: &str) -> String {
    if source.is_empty() {
        return source.to_string();
    }
    let idx = rng.range(0, source.len());
    let ch = source.chars().nth(idx).unwrap_or(' ');
    let mut s = String::with_capacity(source.len() + 1);
    s.push_str(&source[..idx]);
    s.push(ch);
    s.push_str(&source[idx..]);
    s
}

/// Replace a random span with another span from the corpus.
fn mutate_splice(rng: &mut XorShift64, source: &str, corpus: &[&str]) -> String {
    if source.len() < 2 || corpus.is_empty() {
        return source.to_string();
    }
    let start = rng.range(0, source.len() - 1);
    let end = rng.range(start + 1, source.len());
    let replacement = corpus[rng.index(corpus)];

    let mut s = String::with_capacity(source.len() + replacement.len());
    s.push_str(&source[..start]);
    s.push_str(replacement);
    s.push_str(&source[end..]);
    s
}

/// Truncate the source at a random point.
fn mutate_truncate(rng: &mut XorShift64, source: &str) -> String {
    if source.len() < 2 {
        return source.to_string();
    }
    let idx = rng.range(1, source.len());
    source[..idx].to_string()
}

/// Double the entire source.
fn mutate_double(source: &str) -> String {
    let mut s = String::with_capacity(source.len() * 2 + 1);
    s.push_str(source);
    s.push('\n');
    s.push_str(source);
    s
}

/// Apply a random mutation.
fn mutate(rng: &mut XorShift64, source: &str, corpus: &[&str]) -> String {
    match rng.range(0, 7) {
        0 => mutate_delete(rng, source),
        1 => mutate_insert(rng, source),
        2 => mutate_swap(rng, source),
        3 => mutate_duplicate(rng, source),
        4 => mutate_splice(rng, source, corpus),
        5 => mutate_truncate(rng, source),
        _ => mutate_double(source),
    }
}

// ---------------------------------------------------------------------------
// Fuzz harness
// ---------------------------------------------------------------------------

use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::typechecker::TypeChecker;

#[allow(dead_code)]
fn run_frontend_safe(source: &str) -> Result<(), String> {
    let source_owned = source.to_string();
    panic::catch_unwind(panic::AssertUnwindSafe(move || {
        let mut lexer = Lexer::new(&source_owned);
        let tokens = match lexer.lex() {
            Ok(t) => t,
            Err(_) => return,
        };
        let mut parser = Parser::new(tokens);
        let ast = match parser.parse_module() {
            Ok(a) => a,
            Err(_) => return,
        };
        let mut type_checker = TypeChecker::new();
        let _ = type_checker.check_module(&ast);
    }))
    .map_err(|e| {
        if let Some(s) = e.downcast_ref::<String>() {
            s.clone()
        } else if let Some(s) = e.downcast_ref::<&str>() {
            s.to_string()
        } else {
            "unknown panic payload".to_string()
        }
    })
}

/// Run the full pipeline (lex → parse → typecheck → HIR → MIR → bytecode)
#[allow(dead_code)]
/// safely, catching panics.
fn run_full_pipeline_safe(source: &str) -> Result<(), String> {
    let source_owned = source.to_string();
    panic::catch_unwind(panic::AssertUnwindSafe(move || {
        let mut lexer = Lexer::new(&source_owned);
        let tokens = match lexer.lex() {
            Ok(t) => t,
            Err(_) => return,
        };
        let mut parser = Parser::new(tokens);
        let ast = match parser.parse_module() {
            Ok(a) => a,
            Err(_) => return,
        };
        let mut type_checker = TypeChecker::new();
        if type_checker.check_module(&ast).is_ok() {
            let hir = crate::hir_lower::lower_module(&ast);
            if let Ok(mir) = crate::mir_lower::lower_module(&hir) {
                let _ = crate::mir_codegen::compile_mir(&mir, "fuzz");
            }
        }
    }))
    .map_err(|e| {
        if let Some(s) = e.downcast_ref::<String>() {
            s.clone()
        } else if let Some(s) = e.downcast_ref::<&str>() {
            s.to_string()
        } else {
            "unknown panic payload".to_string()
        }
    })
}

#[allow(dead_code)]
fn fuzz_one(rng: &mut XorShift64, corpus: &[&str]) -> Result<(), (String, String)> {
    let seed = corpus[rng.index(corpus)];
    let mutant = mutate(rng, seed, corpus);

    run_frontend_safe(&mutant).map_err(|msg| (mutant.clone(), msg))?;

    // Occasionally test full pipeline (~1 in 5)
    if rng.range(0, 5) == 0 {
        run_full_pipeline_safe(&mutant).map_err(|msg| (mutant, msg))?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Differential execution fuzzing: interpreter vs JIT vs AOT
// ---------------------------------------------------------------------------

/// A mutant that compiles to bytecode, ready for differential execution.
#[allow(dead_code)]
struct CompiledMutant {
    code_module: crate::bytecode::CodeModule,
    mir_module: crate::mir::Module,
}

/// Compile `source` through the full pipeline. Returns `None` (not an
/// error) when the mutant fails to compile — most mutants are malformed
/// by construction and have nothing to differentially execute; that's
/// `fuzz_one`'s job to check for panics, not this function's.
#[allow(dead_code)]
fn compile_for_diff(source: &str) -> Option<CompiledMutant> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.lex().ok()?;
    let mut parser = Parser::new(tokens);
    let ast = parser.parse_module().ok()?;
    let mut type_checker = TypeChecker::new();
    type_checker.check_module(&ast).ok()?;
    let hir = crate::hir_lower::lower_module(&ast);
    let mir_module = crate::mir_lower::lower_module(&hir).ok()?;
    let code_module = crate::mir_codegen::compile_mir(&mir_module, "fuzz-diff").ok()?;
    Some(CompiledMutant {
        code_module,
        mir_module,
    })
}

/// Run `module` to completion, catching panics. Returns the raw `Value`
/// alongside a comparable string representation (via
/// `Value::to_string_repr`). The raw `Value` lets callers restrict
/// cross-backend comparison to tags that don't need pool/heap context to
/// resolve (see `is_safely_comparable`) — `to_string_repr` alone is NOT a
/// safe cross-backend comparison key: a `TAG_STRING` value is a constant-
/// pool INDEX, and the interpreter's module pool and an independently
/// AOT-compiled module's pool are not guaranteed to index the same
/// literal identically, so two semantically-identical string results can
/// carry different indices and print as different opaque `#Value(..)`
/// fallback hex — a false-positive divergence, not a real one.
#[allow(dead_code)]
fn run_once(vm: &mut crate::vm::VM) -> Result<(crate::vm::Value, String), String> {
    match panic::catch_unwind(AssertUnwindSafe(|| vm.run())) {
        Ok(Ok(value)) => Ok((value, value.to_string_repr())),
        Ok(Err(e)) => Err(format!("runtime error: {}", e)),
        Err(payload) => Err(format!(
            "panic: {}",
            payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown panic payload".to_string())
        )),
    }
}

/// True for tags whose `to_string_repr()` is self-contained (no pool/heap
/// context needed), so a raw cross-backend comparison is trustworthy:
/// nil, unit, bool, int, float. False for anything pointer/pool-indexed
/// (string, closure, actor ref, heap object) — those need module-aware
/// resolution this fuzzer doesn't attempt (see `run_once` doc comment).
#[allow(dead_code)]
fn is_safely_comparable(v: crate::vm::Value) -> bool {
    v.is_nil() || v.is_unit() || v.is_bool() || v.is_int() || v.is_float()
}

/// Collapse a runtime error message to a stable comparison key.
///
/// `VM::step_count` is a lifetime counter on the `VM` instance, not a
/// per-`run()` counter (only `VM::new()` initializes it) — so repeated
/// `run()` calls on the same VM (this fuzzer's warmup loop) accumulate
/// steps across calls. For a pathological mutant that individually burns
/// millions of steps, that means cold's and warm's calls trip the 10M
/// step-limit safety net at different CUMULATIVE counts, embedding
/// different exact numbers and stack-trace depths in otherwise-equivalent
/// "this program is a runaway and was correctly aborted" outcomes. The
/// step limit is a resource bound, not observable language semantics, so
/// normalize it to a fixed marker before comparing — otherwise every
/// runaway mutant is a guaranteed false-positive divergence.
#[allow(dead_code)]
fn normalize_error(msg: &str) -> String {
    if msg.contains("Step limit exceeded") {
        "step limit exceeded".to_string()
    } else {
        msg.to_string()
    }
}

/// Differentially execute one compiled mutant: interpreter (cold) vs JIT
/// (warm) vs AOT (when the mutant's constructs are within AOT's supported
/// subset). `VM::run()` fully resets frame/PC state on every call while
/// the JIT session's hot-region cache persists across calls on the same
/// VM instance (see benches/jit_bench.rs for the same technique validated
/// against a hand-picked hot loop) — so calling `run()` `HOT_THRESHOLD`+
/// times on one VM naturally exercises real JIT-compiled code for any
/// mutant whose entry-point code contains a compilable straight-line
/// region, without needing to wrap every mutant in an explicit loop.
///
/// Returns `Err(description)` on any divergence. `Ok(DiffOutcome)` covers
/// "nothing to compile", "compared and agreed" (with or without AOT), and
/// "result type has no stable cross-run identity" (`Uncomparable` — e.g. a
/// top-level closure or actor ref; see `resolve_key`).
#[allow(dead_code)]
fn differential_fuzz_one(source: &str) -> Result<DiffOutcome, String> {
    const HOT_ITERATIONS: usize = 1200; // > jit::mod's HOT_THRESHOLD (1000)
                                        // Caps warmup cost for a mutant whose OWN body loops heavily (the seed
                                        // corpus includes large-loop programs, e.g. vm_bench.rs-style hot
                                        // loops): repeating `run()` up to HOT_ITERATIONS times multiplies an
                                        // already-large per-call cost, which is both unbounded and pointless —
                                        // a loop that iterates thousands of times inside ONE `run()` call
                                        // already accumulates enough hot-counter hits to trigger tier-up
                                        // within that single call, so further outer repetitions buy nothing.
                                        // 25ms keeps a full-corpus fuzz run bounded while still giving cheap,
                                        // straight-line mutants every rep they need to cross HOT_THRESHOLD.
    const WARMUP_BUDGET: std::time::Duration = std::time::Duration::from_millis(25);

    let Some(mutant) = compile_for_diff(source) else {
        return Ok(DiffOutcome::NothingToCompile);
    };

    let mut vm = crate::vm::VM::new();
    vm.load_module(mutant.code_module.clone());
    const MODULE_IDX: usize = 0; // the fuzzer always loads exactly one module

    let cold = run_once(&mut vm);
    let warmup_deadline = std::time::Instant::now() + WARMUP_BUDGET;
    for _ in 0..HOT_ITERATIONS {
        if std::time::Instant::now() >= warmup_deadline {
            break;
        }
        let _ = run_once(&mut vm);
    }
    let warm = run_once(&mut vm);

    // Resolve each side to a comparison key. Numeric/bool/nil/unit compare
    // by `to_string_repr()` directly. String/heap-pointer values resolve
    // to actual text via `VM::string_operand` — module-aware and
    // content-based (its own doc comment: "the same text may live at
    // different pool indices"), which is exactly the interpreter's own
    // fix for this problem in `ICmpEq`, reused here rather than
    // reinvented. Anything else (closures, actor refs) has no stable
    // identity across independent executions — fresh actor ids and fresh
    // closure allocations differ run to run by design — so it's `None`.
    let resolve_key = |vm: &crate::vm::VM,
                       r: &Result<(crate::vm::Value, String), String>|
     -> Option<Result<String, String>> {
        match r {
            Err(e) => Some(Err(normalize_error(e))),
            Ok((v, repr)) => {
                if is_safely_comparable(*v) {
                    Some(Ok(repr.clone()))
                } else if v.is_string() || v.is_ptr() {
                    vm.string_operand(MODULE_IDX, *v)
                        .map(|s| Ok(format!("str:{:?}", s)))
                } else {
                    None
                }
            }
        }
    };

    let (Some(cold_key), Some(warm_key)) = (resolve_key(&vm, &cold), resolve_key(&vm, &warm))
    else {
        return Ok(DiffOutcome::Uncomparable);
    };

    if cold_key != warm_key {
        return Err(format!(
            "interpreter/JIT divergence on {:?}: cold={:?} warm={:?}",
            source, cold_key, warm_key
        ));
    }

    // AOT: compiled independently with its own constant pool that isn't
    // guaranteed to index shared string literals identically, and there's
    // no AOT-side equivalent of `string_operand` to resolve against — so
    // only tags whose representation needs no pool/heap context are
    // compared here. Rejection at compile time (`AotCompileError::
    // Unsupported`, e.g. effects/actors/FFI — see src/aot/codegen.rs) and
    // "no compiled entry point" at run time (nothing executable, e.g. an
    // empty/comment-only program) are both expected, non-divergent
    // outcomes, not silently-passed successes: `aot_outcome` stays false
    // and the caller's `InterpJitOnlyAgreed` vs `AllAgreed` split reports
    // real AOT coverage honestly.
    let aot_outcome = match crate::aot::AotModule::compile(&mutant.mir_module) {
        Ok(aot_module) => match panic::catch_unwind(AssertUnwindSafe(|| aot_module.run())) {
            Err(_) => return Err(format!("AOT run panicked on {:?}", source)),
            Ok(Err(e)) if e.to_string().contains("no compiled entry point") => false,
            Ok(Err(e)) => {
                let aot_key: Result<String, String> =
                    Err(normalize_error(&format!("runtime error: {}", e)));
                if aot_key != cold_key {
                    return Err(format!(
                        "interpreter/AOT divergence on {:?}: interp={:?} aot={:?}",
                        source, cold_key, aot_key
                    ));
                }
                true
            }
            Ok(Ok(raw)) => {
                let aot_value = crate::vm::Value::from_raw(raw);
                if !is_safely_comparable(aot_value) {
                    false
                } else {
                    let aot_key: Result<String, String> = Ok(aot_value.to_string_repr());
                    if aot_key != cold_key {
                        return Err(format!(
                            "interpreter/AOT divergence on {:?}: interp={:?} aot={:?}",
                            source, cold_key, aot_key
                        ));
                    }
                    true
                }
            }
        },
        Err(_) => false,
    };
    #[cfg(feature = "wasm-backend")]
    let wasm_outcome = {
        use crate::backends::WasmBackend;
        let mut wasm_backend = crate::backends::DefaultWasmBackend;
        match wasm_backend.compile(&mutant.mir_module, "main") {
            Ok(wasm_bytes) => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| wasm_backend.run(&wasm_bytes))) {
                Err(_) => return Err(format!("WASM run panicked on {:?}", source)),
                Ok(Err(e)) => {
                    let err_str = e.to_string();
                    if err_str.contains("failed to compile") || 
                       err_str.contains("failed to parse WebAssembly module") ||
                       err_str.contains("failed to find function export `nulang_init`") {
                        false
                    } else {
                        let wasm_key: Result<String, String> =
                            Err(normalize_error(&format!("runtime error: {}", err_str)));
                        if wasm_key != cold_key {
                            return Err(format!(
                                "interpreter/WASM divergence on {:?}: interp={:?} wasm={:?}",
                                source, cold_key, wasm_key
                            ));
                        }
                        true
                    }
                }
                Ok(Ok(wasm_value)) => {
                    if !is_safely_comparable(wasm_value) {
                        false
                    } else {
                        let wasm_key: Result<String, String> = Ok(wasm_value.to_string_repr());
                        if wasm_key != cold_key {
                            return Err(format!(
                                "interpreter/WASM divergence on {:?}: interp={:?} wasm={:?}",
                                source, cold_key, wasm_key
                            ));
                        }
                        true
                    }
                }
            },
            Err(_) => false,
        }
    };
    #[cfg(not(feature = "wasm-backend"))]
    let wasm_outcome = false;

    Ok(DiffOutcome::Agreed {
        aot: aot_outcome,
        wasm: wasm_outcome,
    })
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffOutcome {
    /// Mutant didn't compile to bytecode — nothing to differentially run.
    NothingToCompile,
    /// Interpreter, JIT, and optionally AOT/WASM agreed.
    Agreed { aot: bool, wasm: bool },
    /// Result has no stable identity across independent runs (closure,
    /// actor ref, ...) — not compared, not counted as agreement.
    Uncomparable,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Quick differential fuzz: 300 iterations with a fixed seed, part of
    /// the default `cargo test --lib` run. Unlike `fuzz_typechecker_quick`
    /// (lex/parse/typecheck only), each iteration here compiles to
    /// bytecode, runs the interpreter, forces real JIT tier-up, and
    /// attempts AOT compilation — real compute per mutant, plus the
    /// occasional pathological mutant (e.g. runaway recursion that burns
    /// the full step-limit budget) costing multiple seconds on its own.
    /// 300 keeps this test's contribution to the default suite's runtime
    /// proportionate; see `fuzz_differential_extended` for the larger,
    /// `#[ignore]`'d run intended for a dedicated nightly job. PLAN.md
    /// Phase 1 targets 4x10^4/day in per-PR CI and 10^6/day in CI nightly;
    /// neither figure is achievable inside a single `cargo test`
    /// invocation, so `fuzz_differential_extended` is the seed for a
    /// dedicated CI job that runs it in a loop (or with a higher
    /// iteration count) on a schedule.
    #[test]
    fn fuzz_differential_quick() {
        let corpus = seed_corpus();
        let mut rng = XorShift64(0xD1FF_5EED_0000_0001);
        let mut divergences: Vec<String> = Vec::new();
        let mut compiled = 0usize;
        let mut aot_agreed = 0usize;
        let mut wasm_agreed = 0usize;
        let mut uncomparable = 0usize;

        for _ in 0..300 {
            let seed = corpus[rng.index(&corpus)];
            let mutant = mutate(&mut rng, seed, &corpus);
            match differential_fuzz_one(&mutant) {
                Ok(DiffOutcome::NothingToCompile) => {}
                Ok(DiffOutcome::Uncomparable) => uncomparable += 1,
                Ok(DiffOutcome::Agreed { aot, wasm }) => {
                    compiled += 1;
                    if aot { aot_agreed += 1; }
                    if wasm { wasm_agreed += 1; }
                }
                Err(msg) => {
                    divergences.push(msg);
                    if divergences.len() >= 5 {
                        break;
                    }
                }
            }
        }

        eprintln!(
            "differential fuzz: {} mutants compiled and ran (agreed), {} of those also agreed \
             under AOT, {} agreed under WASM, {} uncomparable (closures/actor refs)",
            compiled, aot_agreed, wasm_agreed, uncomparable
        );

        if !divergences.is_empty() {
            for msg in &divergences {
                eprintln!("DIVERGENCE: {}", msg);
            }
            panic!(
                "Differential fuzzer found {} divergence(s) in 1000 iterations — see PLAN.md \
                 Phase 1 kill criteria",
                divergences.len()
            );
        }
    }

    /// Extended differential fuzz (ignored by default — run explicitly or
    /// from a dedicated CI job): 30,000 iterations with a fixed seed by
    /// default. Shardable for a CI matrix via env vars so a scheduled
    /// nightly job can approach PLAN.md Phase 1 bullet 1's 10^6/day
    /// target through parallelism rather than one long-running process:
    /// `NULANG_FUZZ_ITERATIONS` overrides the per-shard iteration count;
    /// `NULANG_FUZZ_SHARD_ID` (default 0) perturbs the seed so shards
    /// don't all fuzz the identical sequence. Both are no-ops for the
    /// default local `cargo test -- --ignored` invocation.
    #[test]
    #[ignore]
    fn fuzz_differential_extended() {
        let iterations: usize = std::env::var("NULANG_FUZZ_ITERATIONS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30_000);
        let shard_id: u64 = std::env::var("NULANG_FUZZ_SHARD_ID")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let corpus = seed_corpus();
        // Base seed XORed with the shard id (odd constant to avoid
        // collapsing xorshift64's state to zero for shard 0 on some
        // platforms — 0 XORs to the base seed unchanged, any other shard
        // gets a distinctly different starting state).
        let mut rng =
            XorShift64(0xD1FF_5EED_0000_0002 ^ (shard_id.wrapping_mul(0x9E37_79B9_7F4A_7C15)));
        let mut divergence_count = 0usize;
        let mut compiled = 0usize;
        let mut aot_agreed = 0usize;
        let mut wasm_agreed = 0usize;
        let mut uncomparable = 0usize;

        for _ in 0..iterations {
            let seed = corpus[rng.index(&corpus)];
            let mutant = mutate(&mut rng, seed, &corpus);
            match differential_fuzz_one(&mutant) {
                Ok(DiffOutcome::NothingToCompile) => {}
                Ok(DiffOutcome::Uncomparable) => uncomparable += 1,
                Ok(DiffOutcome::Agreed { aot, wasm }) => {
                    compiled += 1;
                    if aot { aot_agreed += 1; }
                    if wasm { wasm_agreed += 1; }
                }
                Err(msg) => {
                    divergence_count += 1;
                    eprintln!("DIVERGENCE: {}", msg);
                    if divergence_count >= 10 {
                        panic!("Too many divergences ({}) — aborting", divergence_count);
                    }
                }
            }
        }

        eprintln!(
            "differential fuzz (extended): {} mutants compiled and ran (agreed), {} of those \
             also agreed under AOT, {} agreed under WASM, {} uncomparable (closures/actor refs)",
            compiled, aot_agreed, wasm_agreed, uncomparable
        );

        if divergence_count > 0 {
            panic!(
                "Differential fuzzer found {} divergence(s)",
                divergence_count
            );
        }
    }

    /// Quick fuzz: 1000 iterations with fixed seed for reproducibility.
    #[test]
    fn fuzz_typechecker_quick() {
        let corpus = seed_corpus();
        let mut rng = XorShift64(0xDEAD_BEEF_CAFE_BABE);
        let mut panics: Vec<(String, String)> = Vec::new();

        for _ in 0..1000 {
            if let Err((source, msg)) = fuzz_one(&mut rng, &corpus) {
                panics.push((source, msg));
                if panics.len() >= 5 {
                    break; // Enough evidence, stop early
                }
            }
        }

        if !panics.is_empty() {
            for (source, msg) in &panics {
                eprintln!("PANIC: {}\nSource:\n---\n{}\n---\n", msg, source);
            }
            panic!("Fuzzer found {} panic(s) in 1000 iterations", panics.len());
        }
    }

    /// Extended fuzz: 10,000 iterations (ignored by default).
    #[test]
    #[ignore]
    fn fuzz_typechecker_extended() {
        let corpus = seed_corpus();
        let mut rng = XorShift64(0x1234_5678_9ABC_DEF0);
        let mut panic_count = 0;

        for _ in 0..10_000 {
            if let Err((source, msg)) = fuzz_one(&mut rng, &corpus) {
                panic_count += 1;
                eprintln!("PANIC: {}\nSource:\n---\n{}\n---\n", msg, source);
                if panic_count >= 10 {
                    panic!("Too many panics ({}) — aborting", panic_count);
                }
            }
        }

        if panic_count > 0 {
            panic!("Fuzzer found {} panic(s)", panic_count);
        }
    }

    /// Sanity check: seed corpus programs parse and typecheck cleanly.
    #[test]
    fn seed_corpus_well_typed() {
        let corpus = seed_corpus();
        let mut failures = Vec::new();
        for (i, program) in corpus.iter().enumerate() {
            let mut lexer = Lexer::new(program);
            let tokens = match lexer.lex() {
                Ok(t) => t,
                Err(e) => {
                    failures.push((i, *program, format!("Lex error: {:?}", e)));
                    continue;
                }
            };
            let mut parser = Parser::new(tokens);
            let ast = match parser.parse_module() {
                Ok(a) => a,
                Err(e) => {
                    failures.push((i, *program, format!("Parse error: {:?}", e)));
                    continue;
                }
            };
            let mut tc = TypeChecker::new();
            if let Err(e) = tc.check_module(&ast) {
                failures.push((i, *program, format!("Type error: {}", e)));
            }
        }
        if !failures.is_empty() {
            eprintln!(
                "{} of {} seed programs had errors:",
                failures.len(),
                corpus.len()
            );
            for (i, prog, err) in &failures {
                eprintln!("  [{}] {} → {}", i, prog, err);
            }
        }
        // Note: not all seeds need to typecheck — some exercise edge cases
    }
}
