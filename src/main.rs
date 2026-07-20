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
//!   nulang --emit-nbc <FILE>   Compile to a .nbc bytecode artifact (don't run)
//!   nulang <FILE>.nbc          Run a pre-compiled .nbc artifact directly
//!   nulang --verify <SRC> <FILE>.nbc   Verify source hash, then run
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use nulang::effect_checker::{CapContext, CapabilityAnalyzer, EffectChecker};
use nulang::lexer::Lexer;
use nulang::parser::Parser;
use nulang::repl::Repl;
use nulang::stdlib::StdLib;
use nulang::typechecker::TypeChecker;
use nulang::types::{NuError, NuResult, Type};
use nulang::vm::VM;
use std::path::PathBuf;

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
            std::process::exit(exit_code(&e));
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
                    eprintln!(
                        "Error: --backend requires an argument (bytecode | native{})",
                        if cfg!(feature = "wasm-backend") {
                            " | wasm | wasm-run | wasm-aot"
                        } else {
                            ""
                        }
                    );
                    std::process::exit(1);
                }
            }
            "--out" => {
                if i + 1 < args.len() {
                    opts.out_file = Some(args[i + 1].clone());
                    i += 1;
                } else {
                    eprintln!("Error: --out requires a file path argument");
                    std::process::exit(1);
                }
            }
            "--" => {
                // Everything after -- is a positional argument.
                for arg in args[i + 1..].iter() {
                    positional.push(arg.to_string());
                }
                break;
            }
            "--emit-stdlib-docs" => {
                if i + 1 < args.len() {
                    opts.emit_stdlib_docs = Some(args[i + 1].clone());
                    i += 1;
                } else {
                    eprintln!("Error: --emit-stdlib-docs requires a directory argument");
                    std::process::exit(1);
                }
            }
            "-v" | "--verbose" => opts.verbose = true,
            "--emit-nbc" => opts.emit_nbc = true,
            "--verify" => {
                if i + 1 < args.len() {
                    opts.verify_source = Some(args[i + 1].clone());
                    i += 1;
                } else {
                    eprintln!("Error: --verify requires a source file path argument");
                    std::process::exit(1);
                }
            }
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
                std::process::exit(exit_code(&e));
            }
        }
        return;
    }

    if let Some(dir) = opts.emit_stdlib_docs {
        match emit_stdlib_docs(&dir) {
            Ok(()) => println!("Stdlib docs written to {}", dir),
            Err(e) => {
                eprintln!("Error: {}", e);
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
        if opts.emit_nbc {
            let out = opts
                .out_file
                .clone()
                .unwrap_or_else(|| "out.nbc".to_string());
            if let Err(e) = compile_source_to_nbc(&code, &out) {
                print_error(&e);
                std::process::exit(exit_code(&e));
            }
            return;
        }
        if let Err(e) = run_source(&code, opts.verbose, &opts.backend, opts.out_file.as_deref()) {
            print_error(&e);
            std::process::exit(exit_code(&e));
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
            std::process::exit(exit_code(&e));
        }
        println!("Type check passed.");
        return;
    }

    // Run a source file, or a pre-compiled `.nbc` artifact.
    if !positional.is_empty() {
        let path = &positional[0];

        // A `.nbc` artifact: load and run directly without invoking the
        // compiler. This is the durable-distribution path — a `.nbc` minted
        // in 2026 runs on any conforming runtime in 2126.
        if path.ends_with(".nbc") {
            if let Err(e) = run_nbc_file(path, opts.verify_source.as_deref()) {
                print_error(&e);
                std::process::exit(exit_code(&e));
            }
            return;
        }

        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error: Cannot read file '{}': {}", path, e);
                std::process::exit(1);
            }
        };

        // `--emit-nbc`: compile to a `.nbc` artifact and write it, don't run.
        if opts.emit_nbc {
            let out = opts.out_file.clone().unwrap_or_else(|| {
                // foo.nula -> foo.nbc; anything else -> <path>.nbc
                if let Some(stem) = path.strip_suffix(".nula") {
                    format!("{stem}.nbc")
                } else {
                    format!("{path}.nbc")
                }
            });
            if let Err(e) = compile_source_to_nbc(&source, &out) {
                print_error(&e);
                std::process::exit(exit_code(&e));
            }
            return;
        }

        if let Err(e) = run_source(
            &source,
            opts.verbose,
            &opts.backend,
            opts.out_file.as_deref(),
        ) {
            print_error(&e);
            std::process::exit(exit_code(&e));
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
    out_file: Option<String>,
    /// Compile the input to a `.nbc` artifact and write it, don't run.
    emit_nbc: bool,
    /// When running a `.nbc` artifact, verify its recorded source hash against
    /// this source file before executing. Refuses on mismatch.
    verify_source: Option<String>,
    /// Output directory for --emit-stdlib-docs.
    emit_stdlib_docs: Option<String>,
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
            out_file: None,
            emit_nbc: false,
            verify_source: None,
            emit_stdlib_docs: None,
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
    println!("  --emit-stdlib-docs <dir>  Generate per-effect stdlib Markdown docs into <dir>");
    println!("  --lsp            Start Language Server (stdio)");
    print!("  --backend <b>    Backend: bytecode (default) | native");
    if cfg!(feature = "wasm-backend") {
        print!(" | wasm | wasm-run | wasm-aot");
    }
    println!();
    if cfg!(feature = "wasm-backend") {
        println!("  --out <file>     Output file for WASM backends (default: out.wasm)");
    }
    println!("  --emit-nbc       Compile <FILE> (or --eval <CODE>) to a .nbc artifact; don't run");
    println!("  --out <file>     Output path for --emit-nbc (default: <FILE> with .nbc extension)");
    println!("  <FILE>.nbc       Run a pre-compiled .nbc artifact directly (no compiler invoked)");
    println!(
        "  --verify <src>   When running a .nbc artifact, verify its source hash against <src>"
    );
    println!("  nula <cmd>       Package manager (new, build, test, run)");
    println!("  --version, -V    Print version and exit");
    println!("  -v, --verbose    Show bytecode and AST");
    println!("  -h, --help       Show this help message");
}

/// Generate per-effect stdlib Markdown docs into the given directory.
fn emit_stdlib_docs(dir: &str) -> Result<(), String> {
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::Write;

    let out_dir = PathBuf::from(dir);
    fs::create_dir_all(&out_dir)
        .map_err(|e| format!("Cannot create directory '{}': {}", dir, e))?;

    let stdlib = StdLib::new();
    let mut by_effect: BTreeMap<&str, Vec<&nulang::stdlib::BuiltinOp>> = BTreeMap::new();
    for op in stdlib.ops() {
        by_effect.entry(op.effect).or_default().push(op);
    }

    for (&effect_name, ops) in &by_effect {
        // Build a per-effect Starlight docs page.
        // These files are auto-generated — never edit them by hand.
        // Source of truth: `src/stdlib.rs` (the `StdLib::new()` registry).
        let mut page = String::new();
        page.push_str("---\n");
        page.push_str(&format!("title: \"{} Effect\"\n", effect_name));
        page.push_str(&format!(
            "description: \"Built-in {} effect operations (auto-generated from src/stdlib.rs)\"\n",
            effect_name
        ));
        page.push_str("sidebar:\n");
        page.push_str(&format!("  label: \"{}\"\n", effect_name));
        page.push_str("editUrl: false\n");
        page.push_str("---\n\n");
        page.push_str("> **This page is auto-generated from `src/stdlib.rs`.**\n");
        page.push_str("> Do not edit it by hand — your changes will be overwritten on the next CI run.\n");
        page.push_str("> To add or update a built-in operation, edit the `StdLib::new()` registry in `src/stdlib.rs`.\n\n");
        page.push_str(&format!("# {} Effect\n\n", effect_name));
        page.push_str(&format!(
            "The `{}` effect provides the following built-in operations, wired into the VM and runtime.\n\n",
            effect_name
        ));
        page.push_str("| Operation | Signature | Description |\n");
        page.push_str("|-----------|-----------|-------------|\n");
        for op in ops {
            page.push_str(&format!(
                "| `{}` | `{}` | {} |\n",
                op.name,
                op.signature.replace('|', "\\|"),
                op.description
            ));
        }
        page.push_str(&format!(
            "\n_Implementation site: {}_\n",
            match ops.first().map(|o| o.implemented_in) {
                Some(nulang::stdlib::ImplSite::StandaloneVm) => "Standalone VM",
                Some(nulang::stdlib::ImplSite::RuntimeHost) => "Runtime Host",
                None => "Unknown",
            }
        ));

        let filename = out_dir.join(format!("{}.md", effect_name.to_lowercase()));
        let mut file = fs::File::create(&filename)
            .map_err(|e| format!("Cannot create '{}': {}", filename.display(), e))?;
        file.write_all(page.as_bytes())
            .map_err(|e| format!("Cannot write '{}': {}", filename.display(), e))?;
    }
    Ok(())
}

fn print_error(err: &NuError) {
    eprintln!("Error: {}", err);
}

/// Map each error kind to a distinct exit code so tooling can
/// discriminate between syntax, type, runtime, and system errors.
fn exit_code(err: &NuError) -> i32 {
    match err {
        NuError::LexError { .. } => 2,
        NuError::ParseError { .. } => 3,
        NuError::TypeError { .. } => 4,
        NuError::EffectError { .. } => 5,
        NuError::CapError { .. } => 6,
        NuError::FFIError { .. } => 7,
        NuError::NotYetImplemented { .. } => 8,
        NuError::RuntimeError(_) => 9,
        NuError::VMError(_) => 10,
        NuError::PythonError(_) => 11,
        NuError::PackageError(_) => 12,
    }
}

/// Shared frontend: lex -> parse -> typecheck -> effect check -> capability
/// analysis. Returns the parsed module ready for compilation.
fn run_frontend(source: &str, verbose: bool) -> NuResult<nulang::ast::AstModule> {
    // 1. Lex
    let mut lexer = Lexer::new(source);
    let tokens = lexer.lex()?;

    // 2. Parse
    let mut parser = Parser::new(tokens);
    let ast = parser.parse_module()?;

    if verbose {
        println!("=== AST ===");
        println!("{:#?}", ast);
        println!();
    }

    // 3. Type check
    let mut type_checker = TypeChecker::new();
    let module_type = type_checker.check_module(&ast)?;

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
    effect_checker.register_function_rows(&flat_decls)?;
    for decl in &flat_decls {
        effect_checker.check_decl(decl)?;
    }

    // 5. Capability analysis over the same body set.
    let mut cap_analyzer = CapabilityAnalyzer::new();
    let cap_ctx = CapContext::new();
    let cap_body = |analyzer: &mut CapabilityAnalyzer, body: &nulang::ast::Expr| -> NuResult<()> {
        analyzer
            .infer_cap(&cap_ctx, body)
            .map(|_| ())
            .map_err(|e| e)
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

#[allow(unused_variables)]
fn run_source(source: &str, verbose: bool, backend: &str, out_file: Option<&str>) -> NuResult<()> {
    let ast = run_frontend(source, verbose)?;

    match backend {
        #[cfg(feature = "wasm-backend")]
        "wasm" => {
            let wasm_file = out_file.unwrap_or("out.wasm");
            let hir = nulang::hir_lower::lower_module(&ast);
            let mir = nulang::mir_lower::lower_module(&hir)?;
            let mut wasm_backend = nulang::mir_wasm::WasmBackend::new();
            let wasm_bytes = wasm_backend.compile(&mir, "main")?;
            if verbose {
                println!("=== WASM ({}) bytes ===", wasm_bytes.len());
            }
            std::fs::write(wasm_file, &wasm_bytes).map_err(|e| {
                nulang::types::NuError::VMError(format!("failed to write {}: {}", wasm_file, e))
            })?;
            println!("Wrote {} ({} bytes)", wasm_file, wasm_bytes.len());
            return Ok(());
        }
        #[cfg(feature = "wasm-backend")]
        "wasm-run" => {
            let wasm_file = out_file.unwrap_or("out.wasm");
            let hir = nulang::hir_lower::lower_module(&ast);
            let mir = nulang::mir_lower::lower_module(&hir)?;
            let mut wasm_backend = nulang::mir_wasm::WasmBackend::new();
            let wasm_bytes = wasm_backend.compile(&mir, "main")?;
            if verbose {
                println!("=== WASM ({}) bytes ===", wasm_bytes.len());
            }
            std::fs::write(wasm_file, &wasm_bytes).map_err(|e| {
                nulang::types::NuError::VMError(format!("failed to write {}: {}", wasm_file, e))
            })?;
            let mut runtime = nulang::wasm_runtime::WasmRuntime::new(&wasm_bytes, None)?;
            runtime.run()?;
            return Ok(());
        }
        #[cfg(feature = "wasm-backend")]
        "wasm-aot" => {
            let wasm_file = out_file.unwrap_or("out.wasm");
            let cwasm_file = wasm_file.replace(".wasm", ".cwasm");
            let cwasm_file = if cwasm_file == wasm_file {
                format!("{}.cwasm", wasm_file)
            } else {
                cwasm_file
            };
            let hir = nulang::hir_lower::lower_module(&ast);
            let mir = nulang::mir_lower::lower_module(&hir)?;
            let mut wasm_backend = nulang::mir_wasm::WasmBackend::new();
            let wasm_bytes = wasm_backend.compile(&mir, "main")?;
            if verbose {
                println!("=== WASM ({}) bytes ===", wasm_bytes.len());
            }
            std::fs::write(&wasm_file, &wasm_bytes).map_err(|e| {
                nulang::types::NuError::VMError(format!("failed to write {}: {}", wasm_file, e))
            })?;
            println!("Wrote {} ({} bytes)", wasm_file, wasm_bytes.len());
            nulang::wasm_runtime::aot_compile(&wasm_file, &cwasm_file)?;
            println!("Wrote {} (precompiled)", cwasm_file);
            return Ok(());
        }
        #[cfg(not(feature = "wasm-backend"))]
        "wasm" | "wasm-run" | "wasm-aot" => {
            return Err(nulang::types::NuError::VMError(
                "wasm backend not compiled in (enable 'wasm-backend' feature)".into(),
            ));
        }
        "native" => {
            let hir = nulang::hir_lower::lower_module(&ast);
            let mir = nulang::mir_lower::lower_module(&hir)?;
            if verbose {
                println!("=== AOT native compilation ===");
                for func in &mir.functions {
                    println!(
                        "  fn {} ({} locals, {} blocks)",
                        func.name,
                        func.locals.len(),
                        func.blocks.len()
                    );
                }
            }
            let aot_module = nulang::aot::AotModule::compile(&mir)?;
            let result_raw = aot_module.run()?;
            let result = nulang::vm::Value::from_raw(result_raw);
            let result_str = result.to_string_repr();
            if !result_str.is_empty() && result_str != "unit" && result_str != "()" {
                println!("{}", result_str);
            }
            return Ok(());
        }
        "bytecode" => {
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
                vm.run().map_err(|e| e)?
            };
            let result_str = value.to_string_repr();
            if !result_str.is_empty() && result_str != "unit" && result_str != "()" {
                println!("{}", result_str);
            }
            Ok(())
        }
        _ => {
            return Err(nulang::types::NuError::VMError(format!(
                "unknown backend '{}' (expected bytecode | native{})",
                backend,
                if cfg!(feature = "wasm-backend") {
                    " | wasm | wasm-run | wasm-aot"
                } else {
                    ""
                }
            )));
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
    let runtime = std::rc::Rc::new(std::cell::RefCell::new(nulang::runtime::Runtime::new()));
    let mut vm = VM::new();
    vm.load_module(m);
    vm.set_actor_callbacks(Box::new(nulang::runtime::RuntimeVmCallbacks::new(
        runtime.clone(),
    )));
    let value = vm.run().map_err(|e| e)?;
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

/// Compile a source string to a `.nbc` artifact and write it to `out_path`.
///
/// The BLAKE3 hash of the source is recorded in the artifact header so a later
/// `--verify` run can confirm the artifact came from this exact source
/// (supply-chain integrity). Does not execute the module.
fn compile_source_to_nbc(source: &str, out_path: &str) -> NuResult<()> {
    let ast = run_frontend(source, false)?;
    let m = compile_with_new_pipeline(&ast, "main")?;
    let source_hash = blake3::hash(source.as_bytes());
    let bytes = m
        .to_nbc(Some(*source_hash.as_bytes()))
        .map_err(|e| nulang::types::NuError::VMError(e.to_string()))?;
    std::fs::write(out_path, &bytes)
        .map_err(|e| nulang::types::NuError::VMError(format!("failed to write {out_path}: {e}")))?;
    println!(
        "Wrote {out_path} ({} bytes, .nbc format v{}, language v{})",
        bytes.len(),
        nulang::format::constants::BYTECODE_VERSION,
        nulang::format::constants::LANGUAGE_VERSION,
    );
    Ok(())
}

/// Load and run a `.nbc` artifact directly, optionally verifying its recorded
/// source hash against a source file. This is the durable-distribution path:
/// no compiler invocation, no source parse — just `from_nbc` + `VM::run`.
fn run_nbc_file(path: &str, verify_source: Option<&str>) -> NuResult<()> {
    let bytes = std::fs::read(path).map_err(|e| {
        nulang::types::NuError::VMError(format!("cannot read .nbc file '{path}': {e}"))
    })?;
    let artifact = nulang::bytecode::CodeModule::from_nbc(&bytes)
        .map_err(|e| nulang::types::NuError::VMError(e.to_string()))?;

    if let Some(src_path) = verify_source {
        let source = std::fs::read_to_string(src_path).map_err(|e| {
            nulang::types::NuError::VMError(format!("cannot read source '{src_path}': {e}"))
        })?;
        let computed = blake3::hash(source.as_bytes());
        match artifact.source_hash {
            Some(h) if h == *computed.as_bytes() => { /* verified */ }
            Some(h) => {
                return Err(nulang::types::NuError::VMError(format!(
                    "source hash mismatch: artifact recorded {} but source {src_path} hashes to {}",
                    hex::encode(h),
                    hex::encode(computed.as_bytes()),
                )));
            }
            None => {
                return Err(nulang::types::NuError::VMError(
                    "artifact carries no source hash; cannot verify".into(),
                ));
            }
        }
    }

    let mut vm = VM::new();
    vm.load_module(artifact.module);
    let value = vm.run().map_err(|e| e)?;
    let result_str = value.to_string_repr();
    if !result_str.is_empty() && result_str != "unit" && result_str != "()" {
        println!("{}", result_str);
    }
    Ok(())
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
