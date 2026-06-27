//! Nulang CLI entry point.
//!
//! Usage:
//!   nulang [OPTIONS] <FILE>
//!   nulang --repl
//!   nulang --eval <CODE>
//!   nulang --check <FILE>
//!   nulang --lsp       Start LSP server
//!
//! Options:
//!   -r, --repl       Start interactive REPL
//!   -e, --eval       Evaluate a code string
//!   -c, --check      Type-check a file (don't run)
//!   --lsp            Start Language Server (stdio)
//!   -v, --verbose    Show bytecode and AST
//!   -h, --help       Show this help message

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use nulang::compiler::Compiler;
use nulang::effect_checker::{CapContext, CapabilityAnalyzer, EffectChecker, EffectContext};
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
        // Default: start REPL
        let mut repl = Repl::new();
        repl.run();
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
            "--lsp" => opts.lsp = true,
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

    if opts.lsp {
        nulang::lsp::run_lsp_server().await;
        return;
    }

    if opts.repl {
        let mut repl = Repl::new();
        repl.run();
        return;
    }

    if let Some(code) = opts.eval_code {
        if let Err(e) = run_source(&code, opts.verbose) {
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
        if let Err(e) = run_source(&source, opts.verbose) {
            print_error(&e);
            std::process::exit(1);
        }
        return;
    }

    // No arguments and no options: start REPL
    let mut repl = Repl::new();
    repl.run();
}

#[derive(Default)]
struct Options {
    repl: bool,
    eval_code: Option<String>,
    check_file: Option<String>,
    lsp: bool,
    verbose: bool,
}

fn print_help() {
    println!("Usage: nulang [OPTIONS] <FILE>");
    println!("       nulang --repl");
    println!("       nulang --eval <CODE>");
    println!("       nulang --check <FILE>");
    println!("       nulang --lsp");
    println!();
    println!("Options:");
    println!("  -r, --repl       Start interactive REPL");
    println!("  -e, --eval       Evaluate a code string");
    println!("  -c, --check      Type-check a file (don't run)");
    println!("  --lsp            Start Language Server (stdio)");
    println!("  -v, --verbose    Show bytecode and AST");
    println!("  -h, --help       Show this help message");
}

fn print_error(err: &NuError) {
    eprintln!("Error: {}", err);
}

/// Full pipeline: parse -> typecheck -> effect check -> compile -> vm.run()
fn run_source(source: &str, verbose: bool) -> NuResult<()> {
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

    // 4. Effect check
    let mut effect_checker = EffectChecker::new();
    let effect_ctx = EffectContext::empty();
    for decl in &ast.decls {
        if let nulang::ast::Decl::Function { body, .. } = decl {
            effect_checker.infer_effects(&effect_ctx, body).map_err(|e| {
                eprintln!("Effect error: {}", e);
                e
            })?;
        }
    }

    // 5. Capability analysis
    let mut cap_analyzer = CapabilityAnalyzer::new();
    let cap_ctx = CapContext::new();
    for decl in &ast.decls {
        if let nulang::ast::Decl::Function { body, .. } = decl {
            cap_analyzer.infer_cap(&cap_ctx, body).map_err(|e| {
                eprintln!("Capability error: {}", e);
                e
            })?;
        }
    }

    // 6. Compile
    let mut compiler = Compiler::new("main");
    let code_module = compiler.compile_module(&ast)?.clone();

    if verbose {
        println!("=== Bytecode ===");
        println!("{}", disassemble(&code_module));
    }

    // 7. Execute
    let mut vm = VM::new();
    vm.load_module(code_module);
    let value = vm.run().map_err(|e| {
        eprintln!("Runtime error: {}", e);
        e
    })?;

    let result_str = value.to_string_repr();
    if result_str != "unit" {
        println!("{}", result_str);
    }

    Ok(())
}

