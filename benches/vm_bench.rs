//! VM throughput benchmarks: arithmetic, function calls, closures, dispatch,
//! record/array access.

use criterion::{black_box, criterion_group, Criterion};
use nulang::effect_checker::{CapContext, CapabilityAnalyzer, EffectChecker};
use nulang::lexer::Lexer;
use nulang::parser::Parser;
use nulang::typechecker::TypeChecker;
use nulang::vm::VM;

/// Compile `source` through the full frontend → bytecode pipeline and return
/// a `VM` ready to run. Panics on compile failure.
fn compile_and_load(source: &str) -> VM {
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
    let hir = nulang::hir_lower::lower_module(&ast);
    let mir = nulang::mir_lower::lower_module(&hir).expect("mir lower failed");
    let code_module = nulang::mir_codegen::compile_mir(&mir).expect("codegen failed");
    let mut vm = VM::new();
    vm.load_module(code_module);
    vm
}

fn bench_int_arithmetic(c: &mut Criterion) {
    let source = "let mut sum = 0; let mut i = 0; while i < 1000 { sum = sum + i * 2 - i / 3; i = i + 1; }; sum";
    let vm = compile_and_load(source);
    c.bench_function("vm/int_arithmetic", |b| {
        b.iter(|| {
            let mut vm = vm.clone();
            black_box(vm.run().unwrap());
        })
    });
}

fn bench_float_arithmetic(c: &mut Criterion) {
    // Float loop: use explicit float literals
    let source = "let mut sum = 0.0; let mut i = 0; while i < 500 { sum = sum + (i as Float) * 2.5 - (i as Float) / 3.0; i = i + 1; }; sum";
    let vm = compile_and_load(source);
    c.bench_function("vm/float_arithmetic", |b| {
        b.iter(|| {
            let mut vm = vm.clone();
            black_box(vm.run().unwrap());
        })
    });
}

fn bench_function_call(c: &mut Criterion) {
    let source = "fn add(x: Int, y: Int) -> Int { x + y }; fn mul(x: Int, y: Int) -> Int { x * y }; let mut sum = 0; let mut i = 0; while i < 500 { sum = add(sum, mul(i, 3)); i = i + 1; }; sum";
    let vm = compile_and_load(source);
    c.bench_function("vm/function_call", |b| {
        b.iter(|| {
            let mut vm = vm.clone();
            black_box(vm.run().unwrap());
        })
    });
}

fn bench_closure_capture(c: &mut Criterion) {
    let source = "let base = 10; let adder = fn(x: Int) -> Int { x + base }; let mut sum = 0; let mut i = 0; while i < 500 { sum = adder(i); i = i + 1; }; sum";
    let vm = compile_and_load(source);
    c.bench_function("vm/closure_capture", |b| {
        b.iter(|| {
            let mut vm = vm.clone();
            black_box(vm.run().unwrap());
        })
    });
}

fn bench_record_access(c: &mut Criterion) {
    let source = "let r = { x: 1, y: 2, z: 3 }; let mut sum = 0; let mut i = 0; while i < 1000 { sum = sum + r.x + r.y + r.z; i = i + 1; }; sum";
    let vm = compile_and_load(source);
    c.bench_function("vm/record_access", |b| {
        b.iter(|| {
            let mut vm = vm.clone();
            black_box(vm.run().unwrap());
        })
    });
}

fn bench_array_indexing(c: &mut Criterion) {
    let source = "let arr = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]; let mut sum = 0; let mut i = 0; while i < 1000 { sum = sum + arr[i % 10]; i = i + 1; }; sum";
    let vm = compile_and_load(source);
    c.bench_function("vm/array_indexing", |b| {
        b.iter(|| {
            let mut vm = vm.clone();
            black_box(vm.run().unwrap());
        })
    });
}

criterion_group!(
    benches,
    bench_int_arithmetic,
    bench_float_arithmetic,
    bench_function_call,
    bench_closure_capture,
    bench_record_access,
    bench_array_indexing,
);
