//! Nulang CLI entry point.
//!
//! Usage:
//!   nulang [OPTIONS] <FILE>
//!   nulang --repl
//!   nulang --eval <CODE>
//!   nulang --check <FILE>
//!   nulang --lsp       Start LSP server
//!   nulang --doc       Generate docs/api.md for the current project
//!
//! Options:
//!   -r, --repl       Start interactive REPL
//!   -e, --eval       Evaluate a code string
//!   -c, --check      Type-check a file (don't run)
//!   --doc            Generate Markdown API docs (docs/api.md)
//!   --lsp            Start Language Server (stdio)
//!   --version, -V    Print version and exit
//!   -v, --verbose    Show bytecode and AST
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use nulang::effect_checker::{CapContext, CapabilityAnalyzer, EffectChecker};
use nulang::lexer::Lexer;
use nulang::parser::Parser;
use nulang::repl::Repl;
use nulang::typechecker::TypeChecker;
use nulang::types::{NuError, NuResult, Type};
use nulang::vm::VM;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() <= 1 {
        // Default: start REPL with the HIR/MIR pipeline.
        let mut repl = Repl::new();
        repl.run();
        return;
    }

    // `nulang nula <cmd>` dispatches to the package manager.
    if args[1] == "nula" {
        if let Err(e) = nulang::package::commands::run(&args[2..]) {
            print_error(&e);
            std::process::exit(1);
        }
        return;
    }

    // Parse arguments
    let mut opts = Options::default();
    let mut positional = Vec::new();
    let mut i = 1;

    while i < args.len() {
        match args[i].as_str() {
            "-r" | "--repl" => opts.repl = true,
            "-e" | "--eval" => {
                if i + 1 < args.len() {
                    opts.eval_code = Some(args[i + 1].clone());
                    i += 1;
                } else {
                    eprintln!("Error: --eval requires a code argument");
                    std::process::exit(1);
                }
            }
            "-c" | "--check" => {
                if i + 1 < args.len() {
                    opts.check_file = Some(args[i + 1].clone());
                    i += 1;
                } else {
                    eprintln!("Error: --check requires a file argument");
                    std::process::exit(1);
                }
            }
            "--version" | "-V" => {
                println!("nulang v{}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--lsp" => opts.lsp = true,
            "--doc" => opts.doc = true,
            "--backend" => {
                if i + 1 < args.len() {
                    opts.backend = args[i + 1].clone();
                    i += 1;
                } else {
                    eprintln!("Error: --backend requires an argument (bytecode | wasm | wasm-run | wasm-aot)");
                    std::process::exit(1);
                }
            }
            "-v" | "--verbose" => opts.verbose = true,
            "-h" | "--help" => {
                print_help();
                return;
            }
            arg if arg.starts_with('-') => {
                eprintln!("Error: Unknown option: {}", arg);
                eprintln!("Run with --help for usage information.");
                std::process::exit(1);
            }
            arg => positional.push(arg.to_string()),
        }
        i += 1;
    }

    if opts.doc {
        let root = match std::env::current_dir() {
            Ok(dir) => dir,
            Err(e) => {
                eprintln!("Error: Cannot determine current directory: {}", e);
                std::process::exit(1);
            }
        };
        match nulang::docgen::write_project_docs(&root) {
            Ok(path) => println!("Wrote {}", path.display()),
            Err(e) => {
                print_error(&e);
                std::process::exit(1);
            }
        }
        return;
    }

    if opts.lsp {
        #[cfg(feature = "lsp")]
        {
            nulang::lsp::run_lsp_server().await;
            return;
        }
        #[cfg(not(feature = "lsp"))]
        {
            eprintln!("Error: this build was compiled without the 'lsp' feature.");
            std::process::exit(1);
        }
    }

    if opts.repl {
        let mut repl = Repl::new();
        repl.run();
        return;
    }

    if let Some(code) = opts.eval_code {
        if let Err(e) = run_source(&code, opts.verbose, &opts.backend) {
            print_error(&e);
            std::process::exit(1);
        }
        return;
    }

    if let Some(path) = opts.check_file {
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error: Cannot read file '{}': {}", path, e);
                std::process::exit(1);
            }
        };
        if let Err(e) = check_source(&source, opts.verbose) {
            print_error(&e);
            std::process::exit(1);
        }
        println!("Type check passed.");
        return;
    }

    // Run a source file
    if !positional.is_empty() {
        let path = &positional[0];
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error: Cannot read file '{}': {}", path, e);
                std::process::exit(1);
            }
        };
        if let Err(e) = run_source(&source, opts.verbose, &opts.backend) {
            print_error(&e);
            std::process::exit(1);
        }
        return;
    }

    // No arguments and no options: start REPL
    let mut repl = Repl::new();
    repl.run();
}

