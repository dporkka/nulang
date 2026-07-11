//! Read-Eval-Print Loop for Nulang.
//!
//! Full-featured REPL with:
//! - Persistent type context across evaluations
//! - Multi-line input support
//! - `:commands` for introspection
//! - Graceful error handling

use crate::ast::{AstModule, Decl, Expr};
use crate::lexer::Lexer;
use crate::effect_checker::{CapContext, CapabilityAnalyzer, EffectChecker, EffectContext};
use crate::parser::Parser;
use crate::typechecker::TypeChecker;
use crate::types::{Capability, NuError, NuResult, Span, Type, TypeContext};
use crate::vm::{Value, VM};

/// REPL state that persists across evaluations.
pub struct Repl {
    vm: VM,
    /// Accumulated declarations from previous inputs (functions, actors, etc.)
    accumulated_decls: Vec<Decl>,
    /// Persistent type context across evaluations
    type_ctx: TypeContext,
    /// Fresh type checker (can be reused)
    type_checker: TypeChecker,
    /// Last compiled bytecode module (for :bytecode command)
    last_bytecode: Option<String>,
    /// Last AST (for :ast command display)
    last_ast: Option<AstModule>,
}

impl Repl {
    pub fn new() -> Self {
        Repl {
            vm: VM::new(),
            accumulated_decls: Vec::new(),
            type_ctx: TypeContext::new(),
            type_checker: TypeChecker::new(),
            last_bytecode: None,
            last_ast: None,
        }
    }

