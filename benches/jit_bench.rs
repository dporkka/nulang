//! JIT tiering benchmarks: hot loop speedup vs interpreter, tier-up latency.

use criterion::{black_box, criterion_group, Criterion};
use nulang::effect_checker::{CapContext, CapabilityAnalyzer, EffectChecker};
use nulang::lexer::Lexer;
use nulang::parser::Parser;
use nulang::typechecker::TypeChecker;
use nulang::vm::VM;

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

fn bench_jit_hot_loop(c: &mut Criterion) {
    let source = "let mut sum = 0; let mut i = 0; while i < 100000 { sum = sum + i * 3 - i / 7; i = i + 1; }; sum";
    let vm_interp = compile_and_load(source);
    c.bench_function("jit/hot_loop_first_run", |b| {
        b.iter(|| {
            let mut vm = vm_interp.clone();
            black_box(vm.run().unwrap());
        })
    });
    let mut vm_warm = compile_and_load(source);
    let _ = vm_warm.run();
    c.bench_function("jit/hot_loop_warm", |b| {
        b.iter(|| {
            let mut vm = vm_warm.clone();
            black_box(vm.run().unwrap());
        })
    });
}

criterion_group!(benches, bench_jit_hot_loop);