struct Options {
    repl: bool,
    eval_code: Option<String>,
    check_file: Option<String>,
    lsp: bool,
    doc: bool,
    verbose: bool,
    backend: String,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            repl: false,
            eval_code: None,
            check_file: None,
            lsp: false,
            doc: false,
            verbose: false,
            backend: "bytecode".to_string(),
        }
    }
}

fn print_help() {
    println!("Usage: nulang [OPTIONS] <FILE>");
    println!("       nulang --repl");
    println!("       nulang --eval <CODE>");
    println!("       nulang --check <FILE>");
    println!("       nulang --lsp");
    println!("       nulang nula <new|build|test|run>");
    println!("       nulang --doc");
    println!();
    println!("Options:");
    println!("  -r, --repl       Start interactive REPL");
    println!("  -e, --eval       Evaluate a code string");
    println!("  -c, --check      Type-check a file (don't run)");
    println!("  --doc            Generate Markdown API docs (docs/api.md)");
    println!("  --lsp            Start Language Server (stdio)");
    println!("  --backend <b>    Backend: bytecode (default) | wasm | wasm-run | wasm-aot");
    println!("  nula <cmd>       Package manager (new, build, test, run)");
    println!("  --version, -V    Print version and exit");
    println!("  -v, --verbose    Show bytecode and AST");
    println!("  -h, --help       Show this help message");
}

fn print_error(err: &NuError) {
    eprintln!("Error: {}", err);
}

/// Shared frontend: lex -> parse -> typecheck -> effect check -> capability
/// analysis. Returns the parsed module ready for compilation.
fn run_frontend(source: &str, verbose: bool) -> NuResult<nulang::ast::AstModule> {
    // 1. Lex
    let mut lexer = Lexer::new(source);
    let tokens = lexer.lex().map_err(|e| {
        eprintln!("Lex error: {}", e);
        e
    })?;

    // 2. Parse
    let mut parser = Parser::new(tokens);
    let ast = parser.parse_module().map_err(|e| {
        eprintln!("Parse error: {}", e);
        e
    })?;

    if verbose {
        println!("=== AST ===");
        println!("{:#?}", ast);
        println!();
    }

    // 3. Type check
    let mut type_checker = TypeChecker::new();
    let module_type = type_checker.check_module(&ast).map_err(|e| {
        eprintln!("Type error: {}", e);
        e
    })?;

    if verbose {
        println!("=== Inferred Type ===");
        println!("{}\n", type_to_string(&module_type));
    }

    // 4. Effect check. Two passes over module functions: first register a
    // name -> EffectRow map (declared rows where present, fixpoint-inferred
    // otherwise) so call sites propagate callee effects, then enforce
    // declared rows. Bodies without a declared row are inference-only.
    // Nested `module {}` decls are flattened first (mirroring the
    // typechecker's flatten_decls).
    let flat_decls = nulang::effect_checker::flatten_decls(&ast.decls);
    let mut effect_checker = EffectChecker::new();
    effect_checker
        .register_function_rows(&flat_decls)
        .map_err(|e| {
            eprintln!("Effect error: {}", e);
            e
        })?;
    for decl in &flat_decls {
        effect_checker.check_decl(decl).map_err(|e| {
            eprintln!("Effect error: {}", e);
            e
        })?;
    }

    // 5. Capability analysis over the same body set.
    let mut cap_analyzer = CapabilityAnalyzer::new();
    let cap_ctx = CapContext::new();
    let cap_body = |analyzer: &mut CapabilityAnalyzer, body: &nulang::ast::Expr| -> NuResult<()> {
        analyzer.infer_cap(&cap_ctx, body).map(|_| ()).map_err(|e| {
            eprintln!("Capability error: {}", e);
            e
        })
    };
    for decl in flat_decls.iter().copied() {
        match decl {
            nulang::ast::Decl::Function { body, .. } => {
                cap_body(&mut cap_analyzer, body)?;
            }
            nulang::ast::Decl::Actor {
                behaviors,
                state_fields,
                init,
                ..
            } => {
                for b in behaviors {
                    cap_body(&mut cap_analyzer, &b.body)?;
                }
                for (_, _, _, default) in state_fields {
                    cap_body(&mut cap_analyzer, default)?;
                }
                for (_, expr) in init {
                    cap_body(&mut cap_analyzer, expr)?;
                }
            }
            nulang::ast::Decl::Workflow {
                items, compensate, ..
            } => {
                for item in items {
                    let steps: &[nulang::ast::WorkflowStep] = match item {
                        nulang::ast::WorkflowItem::Step(s) => std::slice::from_ref(s),
                        nulang::ast::WorkflowItem::Parallel(steps) => steps,
                    };
                    for step in steps {
                        cap_body(&mut cap_analyzer, &step.body)?;
                        if let Some(comp) = &step.compensate {
                            cap_body(&mut cap_analyzer, comp)?;
                        }
                    }
                }
                if let Some(comp) = compensate {
                    cap_body(&mut cap_analyzer, comp)?;
                }
            }
            _ => {}
        }
    }

    Ok(ast)
}