fn check_source(source: &str, verbose: bool) -> NuResult<()> {
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

    // 4. Effect check
    let mut effect_checker = EffectChecker::new();
    let effect_ctx = EffectContext::empty();
    for decl in &ast.decls {
        if let nulang::ast::Decl::Function { body, .. } = decl {
            effect_checker.infer_effects(&effect_ctx, body).map_err(|e| {
                eprintln!("Effect error: {}", e);
                e
            })?;
        }
    }

    // 5. Capability analysis
    let mut cap_analyzer = CapabilityAnalyzer::new();
    let cap_ctx = CapContext::new();
    for decl in &ast.decls {
        if let nulang::ast::Decl::Function { body, .. } = decl {
            cap_analyzer.infer_cap(&cap_ctx, body).map_err(|e| {
                eprintln!("Capability error: {}", e);
                e
            })?;
        }
    }

    if verbose {
        println!("Effect check passed.");
        println!("Capability analysis passed.");
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
            nulang::types::PrimitiveType::Never => "Never".to_string(),
            nulang::types::PrimitiveType::Address => "Address".to_string(),
        },
        Type::Tuple(ts) => format!("({})", ts.iter().map(type_to_string).collect::<Vec<_>>().join(", ")),
        Type::Record(fs) => format!("{{ {} }}", fs.iter().map(|(n, t)| format!("{}: {}", n, type_to_string(t))).collect::<Vec<_>>().join(", ")),
        Type::Variant(vs) => vs.iter().map(|(n, t)| match t { Some(t) => format!("{} {}", n, type_to_string(t)), None => n.clone() }).collect::<Vec<_>>().join(" | "),
        Type::Array(t) => format!("[{}]", type_to_string(t)),
        Type::Function { param, ret, .. } => format!("{} -> {}", type_to_string(param), type_to_string(ret)),
        Type::Actor { state, behavior } => format!("Actor[{}, {}]", type_to_string(state), type_to_string(behavior)),
        Type::Agent { state, policy, memory, tools } => format!("Agent[{}, {}, {}, {}]", type_to_string(state), type_to_string(policy), type_to_string(memory), type_to_string(tools)),
        Type::App { constructor, args } => format!("{}[{}]", type_to_string(constructor), args.iter().map(type_to_string).collect::<Vec<_>>().join(", ")),
        Type::Reference { cap, inner } => format!("&{:?} {}", cap, type_to_string(inner)),
        Type::Scheme { vars, body } => format!("forall {}. {}", vars.iter().map(|v| format!("'t{}", v.0)).collect::<Vec<_>>().join(", "), type_to_string(body)),
    }
}

fn disassemble(module: &nulang::bytecode::CodeModule) -> String {
    use std::fmt::Write;
    let mut output = String::new();
    if !module.constants.is_empty() {
        writeln!(output, "Constants:").unwrap();
        for (i, c) in module.constants.iter().enumerate() { writeln!(output, "  {}: {:?}", i, c).unwrap(); }
        writeln!(output).unwrap();
    }
    writeln!(output, "Instructions:").unwrap();
    for (i, instr) in module.instructions.iter().enumerate() {
        let op_name = format!("{:?}", instr.opcode);
        let comment = match instr.opcode {
            nulang::bytecode::OpCode::ConstU => module.constants.get(instr.imm16() as usize).map(|c| format!("; load {:?}", c)),
            nulang::bytecode::OpCode::Call => Some(format!("; call R{}", instr.op1)),
            nulang::bytecode::OpCode::Closure => Some(format!("; closure @{}", instr.imm16())),
            nulang::bytecode::OpCode::Jmp | nulang::bytecode::OpCode::JmpT | nulang::bytecode::OpCode::JmpF => Some(format!("; -> {}", i as i64 + instr.simm16() as i64)),
            _ => None,
        };
        match comment {
            Some(c) => writeln!(output, "  {:4}: {:12} {:3} {:3} {:3}    {}", i, op_name, instr.op1, instr.op2, instr.op3, c),
            None => writeln!(output, "  {:4}: {:12} {:3} {:3} {:3}", i, op_name, instr.op1, instr.op2, instr.op3),
        }.unwrap();
    }
    if !module.function_table.is_empty() {
        writeln!(output).unwrap();
        writeln!(output, "Function Table:").unwrap();
        for (i, offset) in module.function_table.iter().enumerate() { writeln!(output, "  {}: @{}", i, offset).unwrap(); }
    }
    output
}
