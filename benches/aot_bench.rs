//! AOT native-backend throughput benchmarks.
//!
//! Compile the same workloads as `interp_bench` through the Cranelift AOT
//! backend (`--backend native`) and measure full `AotModule::run()` cost.
//! Unlike `jit_bench`, there is no tier-up: the whole program is native code
//! before the first instruction executes. `run(&self)` is stateless between
//! calls (it installs a fresh standalone heap + constant pool per call), so
//! the same compiled module is reused across iterations.
//!
//! Note: `run()` includes a fixed per-call standalone-heap setup (1 MiB
//! ActorHeap + constant-pool wiring) that is negligible for the hot-loop
//! workloads here but would dominate sub-microsecond programs — interpret
//! low-iteration numbers with that constant in mind.
//!
//! Run with:
//!   cargo bench --bench bench_main -- aot

use criterion::{black_box, criterion_group, BatchSize, Criterion};
use nulang::aot::AotModule;
use nulang::effect_checker::{CapContext, CapabilityAnalyzer, EffectChecker};
use nulang::lexer::Lexer;
use nulang::parser::Parser;
use nulang::typechecker::TypeChecker;

/// Compile `source` through the full frontend → MIR → Cranelift AOT pipeline
/// and return the compiled module. Panics on compile failure.
fn compile(source: &str) -> AotModule {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.lex().expect("lex failed");
    let mut parser = Parser::new(tokens);
    let ast = parser.parse_module().expect("parse failed");
    let mut type_checker = TypeChecker::new();
    type_checker.check_module(&ast).expect("typecheck failed");
    let mut effect_checker = EffectChecker::new();
    effect_checker
        .check_module(&ast.decls)
        .expect("effect check failed");
    let mut cap_analyzer = CapabilityAnalyzer::new();
    let cap_ctx = CapContext::new();
    for decl in nulang::effect_checker::flatten_decls(&ast.decls) {
        match decl {
            nulang::ast::Decl::Function { body, .. } => {
                cap_analyzer
                    .infer_cap(&cap_ctx, body)
                    .expect("cap check failed");
            }
            _ => {}
        }
    }
    let hir = nulang::hir_lower::lower_module(&ast, &type_checker.inferred_decl_types);
    let mir = nulang::mir_lower::lower_module(&hir).expect("mir lower failed");
    AotModule::compile(&mir).expect("aot compile failed")
}

fn bench_aot_int_loop(c: &mut Criterion) {
    let source =
        "var sum = 0; var i = 0; while i < 1000 { sum = sum + i * 2 - i / 3; i = i + 1; }; sum";
    let aot = compile(source);
    c.bench_function("aot/int_loop", |b| {
        b.iter_batched(
            || (),
            |_| black_box(aot.run().unwrap()),
            BatchSize::SmallInput,
        )
    });
}

fn bench_aot_function_call(c: &mut Criterion) {
    let source = "fn add(x: Int, y: Int) -> Int { x + y }; var sum = 0; var i = 0; while i < 500 { sum = add(sum, i); i = i + 1; }; sum";
    let aot = compile(source);
    c.bench_function("aot/function_call", |b| {
        b.iter_batched(
            || (),
            |_| black_box(aot.run().unwrap()),
            BatchSize::SmallInput,
        )
    });
}

fn bench_aot_record_loop(c: &mut Criterion) {
    let source = "let r = { x: 1, y: 2, z: 3 }; var sum = 0; var i = 0; while i < 1000 { sum = sum + r.x + r.y + r.z; i = i + 1; }; sum";
    let aot = compile(source);
    c.bench_function("aot/record_loop", |b| {
        b.iter_batched(
            || (),
            |_| black_box(aot.run().unwrap()),
            BatchSize::SmallInput,
        )
    });
}

fn bench_aot_hot_loop(c: &mut Criterion) {
    let source =
        "var sum = 0; var i = 0; while i < 100000 { sum = sum + i * 3 - i / 7; i = i + 1; }; sum";
    let aot = compile(source);
    // Single run(): no tier-up — the whole loop is native from instruction
    // one, so this is steady-state AOT throughput end to end.
    c.bench_function("aot/hot_loop", |b| {
        b.iter_batched(
            || (),
            |_| black_box(aot.run().unwrap()),
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(
    benches,
    bench_aot_int_loop,
    bench_aot_function_call,
    bench_aot_record_loop,
    bench_aot_hot_loop,
);