/// Full pipeline: parse -> typecheck -> effect check -> compile -> execute.
fn run_source(source: &str, verbose: bool, backend: &str) -> NuResult<()> {
    let ast = run_frontend(source, verbose)?;

    match backend {
        #[cfg(feature = "wasm-backend")]
        "wasm" => {
            let hir = nulang::hir_lower::lower_module(&ast);
            let mir = nulang::mir_lower::lower_module(&hir)?;
            let mut wasm_backend = nulang::mir_wasm::WasmBackend::new();
            let wasm_bytes = wasm_backend.compile(&mir, "main")?;
            if verbose {
                println!("=== WASM ({}) bytes ===", wasm_bytes.len());
            }
            // Write .wasm file.
            std::fs::write("out.wasm", &wasm_bytes).map_err(|e| {
                nulang::types::NuError::VMError(format!("failed to write out.wasm: {}", e))
            })?;
            println!("Wrote out.wasm ({} bytes)", wasm_bytes.len());
            return Ok(());
        }
        #[cfg(feature = "wasm-backend")]
        "wasm-run" => {
            let hir = nulang::hir_lower::lower_module(&ast);
            let mir = nulang::mir_lower::lower_module(&hir)?;
            let mut wasm_backend = nulang::mir_wasm::WasmBackend::new();
            let wasm_bytes = wasm_backend.compile(&mir, "main")?;
            if verbose {
                println!("=== WASM ({}) bytes ===", wasm_bytes.len());
            }
            // Write .wasm file for debugging, then run via Wasmtime.
            std::fs::write("out.wasm", &wasm_bytes).map_err(|e| {
                nulang::types::NuError::VMError(format!("failed to write out.wasm: {}", e))
            })?;
            let mut runtime = nulang::wasm_runtime::WasmRuntime::new(&wasm_bytes, None)?;
            runtime.run()?;
            return Ok(());
        }
        #[cfg(feature = "wasm-backend")]
        "wasm-aot" => {
            let hir = nulang::hir_lower::lower_module(&ast);
            let mir = nulang::mir_lower::lower_module(&hir)?;
            let mut wasm_backend = nulang::mir_wasm::WasmBackend::new();
            let wasm_bytes = wasm_backend.compile(&mir, "main")?;
            if verbose {
                println!("=== WASM ({}) bytes ===", wasm_bytes.len());
            }
            std::fs::write("out.wasm", &wasm_bytes).map_err(|e| {
                nulang::types::NuError::VMError(format!("failed to write out.wasm: {}", e))
            })?;
            println!("Wrote out.wasm ({} bytes)", wasm_bytes.len());
            nulang::wasm_runtime::aot_compile("out.wasm", "out.cwasm")?;
            println!("Wrote out.cwasm (precompiled)");
            return Ok(());
        }
        #[cfg(not(feature = "wasm-backend"))]
        "wasm" | "wasm-run" | "wasm-aot" => {
            return Err(nulang::types::NuError::VMError(
                "wasm backend not compiled in (enable 'wasm-backend' feature)".into(),
            ));
        }
        _ => {
            // Bytecode backend (default).
            let m = compile_with_new_pipeline(&ast, "main")?;
            if verbose {
                println!("=== Bytecode (HIR/MIR pipeline) ===");
                println!("{}", disassemble(&m));
            }
            let has_actors = ast.decls.iter().any(|d| {
                matches!(
                    d,
                    nulang::ast::Decl::Actor { .. } | nulang::ast::Decl::StateMachine { .. }
                )
            });
            let value = if has_actors {
                let (value, _runtime) = run_with_runtime(m)?;
                value
            } else {
                let mut vm = VM::new();
                vm.load_module(m);
                vm.run().map_err(|e| {
                    eprintln!("Runtime error: {}", e);
                    e
                })?
            };
            let result_str = value.to_string_repr();
            if result_str != "unit" {
                println!("{}", result_str);
            }
            Ok(())
        }
    }
}

