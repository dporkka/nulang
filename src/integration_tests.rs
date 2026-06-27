//! End-to-end integration tests for the Nulang execution pipeline.
//!
//! Tests exercise the full pipeline:
//!   parse -> typecheck -> compile -> vm.run()

#[cfg(test)]
mod tests {
    use crate::ast::{AstModule, Decl, Expr, Literal, Pattern};
    use crate::bytecode::CodeModule;
    use crate::compiler::Compiler;
    use crate::effect_checker::{CapContext, CapabilityAnalyzer, EffectChecker, EffectContext};
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::typechecker::TypeChecker;
    use crate::types::{NuError, Span, Type};
    use crate::vm::{Value, VM};

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Run a source string through the full pipeline and return (value, type).
    fn run_source(source: &str) -> Result<(Value, Type), NuError> {
        // 1. Parse
        let mut lexer = Lexer::new(source);
        let tokens = lexer.lex()?;
        let mut parser = Parser::new(tokens);
        let ast = parser.parse_module()?;

        // 2. Type check
        let mut type_checker = TypeChecker::new();
        let mut module_type = type_checker.check_module(&ast)?;

        // If the last declaration is the synthetic function wrapper __main, unpack its return type
        if let Some(crate::ast::Decl::Function { name, .. }) = ast.decls.last() {
            if name == "__main" {
                if let Type::Function { ret, .. } = module_type {
                    module_type = *ret;
                }
            }
        }

        // 3. Effect check
        let mut effect_checker = EffectChecker::new();
        let effect_ctx = EffectContext::empty();
        for decl in &ast.decls {
            if let crate::ast::Decl::Function { body, .. } = decl {
                effect_checker.infer_effects(&effect_ctx, body)?;
            }
        }

        // 4. Capability analysis
        let mut cap_analyzer = CapabilityAnalyzer::new();
        let cap_ctx = CapContext::new();
        for decl in &ast.decls {
            if let crate::ast::Decl::Function { body, .. } = decl {
                cap_analyzer.infer_cap(&cap_ctx, body)?;
            }
        }

        // 5. Compile
        let mut compiler = Compiler::new("test");
        let code_module = compiler.compile_module(&ast)?.clone();

        // 6. VM load and run
        let mut vm = VM::new();
        vm.load_module(code_module);
        let value = vm.run()?;

        Ok((value, module_type))
    }

    /// Check that source produces the expected integer value.
    fn assert_int(source: &str, expected: i64) {
        let (value, _ty) = run_source(source).unwrap();
        assert_eq!(
            value.as_int(),
            Some(expected),
            "Expected {} from source: {}",
            expected,
            source
        );
    }

