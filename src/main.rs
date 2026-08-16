//! Nulang CLI entry point.
//!
//! Usage:
//!   nulang [OPTIONS] <FILE>
//!   nulang --repl
//!   nulang --eval <CODE>
//!   nulang --check <FILE>
//!   nulang --lsp
//!   nulang --dap [FILE]
//!   nulang nula <new|build|build-wasm|test|run|add|remove|publish|deploy|watch|doc>
//!   nulang fmt [--check] [<file>]
//!
//! Options:
//!   -r, --repl               Start interactive REPL
//!   -e, --eval <CODE>        Evaluate a code string
//!   -c, --check <FILE>       Type-check a file (don't run)
//!   --doc                    Generate Markdown API docs (docs/api.md)
//!   --emit-stdlib-docs <dir> Generate per-effect stdlib docs into <dir>
//!   --lsp                    Start Language Server (stdio)
//!   --dap                    Start Debug Adapter (stdio); program from launch request or FILE
//!   --backend <b>            Backend: bytecode (default, full language) | native
//!                            (pure-functional subset only — effects/actors/FFI
//!                            error with a specific unsupported-construct message)
//!                            | wasm* (IO.print/read only; no user-defined effect
//!                            handlers, no actor mailbox — requires wasm-backend)
//!   --out <file>             Output file (WASM backends / --emit-nbc)
//!   --emit-nbc               Compile <FILE> to a .nbc artifact; don't run
//!   <FILE>.nbc               Run a pre-compiled .nbc artifact directly
//!   --verify <src>           Verify .nbc source hash against <src>
//!   nula <cmd>               Package manager (new, init, build, build-wasm, test, run, add, remove, publish, deploy, list, clean)
//!   --version, -V            Print version and exit
//!   -v, --verbose            Show bytecode and AST
//!   --bench [N]             Benchmark: run N times (default 10), print min/mean/median/max
//!   --color auto|always|never  Colorize error output (default: auto)
//!   -h, --help               Show this help message
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