/// Execute a module that declares actors against a real `Runtime`.
///
/// The top-level code runs on a VM with runtime-backed callbacks (so
/// `spawn` creates real actors and `send` enqueues real messages — the
/// same wiring the integration tests use), then the scheduler runs until
/// the run queue drains. Returns the top-level value and the runtime so
/// tests can inspect post-scheduling state.
fn run_with_runtime(
    m: nulang::bytecode::CodeModule,
) -> NuResult<(
    nulang::vm::Value,
    std::rc::Rc<std::cell::RefCell<nulang::runtime::Runtime>>,
)> {
    let runtime =
        std::rc::Rc::new(std::cell::RefCell::new(nulang::runtime::Runtime::new()));
    let mut vm = VM::new();
    vm.load_module(m);
    vm.set_actor_callbacks(Box::new(nulang::runtime::RuntimeVmCallbacks::new(
        runtime.clone(),
    )));
    let value = vm.run().map_err(|e| {
        eprintln!("Runtime error: {}", e);
        e
    })?;
    runtime.borrow_mut().run_scheduler();
    Ok((value, runtime))
}

fn check_source(source: &str, verbose: bool) -> NuResult<()> {
    run_frontend(source, verbose)?;

    if verbose {
        println!("Effect check passed.");
        println!("Capability analysis passed.");
    }

    Ok(())
}

fn compile_with_new_pipeline(
    ast: &nulang::ast::AstModule,
    name: &str,
) -> NuResult<nulang::bytecode::CodeModule> {
    // Anything this pipeline can't yet lower faithfully (see hir_lower.rs
    // and mir_lower.rs module docs) returns an honest NotYetImplemented
    // error, which the caller turns into a loud fallback to the stable
    // compiler.
    let hir = nulang::hir_lower::lower_module(ast);
    let mir = nulang::mir_lower::lower_module(&hir)?;
    nulang::mir_codegen::compile_mir(&mir, name)
}

