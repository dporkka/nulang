//! Read-Eval-Print Loop for Nulang.

use crate::compiler::compile;
use crate::parser::parse;
use crate::vm::VM;

/// Start the interactive REPL.
pub fn run_repl() {
    println!("Nulang REPL v0.1.0");
    println!("Type :quit to exit, :help for commands");
    println!();

    let mut vm = VM::new();

    loop {
        print!("nulang> ");
        use std::io::Write;
        std::io::stdout().flush().unwrap();

        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                eprintln!("Error reading input: {}", e);
                continue;
            }
        }

        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Commands
        if line.starts_with(':') {
            match line {
                ":quit" | ":q" => break,
                ":help" | ":h" => {
                    println!("Commands:");
                    println!("  :quit, :q    Exit the REPL");
                    println!("  :help, :h    Show this help");
                    println!("  :ast         Show AST of last expression");
                    println!("  :bytecode    Show bytecode of last expression");
                }
                _ => println!("Unknown command: {}. Type :help for available commands.", line),
            }
            continue;
        }

        // Parse
        let ast = match parse(line) {
            Ok(ast) => ast,
            Err(e) => {
                eprintln!("Parse error: {}", e);
                continue;
            }
        };

        // Compile
        let module = compile(&ast);

        // Execute
        match vm.load_module(&module) {
            Ok(_) => {
                match vm.call_function("main", &[]) {
                    Ok(result) => println!("{:?}", result),
                    Err(e) => eprintln!("Runtime error: {}", e),
                }
            }
            Err(e) => eprintln!("Load error: {}", e),
        }
    }

    println!("Goodbye!");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repl_creation() {
        let _vm = VM::new();
    }
}
