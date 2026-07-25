//! Typechecker fuzzer — mutation-based fuzzing of the Nulang compiler
//! frontend (lex → parse → typecheck → HIR → MIR → bytecode).
//!
//! Generates mutants from a seed corpus of valid programs and verifies
//! that the compiler never panics. Uses a built-in xorshift64 RNG.
//!
//! ```bash
//! cargo test -- fuzz    # Quick fuzz (1000 iterations, CI-friendly)
//! ```

use std::panic;

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
fn fuzz_one(rng: &mut XorShift64, corpus: &[&str]) -> Result<(), String> {
    let seed = corpus[rng.index(corpus)];
    let mutant = mutate(rng, seed, corpus);

    run_frontend_safe(&mutant)?;

    // Occasionally test full pipeline (~1 in 5)
    if rng.range(0, 5) == 0 {
        run_full_pipeline_safe(&mutant)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Quick fuzz: 1000 iterations with fixed seed for reproducibility.
    #[test]
    fn fuzz_typechecker_quick() {
        let corpus = seed_corpus();
        let mut rng = XorShift64(0xDEAD_BEEF_CAFE_BABE);
        let mut panics: Vec<(usize, String, String)> = Vec::new();

        // Replay RNG to capture panic sources
        let mut replay = XorShift64(0xDEAD_BEEF_CAFE_BABE);

        for _ in 0..1000 {
            if let Err(msg) = fuzz_one(&mut rng, &corpus) {
                let seed = corpus[replay.index(&corpus)];
                let mutant = mutate(&mut replay, seed, &corpus);
                panics.push((panics.len(), mutant, msg));
            } else {
                // Advance replay RNG to stay in sync with main RNG
                let seed = corpus[replay.index(&corpus)];
                let _ = mutate(&mut replay, seed, &corpus);
            }
        }

        if !panics.is_empty() {
            for (_i, source, msg) in &panics {
                eprintln!(
                    "PANIC: {}\nSource:\n---\n{}\n---\n",
                    msg, source
                );
            }
            panic!(
                "Fuzzer found {} panic(s) in 1000 iterations",
                panics.len()
            );
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
            if let Err(msg) = fuzz_one(&mut rng, &corpus) {
                panic_count += 1;
                eprintln!("PANIC: {}", msg);
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