const VERSION: &str = "0.1.0";
use nulang::effect_checker::{CapContext, CapabilityAnalyzer, EffectChecker};
use nulang::lexer::Lexer;
use nulang::parser::Parser;
use nulang::repl::Repl;
use nulang::stdlib::StdLib;
use nulang::typechecker::TypeChecker;
use nulang::types::{NuError, NuResult, Span, Type};
use nulang::vm::VM;
use std::io::IsTerminal;
use std::io::Read;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::time::Instant;
use tracing::instrument;
fn main() {
    // Initialize structured tracing (RUST_LOG env var controls verbosity).
    // Default level: warn (silent for normal runs). Users opt in with
    // RUST_LOG=nulang=debug or RUST_LOG=info.
    #[cfg(feature = "otel")]
    {
        // Forward spans to both the terminal and OTLP (when a tracer
        // provider has been configured). Fall back to terminal-only logging
        // if the subscriber cannot be installed.
        match nulang::observability::init_tracing("nulang-runtime") {
            Ok(()) => {}
            Err(e) => {
                eprintln!("OTLP tracing init failed ({e}); terminal logging only");
                use tracing_subscriber::{fmt, EnvFilter};
                let env_filter =
                    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
                fmt().with_env_filter(env_filter).with_target(false).init();
            }
        }
    }
    #[cfg(not(feature = "otel"))]
    {
        use tracing_subscriber::{fmt, EnvFilter};
        let env_filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
        fmt().with_env_filter(env_filter).with_target(false).init();
    }

    let args: Vec<String> = std::env::args().collect();

    if args.len() <= 1 {
        // If stdin is piped, execute as a script; otherwise start REPL.
        if !std::io::stdin().is_terminal() {
            let mut source = String::new();
            std::io::stdin()
                .read_to_string(&mut source)
                .expect("Failed to read stdin");
            let opts = Options::default();
            let use_color = color_enabled(&opts);
            if let Err(e) = run_source(
                &source,
                None,
                opts.verbose,
                &opts.backend,
                opts.out_file.as_deref(),
                opts.metrics_port,
                &opts.target, &opts.with_capabilities,
            ) {
                print_error(&e, use_color);
                std::process::exit(exit_code(&e));
            }
            return;
        }
        let mut repl = Repl::new();
        repl.run();
        return;
    }

    // `nulang registry serve` — start a package registry server.
    if args.len() >= 3 && args[1] == "registry" && args[2] == "serve" {
        let mut bind = "127.0.0.1:8087".to_string();
        let mut data_dir = ".nula-registry".to_string();
        let mut auth_token: Option<String> = None;
        let mut i = 3;
        while i < args.len() {
            match args[i].as_str() {
                "--bind" => {
                    i += 1;
                    if i < args.len() {
                        bind = args[i].clone();
                    }
                }
                "--dir" => {
                    i += 1;
                    if i < args.len() {
                        data_dir = args[i].clone();
                    }
                }
                "--token" => {
                    i += 1;
                    if i < args.len() {
                        auth_token = Some(args[i].clone());
                    }
                }
                other => {
                    eprintln!("Unknown registry serve option: {}", other);
                    std::process::exit(1);
                }
            }
            i += 1;
        }
        let server =
            nulang::registry::RegistryServer::new(std::path::PathBuf::from(&data_dir), auth_token);
        eprintln!("Registry listening on {} (data: {})", bind, data_dir);
        if let Err(e) = server.start(&bind) {
            eprintln!("Registry server error: {}", e);
            std::process::exit(1);
        }
        // Run until interrupted
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    // `nulang nula <cmd>` dispatches to the package manager.
    if args[1] == "fmt" {
        let mut check_mode = false;
        let mut file_arg: Option<&str> = None;
        let mut i = 2;
        while i < args.len() {
            if args[i] == "--check" {
                check_mode = true;
            } else if !args[i].starts_with('-') {
                file_arg = Some(&args[i]);
            } else {
                eprintln!("Unknown fmt option: {}", args[i]);
                std::process::exit(1);
            }
            i += 1;
        }

        if let Some(p) = file_arg {
            let s = std::fs::read_to_string(p).unwrap_or_else(|e| {
                eprintln!("Cannot read '{}': {}", p, e);
                std::process::exit(1);
            });
            match nulang::fmt::format_source(&s) {
                Ok(f) => {
                    if check_mode {
                        if f != s {
                            eprintln!("Would reformat {}", p);
                            std::process::exit(1);
                        }
                    } else {
                        if f != s {
                            std::fs::write(p, &f).unwrap_or_else(|e| {
                                eprintln!("Cannot write '{}': {}", p, e);
                                std::process::exit(1);
                            });
                            println!("Formatted {}", p);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("{}: {}", p, e);
                    std::process::exit(1);
                }
            }
        } else {
            let dir = std::path::Path::new("src");
            if !dir.is_dir() {
                eprintln!("Not a package directory (no src/)");
                std::process::exit(1);
            }
            if let Err(e) = nulang::fmt::format_directory(dir, check_mode) {
                eprintln!("{}", e);
                std::process::exit(exit_code(&e));
            }
        }
        return;
    }
    // `nulang node --listen <ADDR> [--seed <ADDR>] ...` — run a distributed
    // actor node (shard 0, network-enabled).
    if args[1] == "node" {
        if let Err(e) = run_node_cmd(&args[2..]) {
            print_error(&e, true);
            std::process::exit(exit_code(&e));
        }
        return;
    }

    if args[1] == "nula" {
        if let Err(e) = nulang::package::commands::run(&args[2..]) {
            print_error(&e, true);
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
                println!("nulang {}", VERSION);
                println!(
                    "language {}",
                    nulang::format::constants::LANGUAGE_VERSION_STR
                );
                return;
            }
            "--language-version" => {
                println!("{}", nulang::format::constants::LANGUAGE_VERSION_STR);
                return;
            }
            "--lsp" => opts.lsp = true,
            "--dap" => opts.dap = true,
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
            "--target" => {
                if i + 1 < args.len() {
                    opts.target = args[i + 1].clone();
                    i += 1;
                } else {
                    eprintln!("Error: --target requires an argument (native | ptx | riscv64)");
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
            "--ffi-sandbox" => opts.ffi_sandbox = true,
            "--ffi-allow" => {
                if i + 1 < args.len() {
                    opts.ffi_allow.push(args[i + 1].clone());
                    i += 1;
                } else {
                    eprintln!("Error: --ffi-allow requires a library name or path argument");
                    std::process::exit(1);
                }
            }
            "--with" => {
                if i + 1 < args.len() {
                    for cap in args[i + 1].split(',') {
                        let cap = cap.trim();
                        if !cap.is_empty() {
                            opts.with_capabilities.push(cap.to_string());
                        }
                    }
                    i += 1;
                } else {
                    eprintln!("Error: --with requires a comma-separated capability list (fs,net,os)");
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
            "init" => {
                if i + 1 < args.len() {
                    opts.init = Some(args[i + 1].clone());
                    i += 1;
                } else {
                    eprintln!("init requires a name");
                    std::process::exit(1);
                }
            }
            "--watch" => {
                if i + 1 < args.len() {
                    opts.watch = Some(args[i + 1].clone());
                    i += 1;
                } else {
                    eprintln!("--watch requires a file");
                    std::process::exit(1);
                }
            }
            "--explain" => {
                if i + 1 < args.len() {
                    opts.explain = Some(args[i + 1].clone());
                    i += 1;
                } else {
                    eprintln!("--explain requires a code");
                    std::process::exit(1);
                }
            }
            "-v" | "--verbose" => opts.verbose = true,
            "--all-errors" => opts.all_errors = true,
            "--metrics-port" => {
                if i + 1 < args.len() {
                    match args[i + 1].parse::<u16>() {
                        Ok(port) => opts.metrics_port = Some(port),
                        Err(_) => {
                            eprintln!("Error: --metrics-port requires a valid port number");
                            std::process::exit(1);
                        }
                    }
                } else {
                    eprintln!("Error: --metrics-port requires a port number");
                    std::process::exit(1);
                }
            }
            "--color" => {
                if i + 1 < args.len() {
                    let val = args[i + 1].clone();
                    if val != "auto" && val != "always" && val != "never" {
                        eprintln!(
                            "Error: --color must be 'auto', 'always', or 'never', got '{}'",
                            val
                        );
                        std::process::exit(1);
                    }
                    opts.color = val;
                    i += 1;
                } else {
                    eprintln!("Error: --color requires an argument (auto|always|never)");
                    std::process::exit(1);
                }
            }
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
            "--bench" => {
                opts.bench_count = Some(10); // default
                if i + 1 < args.len() {
                    if let Ok(n) = args[i + 1].parse::<usize>() {
                        if n > 0 {
                            opts.bench_count = Some(n);
                            i += 1;
                        }
                    }
                }
            }
            "-h" | "--help" => {
                print_help();
                return;
            }
            arg if arg.starts_with('-') => {
                let known: &[&str] = &[
                    "--repl",
                    "--eval",
                    "--check",
                    "--lsp",
                    "--dap",
                    "--doc",
                    "--backend",
                    "--out",
                    "--emit-nbc",
                    "--verify",
                    "--bench",
                    "--version",
                    "--verbose",
                    "--color",
                    "--help",
                    "--emit-stdlib-docs",
                    "-r",
                    "-e",
                    "-c",
                    "-V",
                    "-v",
                    "-h",
                ];
                let suggestion = known
                    .iter()
                    .min_by_key(|k| levenshtein_distance(arg, k))
                    .filter(|k| levenshtein_distance(arg, k) <= 3);
                eprint!("Error: Unknown option: {}", arg);
                if let Some(sug) = suggestion {
                    eprint!(". Did you mean '{}'?