    /// Run the interactive REPL loop.
    pub fn run(&mut self) {
        println!("Nulang v0.1.0 \u{2014} Actor-Based Distributed Language");
        println!("Type :help for commands, :quit to exit\n");

        let mut buffer = String::new();
        let mut depth: i32 = 0; // Brace/paren/bracket nesting depth

        loop {
            let prompt = if depth > 0 { "... " } else { "nulang> " };
            print!("{}", prompt);
            let _ = std::io::Write::flush(&mut std::io::stdout());

            let mut line = String::new();
            match std::io::stdin().read_line(&mut line) {
                Ok(0) => break, // EOF
                Ok(_) => {}
                Err(e) => {
                    eprintln!("Error reading input: {}", e);
                    continue;
                }
            }

            let trimmed = line.trim();

            // REPL commands (only when not in multi-line mode)
            if depth == 0 && trimmed.starts_with(':') {
                // Extract command and rest
                let mut parts = trimmed[1..].splitn(2, ' ');
                let cmd = parts.next().unwrap_or("");
                let rest = parts.next().unwrap_or("").trim();

                match cmd {
                    "quit" | "q" => {
                        println!("Goodbye!");
                        break;
                    }
                    "help" | "h" => self.print_help(),
                    "type" => {
                        if rest.is_empty() {
                            eprintln!("Usage: :type <expression>");
                        } else if let Err(e) = self.show_type(rest) {
                            self.print_error(&e);
                        }
                    }
                    "ast" => {
                        if rest.is_empty() {
                            eprintln!("Usage: :ast <expression>");
                        } else if let Err(e) = self.show_ast(rest) {
                            self.print_error(&e);
                        }
                    }
                    "bytecode" | "bc" => self.show_bytecode(),
                    "clear" => {
                        print!("\x1B[2J\x1B[1;1H"); // ANSI clear screen
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                    }
                    "reset" => {
                        self.accumulated_decls.clear();
                        self.type_ctx = TypeContext::new();
                        self.type_checker = TypeChecker::new();
                        self.last_bytecode = None;
                        self.last_ast = None;
                        println!("Environment reset.");
                    }
                    "version" | "ver" => {
                        println!("nulang v{}", env!("CARGO_PKG_VERSION"));
                    }
                    unknown => {
                        println!("Unknown command: :{}. Type :help for help.", unknown);
                    }
                }
                continue;
            }

            buffer.push_str(&line);

            // Track brace depth for multi-line input
            for c in line.chars() {
                match c {
                    '{' | '(' | '[' => depth += 1,
                    '}' | ')' | ']' => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }

            if depth > 0 {
                continue; // Wait for more input
            }

            // Execute buffered input
            let input = buffer.trim();
            if !input.is_empty() {
                if let Err(e) = self.evaluate(input) {
                    self.print_error(&e);
                }
            }
            buffer.clear();
        }

        println!();
    }

    /// Evaluate a source string, showing value and type.
    fn evaluate(&mut self, source: &str) -> NuResult<()> {
        // Parse
        let ast = parse_source(source)?;
        self.last_ast = Some(ast.clone());

        // Separate declarations from the __main expression
        let mut new_decls = Vec::new();
        let mut main_expr: Option<Expr> = None;

        for decl in &ast.decls {
            if let Decl::Function { name, .. } = decl {
                if name == "__main" {
                    // Extract the body expression of __main
                    if let Decl::Function { body, .. } = decl {
                        main_expr = Some(body.clone());
                    }
                    continue;
                }
            }
            new_decls.push(decl.clone());
        }

        // Build combined module: accumulated + new declarations + __main if present
        let mut combined_decls = self.accumulated_decls.clone();
        combined_decls.extend(new_decls.clone());

        if let Some(ref expr) = main_expr {
            combined_decls.push(Decl::Function {
                name: "__main".to_string(),
                type_params: vec![],
                params: vec![],
                ret_type: None,
                effect: None,
                cap: None,
                body: expr.clone(),
                annotations: vec![],
                public: false,
                span: Span::default(),
            });
        }

        let combined_module = AstModule {
            name: "repl".to_string(),
            decls: combined_decls,
        };

        // Type check the combined module
        let module_type = self.type_checker.check_module(&combined_module)?;

        // Effect check
        let mut effect_checker = EffectChecker::new();
        let effect_ctx = EffectContext::empty();
        if let Some(ref expr) = main_expr {
            let _effects = effect_checker.infer_effects(&effect_ctx, expr)?;
        }
        for decl in &new_decls {
            // Check effects on each new declaration
            if let Decl::Function { body, .. } = decl {
                let _effects = effect_checker.infer_effects(&effect_ctx, body)?;
            }
        }

        // Capability analysis
        let mut cap_analyzer = CapabilityAnalyzer::new();
        let cap_ctx = CapContext::new();
        if let Some(ref expr) = main_expr {
            let _cap = cap_analyzer.infer_cap(&cap_ctx, expr)?;
        }

        // Compile the combined module via the HIR/MIR pipeline.
        let code_module = compile_with_new_pipeline(&combined_module, "repl")?;
        self.last_bytecode = Some(disassemble_module(&code_module));
        // from scratch (see `combined_module` above), so no closure created
        // by a previous evaluation can still be reachable — safe to reclaim
        // their capture environments before this run instead of leaking them
        // for the life of the REPL session.
        self.vm.clear_closure_envs();
        // Load and execute
        self.vm.load_module(code_module);
        let value = self.vm.run()?;

        // Print results
        if let Some(ref _expr) = main_expr {
            let val_str = value_to_pretty_string(&value);
            let ty_str = type_to_string(&module_type);
            println!("{} : {}", val_str, ty_str);
        } else if !new_decls.is_empty() {
            // Print declaration info
            for decl in &new_decls {
                if let Decl::Function { name, .. } = decl {
                    // Get the type from the accumulated context after this eval
                    let decl_ty = self.type_checker.check_module(&AstModule {
                        name: "repl".to_string(),
                        decls: vec![decl.clone()],
                    })?;
                    println!("{} : {}", name, type_to_string(&decl_ty));
                }
            }
        }

        // Update accumulated state with new declarations
        self.accumulated_decls.extend(new_decls);

        Ok(())
    }

    /// Show the inferred type of an expression (without executing).
    fn show_type(&mut self, source: &str) -> NuResult<()> {
        // Wrap in let ... in ... if needed to make it a valid module expression
        let wrapped = if !source.contains("let ") && !source.contains("fn ") {
            format!("{}", source)
        } else {
            source.to_string()
        };

        let ast = parse_source(&wrapped)?;

        // Extract the expression to type-check
        let expr = extract_main_expr(&ast)?;

        // Build combined module with accumulated decls + this expression
        let mut combined_decls = self.accumulated_decls.clone();
        combined_decls.push(Decl::Function {
            name: "__main".to_string(),
            type_params: vec![],
            params: vec![],
            ret_type: None,
            effect: None,
            cap: None,
            body: expr,
            annotations: vec![],
            public: false,
            span: Span::default(),
        });

        let module = AstModule {
            name: "typecheck".to_string(),
            decls: combined_decls,
        };

        let ty = self.type_checker.check_module(&module)?;
        println!("{}", type_to_string(&ty));
        Ok(())
    }

    /// Show the AST of an expression.
    fn show_ast(&mut self, source: &str) -> NuResult<()> {
        let ast = parse_source(source)?;
        let expr = extract_main_expr(&ast)?;
        println!("{:#?}", expr);
        Ok(())
    }

    /// Show bytecode for the last compiled expression.
    fn show_bytecode(&self) {
        match &self.last_bytecode {
            Some(bc) => println!("{}", bc),
            None => println!("No bytecode available. Evaluate an expression first."),
        }
    }

    fn print_help(&self) {
        println!("Commands:");
        println!("  :quit, :q        Exit the REPL");
        println!("  :help, :h        Show this help message");
        println!("  :type <expr>     Show the inferred type of an expression");
        println!("  :ast <expr>      Show the AST of an expression");
        println!("  :bytecode, :bc   Show bytecode for the last expression");
        println!("  :clear           Clear the screen");
        println!("  :reset           Reset the environment");
        println!("  :version, :ver   Print version and exit (repl keeps running)");
    }

    fn print_error(&self, err: &NuError) {
        eprintln!("Error: {}", err);
    }

    /// Execute source code without running the interactive loop.
    /// Used by the CLI for --eval mode.
    pub fn execute(&mut self, source: &str) -> NuResult<Value> {
        self.evaluate(source)?;
        Ok(Value::unit())
    }

    /// Number of closure capture environments currently retained by the
    /// REPL's VM. Exposed for testing that `clear_closure_envs` keeps this
    /// bounded across repeated evaluations instead of growing forever.
    #[cfg(test)]
    pub(crate) fn closure_env_count(&self) -> usize {
        self.vm.closure_env_count()
    }


    /// The last evaluation's disassembled bytecode, if any. Exposed for
    /// testing which compiler backend actually ran (the two backends use
    /// different register-allocation schemes, so their disassembly differs
    /// even for trivial programs).
    #[cfg(test)]
    pub(crate) fn last_bytecode(&self) -> Option<&str> {
        self.last_bytecode.as_deref()
    }
}

impl Default for Repl {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compile via the HIR/MIR pipeline.
fn compile_with_new_pipeline(ast: &AstModule, name: &str) -> NuResult<crate::bytecode::CodeModule> {
    let hir = crate::hir_lower::lower_module(ast);
    let mir = crate::mir_lower::lower_module(&hir)?;
    crate::mir_codegen::compile_mir(&mir, name)
}

/// Parse source code into an AST module.
fn parse_source(source: &str) -> NuResult<AstModule> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.lex()?;
    let mut parser = Parser::new(tokens);
    parser.parse_module()
}

/// Extract the body of the synthetic `__main` function, or return an error
/// if the module doesn't contain an expression.
fn extract_main_expr(ast: &AstModule) -> NuResult<Expr> {
    for decl in &ast.decls {
        if let Decl::Function { name, body, .. } = decl {
            if name == "__main" {
                return Ok(body.clone());
            }
        }
    }
    Err(NuError::ParseError {
        msg: "Expected an expression".to_string(),
        span: Span::default(),
    })
}

/// Convert a runtime Value to a pretty display string.
fn value_to_pretty_string(value: &Value) -> String {
    value.to_string_repr()
}

/// Convert a Type to a human-readable string.
pub fn type_to_string(ty: &Type) -> String {
    match ty {
        Type::Var(v) => format!("'t{}", v.0),
        Type::Primitive(p) => match p {
            crate::types::PrimitiveType::Int => "Int".to_string(),
            crate::types::PrimitiveType::Float => "Float".to_string(),
            crate::types::PrimitiveType::Bool => "Bool".to_string(),
            crate::types::PrimitiveType::String => "String".to_string(),
            crate::types::PrimitiveType::Unit => "Unit".to_string(),
            crate::types::PrimitiveType::Nil => "Nil".to_string(),
            crate::types::PrimitiveType::Never => "Never".to_string(),
            crate::types::PrimitiveType::Address => "Address".to_string(),
        },
        Type::Tuple(ts) => {
            let parts: Vec<String> = ts.iter().map(type_to_string).collect();
            format!("({})", parts.join(", "))
        }
        Type::Record(fs) => {
            let parts: Vec<String> = fs
                .iter()
                .map(|(n, t)| format!("{}: {}", n, type_to_string(t)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        Type::Variant(vs) => {
            let parts: Vec<String> = vs
                .iter()
                .map(|(n, t)| match t {
                    Some(t) => format!("{} {}", n, type_to_string(t)),
                    None => n.clone(),
                })
                .collect();
            format!("{}", parts.join(" | "))
        }
        Type::Array(t) => format!("[{}]", type_to_string(t)),
        Type::Function {
            param,
            ret,
            effect,
            cap,
        } => {
            let param_str = type_to_string(param);
            let ret_str = type_to_string(ret);
            let eff_str = if effect.effects().is_empty() {
                String::new()
            } else {
                format!(" !{:?}", effect)
            };
            let cap_str = if *cap == Capability::Ref {
                String::new()
            } else {
                format!(" :{:?}", cap)
            };
            format!(
                "{} -> {}{}{}",
                param_str, ret_str, eff_str, cap_str
            )
        }
        Type::Actor { state, behavior } => {
            format!(
                "Actor[{}, {}]",
                type_to_string(state),
                type_to_string(behavior)
            )
        }
        Type::App { constructor, args } => {
            let cstr = type_to_string(constructor);
            let args_str: Vec<String> = args.iter().map(type_to_string).collect();
            format!("{}[{}]", cstr, args_str.join(", "))
        }
        Type::Reference { cap, inner } => {
            format!("&{:?} {}", cap, type_to_string(inner))
        }
        Type::Scheme { vars, body } => {
            let var_names: Vec<String> = vars.iter().map(|v| format!("'t{}", v.0)).collect();
            format!(
                "forall {}. {}",
                var_names.join(", "),
                type_to_string(body)
            )
        }
    }
}

/// Disassemble a CodeModule into a human-readable string.
fn disassemble_module(module: &crate::bytecode::CodeModule) -> String {
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
            crate::bytecode::OpCode::ConstU => {
                let idx = instr.imm16();
                module.constants.get(idx as usize).map(|c| format!("; load {:?}", c))
            }
            crate::bytecode::OpCode::Call => Some(format!(
                "; call R{}",
                instr.op1
            )),
            crate::bytecode::OpCode::Closure => Some(format!(
                "; closure @{}",
                instr.imm16()
            )),
            crate::bytecode::OpCode::Jmp | crate::bytecode::OpCode::JmpT | crate::bytecode::OpCode::JmpF => {
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

    /// Regression test: each REPL evaluation recompiles and reruns the full
    /// accumulated program from scratch, so no closure from a previous
    /// evaluation can still be reachable. Without `clear_closure_envs` in
    /// `evaluate`, every capturing closure ever created in a REPL session
    /// would accumulate in the VM forever.
    #[test]
    fn test_repl_does_not_leak_closure_envs_across_evaluations() {
        let mut repl = Repl::new();
        for _ in 0..20 {
            repl.execute("let a = 40 in let add = fn(x) { x + a } in add(2)")
                .unwrap();
        }
        assert!(
            repl.closure_env_count() <= 1,
            "closure envs should not accumulate across REPL evaluations, got {}",
            repl.closure_env_count()
        );
    }

    /// The REPL compiles through the HIR/MIR pipeline.
    #[test]
    fn test_repl_uses_mir_pipeline() {
        let mut repl = Repl::new();
        repl.execute("1 + 2").unwrap();
        assert!(repl.last_bytecode().unwrap().contains("Function Table"));
    }
}