fn type_to_string(ty: &Type) -> String {
    match ty {
        Type::Var(v) => format!("'t{}", v.0),
        Type::Primitive(p) => match p {
            nulang::types::PrimitiveType::Int => "Int".to_string(),
            nulang::types::PrimitiveType::Float => "Float".to_string(),
            nulang::types::PrimitiveType::Bool => "Bool".to_string(),
            nulang::types::PrimitiveType::String => "String".to_string(),
            nulang::types::PrimitiveType::Unit => "Unit".to_string(),
            nulang::types::PrimitiveType::Nil => "Nil".to_string(),
            nulang::types::PrimitiveType::Never => "Never".to_string(),
            nulang::types::PrimitiveType::Address => "Address".to_string(),
        },
        Type::Tuple(ts) => format!(
            "({})",
            ts.iter().map(type_to_string).collect::<Vec<_>>().join(", ")
        ),
        Type::Record(fs) => format!(
            "{{ {} }}",
            fs.iter()
                .map(|(n, t)| format!("{}: {}", n, type_to_string(t)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Type::Variant(vs) => vs
            .iter()
            .map(|(n, t)| match t {
                Some(t) => format!("{} {}", n, type_to_string(t)),
                None => n.clone(),
            })
            .collect::<Vec<_>>()
            .join(" | "),
        Type::Array(t) => format!("[{}]", type_to_string(t)),
        Type::Function { param, ret, .. } => {
            format!("{} -> {}", type_to_string(param), type_to_string(ret))
        }
        Type::Actor { state, behavior } => format!(
            "Actor[{}, {}]",
            type_to_string(state),
            type_to_string(behavior)
        ),
        Type::App { constructor, args } => format!(
            "{}[{}]",
            type_to_string(constructor),
            args.iter()
                .map(type_to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Type::Reference { cap, inner } => format!("&{:?} {}", cap, type_to_string(inner)),
        Type::Scheme { vars, body } => format!(
            "forall {}. {}",
            vars.iter()
                .map(|v| format!("'t{}", v.0))
                .collect::<Vec<_>>()
                .join(", "),
            type_to_string(body)
        ),
    }
}

fn disassemble(module: &nulang::bytecode::CodeModule) -> String {
    use std::fmt::Write;
    let mut output = String::new();
    if !module.constants.is_empty() {
        writeln!(output, "Constants:").unwrap();
        for (i, c) in module.constants.iter().enumerate() {
            writeln!(output, "  {}: {:?}", i, c).unwrap();
        }
        writeln!(output).unwrap();
    }
    writeln!(output, "Instructions:").unwrap();
    for (i, instr) in module.instructions.iter().enumerate() {
        let op_name = format!("{:?}", instr.opcode);
        let comment = match instr.opcode {
            nulang::bytecode::OpCode::ConstU => module
                .constants
                .get(instr.imm16() as usize)
                .map(|c| format!("; load {:?}", c)),
            nulang::bytecode::OpCode::Call => Some(format!("; call R{}", instr.op1)),
            nulang::bytecode::OpCode::Closure => Some(format!("; closure @{}", instr.imm16())),
            nulang::bytecode::OpCode::Jmp
            | nulang::bytecode::OpCode::JmpT
            | nulang::bytecode::OpCode::JmpF => {
                Some(format!("; -> {}", i as i64 + instr.simm16() as i64))
            }
            _ => None,
        };
        match comment {
            Some(c) => writeln!(
                output,
                "  {:4}: {:12} {:3} {:3} {:3}    {}",
                i, op_name, instr.op1, instr.op2, instr.op3, c
            ),
            None => writeln!(
                output,
                "  {:4}: {:12} {:3} {:3} {:3}",
                i, op_name, instr.op1, instr.op2, instr.op3
            ),
        }
        .unwrap();
    }
    if !module.function_table.is_empty() {
        writeln!(output).unwrap();
        writeln!(output, "Function Table:").unwrap();
        for (i, offset) in module.function_table.iter().enumerate() {
            writeln!(output, "  {}: @{}", i, offset).unwrap();
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An actor program run through the CLI path must create real actors
    /// and deliver sent messages: with the bare standalone VM the stub
    /// spawn/send callbacks would leave the counter at 0.
    #[test]
    fn test_run_source_actor_program_schedules_and_delivers() {
        let source = r#"
            actor Counter {
                state count: Int = 0
                behavior inc() { self.count = self.count + 1 }
            }
            let c = spawn Counter {} in {
                send c inc()
                send c inc()
                c
            }
        "#;
        let ast = run_frontend(source, false).expect("frontend should accept the actor program");
        let module = compile_with_new_pipeline(&ast, "test").expect("actor program should compile");
        let (_value, runtime) = run_with_runtime(module).expect("actor program should run");
        let rt = runtime.borrow();
        let actor = rt.actors.values().next().expect("one actor should exist");
        assert_eq!(
            actor.get_state_field("count").and_then(|v| v.as_int()),
            Some(2),
            "both inc messages must be delivered by run_scheduler"
        );
    }
}