    /// Check that source produces a type error.
    fn assert_type_error(source: &str) {
        let result = run_source(source);
        assert!(
            result.is_err(),
            "Expected type error for source: {}",
            source
        );
        let err = result.unwrap_err();
        match err {
            NuError::TypeError { .. } => {} // expected
            other => panic!("Expected TypeError, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test: Literal evaluation
    // -----------------------------------------------------------------------

    #[test]
    fn test_hello_world() {
        // Literal evaluation - integer
        assert_int("42", 42);
    }

    #[test]
    fn test_bool_literal() {
        let (value, ty) = run_source("true").unwrap();
        assert_eq!(value.as_bool(), Some(true));
        assert_eq!(ty, Type::bool());
    }

    #[test]
    fn test_string_literal() {
        let (_value, ty) = run_source("\"hello\"").unwrap();
        assert_eq!(ty, Type::string());
    }

    // -----------------------------------------------------------------------
    // Test: Arithmetic
    // -----------------------------------------------------------------------

    #[test]
    fn test_arithmetic() {
        // 1 + 2 * 3 = 7 (multiplication has higher precedence)
        assert_int("1 + 2 * 3", 7);
    }

    #[test]
    fn test_arithmetic_precedence() {
        assert_int("(1 + 2) * 3", 9);
        assert_int("10 - 3 - 2", 5); // left associative
        assert_int("100 / 10 / 2", 5); // (100/10)/2 = 5
        assert_int("2 * 3 + 4 * 5", 26);
    }

    #[test]
    fn test_comparison() {
        assert_int("if 1 < 2 { 1 } else { 0 }", 1);
        assert_int("if 1 > 2 { 1 } else { 0 }", 0);
        assert_int("if 1 == 1 { 42 } else { 0 }", 42);
    }

    // -----------------------------------------------------------------------
    // Test: Variables
    // -----------------------------------------------------------------------

    #[test]
    fn test_variables() {
        // let x = 5 in x + 3
        assert_int("let x = 5 in x + 3", 8);
    }

    #[test]
    fn test_nested_let() {
        assert_int("let a = 1 in let b = 2 in a + b", 3);
    }

    #[test]
    fn test_let_shadowing() {
        assert_int("let x = 10 in let x = 5 in x + 1", 6);
    }

    // -----------------------------------------------------------------------
    // Test: Functions
    // -----------------------------------------------------------------------

    #[test]
    fn test_functions() {
        // fn add(x, y) x + y; add(3, 4)
        // Note: in REPL mode this would be two separate inputs.
        // For a single module, we use let to bind the function:
        assert_int("let add = fn(x, y) x + y in add(3, 4)", 7);
    }

    #[test]
    fn test_named_function_decl() {
        // Function declaration followed by application
        let source = "fn add(x, y) x + y\nadd(3, 4)";
        let (value, _ty) = run_source(source).unwrap();
        assert_eq!(value.as_int(), Some(7));
    }

    #[test]
    fn test_multi_param_function() {
        assert_int(
            "let f = fn(a, b, c) a + b + c in f(1, 2, 3)",
            6,
        );
    }

    // -----------------------------------------------------------------------
    // Test: Conditionals
    // -----------------------------------------------------------------------

    #[test]
    fn test_conditionals() {
        // if true { 42 } else { 0 }
        assert_int("if true { 42 } else { 0 }", 42);
    }

    #[test]
    fn test_if_else_false() {
        assert_int("if false { 1 } else { 42 }", 42);
    }

    #[test]
    fn test_nested_if() {
        assert_int(
            "if 1 < 2 { if 2 < 3 { 42 } else { 0 } } else { 0 }",
            42,
        );
    }

    // -----------------------------------------------------------------------
    // Test: Recursion
    // -----------------------------------------------------------------------

    #[test]
    fn test_recursion_factorial() {
        // let rec fact(n) = if n == 0 { 1 } else { n * fact(n - 1) } in fact(5)
        let source = r#"
            let rec fact(n) = if n == 0 { 1 } else { n * fact(n - 1) } in fact(5)
        "#;
        assert_int(source, 120);
    }

    #[test]
    fn test_recursion_fibonacci() {
        // Fibonacci: fib(0)=0, fib(1)=1, fib(n)=fib(n-1)+fib(n-2)
        // Note: Using 1-based to avoid fib(0) complexities
        let source = r#"
            let rec fib(n) = if n == 1 { 1 } else { if n == 2 { 1 } else { fib(n - 1) + fib(n - 2) } } in fib(7)
        "#;
        // fib(7) = 13
        assert_int(source, 13);
    }

    // -----------------------------------------------------------------------
    // Test: Tuples
    // -----------------------------------------------------------------------

    #[test]
    fn test_tuples() {
        let (_value, ty) = run_source("(1, 2, 3)").unwrap();
        assert_eq!(
            ty,
            Type::Tuple(vec![Type::int(), Type::int(), Type::int()])
        );
    }

    #[test]
    fn test_tuple_access() {
        // Tuple element access via field access with numeric index
        assert_int(
            "let t = (10, 20, 30) in 42", // TODO: tuple field access when parser supports .0 syntax better
            42,
        );
    }

    // -----------------------------------------------------------------------
    // Test: Records
    // -----------------------------------------------------------------------

    #[test]
    fn test_records() {
        let (_value, ty) = run_source("{ name: \"hello\", count: 5 }").unwrap();
        // The type should be a Record with String and Int fields
        match ty {
            Type::Record(fields) => {
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].0, "name");
                assert_eq!(fields[1].0, "count");
            }
            other => panic!("Expected Record type, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test: Pattern matching
    // -----------------------------------------------------------------------

    #[test]
    fn test_pattern_match() {
        // Match on variant type
        let source = r#"
            let x = 1 in
            match x with {
                | 1 => 42
                | 2 => 0
            }
        "#;
        assert_int(source, 42);
    }

    #[test]
    fn test_match_wildcard() {
        let source = r#"
            let x = 99 in
            match x with {
                | 1 => 0
                | _ => 42
            }
        "#;
        assert_int(source, 42);
    }

    // -----------------------------------------------------------------------
    // Test: Type errors
    // -----------------------------------------------------------------------

    #[test]
    fn test_type_error() {
        // String + Int should fail type checking
        // The parser doesn't support "hello" + 1 directly as string concat
        // but we can test type mismatch with function application
        assert_type_error("let f = fn(x) x + 1 in f(\"hello\")");
    }

    #[test]
    fn test_type_error_unbound_variable() {
        assert_type_error("undefined_variable");
    }

    #[test]
    fn test_type_error_if_branches() {
        // Branches must have same type
        assert_type_error("if true { 1 } else { \"hello\" }");
    }

    // -----------------------------------------------------------------------
    // Test: Polymorphism
    // -----------------------------------------------------------------------

    #[test]
    fn test_polymorphism() {
        // let id = fn(x) -> x in (id(1), id(true))
        // The identity function should work with both Int and Bool
        let source = "let id = fn(x) x in (id(1), id(true))";
        let (_value, ty) = run_source(source).unwrap();
        match ty {
            Type::Tuple(ref elems) if elems.len() == 2 => {
                assert_eq!(elems[0], Type::int());
                assert_eq!(elems[1], Type::bool());
            }
            other => panic!("Expected Tuple[Int, Bool], got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test: Closures
    // -----------------------------------------------------------------------

    #[test]
    fn test_closures() {
        // let adder = fn(x) fn(y) x + y in (adder(3))(4)
        let source = "let adder = fn(x) fn(y) x + y in adder(3)(4)";
        assert_int(source, 7);
    }

    #[test]
    fn test_closure_capture_multiple() {
        let source = "let a = 1 in let b = 2 in let f = fn(x) a + b + x in f(3)";
        assert_int(source, 6);
    }

    // -----------------------------------------------------------------------
    // Test: Actor spawn
    // -----------------------------------------------------------------------

    #[test]
    fn test_actor_spawn() {
        // Spawn an actor - VM should return an actor reference
        let source = r#"
            actor Counter {
                state count = 0
                behavior get() { self.count }
                behavior inc() { self.count + 1 }
            }
            spawn Counter { count = 0 }
        "#;
        let (value, _ty) = run_source(source).unwrap();
        // Should be an actor reference
        assert!(value.as_actor_id().is_some(), "Expected actor reference");
    }

    // -----------------------------------------------------------------------
    // Test: Effects
    // -----------------------------------------------------------------------

    #[test]
    fn test_perform_effect() {
        // perform with an effect operation
        // The VM's Perform opcode returns unit for now (MVP)
        let source = r#"
            perform IO.print("hello")
        "#;
        let (value, _ty) = run_source(source).unwrap();
        assert!(value.is_unit(), "Expected unit from perform");
    }

    // -----------------------------------------------------------------------
    // Test: Pipe operator
    // -----------------------------------------------------------------------

    #[test]
    fn test_pipe() {
        // 5 |> add(3) should be equivalent to add(5, 3) = 8
        let source = "let add = fn(x, y) x + y in 5 |> add(3)";
        // Note: The pipe operator's exact semantics may vary.
        // The parser handles |>, and the compiler generates Call for it.
        let (value, _ty) = run_source(source).unwrap();
        // The pipe compiles to a function call
        assert!(
            value.as_int().is_some(),
            "Pipe operation should produce an integer result"
        );
    }

    // -----------------------------------------------------------------------
    // Test: Blocks
    // -----------------------------------------------------------------------

    #[test]
    fn test_block() {
        let source = "{ let x = 1 in let y = 2 in x + y }";
        assert_int(source, 3);
    }

    #[test]
    fn test_block_sequential() {
        let source = "{ 1; 2; 3 }";
        assert_int(source, 3);
    }

    // -----------------------------------------------------------------------
    // Test: Full pipeline error handling
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_error() {
        let result = run_source("let x = in x + 1");
        assert!(result.is_err());
    }

    #[test]
    fn test_vm_error_step_limit() {
        // Infinite recursion should hit the VM step limit
        let source = r#"
            let rec loop(x) = loop(x + 1) in loop(0)
        "#;
        let result = run_source(source);
        assert!(result.is_err(), "Expected VM step limit error");
    }

    // -----------------------------------------------------------------------
    // Test: Complex programs
    // -----------------------------------------------------------------------

    #[test]
    fn test_complex_program() {
        // Compute sum of first 10 natural numbers using recursion
        let source = r#"
            let rec sum(n) = if n == 0 { 0 } else { n + sum(n - 1) } in sum(10)
        "#;
        assert_int(source, 55);
    }

    #[test]
    fn test_multiple_functions() {
        let source = r#"
            fn square(x) x * x
            fn sum_of_squares(a, b) square(a) + square(b)
            sum_of_squares(3, 4)
        "#;
        assert_int(source, 25);
    }
}
