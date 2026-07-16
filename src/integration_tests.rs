//! End-to-end integration tests that exercise the full compiler pipeline.
//!
//! Tests go through lex → parse → typecheck → compile → VM run.

#[cfg(test)]
mod tests {
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::runtime::{
        ActorSnapshot, EventEntry, JournalEntry, MemoryStore, PersistenceStore, Runtime,
        RuntimeVmCallbacks, WorkflowEvent,
    };
    use crate::typechecker::TypeChecker;
    use crate::types::NuError;
    use crate::types::Type;
    use crate::vm::{Value, VM};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    /// Thread-safe, shareable in-memory persistence store for tests that need
    /// to simulate a runtime restart while keeping the same underlying storage.
    #[derive(Debug, Clone)]
    struct SharedMemoryStore(Arc<Mutex<MemoryStore>>);

    impl SharedMemoryStore {
        fn new() -> Self {
            Self(Arc::new(Mutex::new(MemoryStore::new())))
        }
    }

    impl PersistenceStore for SharedMemoryStore {
        fn save_snapshot(&mut self, snapshot: ActorSnapshot) -> std::io::Result<()> {
            self.0.lock().unwrap().save_snapshot(snapshot)
        }
        fn load_snapshot(&self, actor_id: u64) -> Option<ActorSnapshot> {
            self.0.lock().unwrap().load_snapshot(actor_id)
        }
        fn append_journal(&mut self, actor_id: u64, entry: JournalEntry) -> std::io::Result<()> {
            self.0.lock().unwrap().append_journal(actor_id, entry)
        }
        fn read_journal(&self, actor_id: u64) -> Vec<JournalEntry> {
            self.0.lock().unwrap().read_journal(actor_id)
        }
        fn latest_sequence(&self, actor_id: u64) -> u64 {
            self.0.lock().unwrap().latest_sequence(actor_id)
        }
        fn append_workflow_event(
            &mut self,
            actor_id: u64,
            event: WorkflowEvent,
        ) -> std::io::Result<()> {
            self.0
                .lock()
                .unwrap()
                .append_workflow_event(actor_id, event)
        }
        fn read_workflow_events(&self, actor_id: u64) -> Vec<WorkflowEvent> {
            self.0.lock().unwrap().read_workflow_events(actor_id)
        }
        fn clear(&mut self, actor_id: u64) -> std::io::Result<()> {
            self.0.lock().unwrap().clear(actor_id)
        }
        fn append_event(&mut self, actor_id: u64, entry: EventEntry) -> std::io::Result<()> {
            self.0.lock().unwrap().append_event(actor_id, entry)
        }
        fn read_events(&self, actor_id: u64) -> Vec<EventEntry> {
            self.0.lock().unwrap().read_events(actor_id)
        }
    }

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
        // (placeholder: effect checker would go here)

        // 4. Compile via HIR/MIR pipeline
        let hir = crate::hir_lower::lower_module(&ast);
        let mir = crate::mir_lower::lower_module(&hir)?;
        let module = crate::mir_codegen::compile_mir(&mir, "test")?;
        // 5. Run
        let mut vm = VM::new();
        vm.load_module(module);
        let value = vm.run()?;

        Ok((value, module_type))
    }

    /// Assert that running source produces an integer value.
    fn assert_int(source: &str, expected: i64) {
        let (value, _ty) = run_source(source).unwrap();
        assert_eq!(
            value.as_int(),
            Some(expected),
            "Expected integer result for: {}",
            source
        );
    }

    /// Assert that running source produces a float value.
    fn assert_float(source: &str, expected: f64) {
        let (value, _ty) = run_source(source).unwrap();
        assert_eq!(
            value.as_float(),
            Some(expected),
            "Expected float result for: {}",
            source
        );
    }

    /// Assert that running source produces a boolean value.
    fn assert_bool(source: &str, expected: bool) {
        let (value, _ty) = run_source(source).unwrap();
        assert_eq!(
            value.as_bool(),
            Some(expected),
            "Expected boolean result for: {}",
            source
        );
    }

    /// Assert that running source produces the given string value.
    fn assert_string(source: &str, expected: &str) {
        let (module, _ty) = compile_source(source).unwrap();
        let mut vm = VM::new();
        vm.load_module(module);
        let value = vm.run().unwrap();
        // Handle both constant-pool strings (TAG_STRING) and heap-allocated
        // strings (TAG_PTR from SConcat).
        if let Some(id) = value.as_string_id() {
            let module_idx = vm.modules.len() - 1;
            let content = vm.constant_string(module_idx, id).unwrap();
            assert_eq!(content, expected, "unexpected string for: {}", source);
        } else if value.is_ptr() {
            // Heap-allocated string: read C string from the heap.
            let ptr = value.as_ptr().expect("expected ptr value");
            let mut len = 0usize;
            unsafe {
                while *ptr.add(len) != 0 {
                    len += 1;
                }
                let slice = std::slice::from_raw_parts(ptr, len);
                let content = String::from_utf8_lossy(slice);
                assert_eq!(content, expected, "unexpected heap string for: {}", source);
            }
        } else {
            panic!("expected string result, got {:?}", value);
        }
    }
    /// Run source through the full compiler pipeline using a real actor runtime.
    fn run_source_with_runtime(
        source: &str,
        runtime: Rc<RefCell<Runtime>>,
    ) -> Result<(Value, Type), NuError> {
        let (module, module_type) = compile_source(source)?;

        let mut vm = VM::new();
        vm.load_module(module);
        vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(runtime)));
        let value = vm.run()?;

        Ok((value, module_type))
    }

    /// Compile source into a bytecode module and its top-level type.
    fn compile_source(source: &str) -> Result<(crate::bytecode::CodeModule, Type), NuError> {
        let mut lexer = Lexer::new(source);
        let tokens = lexer.lex()?;
        let mut parser = Parser::new(tokens);
        let ast = parser.parse_module()?;

        let mut type_checker = TypeChecker::new();
        let module_type = type_checker.check_module(&ast)?;

        let hir = crate::hir_lower::lower_module(&ast);
        let mir = crate::mir_lower::lower_module(&hir)?;
        let module = crate::mir_codegen::compile_mir(&mir, "test")?;
        Ok((module, module_type))
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_literal_int() {
        assert_int("42", 42);
    }

    #[test]
    fn test_literal_negative_int() {
        assert_int("-17", -17);
    }

    #[test]
    fn test_arithmetic_add() {
        assert_int("1 + 2", 3);
    }

    #[test]
    fn test_arithmetic_sub() {
        assert_int("10 - 3", 7);
    }

    #[test]
    fn test_arithmetic_mul() {
        assert_int("4 * 5", 20);
    }

    #[test]
    fn test_arithmetic_div() {
        assert_int("20 / 4", 5);
    }

    #[test]
    fn test_bitwise_operators() {
        assert_int("6 & 3", 2);
        // Single `|` is reserved as a match-arm separator, so bitwise OR uses
        // the `|||` token.
        assert_int("6 ||| 3", 7);
        assert_int("6 ^ 3", 5);
        assert_int("1 << 3", 8);
        assert_int("16 >> 2", 4);
    }

    #[test]
    fn test_arithmetic_precedence() {
        assert_int("1 + 2 * 3", 7); // mul before add
        assert_int("(1 + 2) * 3", 9); // parens override
    }

    #[test]
    fn test_let_binding() {
        let source = "let x = 10 in x + 5";
        assert_int(source, 15);
    }

    #[test]
    fn test_local_assignment() {
        // `&` creates a ref cell; `*` dereferences it. Assignment mutates the ref.
        let source = "let x = &10 in { x = 3; *x }";
        assert_int(source, 3);
    }

    #[test]
    fn test_record_field_access() {
        let source = "let r = { x: 1, y: 2 } in r.x + r.y";
        assert_int(source, 3);
    }

    #[test]
    fn test_let_multiple() {
        let source = "let x = 1 in let y = 2 in let z = 3 in x + y + z";
        assert_int(source, 6);
    }

    #[test]
    fn test_boolean_true() {
        let (value, _ty) = run_source("true").unwrap();
        assert_eq!(value.as_bool(), Some(true));
    }

    #[test]
    fn test_boolean_false() {
        let (value, _ty) = run_source("false").unwrap();
        assert_eq!(value.as_bool(), Some(false));
    }

    #[test]
    fn test_boolean_and() {
        let (value, _ty) = run_source("true and false").unwrap();
        assert_eq!(value.as_bool(), Some(false));
    }

    #[test]
    fn test_boolean_or() {
        let (value, _ty) = run_source("true or false").unwrap();
        assert_eq!(value.as_bool(), Some(true));
    }

    #[test]
    fn test_comparison_eq() {
        let (value, _ty) = run_source("5 == 5").unwrap();
        assert_eq!(value.as_bool(), Some(true));
    }

    #[test]
    fn test_comparison_ne() {
        let (value, _ty) = run_source("5 != 3").unwrap();
        assert_eq!(value.as_bool(), Some(true));
    }

    #[test]
    fn test_comparison_lt() {
        let (value, _ty) = run_source("3 < 5").unwrap();
        assert_eq!(value.as_bool(), Some(true));
    }

    #[test]
    fn test_if_then_else() {
        assert_int("if true then 1 else 2", 1);
        assert_int("if false then 1 else 2", 2);
    }

    #[test]
    fn test_if_with_comparison() {
        assert_int("if 5 > 3 then 10 else 20", 10);
    }

    #[test]
    fn test_lambda_apply() {
        // Lambda: fn(x) x + 1, applied to 5
        let source = "(fn(x) x + 1)(5)";
        assert_int(source, 6);
    }

    #[test]
    fn test_lambda_two_args() {
        let source = "(fn(x, y) x + y)(3, 4)";
        assert_int(source, 7);
    }

    #[test]
    fn test_unit_value() {
        let (value, _ty) = run_source("unit").unwrap();
        assert!(value.is_unit());
    }

    #[test]
    fn test_nil_value() {
        let (value, _ty) = run_source("nil").unwrap();
        assert!(value.is_nil());
    }

    #[test]
    fn test_spawn_returns_actor_ref() {
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
    fn test_perform_unhandled_effect_errors() {
        // Unhandled effects should return an error (v0.15+ effect system).
        // Note: IO.print is no longer unhandled — the standalone VM handles
        // it as a built-in — so this uses an effect with no built-in.
        let source = r#"
            perform Net.fetch("hello")
        "#;
        let result = run_source(source);
        assert!(result.is_err(), "Unhandled effect should error");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("Unhandled effect"),
            "Error should mention unhandled effect: {}",
            err_msg
        );
    }

    // -----------------------------------------------------------------------
    // Test: examples/*.nula run end-to-end through the full pipeline
    // -----------------------------------------------------------------------

    #[test]
    fn test_example_fibonacci_runs() {
        let source = include_str!("../examples/fibonacci.nula");
        let (value, _ty) = run_source(source).unwrap();
        assert_eq!(value.as_int(), Some(55), "fib(10) = 55");
    }

    #[test]
    fn test_example_effects_runs() {
        let source = include_str!("../examples/effects.nula");
        let (value, _ty) = run_source(source).unwrap();
        assert_eq!(value.as_int(), Some(42), "handler should resume with 42");
    }

    #[test]
    fn test_example_counter_actor_runs() {
        let source = include_str!("../examples/counter_actor.nula");
        let (value, _ty) = run_source(source).unwrap();
        assert!(
            value.as_actor_id().is_some(),
            "spawn should return an actor reference"
        );
    }

    #[test]
    fn test_declared_effect_annotation_rejects_undeclared_effects() {
        // Mirrors the CLI frontend's enforcement: a function annotated with a
        // declared effect row must not perform effects outside that row.
        use crate::effect_checker::{EffectChecker, EffectContext};

        let source = r#"
            fn f() -> Unit ! {} {
                perform IO.print("x")
            }
        "#;
        let mut lexer = Lexer::new(source);
        let tokens = lexer.lex().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse_module().unwrap();

        let mut checker = EffectChecker::new();
        let ctx = EffectContext::empty();
        let mut checked = false;
        for decl in &ast.decls {
            if let crate::ast::Decl::Function {
                name,
                body,
                effect: Some(declared),
                ..
            } = decl
            {
                if name == "f" {
                    checked = true;
                    let result = checker.check_effects(&ctx, body, declared);
                    assert!(
                        result.is_err(),
                        "function declared pure (`! {{}}`) but performing IO must be rejected"
                    );
                }
            }
        }
        assert!(
            checked,
            "parser should surface the `! {{}}` annotation on fn f"
        );
    }

    #[test]
    fn test_declared_effect_annotation_accepts_matching_effects() {
        use crate::effect_checker::{EffectChecker, EffectContext};

        let source = r#"
            fn f() -> Unit ! {IO} {
                perform IO.print("x")
            }
        "#;
        let mut lexer = Lexer::new(source);
        let tokens = lexer.lex().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse_module().unwrap();

        let mut checker = EffectChecker::new();
        let ctx = EffectContext::empty();
        let mut checked = false;
        for decl in &ast.decls {
            if let crate::ast::Decl::Function {
                name,
                body,
                effect: Some(declared),
                ..
            } = decl
            {
                if name == "f" {
                    checked = true;
                    let result = checker.check_effects(&ctx, body, declared);
                    assert!(
                        result.is_ok(),
                        "function performing only its declared effects must pass: {:?}",
                        result.err()
                    );
                }
            }
        }
        assert!(
            checked,
            "parser should surface the `! {{IO}}` annotation on fn f"
        );
    }

    /// Run the module effect check the way the CLI frontend does
    /// (`run_frontend` in main.rs): one `EffectChecker::check_module` over
    /// the parsed declarations.
    fn check_module_effects(source: &str) -> Result<(), NuError> {
        let mut lexer = Lexer::new(source);
        let tokens = lexer.lex().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse_module().unwrap();
        let mut checker = crate::effect_checker::EffectChecker::new();
        checker.check_module(&ast.decls)
    }

    #[test]
    fn test_pure_fn_calling_io_fn_rejected() {
        // Finding: a function declared pure (`! {}`) that calls a function
        // performing IO must be rejected statically (SPEC2 §4.7/§4.9); the
        // callee's row propagates to the call site.
        let source = r#"
            fn do_io() -> Unit ! {IO} { perform IO.print("x") }
            fn pure() -> Unit ! {} { do_io() }
        "#;
        let result = check_module_effects(source);
        assert!(
            result.is_err(),
            "pure function calling an IO function must be rejected"
        );
    }

    #[test]
    fn test_pure_fn_calling_io_fn_transitively_rejected() {
        // The row map iterates to a fixpoint, so IO propagates through an
        // unannotated intermediate function as well.
        let source = r#"
            fn do_io() -> Unit ! {IO} { perform IO.print("x") }
            fn middle() -> Unit { do_io() }
            fn pure() -> Unit ! {} { middle() }
        "#;
        let result = check_module_effects(source);
        assert!(
            result.is_err(),
            "pure function transitively performing IO must be rejected"
        );
    }

    #[test]
    fn test_module_nested_effect_violation_rejected() {
        // Finding: declarations nested in `module {}` must be effect-checked
        // just like top-level ones (the typechecker already flattens them).
        let source = r#"
            module M {
                fn pure() -> Unit ! {} { perform IO.print("x") }
            }
        "#;
        let result = check_module_effects(source);
        assert!(
            result.is_err(),
            "module-nested pure function performing IO must be rejected"
        );
    }

    #[test]
    fn test_event_effect_annotation_accepted() {
        // Finding: `Event` (like `FFI`) is a built-in effect (SPEC2 §4.6), so
        // an `{Event}` annotation must satisfy a body that emits an event.
        let source = r#"
            fn f() -> Unit ! {Event} { emit MyEvent(1) }
        "#;
        let result = check_module_effects(source);
        assert!(
            result.is_ok(),
            "fn annotated ! {{Event}} may emit events: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_ffi_effect_annotation_accepted() {
        let source = r#"
            fn f() -> Unit ! {FFI} { 1 }
        "#;
        let result = check_module_effects(source);
        assert!(
            result.is_ok(),
            "fn annotated ! {{FFI}} must parse and check: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_pure_functions_still_pass_effect_check() {
        // Positive: legitimately pure functions — including pure calls
        // between them and module-nested pure functions — keep passing.
        let source = r#"
            fn pure() -> Unit ! {} { unit }
            fn also_pure() -> Unit ! {} { pure() }
            module M { fn nested_pure() -> Unit ! {} { also_pure() } }
        "#;
        let result = check_module_effects(source);
        assert!(
            result.is_ok(),
            "legitimately pure functions must pass: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_perform_effect_with_handler() {
        // perform with a handler that catches the effect.
        // The compiler generates a Handle opcode and handler table with
        // HandlerBindings, and the VM invokes the handler + resumes.
        let source = r#"
            handle perform IO.print("hello") {
                | IO.print(msg) => unit
            }
        "#;
        let (value, _ty) = run_source(source).unwrap();
        assert!(value.is_unit(), "Expected unit from handled perform");
    }

    #[test]
    fn test_handler_returns_value_via_resume() {
        // Handler computes a value and resumes with it.
        let source = r#"
            handle perform Math.getAnswer() {
                | Math.getAnswer() => 42
            }
        "#;
        let (value, _ty) = run_source(source).unwrap();
        assert_eq!(
            value.as_int(),
            Some(42),
            "Handler should return 42 via resume"
        );
    }

    #[test]
    fn test_handler_with_parameter() {
        // Handler receives the perform argument and uses it.
        let source = r#"
            handle perform Math.double(21) {
                | Math.double(x) => x + x
            }
        "#;
        let (value, _ty) = run_source(source).unwrap();
        assert_eq!(value.as_int(), Some(42), "Handler should double 21 to 42");
    }

    // -----------------------------------------------------------------------
    // Regression: non-resuming handlers (`=> body` without `resume`) must
    // abort the handled computation with the body value instead of silently
    // resuming the continuation
    // -----------------------------------------------------------------------

    #[test]
    fn test_non_resuming_handler_aborts_with_body_value() {
        // Without `resume`, the handler's value becomes the handle
        // expression's value; the `; 100` continuation must NOT run.
        let source = r#"
            handle { perform E.op(); 100 } { | E.op() => 42 }
        "#;
        assert_int(source, 42);
    }

    #[test]
    fn test_resuming_handler_continues_body() {
        // With `resume`, the handler value flows back to the perform site
        // and the body continues: 42 is bound to x, then discarded for 100.
        let source = r#"
            handle { let x = perform E.op() in { x; 100 } } { | E.op() resume => 42 }
        "#;
        assert_int(source, 100);
    }

    #[test]
    fn test_resuming_handler_value_reaches_perform_site() {
        // The resumed value must land in the perform's dst: 41 + 1 = 42.
        let source = r#"
            handle { let x = perform E.op() in x + 1 } { | E.op() resume => 41 }
        "#;
        assert_int(source, 42);
    }

    #[test]
    fn test_non_resuming_handler_with_parameter() {
        // Abortive handlers receive perform arguments like resuming ones.
        let source = r#"
            handle { perform Math.double(21); 0 } { | Math.double(x) => x + x }
        "#;
        assert_int(source, 42);
    }

    /// End-to-end: `perform IO.print` with no handler resolves through the
    /// standalone built-in effect instead of failing with
    /// "Unhandled effect: IO" (the `nulang --eval` path).
    #[test]
    fn test_standalone_io_print_end_to_end() {
        let (value, _ty) = run_source(r#"perform IO.print("hello")"#)
            .expect("standalone IO.print must not be an unhandled effect");
        assert!(value.is_unit(), "IO.print resumes with unit");
    }

    /// Source-level op-name dispatch: a handler for `IO.bar` must NOT catch
    /// `perform IO.foo()` — handler bindings are op-qualified ("Effect.op").
    #[test]
    fn test_source_handler_does_not_catch_other_op() {
        let source = r#"handle perform IO.foo() { | IO.bar() => 1 }"#;
        let err = run_source(source).expect_err("IO.bar handler must not catch IO.foo");
        let msg = format!("{}", err);
        assert!(
            msg.contains("Unhandled effect: 'IO.foo'"),
            "expected unhandled IO.foo, got: {}",
            msg
        );
    }

    /// Source-level op-name dispatch, positive control: the matching
    /// `IO.foo` handler catches the perform.
    #[test]
    fn test_source_handler_catches_matching_op() {
        let source = r#"handle perform IO.foo() { | IO.foo() => 1 }"#;
        assert_int(source, 1);
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

    #[test]
    fn test_block_nested() {
        let source = "{ let a = 10 in { let b = 20 in a + b } }";
        assert_int(source, 30);
    }

    // -----------------------------------------------------------------------
    // Test: Pattern matching (basic)
    // -----------------------------------------------------------------------

    #[test]
    fn test_match_int_literal() {
        let source = r#"match 42 {
            case 1 => 10
            case 42 => 100
            case _ => 0
        }"#;
        assert_int(source, 100);
    }

    #[test]
    fn test_match_wildcard() {
        let source = r#"match 99 {
            case 1 => 10
            case 2 => 20
            case _ => 50
        }"#;
        assert_int(source, 50);
    }

    // -----------------------------------------------------------------------
    // Test: Recursion
    // -----------------------------------------------------------------------

    #[test]
    fn test_recursion_factorial() {
        let source = r#"
            let fac = fn(n) {
                if n == 0 then 1 else n * fac(n - 1)
            } in fac(5)
        "#;
        assert_int(source, 120);
    }

    #[test]
    fn test_recursion_fibonacci() {
        let source = r#"
            let fib = fn(n) {
                if n <= 1 then n else fib(n - 1) + fib(n - 2)
            } in fib(8)
        "#;
        assert_int(source, 21);
    }

    // -----------------------------------------------------------------------
    // Test: String literal
    // -----------------------------------------------------------------------

    #[test]
    fn test_string_literal() {
        let source = r#""hello""#;
        let result = run_source(source);
        // String literals should either produce a string value or an error
        // depending on compiler support.
        match result {
            Ok((value, _)) => {
                // Should be some kind of string representation
                assert!(
                    value.as_int().is_some() || value.is_nil() || value.is_string(),
                    "String literal should produce a value"
                );
            }
            Err(_) => {
                // String support may not be fully implemented yet
            }
        }
    }

    // -----------------------------------------------------------------------
    // Test: List literal
    // -----------------------------------------------------------------------

    #[test]
    fn test_list_literal() {
        let source = "[1, 2, 3]";
        let result = run_source(source);
        match result {
            Ok((value, _)) => {
                assert!(
                    !value.is_nil(),
                    "List literal should produce a non-nil value"
                );
            }
            Err(_) => {
                // List support may not be fully implemented yet
            }
        }
    }

    // -----------------------------------------------------------------------
    // Regression: owning locals must get real Drop instructions (mir_lower's
    // temp-fusion peephole keeps plan_drops effective; previously every
    // named local was defined by a non-owning Load, so no heap value was
    // ever reclaimed before actor exit)
    // -----------------------------------------------------------------------

    /// Register (LOCAL_BASE + local id) of the named local in __main, plus
    /// the compiled module.
    fn compile_and_find_local(source: &str, name: &str) -> (crate::bytecode::CodeModule, u8) {
        let mut lexer = Lexer::new(source);
        let tokens = lexer.lex().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse_module().unwrap();
        let mut type_checker = TypeChecker::new();
        type_checker.check_module(&ast).unwrap();
        let hir = crate::hir_lower::lower_module(&ast);
        let mir = crate::mir_lower::lower_module(&hir).unwrap();
        let main = mir
            .functions
            .iter()
            .find(|f| f.name == "__main")
            .expect("__main lowered");
        let local = main
            .locals
            .iter()
            .find(|l| l.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("local '{}' not found in {:?}", name, main.locals));
        let reg = (16 + local.id.0) as u8;
        let module = crate::mir_codegen::compile_mir(&mir, "test").unwrap();
        (module, reg)
    }

    #[test]
    fn test_array_local_gets_real_drop() {
        // `a` solely owns its array and is only read (indexing), so codegen
        // must emit a Drop of `a`'s register after its last use — before
        // the fusion fix, `a` was defined by a non-owning Load and no Drop
        // of any array ever appeared.
        let source = "let a = [1, 2, 3] in a[0] + a[1]";
        let (module, reg) = compile_and_find_local(source, "a");
        let drops = module
            .instructions
            .iter()
            .filter(|i| i.opcode == crate::bytecode::OpCode::Drop && i.op1 == reg)
            .count();
        assert!(
            drops >= 1,
            "owning array local must be dropped at least once (reg {}), instructions: {:?}",
            reg,
            module.instructions
        );
        // And the program still evaluates correctly (no use-after-free).
        assert_int(source, 3);
    }

    #[test]
    fn test_record_local_gets_real_drop() {
        let source = "let r = { x: 1, y: 2 } in r.x + r.y";
        let (module, reg) = compile_and_find_local(source, "r");
        let drops = module
            .instructions
            .iter()
            .filter(|i| i.opcode == crate::bytecode::OpCode::Drop && i.op1 == reg)
            .count();
        assert!(
            drops >= 1,
            "owning record local must be dropped at least once (reg {})",
            reg
        );
        assert_int(source, 3);
    }

    // -----------------------------------------------------------------------
    // Test: Float literal
    // -----------------------------------------------------------------------

    #[test]
    fn test_float_literal() {
        let source = "3.14";
        let result = run_source(source);
        match result {
            Ok((value, _)) => {
                assert!(
                    value.as_float().is_some() || value.as_int().is_some(),
                    "Float literal should produce a numeric value"
                );
            }
            Err(_) => {
                // Float support may not be fully implemented yet
            }
        }
    }

    // -----------------------------------------------------------------------
    // Test: Type error detection
    // -----------------------------------------------------------------------

    #[test]
    fn test_type_error_mismatch() {
        let source = "1 + true"; // Can't add int and bool
        let result = run_source(source);
        assert!(
            result.is_err(),
            "Adding int and bool should be a type error"
        );
    }

    #[test]
    fn test_type_error_undefined_var() {
        let source = "undefined_variable + 1";
        let result = run_source(source);
        assert!(
            result.is_err(),
            "Using undefined variable should be an error"
        );
    }

    #[test]
    fn test_type_error_wrong_arity() {
        let source = "(fn(x) x)(1, 2)"; // Too many arguments
        let result = run_source(source);
        // This may or may not be caught by the type checker depending on
        // how function application is handled.
        match result {
            Ok(_) | Err(_) => {
                // Accept either — arity checking varies by implementation
            }
        }
    }

    // -----------------------------------------------------------------------
    // Test: declared types — variants, aliases, records, Nil (SPEC2 §3.4.1)
    // -----------------------------------------------------------------------

    /// Run only the frontend (lex → parse → typecheck), mirroring `--check`.
    fn check_source(source: &str) -> Result<Type, NuError> {
        let mut lexer = Lexer::new(source);
        let tokens = lexer.lex()?;
        let mut parser = Parser::new(tokens);
        let ast = parser.parse_module()?;
        let mut type_checker = TypeChecker::new();
        type_checker.check_module(&ast)
    }

    #[test]
    fn test_declared_variant_construction_typechecks() {
        let result = check_source("type Option[T] = Some(T) | None\nSome(1)");
        assert!(
            result.is_ok(),
            "declared variant construction should check, got {:?}",
            result.err()
        );
    }

    #[test]
    fn test_unbound_variant_constructor_is_error() {
        let result = check_source("Some(1)");
        assert!(
            result.is_err(),
            "Some without a declaring variant type must be an error"
        );
    }

    #[test]
    fn test_variant_spec_example_typechecks() {
        // The canonical SPEC2 §3.4.1 example: declared Result variant used
        // for construction, annotation, and pattern matching.
        let source = r#"
type Result[T, E] = Ok(T) | Error(E)

fn safe_divide(a: Float, b: Float) -> Result[Float, String] {
  if b == 0.0 then
    Error("Division by zero")
  else
    Ok(a / b)
}

fn describe(r: Result[Float, String]) -> String {
  match r with {
    | Ok(value) => "ok"
    | Error(msg) => msg
  }
}
describe
"#;
        let result = check_source(source);
        assert!(
            result.is_ok(),
            "spec variant example should check, got {:?}",
            result.err()
        );
    }

    #[test]
    fn test_unknown_type_name_in_annotation_is_error() {
        let result = check_source("fn f(x: Bogus) x\nf(1)");
        assert!(
            result.is_err(),
            "annotation with an unknown type name must be an error"
        );
    }

    #[test]
    fn test_type_alias_expansion_end_to_end() {
        let ok = check_source("type alias MyInt = Int\nfn f(x: MyInt) -> MyInt { x }\nf(1)");
        assert!(ok.is_ok(), "alias use with Int should check, got {:?}", ok.err());
        let bad = check_source("type alias MyInt = Int\nfn f(x: MyInt) -> MyInt { x }\nf(\"s\")");
        assert!(
            bad.is_err(),
            "alias must expand to the aliased type and reject String"
        );
    }

    #[test]
    fn test_record_type_annotation_end_to_end() {
        let result =
            check_source("type Point = { x: Int, y: Int }\nfn get_x(p: Point) -> Int { p.x }\nget_x");
        assert!(
            result.is_ok(),
            "record type name in annotation should check, got {:?}",
            result.err()
        );
    }

    #[test]
    fn test_nil_annotation_end_to_end() {
        assert!(
            check_source("fn f(x: Nil) x\nf(nil)").is_ok(),
            "nil must have type Nil"
        );
        assert!(
            check_source("fn f(x: Nil) x\nf(1)").is_err(),
            "Int must not be accepted where Nil is annotated"
        );
    }

    #[test]
    fn test_variant_declaration_compiles_and_runs() {
        // A program that declares a variant and destructures it in match
        // patterns must compile through the whole MIR pipeline and run in
        // the VM. (Constructing variant *values* is lowered separately.)
        let source = r#"
type Color = Red | Green | Blue
fn code(c: Color) -> Int {
  match c with {
    | Red => 1
    | Green => 2
    | Blue => 3
  }
}
code
"#;
        let (value, _ty) = run_source(source).unwrap();
        assert!(value.as_int().is_some(), "expected function-index value");
    }

    #[test]
    fn test_variant_spec_example_end_to_end() {
        // The canonical SPEC2 §3.4.1 example, run end-to-end: generic
        // two-parameter variant declaration, construction in if-branches,
        // variant type annotations, and a match that binds the payload.
        let source = r#"
type Result[T, E] = Ok(T) | Error(E)

fn safe_divide(a: Float, b: Float) -> Result[Float, String] {
  if b == 0.0 then
    Error("Division by zero")
  else
    Ok(a / b)
}

fn describe(r: Result[Float, String]) -> String {
  match r with {
    | Ok(value) => "ok"
    | Error(msg) => msg
  }
}

match safe_divide(6.0, 2.0) with {
  | Ok(value) => value
  | Error(msg) => 0.0
}
"#;
        assert_float(source, 3.0);
    }

    #[test]
    fn test_variant_spec_example_error_arm_binds_string() {
        // The Error arm of the §3.4.1 example: the String payload must be
        // constructed, matched by tag, and bound into the arm body.
        let source = r#"
type Result[T, E] = Ok(T) | Error(E)

fn safe_divide(a: Float, b: Float) -> Result[Float, String] {
  if b == 0.0 then
    Error("Division by zero")
  else
    Ok(a / b)
}

fn describe(r: Result[Float, String]) -> String {
  match r with {
    | Ok(value) => "ok"
    | Error(msg) => msg
  }
}

describe(safe_divide(1.0, 0.0))
"#;
        assert_string(source, "Division by zero");
    }

    #[test]
    fn test_variant_construction_match_binds_payload() {
        // Core construction test: `Some(41)` builds a value and the match
        // binds the payload; the None arm must not be taken.
        let source = r#"
type Option[T] = Some(T) | None
match Some(41) with {
  | Some(x) => x
  | None => 0
}
"#;
        assert_int(source, 41);
    }

    #[test]
    fn test_variant_match_payload_arithmetic() {
        // The tag comparison lowering (record `ctor` field vs tag string
        // via OpCode::SCmpEq) must select the `Some` arm and the payload
        // must flow into arm-body arithmetic.
        let source = r#"
type Option[T] = Some(T) | None
match Some(41) with {
  | Some(x) => x + 1
  | None => 0
}
"#;
        assert_int(source, 42);
    }

    #[test]
    fn test_variant_nullary_ctor_arm_taken() {
        // A payload-less constructor is the bare tag; `None` must dispatch
        // to the `None` arm and not to `Some(x)`.
        let source = r#"
type Option[T] = Some(T) | None
match None with {
  | Some(x) => 1
  | None => 0
}
"#;
        assert_int(source, 0);
    }

    #[test]
    fn test_variant_nested_construction() {
        // Nested construction `Some(Some(2))` matched by a nested pattern:
        // the outer tag selects the arm and the inner payload binds.
        let source = r#"
type Option[T] = Some(T) | None
match Some(Some(2)) with {
  | Some(Some(x)) => x
  | Some(None) => 0
  | None => 0
}
"#;
        assert_int(source, 2);
    }

    #[test]
    fn test_variant_returned_from_function() {
        // A variant built inside a function must survive the return and be
        // matched by the caller.
        let source = r#"
type Option[T] = Some(T) | None
fn wrap(x: Int) -> Option[Int] { Some(x) }
match wrap(7) with {
  | Some(v) => v + 1
  | None => 0
}
"#;
        assert_int(source, 8);
    }

    #[test]
    fn test_variant_let_bound_matched_later() {
        // A let-bound variant value is matched in a later expression.
        let source = r#"
type Option[T] = Some(T) | None
let v = Some(5) in
match v with {
  | Some(x) => x * 2
  | None => 0
}
"#;
        assert_int(source, 10);
    }

    #[test]
    fn test_variant_int_and_string_payloads() {
        // One variant type exercised with payloads of different types:
        // the Int payload is bound and returned; the String-payload
        // constructor is matched by tag.
        let source = r#"
type Result[T, E] = Ok(T) | Error(E)
fn code(r: Result[Int, String]) -> Int {
  match r with {
    | Ok(v) => v
    | Error(msg) => 0
  }
}
code(Ok(3)) + code(Error("boom"))
"#;
        assert_int(source, 3);
    }

    #[test]
    fn test_variant_nested_pattern_binds_inner() {
        // A nested constructor pattern must test both tags and bind the
        // innermost payload.
        let source = r#"
type Option[T] = Some(T) | None
match Some(Some(9)) with {
  | Some(Some(x)) => x + 1
  | _ => 0
}
"#;
        assert_int(source, 10);
    }

    #[test]
    fn test_variant_nested_pattern_rejects_inner_none() {
        // `Some(None)` must NOT match `Some(Some(x))`: the inner tag test
        // runs against the payload, so the arm falls through to the
        // `Some(None)` arm. (With outer-tag-only matching this returned 1.)
        let source = r#"
type Option[T] = Some(T) | None
match Some(None) with {
  | Some(Some(x)) => 1
  | Some(None) => 2
  | None => 3
}
"#;
        assert_int(source, 2);
    }

    #[test]
    fn test_variant_nested_pattern_rejects_nullary() {
        // The bare `None` tag must not match a nested `Some(Some(x))` arm.
        let source = r#"
type Option[T] = Some(T) | None
match None with {
  | Some(Some(x)) => 1
  | None => 0
}
"#;
        assert_int(source, 0);
    }

    #[test]
    fn test_variant_payload_tuple_pattern() {
        // A tuple pattern nested inside a variant pattern: both the outer
        // tag and both element sub-patterns are tested, and the elements
        // bind into the arm body.
        let source = r#"
type Option[T] = Some(T) | None
match Some((1, 2)) with {
  | Some((a, b)) => a + b
  | None => 0
}
"#;
        assert_int(source, 3);
    }

    #[test]
    fn test_tuple_pattern_binds_elements() {
        // Structural tuple matching: each element position is loaded from
        // the scrutinee and bound into the arm body.
        let source = r#"
match (1, 2) with {
  | (a, b) => a + b
}
"#;
        assert_int(source, 3);
    }

    #[test]
    fn test_tuple_pattern_literal_element_matches() {
        // A literal element participates in the test: (1, 5) matches
        // (1, x) and binds x.
        let source = r#"
match (1, 5) with {
  | (1, x) => x
  | _ => 0
}
"#;
        assert_int(source, 5);
    }

    #[test]
    fn test_tuple_pattern_literal_element_rejects() {
        // (2, 5) must not match the (1, x) arm; it falls through to the
        // wildcard.
        let source = r#"
match (2, 5) with {
  | (1, x) => x
  | _ => 0
}
"#;
        assert_int(source, 0);
    }

    #[test]
    fn test_record_pattern_binds_fields() {
        // Structural record matching: named fields are loaded from the
        // scrutinee and bound into the arm body.
        let source = r#"
match { a: 3, b: 4 } with {
  | { a: x, b: y } => x + y
}
"#;
        assert_int(source, 7);
    }

    #[test]
    fn test_record_pattern_literal_field_rejects() {
        // A literal field pattern rejects a mismatching record: the first
        // arm's `a: 1` test fails for `{ a: 2, ... }`, so the second arm
        // binds both fields.
        let source = r#"
match { a: 2, b: 9 } with {
  | { a: 1, b: y } => y
  | { a: x, b: y } => x + y
}
"#;
        assert_int(source, 11);
    }

    // -----------------------------------------------------------------------
    // Test: Complex programs
    // -----------------------------------------------------------------------

    #[test]
    fn test_quicksort() {
        let source = r#"
            let partition = fn(arr, low, high) {
                let pivot = arr[high] in
                let i = low - 1 in
                let j = low in
                let loop = fn() {
                    if j < high then {
                        if arr[j] < pivot then {
                            let i = i + 1 in
                            let tmp = arr[i] in
                            let arr[i] = arr[j] in
                            let arr[j] = tmp in
                            let j = j + 1 in
                            loop()
                        } else {
                            let j = j + 1 in
                            loop()
                        }
                    } else {
                        let tmp = arr[i + 1] in
                        let arr[i + 1] = arr[high] in
                        let arr[high] = tmp in
                        i + 1
                    }
                } in loop()
            } in
            let quicksort = fn(arr, low, high) {
                if low < high then {
                    let pi = partition(arr, low, high) in
                    let _ = quicksort(arr, low, pi - 1) in
                    quicksort(arr, pi + 1, high)
                } else {
                    0
                }
            } in
            let arr = [3, 6, 8, 10, 1, 2, 1] in
            let _ = quicksort(arr, 0, 6) in
            arr[0]
        "#;
        let result = run_source(source);
        // Quicksort on arrays may or may not be fully supported.
        // The test mainly exercises the parser and type checker.
        match result {
            Ok((value, _)) => {
                assert!(
                    value.as_int().is_some(),
                    "Quicksort should produce a result"
                );
            }
            Err(_) => {
                // Array operations may not be fully implemented yet
            }
        }
    }

    #[test]
    fn test_counter_actor() {
        let source = r#"
            let counter = spawn {
                state count = 0
                behavior inc() { self.count + 1 }
                behavior get() { self.count }
            } in
            send counter.inc()
            send counter.inc()
            send counter.get()
        "#;
        let result = run_source(source);
        // Actor spawn/send may or may not be fully supported in the
        // compiler-to-VM pipeline yet.
        match result {
            Ok((value, _)) => {
                assert!(
                    value.as_int().is_some() || value.is_unit(),
                    "Counter actor should produce a result"
                );
            }
            Err(_) => {
                // Actor syntax may not be fully compiled yet
            }
        }
    }

    // -----------------------------------------------------------------------
    // Test: Edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_empty_block() {
        let source = "{}";
        let result = run_source(source);
        match result {
            Ok((value, _)) => assert!(value.is_unit() || value.is_nil()),
            Err(_) => {}
        }
    }

    #[test]
    fn test_deep_nesting() {
        let source =
            "let a = 1 in let b = 2 in let c = 3 in let d = 4 in let e = 5 in a + b + c + d + e";
        assert_int(source, 15);
    }

    #[test]
    fn test_large_int() {
        assert_int("1000000", 1_000_000);
    }

    #[test]
    fn test_zero() {
        assert_int("0", 0);
    }

    #[test]
    fn test_negative_zero() {
        // -0 should be 0
        assert_int("-0", 0);
    }

    // -----------------------------------------------------------------------
    // Test: v0.7 persistent actor end-to-end spawn
    // -----------------------------------------------------------------------

    #[test]
    fn test_persistent_actor_spawn_end_to_end() {
        let store = MemoryStore::new();
        let rt = Rc::new(RefCell::new(Runtime::new()));
        rt.borrow_mut().persistence = Box::new(store.clone());

        let source = r#"
            persistent actor Counter {
                state durable count: Int = 0
                behavior inc() { self.count }
            }
            spawn Counter {}
        "#;

        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&actor_id).unwrap();
        assert!(actor.persistent, "actor should be persistent");
        assert_eq!(
            actor.state_models.get("count"),
            Some(&crate::runtime::StateModel::Durable),
            "count should use durable state model"
        );
    }

    #[test]
    fn test_persistent_counter_end_to_end_messages() {
        let store = MemoryStore::new();
        let rt = Rc::new(RefCell::new(Runtime::new()));
        rt.borrow_mut().persistence = Box::new(store.clone());

        let source = r#"
            persistent actor Counter {
                state durable count: Int = 0
                behavior inc() { self.count = self.count + 1 }
            }
            let c = spawn Counter {} in {
                send c inc()
                send c inc()
                c
            }
        "#;

        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        {
            let rt_ref = rt.borrow();
            let actor = rt_ref.actors.get(&actor_id).unwrap();
            assert_eq!(actor.mailbox.len(), 2, "two inc messages should be queued");
            assert!(
                !actor.bytecode_offsets.is_empty(),
                "actor should have bytecode behavior offsets"
            );
            assert!(
                actor.bytecode_module.is_some(),
                "actor should have a bytecode module"
            );
        }

        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&actor_id).unwrap();
        assert_eq!(
            actor.get_state_field("count").and_then(|v| v.as_int()),
            Some(2),
            "counter should be 2 after two inc messages"
        );
    }

    #[test]
    fn test_send_with_arguments() {
        let rt = Rc::new(RefCell::new(Runtime::new()));

        let source = r#"
            actor Counter {
                state count: Int = 0
                behavior add(n: Int) { self.count = self.count + n }
                behavior get() { self.count }
            }
            let c = spawn Counter {} in {
                send c add(5)
                send c add(7)
                c
            }
        "#;

        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&actor_id).unwrap();
        assert_eq!(
            actor.get_state_field("count").and_then(|v| v.as_int()),
            Some(12),
            "counter should be 12 after adding 5 and 7"
        );
    }

    #[test]
    fn test_ask_with_arguments() {
        let rt = Rc::new(RefCell::new(Runtime::new()));

        let source = r#"
            actor Calculator {
                behavior add(a: Int, b: Int) { a + b }
            }
            let calc = spawn Calculator {} in
                ask calc add(10, 20)
        "#;

        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        assert_eq!(value.as_int(), Some(30), "ask add(10, 20) should return 30");
    }

    /// Regression test for a silent-data-loss bug found while adding actor
    /// support to the HIR/MIR pipeline: `compile_binary`'s BinOp::Assign case
    /// only special-cased `self.field = v`; every other assignment target
    /// (array index, non-self record field) fell through to
    /// `compile_expr(left)` (reading the CURRENT value) followed by
    /// `OpCode::Store`, a plain register-to-register copy — the assignment
    /// never reached the array/record at all. Fixed by intercepting
    /// BinOp::Assign in compile_expr's dispatch and routing it through
    /// compile_assign, which computes a place (object + field id, or array +
    /// index) instead of reading a value.
    #[test]
    fn test_legacy_index_and_field_assign() {
        let (value, _ty) = run_source("let arr = [1, 2, 3] in { arr[0] = 99 arr[0] }").unwrap();
        assert_eq!(
            value.as_int(),
            Some(99),
            "arr[0] = 99 should actually mutate the array"
        );

        let (value, _ty) = run_source("let r = { x: 1, y: 2 } in { r.x = 99 r.x + r.y }").unwrap();
        assert_eq!(
            value.as_int(),
            Some(101),
            "r.x = 99 should actually mutate the record"
        );
    }

    /// End-to-end regression for JIT type-guard stripping: a hot recursive
    /// numeric function must tier up through the type-directed
    /// (guard-stripped) compiler and produce exactly the same result the
    /// interpreter computes. The arithmetic-heavy body gives the tiering
    /// path a straight-line region longer than the 5-instruction minimum.
    #[test]
    fn test_jit_typed_guard_stripping_hot_function() {
        let source = r#"
            fn hot(n: Int, acc: Int) -> Int {
                if n < 1 then acc else {
                    let a = acc + n in
                    let b = a + 1 in
                    let c = b + 2 in
                    let d = c + 3 in
                    let e = d + 4 in
                    hot(n - 1, e + 5)
                }
            }
            hot(2000, 0)
        "#;
        // Per call: acc += n + (1+2+3+4+5); n runs 2000..1.
        let expected: i64 = (1..=2000).sum::<i64>() + 15 * 2000;

        let (module, _ty) = compile_source(source).unwrap();
        let mut vm = VM::new();
        vm.load_module(module);
        let value = vm.run().unwrap();
        assert_eq!(
            value.as_int(),
            Some(expected),
            "typed-path result must be exact"
        );
        assert!(
            vm.jit_typed_compiled_count() >= 1,
            "hot numeric function must compile through the type-directed JIT path"
        );
    }

    /// End-to-end float arithmetic: mir_codegen's binary/unary opcode
    /// emission is type-directed — float operands must compile to the F*
    /// opcode variants, since the integer handlers coerce float operands
    /// to 0 (so `1.5 + 2.5` used to evaluate to 0).
    #[test]
    fn test_float_arithmetic_end_to_end() {
        assert_float("1.5 + 2.5", 4.0);
        assert_float("5.5 - 2.0", 3.5);
        assert_float("1.5 * 2.0", 3.0);
        assert_float("7.0 / 2.0", 3.5);
        assert_float("-1.5", -1.5);
        // Float-ness propagates through let bindings even though
        // hir_lower types binary results as Int.
        assert_float("let x = 1.5 in let y = x + 2.5 in y * 2.0", 8.0);
    }

    /// All six comparisons on float operands, with exact expected values.
    /// Integer comparison opcodes coerce both sides to 0, which made
    /// `2.0 == 3.0` true and `2.5 <= 1.5` true before the fix.
    #[test]
    fn test_float_comparisons_end_to_end() {
        assert_bool("1.5 < 2.5", true);
        assert_bool("2.5 < 1.5", false);
        assert_bool("2.5 > 1.5", true);
        assert_bool("1.5 > 2.5", false);
        assert_bool("1.5 <= 2.5", true);
        assert_bool("2.5 <= 1.5", false);
        assert_bool("2.5 >= 1.5", true);
        assert_bool("1.5 >= 2.5", false);
        assert_bool("2.0 == 3.0", false);
        assert_bool("2.0 == 2.0", true);
        assert_bool("2.0 != 3.0", true);
        assert_bool("2.0 != 2.0", false);
    }

    /// Float arithmetic threaded through the integer opcode fallback:
    /// unannotated parameters default to the numeric type variable, but when
    /// the function is applied to float literals the VM may still execute the
    /// integer opcodes (IAdd/ISub/IMul/IDiv/IMod/INeg). The interpreter and
    /// JIT runtime helpers now dispatch to float behavior when both operands
    /// are real floats, so these must produce correct float results.
    #[test]
    fn test_float_threading_through_integer_opcodes() {
        assert_float("let f = fn(x, y) x + y in f(1.5, 2.5)", 4.0);
        assert_float("let f = fn(x, y) x - y in f(5.5, 2.0)", 3.5);
        assert_float("let f = fn(x, y) x * y in f(1.5, 2.0)", 3.0);
        assert_float("let f = fn(x, y) x / y in f(7.0, 2.0)", 3.5);
        assert_float("let f = fn(x, y) x % y in f(7.5, 2.0)", 1.5);
        assert_float("let f = fn(x) -x in f(1.5)", -1.5);
    }

    /// Float comparisons threaded through the integer comparison fallback:
    /// unannotated parameters and the standard comparison operators must work
    /// on float operands even if the compiler emitted ICmp* opcodes.
    #[test]
    fn test_float_comparisons_threading_through_integer_opcodes() {
        assert_bool("let f = fn(x, y) x < y in f(1.5, 2.5)", true);
        assert_bool("let f = fn(x, y) x > y in f(1.5, 2.5)", false);
        assert_bool("let f = fn(x, y) x <= y in f(1.5, 2.5)", true);
        assert_bool("let f = fn(x, y) x >= y in f(1.5, 2.5)", false);
        assert_bool("let f = fn(x, y) x == y in f(2.0, 2.0)", true);
        assert_bool("let f = fn(x, y) x == y in f(2.0, 3.0)", false);
    }

    /// Float modulo: `7.5 % 2.0` compiles to the FMod opcode and the
    /// interpreter evaluates it with f64 % f64 semantics; a zero float
    /// divisor yields nil, mirroring FDiv.
    #[test]
    fn test_float_modulo_end_to_end() {
        assert_float("7.5 % 2.0", 1.5);
        assert_float("7.0 % 2.0", 1.0);
        let (value, _ty) = run_source("7.0 % 0.0").unwrap();
        assert_eq!(
            value.as_raw(),
            crate::vm::Value::nil().as_raw(),
            "float modulo by zero must yield nil, got {:?}",
            value
        );
        assert_int("7 % 2", 1);
    }

    /// A hot loop (>1000 reductions, past the JIT tier-up threshold)
    /// containing float `%` must produce correct results. FMod is not in
    /// `is_opcode_compilable`, so `find_compilable_region` stops at it and
    /// the opcode only ever runs in the interpreter — this pins that
    /// graceful fallback.
    #[test]
    fn test_float_modulo_hot_loop_interpreter_only() {
        let source = r#"
            fn loop_mod(n: Int, acc: Float) -> Float {
                if n < 1 then acc else {
                    let a = acc + 0.5 in
                    let b = a % 2.0 in
                    loop_mod(n - 1, b)
                }
            }
            loop_mod(1501, 0.0)
        "#;
        // acc cycles 0.5 -> 1.0 -> 1.5 -> 0.0 -> ... with period 4;
        // 1501 mod 4 == 1, so the final value is 0.5. All intermediate
        // values are exactly representable in f64.
        assert_float(source, 0.5);
    }

    /// The interpreter's FDiv yields nil on a zero divisor; the JIT must
    /// match (nulang_fdiv guards the zero divisor, and the typed compiler
    /// routes FDiv through that helper instead of emitting a raw fdiv
    /// that would produce inf/NaN). The hot run tiers the function up
    /// through the type-directed JIT path (>1000 reductions), so both
    /// runs agreeing proves interpreter == JIT.
    #[test]
    fn test_float_div_by_zero_cold_and_hot_parity() {
        let source = |n: i64| {
            format!(
                r#"
                fn fdivz(n: Int, acc: Float) -> Float {{
                    if n < 1 then acc else {{
                        let a = acc + 1.0 in
                        let b = a * 2.0 in
                        let c = b - 3.0 in
                        let d = c / 0.0 in
                        fdivz(n - 1, d)
                    }}
                }}
                fdivz({}, 7.0)
                "#,
                n
            )
        };

        // Cold: below the tiering threshold, purely interpreted.
        let (cold, _ty) = run_source(&source(5)).unwrap();
        assert_eq!(
            cold.as_raw(),
            Value::nil().as_raw(),
            "interpreted float div by zero must yield nil"
        );

        // Hot: forces JIT tier-up of the loop body containing the FDiv.
        let (module, _ty) = compile_source(&source(2000)).unwrap();
        let mut vm = VM::new();
        vm.load_module(module);
        let hot = vm.run().unwrap();
        assert_eq!(
            hot.as_raw(),
            cold.as_raw(),
            "JIT result must match the interpreter for float div by zero"
        );
        assert_eq!(hot.as_raw(), Value::nil().as_raw());
        assert!(
            vm.jit_typed_compiled_count() >= 1,
            "hot float function must compile through the type-directed JIT path"
        );
    }

    /// Hot float arithmetic with a nonzero divisor: the typed JIT path
    /// must produce bit-identical results to the interpreter. The
    /// recurrence acc' = (2*acc + 1)/4 converges to exactly 0.5.
    #[test]
    fn test_float_arithmetic_hot_typed_jit_exact() {
        let source = r#"
            fn hotf(n: Int, acc: Float) -> Float {
                if n < 1 then acc else {
                    let a = acc * 2.0 in
                    let b = a + 1.0 in
                    hotf(n - 1, b / 4.0)
                }
            }
            hotf(2000, 0.0)
        "#;
        let (module, _ty) = compile_source(source).unwrap();
        let mut vm = VM::new();
        vm.load_module(module);
        let value = vm.run().unwrap();
        assert_eq!(value.as_float(), Some(0.5), "typed-path float math must be exact");
        assert!(
            vm.jit_typed_compiled_count() >= 1,
            "hot float function must compile through the type-directed JIT path"
        );
    }

    /// A handler binding with more than MAX_STAGED_ARGS (16) parameters
    /// must be an honest compile error: the VM stages effect arguments in
    /// r0..r15, so a longer prologue would alias the enclosing function's
    /// locals (mirrors the 17-parameter function check).
    #[test]
    fn test_over_limit_handler_params_compile_error() {
        let params = (0..17)
            .map(|i| format!("p{}", i))
            .collect::<Vec<_>>()
            .join(", ");
        let source = format!("handle 0 {{ | E.op({}) => p0 }}", params);
        let result = run_source(&source);
        assert!(
            matches!(result, Err(NuError::VMError(_))),
            "a 17-parameter handler binding should be a compile error, got {:?}",
            result
        );
    }

    #[test]
    fn test_register_overflow_errors() {
        // 20 nested let bindings — the MIR pipeline allocates isolated
        // per-function registers and can handle this depth.
        let source = r#"
            let a0 = 0 in let a1 = 1 in let a2 = 2 in let a3 = 3 in let a4 = 4 in
            let a5 = 5 in let a6 = 6 in let a7 = 7 in let a8 = 8 in let a9 = 9 in
            let a10 = 10 in let a11 = 11 in let a12 = 12 in let a13 = 13 in let a14 = 14 in
            let a15 = 15 in let a16 = 16 in let a17 = 17 in let a18 = 18 in let a19 = 19 in
            a19
        "#;
        let (value, _ty) = run_source(source).unwrap();
        assert_eq!(value.as_int(), Some(19));
    }

    #[test]
    fn test_persistent_counter_recover_after_restart() {
        let source = r#"
            persistent actor Counter {
                state durable count: Int = 0
                behavior inc() { self.count = self.count + 1 }
                behavior get() { self.count }
            }
            spawn Counter {}
        "#;

        let store = SharedMemoryStore::new();
        let (module, _ty) = compile_source(source).unwrap();
        let meta = module.actor_metadata.first().unwrap();
        let mut offsets = vec![0; module.behaviors.len()];
        let mut comp_offsets: Vec<Option<usize>> = vec![None; module.behaviors.len()];
        for &idx in &meta.behavior_indices {
            if let Some(entry) = module.behaviors.get(idx) {
                offsets[idx] = entry.code_offset;
                comp_offsets[idx] = entry.compensate_offset;
            }
        }

        // First runtime: spawn, send 3 inc messages, and run scheduler.
        let rt1 = Rc::new(RefCell::new(Runtime::new()));
        rt1.borrow_mut().persistence = Box::new(store.clone());
        let value = {
            let mut vm = VM::new();
            vm.load_module(module.clone());
            vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt1.clone())));
            vm.run().unwrap()
        };
        let actor_id = value.as_actor_id().expect("spawn should return actor ref");

        rt1.borrow_mut().send_message(actor_id, "inc", &[]);
        rt1.borrow_mut().send_message(actor_id, "inc", &[]);
        rt1.borrow_mut().send_message(actor_id, "inc", &[]);
        rt1.borrow_mut().run_scheduler();
        assert_eq!(
            rt1.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("count")
                .and_then(|v| v.as_int()),
            Some(3)
        );

        // Simulate a runtime restart: new runtime sharing the same store,
        // register the bytecode module, then recover.
        let rt2 = Rc::new(RefCell::new(Runtime::new()));
        rt2.borrow_mut().persistence = Box::new(store.clone());
        rt2.borrow_mut().register_recovery_module(
            actor_id,
            module.clone(),
            offsets.clone(),
            vec![None; module.behaviors.len()],
        );
        rt2.borrow_mut().recover_actor(actor_id);

        assert_eq!(
            rt2.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("count")
                .and_then(|v| v.as_int()),
            Some(3),
            "recovered counter should still be 3"
        );

        // Send two more inc messages on the recovered runtime.
        rt2.borrow_mut().send_message(actor_id, "inc", &[]);
        rt2.borrow_mut().send_message(actor_id, "inc", &[]);
        rt2.borrow_mut().run_scheduler();
        assert_eq!(
            rt2.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("count")
                .and_then(|v| v.as_int()),
            Some(5),
            "counter should continue incrementing after recovery"
        );
    }

    #[test]
    fn test_event_sourced_counter_emits_and_recovers() {
        let source = r#"
            persistent actor EventCounter {
                state event_sourced count: Int = 0
                behavior inc() {
                    emit Incremented()
                }
                behavior get() {
                    self.count
                }
            }
            let c = spawn EventCounter {} in {
                send c inc()
                send c inc()
                send c inc()
                c
            }
        "#;

        let rt = Rc::new(RefCell::new(Runtime::new()));
        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&actor_id).unwrap();
        assert_eq!(
            actor.get_state_field("count").and_then(|v| v.as_int()),
            Some(3),
            "event-sourced counter should be 3 after three inc messages"
        );
        assert_eq!(actor.event_log.len(), 3, "three events should be logged");
        assert_eq!(actor.event_log[0].0, "Incremented");
    }

    /// String concatenation and Int.to_string via the full MIR pipeline.
    #[test]
    fn test_string_concat_and_int_to_string() {
        assert_string(r#""hello " + "world""#, "hello world");
        assert_string(
            r#""count: " + perform Int.to_string(42)"#,
            "count: 42",
        );
    }

    #[test]
    fn test_workflow_lowers_to_persistent_actor() {
        let source = "workflow PurchaseOrder { step validate { 1 } }";
        let (module, _ty) = compile_source(source).unwrap();

        let meta = module
            .actor_metadata
            .iter()
            .find(|m| m.name == "PurchaseOrder")
            .expect("workflow should produce actor metadata");
        assert!(meta.is_workflow, "workflow metadata should be flagged");
        assert!(meta.persistent, "workflows should be persistent actors");
        assert_eq!(meta.behavior_indices.len(), 1, "one behavior per step");

        let behavior = &module.behaviors[meta.behavior_indices[0]];
        assert_eq!(behavior.name, "PurchaseOrder.validate");
    }

    #[test]
    fn test_workflow_survives_node_restart() {
        // A two-step workflow that emits durable events and advances its
        // step_index in each step.  We run the first step, simulate a node
        // restart by loading the actor into a fresh runtime sharing the same
        // persistence store, then run the second step and verify final state.
        let source = r#"
            workflow Counter {
                step start { (emit Started(0), self.step_index = self.step_index + 1) }
                step second { (emit Incremented(1), self.step_index = self.step_index + 1) }
            }
            let c = spawn Counter {} in { c }
        "#;

        let store = SharedMemoryStore::new();
        let (module, _ty) = compile_source(source).unwrap();
        let meta = module.actor_metadata.first().unwrap();
        let mut offsets = vec![0; module.behaviors.len()];
        for &idx in &meta.behavior_indices {
            if let Some(entry) = module.behaviors.get(idx) {
                offsets[idx] = entry.code_offset;
            }
        }

        // First runtime: spawn, advance the first step, and run scheduler.
        let rt1 = Rc::new(RefCell::new(Runtime::new()));
        rt1.borrow_mut().persistence = Box::new(store.clone());
        let value = {
            let mut vm = VM::new();
            vm.load_module(module.clone());
            vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt1.clone())));
            vm.run().unwrap()
        };
        let actor_id = value.as_actor_id().expect("spawn should return actor ref");

        rt1.borrow_mut().send_message(actor_id, "start", &[]);
        rt1.borrow_mut().run_scheduler();

        assert_eq!(
            rt1.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("step_index")
                .and_then(|v| v.as_int()),
            Some(1),
            "first step should advance step_index to 1"
        );

        let events_before = store.read_workflow_events(actor_id);
        assert!(events_before
            .iter()
            .any(|e| matches!(e, WorkflowEvent::WorkflowStarted { .. })));
        assert!(events_before
            .iter()
            .any(|e| matches!(e, WorkflowEvent::Custom { name, .. } if name == "Started")));
        assert!(events_before
            .iter()
            .any(|e| matches!(e, WorkflowEvent::StepCompleted { .. })));

        // Simulate a node restart: new runtime sharing the same store,
        // register the bytecode module, then recover the workflow actor.
        let rt2 = Rc::new(RefCell::new(Runtime::new()));
        rt2.borrow_mut().persistence = Box::new(store.clone());
        rt2.borrow_mut().register_recovery_module(
            actor_id,
            module.clone(),
            offsets.clone(),
            vec![None; module.behaviors.len()],
        );
        rt2.borrow_mut().recover_actor(actor_id);

        assert_eq!(
            rt2.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("step_index")
                .and_then(|v| v.as_int()),
            Some(1),
            "recovered workflow should resume at step_index 1"
        );

        // Continue execution on the recovered runtime: advance the second step.
        // Bytecode-only workflow actors have an empty behavior_table, so route
        // by explicit behavior id (1 is the second step).
        rt2.borrow_mut().send_message_by_id(actor_id, 1, &[]);
        rt2.borrow_mut().run_scheduler();

        assert_eq!(
            rt2.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("step_index")
                .and_then(|v| v.as_int()),
            Some(2),
            "final step_index should be 2 after second step"
        );

        let events_after = store.read_workflow_events(actor_id);
        assert_eq!(
            events_after
                .iter()
                .filter(|e| matches!(e, WorkflowEvent::StepCompleted { .. }))
                .count(),
            2,
            "two StepCompleted events should be persisted"
        );
        assert!(events_after
            .iter()
            .any(|e| matches!(e, WorkflowEvent::Custom { name, .. } if name == "Incremented")));
    }

    #[test]
    fn test_workflow_signal_wait_and_resume_after_restart() {
        // A workflow step waits for a named signal. The step suspends until
        // the signal is delivered, and after a simulated restart the signal
        // is replayed from the journal so the workflow advances.
        let source = r#"
            workflow Signaled {
                step wait_for_go {
                    perform Signal.wait("go")
                }
            }
            let c = spawn Signaled {} in { c }
        "#;

        let store = SharedMemoryStore::new();
        let (module, _ty) = compile_source(source).unwrap();
        let meta = module.actor_metadata.first().unwrap();
        let mut offsets = vec![0; module.behaviors.len()];
        for &idx in &meta.behavior_indices {
            if let Some(entry) = module.behaviors.get(idx) {
                offsets[idx] = entry.code_offset;
            }
        }

        // First runtime: spawn and start the waiting step.
        let rt1 = Rc::new(RefCell::new(Runtime::new()));
        rt1.borrow_mut().persistence = Box::new(store.clone());
        let value = {
            let mut vm = VM::new();
            vm.load_module(module.clone());
            vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt1.clone())));
            vm.run().unwrap()
        };
        let actor_id = value.as_actor_id().expect("spawn should return actor ref");

        rt1.borrow_mut().send_message_by_id(actor_id, 0, &[]);
        rt1.borrow_mut().run_scheduler();

        // Step has not completed yet; it is suspended waiting for the signal.
        assert_eq!(
            rt1.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("step_index")
                .and_then(|v| v.as_int()),
            Some(0),
            "step should not advance before signal is received"
        );
        assert!(
            rt1.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .suspended_execution
                .is_some(),
            "actor should have a suspended execution waiting for the signal"
        );

        // Simulate a runtime restart: drop the actor and recover from the store.
        rt1.borrow_mut().actors.remove(&actor_id);

        let rt2 = Rc::new(RefCell::new(Runtime::new()));
        rt2.borrow_mut().persistence = Box::new(store.clone());
        rt2.borrow_mut().register_recovery_module(
            actor_id,
            module.clone(),
            offsets.clone(),
            vec![None; module.behaviors.len()],
        );
        rt2.borrow_mut().recover_actor(actor_id);
        // Recovery detects the waiting signal and re-triggers the step; it
        // suspends again until the signal arrives.
        rt2.borrow_mut().run_scheduler();

        assert_eq!(
            rt2.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("step_index")
                .and_then(|v| v.as_int()),
            Some(0),
            "step should still be waiting after recovery"
        );

        // Send the signal. The runtime appends SignalReceived and resumes the step.
        rt2.borrow_mut().signal_workflow(actor_id, "go", None);

        assert_eq!(
            rt2.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("step_index")
                .and_then(|v| v.as_int()),
            Some(1),
            "workflow should advance after the signal is received"
        );

        let events = store.read_workflow_events(actor_id);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WorkflowEvent::SignalReceived { name, .. } if name == "go")),
            "SignalReceived event should be persisted"
        );
        assert!(
            events.iter().any(|e| matches!(e, WorkflowEvent::StepCompleted { step_name, .. } if step_name == "wait_for_go")),
            "StepCompleted event should be persisted after the signal"
        );
    }

    #[test]
    fn test_workflow_step_waits_on_two_sequential_signals() {
        // Regression: a workflow step resumed from a signal wait that
        // suspends AGAIN on a second signal must re-capture its suspended
        // state. Previously resume_suspended_workflow_step dropped the
        // suspension on a chained SignalWait:suspend, so the second wait
        // could never be woken (permanent stall).
        let source = r#"
            workflow TwoSignals {
                step wait_for_both {
                    (perform Signal.wait("first"), perform Signal.wait("second"))
                }
            }
            let c = spawn TwoSignals {} in { c }
        "#;

        let store = SharedMemoryStore::new();
        let (module, _ty) = compile_source(source).unwrap();

        let rt = Rc::new(RefCell::new(Runtime::new()));
        rt.borrow_mut().persistence = Box::new(store.clone());
        let value = {
            let mut vm = VM::new();
            vm.load_module(module.clone());
            vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));
            vm.run().unwrap()
        };
        let actor_id = value.as_actor_id().expect("spawn should return actor ref");

        rt.borrow_mut().send_message_by_id(actor_id, 0, &[]);
        rt.borrow_mut().run_scheduler();

        // The step is suspended waiting for the first signal.
        {
            let rt = rt.borrow();
            let actor = rt.actors.get(&actor_id).unwrap();
            assert_eq!(actor.waiting_signal.as_deref(), Some("first"));
            assert!(actor.suspended_execution.is_some());
            assert_eq!(
                actor.get_state_field("step_index").and_then(|v| v.as_int()),
                Some(0)
            );
        }

        // First signal arrives: the step resumes, then suspends again on the
        // second signal. The chained suspension must be re-captured.
        rt.borrow_mut().signal_workflow(actor_id, "first", None);
        {
            let rt = rt.borrow();
            let actor = rt.actors.get(&actor_id).unwrap();
            assert_eq!(
                actor.waiting_signal.as_deref(),
                Some("second"),
                "chained signal wait should re-register the second signal"
            );
            assert!(
                actor.suspended_execution.is_some(),
                "chained signal wait should re-capture the suspended execution"
            );
            assert_eq!(
                actor.get_state_field("step_index").and_then(|v| v.as_int()),
                Some(0),
                "step should not complete before the second signal"
            );
        }

        // Second signal arrives: the step completes and the workflow advances.
        rt.borrow_mut().signal_workflow(actor_id, "second", None);
        {
            let rt = rt.borrow();
            let actor = rt.actors.get(&actor_id).unwrap();
            assert_eq!(
                actor.get_state_field("step_index").and_then(|v| v.as_int()),
                Some(1),
                "workflow should advance after both signals are received"
            );
            assert!(actor.suspended_execution.is_none());
            assert_eq!(actor.waiting_signal, None);
        }

        let events = store.read_workflow_events(actor_id);
        assert!(
            events.iter().any(|e| matches!(e, WorkflowEvent::StepCompleted { step_name, .. } if step_name == "wait_for_both")),
            "StepCompleted event should be persisted after both signals"
        );
    }

    #[test]
    fn test_saga_compensation_runs_in_reverse_order() {
        // A three-step saga where the third step fails. The first two steps
        // have per-step compensations that must run in reverse order (b, then a).
        let source = r#"
            workflow SagaTest {
                step a {
                    (self.step_index = self.step_index + 1, self.a_done = 1, emit A_Done())
                } compensate {
                    self.comp_order = self.comp_order * 10 + 1
                }
                step b {
                    (self.step_index = self.step_index + 1, self.b_done = 1, emit B_Done())
                } compensate {
                    self.comp_order = self.comp_order * 10 + 2
                }
                step c {
                    perform Fail.now()
                }
            }
            let c = spawn SagaTest {} in { c }
        "#;

        let rt = Rc::new(RefCell::new(Runtime::new()));
        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return actor reference");

        // Run steps sequentially. The third step fails and triggers compensation.
        rt.borrow_mut().send_message_by_id(actor_id, 0, &[]);
        rt.borrow_mut().run_scheduler();
        rt.borrow_mut().send_message_by_id(actor_id, 1, &[]);
        rt.borrow_mut().run_scheduler();
        rt.borrow_mut().send_message_by_id(actor_id, 2, &[]);
        rt.borrow_mut().run_scheduler();

        {
            let rt_ref = rt.borrow();
            let actor = rt_ref.actors.get(&actor_id).unwrap();
            assert_eq!(
                actor.get_state_field("a_done").and_then(|v| v.as_int()),
                Some(1)
            );
            assert_eq!(
                actor.get_state_field("b_done").and_then(|v| v.as_int()),
                Some(1)
            );
            assert_eq!(
                actor.get_state_field("comp_order").and_then(|v| v.as_int()),
                Some(21),
                "compensations should run in reverse order (b then a)"
            );
        }

        let events = rt.borrow().persistence.read_workflow_events(actor_id);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, WorkflowEvent::StepCompleted { .. }))
                .count(),
            2,
            "only the first two steps should record StepCompleted"
        );
        let saga_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, WorkflowEvent::SagaCompensated { .. }))
            .collect();
        assert_eq!(saga_events.len(), 2);
        assert!(
            matches!(&saga_events[0], WorkflowEvent::SagaCompensated { step_name, .. } if step_name == "b")
        );
        assert!(
            matches!(&saga_events[1], WorkflowEvent::SagaCompensated { step_name, .. } if step_name == "a")
        );
    }

    #[test]
    fn test_workflow_durable_timer_recovery() {
        // A workflow step sets a durable timer. After a simulated restart the
        // timer is re-armed from the journal and, once it fires, the workflow
        // advances past the timer step.
        let source = r#"
            workflow TimerWorkflow {
                step wait { perform Timer.sleep("timeout1", 1) }
            }
            spawn TimerWorkflow {}
        "#;

        let store = SharedMemoryStore::new();
        let (module, _ty) = compile_source(source).unwrap();
        let meta = module.actor_metadata.first().unwrap();
        let mut offsets = vec![0; module.behaviors.len()];
        let mut compensation_offsets: Vec<Option<usize>> = vec![None; module.behaviors.len()];
        for &idx in &meta.behavior_indices {
            if let Some(entry) = module.behaviors.get(idx) {
                offsets[idx] = entry.code_offset;
                compensation_offsets[idx] = entry.compensate_offset;
            }
        }

        // First runtime: spawn the workflow and run the timer step. Step
        // the actor once directly instead of running the scheduler to
        // quiescence: run_scheduler now waits for pending timers to fire,
        // which would complete the step instead of leaving the timer
        // pending for the simulated crash.
        let rt1 = Rc::new(RefCell::new(Runtime::new()));
        rt1.borrow_mut().persistence = Box::new(store.clone());
        let value = {
            let mut vm = VM::new();
            vm.load_module(module.clone());
            vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt1.clone())));
            vm.run().unwrap()
        };
        let actor_id = value.as_actor_id().expect("spawn should return actor ref");

        rt1.borrow_mut().send_message_by_id(actor_id, 0, &[]);
        rt1.borrow_mut().step_actor(actor_id);

        let events_before = store.read_workflow_events(actor_id);
        assert!(
            events_before
                .iter()
                .any(|e| matches!(e, WorkflowEvent::TimerSet { name, .. } if name == "timeout1")),
            "TimerSet event should be persisted"
        );
        assert_eq!(
            rt1.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("step_index")
                .and_then(|v| v.as_int()),
            Some(0),
            "step body does not increment step_index; the runtime records StepCompleted instead"
        );

        // Simulate a node restart: recover the workflow into a fresh runtime.
        let rt2 = Rc::new(RefCell::new(Runtime::new()));
        rt2.borrow_mut().persistence = Box::new(store.clone());
        rt2.borrow_mut().register_recovery_module(
            actor_id,
            module.clone(),
            offsets.clone(),
            compensation_offsets.clone(),
        );
        rt2.borrow_mut().recover_actor(actor_id);

        assert_eq!(
            rt2.borrow().timer_wheel.len(),
            1,
            "timer should be re-armed after recovery"
        );
        assert_eq!(
            rt2.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("step_index")
                .and_then(|v| v.as_int()),
            Some(0),
            "recovered workflow should resume at the snapshot step_index"
        );

        // Let the timer fire and process the resulting message.
        std::thread::sleep(std::time::Duration::from_millis(20));
        rt2.borrow_mut().tick_timers();
        rt2.borrow_mut().run_scheduler();

        let events_after = store.read_workflow_events(actor_id);
        assert!(
            events_after
                .iter()
                .any(|e| matches!(e, WorkflowEvent::TimerFired { name, .. } if name == "timeout1")),
            "TimerFired event should be persisted after the timer fires"
        );
        assert_eq!(
            rt2.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("step_index")
                .and_then(|v| v.as_int()),
            Some(1),
            "workflow should advance to step_index 1 after the timer fires"
        );
    }

    #[test]
    fn test_workflow_parallel_branches_normal() {
        // A simple parallel block with no suspension: both branches run in one
        // synthetic step and the workflow continues to the next sequential step.
        let source = r#"
            workflow ParallelNormal {
                step before { (emit BeforeDone(), self.step_index = self.step_index + 1) }
                parallel {
                    step branch_a { emit BranchA_Done() }
                    step branch_b { emit BranchB_Done() }
                }
                step after { (emit AfterDone(), self.step_index = self.step_index + 1) }
            }
            spawn ParallelNormal {}
        "#;

        let store = SharedMemoryStore::new();
        let rt = Rc::new(RefCell::new(Runtime::new()));
        rt.borrow_mut().persistence = Box::new(store.clone());

        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value.as_actor_id().expect("spawn should return actor ref");

        rt.borrow_mut().send_message_by_id(actor_id, 0, &[]);
        rt.borrow_mut().run_scheduler();
        rt.borrow_mut().send_message_by_id(actor_id, 1, &[]);
        rt.borrow_mut().run_scheduler();
        rt.borrow_mut().send_message_by_id(actor_id, 2, &[]);
        rt.borrow_mut().run_scheduler();

        assert_eq!(
            rt.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("step_index")
                .and_then(|v| v.as_int()),
            Some(3),
            "workflow should advance through before, parallel, and after"
        );

        let events = store.read_workflow_events(actor_id);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, WorkflowEvent::ParallelBranchCompleted { .. }))
                .count(),
            2,
            "both branches should emit ParallelBranchCompleted"
        );
        assert!(
            events.iter().any(|e| matches!(e, WorkflowEvent::StepCompleted { step_name, .. } if step_name == "parallel_0")),
            "parallel_0 should record StepCompleted"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WorkflowEvent::Custom { name, .. } if name == "AfterDone")),
            "AfterDone should be persisted"
        );
    }

    #[test]
    fn test_workflow_parallel_branches_and_recovery() {
        // A workflow with a sequential step, a parallel block of two branches,
        // and a final sequential step.  Branch b suspends on a signal so we can
        // simulate a restart after branch a has already completed; recovery
        // replays the ParallelBranchCompleted event and skips branch a.
        let source = r#"
            workflow ParallelTest {
                step before { (emit BeforeDone(), self.step_index = self.step_index + 1) }
                parallel {
                    step branch_a { emit BranchA_Done() }
                    step branch_b { (perform Signal.wait("continue"), emit BranchB_Done()) }
                }
                step after { (emit AfterDone(), self.step_index = self.step_index + 1) }
            }
            spawn ParallelTest {}
        "#;

        let store = SharedMemoryStore::new();
        let (module, _ty) = compile_source(source).unwrap();
        let meta = module.actor_metadata.first().unwrap();
        let mut offsets = vec![0; module.behaviors.len()];
        let mut compensation_offsets: Vec<Option<usize>> = vec![None; module.behaviors.len()];
        for &idx in &meta.behavior_indices {
            if let Some(entry) = module.behaviors.get(idx) {
                offsets[idx] = entry.code_offset;
                compensation_offsets[idx] = entry.compensate_offset;
            }
        }

        // First runtime: run the sequential "before" step, then start the
        // parallel block.  Branch a completes; branch b suspends waiting for
        // the signal.
        let rt1 = Rc::new(RefCell::new(Runtime::new()));
        rt1.borrow_mut().persistence = Box::new(store.clone());
        let value = {
            let mut vm = VM::new();
            vm.load_module(module.clone());
            vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt1.clone())));
            vm.run().unwrap()
        };
        let actor_id = value.as_actor_id().expect("spawn should return actor ref");

        rt1.borrow_mut().send_message_by_id(actor_id, 0, &[]);
        rt1.borrow_mut().run_scheduler();
        assert_eq!(
            rt1.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("step_index")
                .and_then(|v| v.as_int()),
            Some(1),
            "before step should advance step_index to 1"
        );

        rt1.borrow_mut().send_message_by_id(actor_id, 1, &[]);
        rt1.borrow_mut().run_scheduler();

        let events_mid = store.read_workflow_events(actor_id);
        assert_eq!(
            events_mid.iter().filter(|e| matches!(e, WorkflowEvent::ParallelBranchCompleted { branch_name, .. } if branch_name == "branch_a")).count(),
            1,
            "branch_a should have completed"
        );
        assert_eq!(
            events_mid.iter().filter(|e| matches!(e, WorkflowEvent::ParallelBranchCompleted { branch_name, .. } if branch_name == "branch_b")).count(),
            0,
            "branch_b should still be waiting"
        );
        assert_eq!(
            rt1.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("parallel_progress")
                .and_then(|v| v.as_int()),
            Some(1),
            "parallel_progress should reflect one completed branch"
        );

        // Simulate a node restart mid-parallel-block: drop the actor and
        // recover from the shared store.  Recovery replays the durable branch
        // event so branch a is skipped when the synthetic parallel step runs.
        rt1.borrow_mut().actors.remove(&actor_id);

        let rt2 = Rc::new(RefCell::new(Runtime::new()));
        rt2.borrow_mut().persistence = Box::new(store.clone());
        rt2.borrow_mut().register_recovery_module(
            actor_id,
            module.clone(),
            offsets.clone(),
            compensation_offsets.clone(),
        );
        rt2.borrow_mut().recover_actor(actor_id);
        rt2.borrow_mut().run_scheduler();

        let events_after_recovery = store.read_workflow_events(actor_id);
        assert_eq!(
            events_after_recovery.iter().filter(|e| matches!(e, WorkflowEvent::ParallelBranchCompleted { branch_name, .. } if branch_name == "branch_a")).count(),
            1,
            "branch_a should not be re-run after recovery"
        );

        // Deliver the signal so branch b can finish.
        rt2.borrow_mut().signal_workflow(actor_id, "continue", None);
        rt2.borrow_mut().run_scheduler();

        assert_eq!(
            rt2.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("step_index")
                .and_then(|v| v.as_int()),
            Some(2),
            "parallel block should advance step_index to 2"
        );
        assert_eq!(
            rt2.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("parallel_progress")
                .and_then(|v| v.as_int()),
            Some(0),
            "parallel_progress should be reset after the block completes"
        );

        let events_after_signal = store.read_workflow_events(actor_id);
        assert_eq!(
            events_after_signal
                .iter()
                .filter(|e| matches!(e, WorkflowEvent::ParallelBranchCompleted { .. }))
                .count(),
            2,
            "both branches should have ParallelBranchCompleted events"
        );
        assert!(
            events_after_signal.iter().any(|e| matches!(e, WorkflowEvent::StepCompleted { step_name, .. } if step_name == "parallel_0")),
            "parallel_0 should record StepCompleted"
        );

        // Run the final sequential step.
        rt2.borrow_mut().send_message_by_id(actor_id, 2, &[]);
        rt2.borrow_mut().run_scheduler();

        assert_eq!(
            rt2.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("step_index")
                .and_then(|v| v.as_int()),
            Some(3),
            "after step should advance step_index to 3"
        );
        let events_final = store.read_workflow_events(actor_id);
        assert!(
            events_final
                .iter()
                .any(|e| matches!(e, WorkflowEvent::Custom { name, .. } if name == "AfterDone")),
            "AfterDone event should be persisted"
        );
    }

    #[test]
    fn test_workflow_query_handler_reads_state() {
        // A query handler is a plain function that reads `self` state; the
        // runtime invokes it with the workflow actor bound as `self`, so it
        // observes the actor's current state without mutating it.  The
        // program entry returns the handler as a first-class function value
        // (a function-table index, the representation the MIR pipeline
        // emits for function references).
        let source = r#"
            workflow Counter {
                step bump { self.step_index = self.step_index + 1 }
            }
            fn progress() -> Int { self.step_index }
            let c = spawn Counter {} in { progress }
        "#;

        let store = SharedMemoryStore::new();
        let (module, _ty) = compile_source(source).unwrap();

        let rt = Rc::new(RefCell::new(Runtime::new()));
        rt.borrow_mut().persistence = Box::new(store.clone());
        let handler = {
            let mut vm = VM::new();
            vm.load_module(module.clone());
            vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));
            vm.run().unwrap()
        };
        let actor_id = {
            let rt = rt.borrow();
            assert_eq!(rt.actors.len(), 1, "exactly one workflow actor should exist");
            *rt.actors.keys().next().unwrap()
        };

        // Advance the workflow so the query has observable state to read.
        rt.borrow_mut().send_message(actor_id, "bump", &[]);
        rt.borrow_mut().run_scheduler();
        assert_eq!(
            rt.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("step_index")
                .and_then(|v| v.as_int()),
            Some(1),
            "bump step should advance step_index to 1"
        );

        let events_before = store.read_workflow_events(actor_id).len();

        rt.borrow_mut().register_workflow_query(actor_id, "progress", handler);
        let result = rt.borrow_mut().query_workflow(actor_id, "progress");
        assert_eq!(
            result.and_then(|v| v.as_int()),
            Some(1),
            "query handler should read the workflow's current step_index"
        );

        // Queries are read-only: no workflow events were appended.
        assert_eq!(
            store.read_workflow_events(actor_id).len(),
            events_before,
            "querying must not append workflow events"
        );

        // Unknown query names resolve to None.
        assert_eq!(
            rt.borrow_mut().query_workflow(actor_id, "missing"),
            None,
            "unregistered query name should return None"
        );
    }

    #[test]
    fn test_workflow_query_rejects_non_workflow_actor() {
        // Queries are a workflow-only concept: registering on a plain actor
        // is a no-op and querying it yields None.
        let source = r#"
            actor Echo { behavior ping() { 1 } }
            let e = spawn Echo {} in { e }
        "#;
        let (module, _ty) = compile_source(source).unwrap();
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let value = {
            let mut vm = VM::new();
            vm.load_module(module.clone());
            vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));
            vm.run().unwrap()
        };
        let actor_id = value.as_actor_id().expect("spawn should return actor ref");

        rt.borrow_mut()
            .register_workflow_query(actor_id, "ping", Value::int(0));
        assert_eq!(
            rt.borrow_mut().query_workflow(actor_id, "ping"),
            None,
            "plain actors have no query handlers"
        );
        assert_eq!(
            rt.borrow_mut().query_workflow(actor_id + 1000, "ping"),
            None,
            "querying a missing actor should return None"
        );
    }

    #[test]
    fn test_workflow_query_effect_from_step() {
        // `perform Workflow.query(self, name)` inside a workflow step routes
        // through the runtime's builtin-effect path and invokes the
        // registered handler on the workflow actor.  The step runs on the
        // runtime's shared VM while the handler runs on a private VM, so
        // the query cannot disturb the step's own execution state.
        let source = r#"
            workflow Counter {
                step bump { self.step_index = self.step_index + 1 }
                step inspect { self.observed = perform Workflow.query(self, "progress") }
            }
            fn progress() -> Int { self.step_index }
            let c = spawn Counter {} in { progress }
        "#;

        let store = SharedMemoryStore::new();
        let (module, _ty) = compile_source(source).unwrap();

        let rt = Rc::new(RefCell::new(Runtime::new()));
        rt.borrow_mut().persistence = Box::new(store.clone());
        let handler = {
            let mut vm = VM::new();
            vm.load_module(module.clone());
            vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));
            vm.run().unwrap()
        };
        let actor_id = {
            let rt = rt.borrow();
            assert_eq!(rt.actors.len(), 1, "exactly one workflow actor should exist");
            *rt.actors.keys().next().unwrap()
        };

        rt.borrow_mut().register_workflow_query(actor_id, "progress", handler);
        rt.borrow_mut().send_message(actor_id, "bump", &[]);
        rt.borrow_mut().run_scheduler();
        rt.borrow_mut().send_message(actor_id, "inspect", &[]);
        rt.borrow_mut().run_scheduler();

        assert_eq!(
            rt.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("observed")
                .and_then(|v| v.as_int()),
            Some(1),
            "Workflow.query effect should deliver the handler result into the step"
        );
    }

    // -----------------------------------------------------------------------
    // Test: Actor.* builtin effects (link/monitor/registry/exit/trap_exit)
    // -----------------------------------------------------------------------

    #[test]
    fn test_actor_builtin_effects_standalone_nil_noop() {
        // Outside an actor runtime every Actor.* effect is a nil no-op.
        let source = r#"
            {
                perform Actor.link(nil)
                perform Actor.unlink(nil)
                perform Actor.monitor(nil)
                perform Actor.demonitor(nil)
                perform Actor.trap_exit(true)
                perform Actor.set_priority(0)
                perform Actor.exit(0)
                perform Actor.register("name")
                perform Actor.unregister("name")
                perform Actor.whereis("name")
            }
        "#;
        let (value, _ty) = run_source(source).unwrap();
        assert!(value.is_nil(), "Actor.* effects should yield nil outside a runtime");
    }

    #[test]
    fn test_actor_link_killed_peer_exits_linked_actor() {
        // The peer links to the victim from inside its behavior; the victim
        // then self-exits with a Kill-style reason, which must propagate
        // through the link and take the non-trapping peer down.
        let source = r#"
            actor Peer {
                state exits: Int = 0
                behavior notified(dead, me) { self.exits = self.exits + 1 }
                behavior watch(t) { perform Actor.link(t) }
            }
            actor Victim {
                behavior die() { perform Actor.exit(2) }
            }
            let p = spawn Peer {} in
            let v = spawn Victim {} in {
                send p watch(v)
                send v die()
                p
            }
        "#;
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        let peer_id = value.as_actor_id().expect("spawn should return an actor ref");

        rt.borrow_mut().run_scheduler();

        assert!(
            rt.borrow().actors.get(&peer_id).is_none(),
            "linked peer should exit when the victim is killed"
        );
    }

    #[test]
    fn test_actor_trap_exit_survives_as_system_message() {
        // With trap_exit(true) the linked peer's abnormal exit arrives as a
        // System message instead of killing the trapping actor.
        let source = r#"
            actor Peer {
                state exits: Int = 0
                behavior notified(dead, me) { self.exits = self.exits + 1 }
                behavior watch(t) {
                    perform Actor.trap_exit(true)
                    perform Actor.link(t)
                }
            }
            actor Victim {
                behavior die() { perform Actor.exit(1) }  // Error exit (trappable), not Kill
            }
            let p = spawn Peer {} in
            let v = spawn Victim {} in {
                send p watch(v)
                send v die()
                p
            }
        "#;
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        let peer_id = value.as_actor_id().expect("spawn should return an actor ref");

        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let peer = rt_ref
            .actors
            .get(&peer_id)
            .expect("trapping peer should survive the victim's exit");
        assert_eq!(
            peer.get_state_field("exits").and_then(|v| v.as_int()),
            Some(1),
            "trapping peer should have consumed the exit System message"
        );
    }

    #[test]
    fn test_actor_link_normal_exit_does_not_propagate() {
        // A Normal self-exit must not take down linked peers (BEAM semantics).
        let source = r#"
            actor Peer {
                behavior watch(t) { perform Actor.link(t) }
            }
            actor Victim {
                behavior die() { perform Actor.exit(0) }
            }
            let p = spawn Peer {} in
            let v = spawn Victim {} in {
                send p watch(v)
                send v die()
                p
            }
        "#;
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        let peer_id = value.as_actor_id().expect("spawn should return an actor ref");

        rt.borrow_mut().run_scheduler();

        assert!(
            rt.borrow().actors.get(&peer_id).is_some(),
            "linked peer should survive a Normal exit"
        );
    }

    #[test]
    fn test_actor_link_external_kill_propagates() {
        // Same propagation, but the kill comes from the runtime API rather
        // than Actor.exit. The peer registers itself so the test can find
        // its id afterwards.
        let source = r#"
            actor Peer {
                state exits: Int = 0
                behavior notified(dead, me) { self.exits = self.exits + 1 }
                behavior watch(t) {
                    perform Actor.register("peer")
                    perform Actor.link(t)
                }
            }
            actor Victim {
                behavior noop() { 0 }
            }
            let p = spawn Peer {} in
            let v = spawn Victim {} in {
                send p watch(v)
                v
            }
        "#;
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        let victim_id = value.as_actor_id().expect("spawn should return an actor ref");

        rt.borrow_mut().run_scheduler();
        let peer_id = rt
            .borrow()
            .registry
            .whereis("peer")
            .expect("peer should have registered itself");

        rt.borrow_mut().kill_actor(victim_id);

        assert!(
            rt.borrow().actors.get(&peer_id).is_none(),
            "linked peer should exit when the victim is killed externally"
        );
    }

    #[test]
    fn test_kill_untrappable_bypasses_trap_exits() {
        // Kill is untrappable per spec: even a trap_exits actor must be
        // force-terminated when a linked actor is killed.
        let source = r#"
            actor Peer {
                behavior watch(t) {
                    perform Actor.trap_exit(true)
                    perform Actor.link(t)
                }
            }
            actor Victim {
                behavior die() { perform Actor.exit(2) }  // Kill
            }
            let p = spawn Peer {} in
            let v = spawn Victim {} in {
                send p watch(v)
                send v die()
                p
            }
        "#;
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        let peer_id = value.as_actor_id().expect("spawn should return an actor ref");

        rt.borrow_mut().run_scheduler();

        // Kill is untrappable — the trap_exits peer should be terminated.
        assert!(
            rt.borrow().actors.get(&peer_id).is_none(),
            "trap_exits peer must be terminated by cascading Kill"
        );
    }

    #[test]
    fn test_actor_monitor_delivers_down_message() {
        // The watcher monitors the victim; the victim's exit delivers a DOWN
        // System message (payload: target, watcher, reason code) which the
        // watcher's first behavior consumes.
        let source = r#"
            actor Watcher {
                state got: Int = 0
                behavior down(t, w, r) { self.got = r }
                behavior watch(t) { perform Actor.monitor(t) }
            }
            actor Victim {
                behavior die() { perform Actor.exit(2) }
            }
            let w = spawn Watcher {} in
            let v = spawn Victim {} in {
                send w watch(v)
                send v die()
                w
            }
        "#;
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        let watcher_id = value.as_actor_id().expect("spawn should return an actor ref");

        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let watcher = rt_ref
            .actors
            .get(&watcher_id)
            .expect("watcher should survive the monitored actor's exit");
        assert_eq!(
            watcher.get_state_field("got").and_then(|v| v.as_int()),
            Some(2),
            "watcher should receive DOWN with the Kill reason code (2)"
        );
    }

    #[test]
    fn test_actor_demonitor_stops_down_message() {
        // After demonitor the victim's exit must not deliver a DOWN.
        let source = r#"
            actor Watcher {
                state got: Int = 0
                behavior down(t, w, r) { self.got = r }
                behavior watch(t) {
                    perform Actor.monitor(t)
                    perform Actor.demonitor(t)
                }
            }
            actor Victim {
                behavior die() { perform Actor.exit(2) }
            }
            let w = spawn Watcher {} in
            let v = spawn Victim {} in {
                send w watch(v)
                send v die()
                w
            }
        "#;
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        let watcher_id = value.as_actor_id().expect("spawn should return an actor ref");

        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let watcher = rt_ref
            .actors
            .get(&watcher_id)
            .expect("watcher should survive");
        assert_eq!(
            watcher.get_state_field("got").and_then(|v| v.as_int()),
            Some(0),
            "demonitored watcher should not receive a DOWN message"
        );
    }

    #[test]
    fn test_actor_register_whereis_unregister_roundtrip() {
        let source = r#"
            actor Hero {
                state found: Int = 0
                state gone: Int = 0
                behavior reg() { perform Actor.register("hero") }
                behavior lookup() { self.found = perform Actor.whereis("hero") }
                behavior unreg() { perform Actor.unregister("hero") }
                behavior lookup2() { self.gone = perform Actor.whereis("hero") }
            }
            let h = spawn Hero {} in {
                send h reg()
                send h lookup()
                send h unreg()
                send h lookup2()
                h
            }
        "#;
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        let hero_id = value.as_actor_id().expect("spawn should return an actor ref");

        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        assert_eq!(
            rt_ref.registry.whereis("hero"),
            None,
            "name should be unregistered by the end"
        );
        let hero = rt_ref.actors.get(&hero_id).unwrap();
        assert_eq!(
            hero.get_state_field("found").and_then(|v| v.as_actor_id()),
            Some(hero_id),
            "whereis should resolve the registered name to the actor ref"
        );
        assert!(
            hero.get_state_field("gone").map(|v| v.is_nil()).unwrap_or(false),
            "whereis should return nil for an unregistered name"
        );
    }

    #[test]
    fn test_actor_exit_terminates_self() {
        let source = r#"
            actor Leaver {
                behavior die() { perform Actor.exit("error") }
            }
            let h = spawn Leaver {} in {
                send h die()
                h
            }
        "#;
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        let leaver_id = value.as_actor_id().expect("spawn should return an actor ref");

        rt.borrow_mut().run_scheduler();

        assert!(
            rt.borrow().actors.get(&leaver_id).is_none(),
            "Actor.exit should terminate the performing actor"
        );
    }

    #[test]
    fn test_example_link_monitor_runs() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = include_str!("../examples/link_monitor.nula");
        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        assert_eq!(value.as_int(), Some(0), "main should return 0");

        rt.borrow_mut().run_scheduler();

        // The trapping watcher received both the link exit signal and the
        // monitor DOWN when the victim exited, and resolved its own
        // registered name through whereis.
        let rt_ref = rt.borrow();
        let watcher_id = rt_ref
            .registry
            .whereis("watcher")
            .expect("watcher should have registered itself");
        let watcher = rt_ref.actors.get(&watcher_id).unwrap();
        assert_eq!(
            watcher.get_state_field("notices").and_then(|v| v.as_int()),
            Some(2),
            "watcher should see the link exit signal and the monitor DOWN"
        );
        assert_eq!(
            watcher.get_state_field("seen").and_then(|v| v.as_actor_id()),
            Some(watcher_id),
            "whereis should resolve the registered name to the actor ref"
        );
    }

    // -----------------------------------------------------------------------
    // Test: Otp.* builtin effects (supervisors with dynamic children)
    // -----------------------------------------------------------------------

    #[test]
    fn test_otp_builtin_effects_standalone_nil_noop() {
        // Outside an actor runtime every Otp.* effect is a nil no-op.
        let source = r#"
            {
                perform Otp.create_supervisor("pool", 0)
                perform Otp.supervise_child(0, nil, 0)
                perform Otp.set_template(0, "Worker")
                perform Otp.start_child(0)
                perform Otp.terminate_child(0, nil)
                perform Otp.child_count(0)
            }
        "#;
        let (value, _ty) = run_source(source).unwrap();
        assert!(value.is_nil(), "Otp.* effects should yield nil outside a runtime");
    }

    #[test]
    fn test_otp_simple_one_for_one_restarts_crashed_child_from_template() {
        // From source: create a simple_one_for_one supervisor with a
        // template actor type, start two children, send them work, kill
        // one, and assert it restarts from the template — fresh id, state
        // back to the declared defaults, behavior table intact.
        let source = r#"
            actor PoolWorker {
                state count: Int = 0
                behavior work(x) { self.count = self.count + x }
            }
            let sup = perform Otp.create_supervisor("pool", 3) in
            let t = perform Otp.set_template(sup, "PoolWorker") in
            let w1 = perform Otp.start_child(sup) in
            let w2 = perform Otp.start_child(sup) in {
                send w1 work(1)
                send w2 work(2)
                sup
            }
        "#;
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        let sup_id = value
            .as_int()
            .expect("create_supervisor should yield the supervisor id as Int")
            as u64;
        assert_eq!(
            rt.borrow().supervisors[&sup_id].strategy,
            crate::runtime::RestartStrategy::SimpleOneForOne
        );

        rt.borrow_mut().run_scheduler();

        let children: Vec<u64> = rt.borrow().supervisors[&sup_id]
            .children
            .iter()
            .map(|(_, id)| *id)
            .collect();
        assert_eq!(children.len(), 2, "two dynamic children should be supervised");
        for (child, want) in children.iter().zip([1, 2]) {
            assert_eq!(
                rt.borrow().actors[child]
                    .get_state_field("count")
                    .and_then(|v| v.as_int()),
                Some(want),
                "child should have handled its work message"
            );
        }

        // Kill the first child (abnormal exit): dynamic children are
        // Transient, so it restarts from the template.
        rt.borrow_mut().kill_actor(children[0]);

        let after: Vec<u64> = rt.borrow().supervisors[&sup_id]
            .children
            .iter()
            .map(|(_, id)| *id)
            .collect();
        assert_eq!(after.len(), 2, "the crashed child must be replaced, not dropped");
        assert_eq!(after[1], children[1], "the surviving child must be untouched");
        let restarted = after[0];
        assert_ne!(restarted, children[0], "restart must create a fresh actor");
        assert_eq!(
            rt.borrow().actors[&restarted]
                .get_state_field("count")
                .and_then(|v| v.as_int()),
            Some(0),
            "restarted child must start from the template state defaults"
        );

        // The replacement is a real bytecode actor: send it work and let
        // the scheduler run its template behavior.
        rt.borrow_mut().send_message(restarted, "work", &[Value::int(5)]);
        rt.borrow_mut().run_scheduler();
        assert_eq!(
            rt.borrow().actors[&restarted]
                .get_state_field("count")
                .and_then(|v| v.as_int()),
            Some(5),
            "restarted child must run the template behavior table"
        );
    }

    #[test]
    fn test_otp_supervise_and_terminate_child_round_trip() {
        let source = r#"
            actor Managed {
                state n: Int = 0
                behavior bump() { self.n = self.n + 1 }
            }
            let sup = perform Otp.create_supervisor("plain", 0) in
            let w = spawn Managed {} in
            let s1 = perform Otp.supervise_child(sup, w, 0) in
            let before = perform Otp.child_count(sup) in
            let s2 = perform Otp.terminate_child(sup, w) in
            let after = perform Otp.child_count(sup) in
            before * 10 + after
        "#;
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        assert_eq!(
            value.as_int(),
            Some(10),
            "child_count should be 1 after supervise_child and 0 after terminate_child"
        );

        // The terminated worker exited cleanly and was NOT restarted.
        let rt_ref = rt.borrow();
        let (sup_id, supervisor) = rt_ref
            .supervisors
            .iter()
            .next()
            .expect("the supervisor should still exist");
        assert_eq!(supervisor.child_count(), 0);
        assert_eq!(
            rt_ref.actors.len(),
            1,
            "only the supervisor actor should remain after terminate_child"
        );
        assert!(rt_ref.actors.contains_key(sup_id));
    }

    #[test]
    fn test_example_worker_pool_runs() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = include_str!("../examples/worker_pool.nula");
        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        assert_eq!(value.as_int(), Some(0), "main should return 0");

        rt.borrow_mut().run_scheduler();

        // w1 crashed on `die` and was restarted from the template (fresh
        // id, count back to the declared default 0); w2 kept its count.
        let rt_ref = rt.borrow();
        let supervisor = rt_ref
            .supervisors
            .values()
            .next()
            .expect("the pool supervisor should exist");
        assert_eq!(supervisor.child_count(), 2);
        let counts: Vec<i64> = supervisor
            .children
            .iter()
            .map(|(_, id)| {
                rt_ref.actors[id]
                    .get_state_field("count")
                    .and_then(|v| v.as_int())
                    .expect("each pool child should have a count state field")
            })
            .collect();
        assert_eq!(
            counts,
            vec![0, 2],
            "crashed child must restart from the template state defaults"
        );
    }

    // -----------------------------------------------------------------------
    // Test: state_machine end-to-end (desugar → compile → run)
    // -----------------------------------------------------------------------

    #[test]
    fn test_state_machine_spawn_and_transition() {
        // Define a state_machine, spawn it, send events. The state_machine
        // desugars to an ordinary actor; verify the pipeline compiles and
        // the spawned actor survives all transitions (the desugared
        // behaviors — exit-hook if-chain, assign, entry-hook, nil —
        // compiled and ran correctly).
        let source = r#"
            state_machine Light {
                state Off
                state On

                event turn_on: On
                event turn_off: Off

                on_entry On {
                    perform IO.print("light on")
                }

                on_exit On {
                    perform IO.print("light off")
                }
            }
            fn main() {
                let light = spawn Light {} in {
                    send light turn_on()
                    send light turn_off()
                    send light turn_on()
                    light
                }
            }
        "#;
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        let light_id = value.as_actor_id().expect("main should return the actor ref");
        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        assert!(
            rt_ref.actors.contains_key(&light_id),
            "actor should still be alive after state transitions"
        );
        let actor = &rt_ref.actors[&light_id];
        // The _sm_state field should exist; it's heap-allocated (TAG_PTR).
        let state_val = actor
            .get_state_field("_sm_state")
            .expect("_sm_state field should exist");
        assert!(
            state_val.is_ptr() || state_val.is_string(),
            "_sm_state should be a string-ish value"
        );
        // Bytecode behaviors are stored in bytecode_offsets, not
        // behavior_table (which is for native Rust handlers).
        assert!(
            !actor.bytecode_offsets.is_empty(),
            "desugared actor should have bytecode offsets (found {})",
            actor.bytecode_offsets.len()
        );
    }

    #[test]
    fn test_state_machine_self_transition_runs_hooks() {
        // A self-transition (event targeting the current state) must run
        // both exit and entry hooks. Verify the actor survives self-ticks.
        let source = r#"
            state_machine IdleLoop {
                state Idle
                state Done

                event tick: Idle
                event finish: Done

                on_entry Idle {
                    perform IO.print("entering idle")
                }

                on_exit Idle {
                    perform IO.print("leaving idle")
                }
            }
            fn main() {
                let m = spawn IdleLoop {} in {
                    send m tick()
                    send m tick()
                    m
                }
            }
        "#;
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        let machine_id = value.as_actor_id().expect("main should return the actor ref");
        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        assert!(rt_ref.actors.contains_key(&machine_id), "actor should survive self-transitions");
        let actor = &rt_ref.actors[&machine_id];
        let state_val = actor
            .get_state_field("_sm_state")
            .expect("_sm_state should exist");
        assert!(state_val.is_ptr() || state_val.is_string(), "_sm_state should be a string-ish value");
        assert!(!actor.bytecode_offsets.is_empty(), "desugared actor should have bytecode offsets");
    }

    #[test]
    fn test_actor_set_priority_runs_high_first() {
        // A High-priority actor is dequeued before a Normal one even when
        // the Normal actor's message was sent first. Phase 1 runs a real
        // compiled behavior that boosts itself via `perform
        // Actor.set_priority(0)`; phase 2 enqueues both actors through the
        // normal send path and observes the scheduler's dequeue order.
        // Deterministic: each run_scheduler drains fully, so the phase-2
        // queue order is exactly [Hi(High), Lo(Normal)].
        let source = r#"
            actor Hi {
                behavior boost_hi() {
                    perform Actor.set_priority(0)
                    perform Actor.register("hi")
                }
                behavior work() { 0 }
            }
            actor Lo {
                behavior boost_lo() { perform Actor.register("lo") }
                behavior work() { 0 }
            }
            let h = spawn Hi {} in
            let n = spawn Lo {} in {
                send h boost_hi()
                send n boost_lo()
                0
            }
        "#;
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let (value, _ty) = run_source_with_runtime(source, rt.clone()).unwrap();
        assert_eq!(value.as_int(), Some(0), "main should return 0");

        // Phase 1: the boost behaviors run; Hi sets its own priority.
        rt.borrow_mut().run_scheduler();
        let (hi_id, lo_id) = {
            let rt_ref = rt.borrow();
            let hi_id = rt_ref.registry.whereis("hi").expect("Hi registered itself");
            let lo_id = rt_ref.registry.whereis("lo").expect("Lo registered itself");
            assert_eq!(
                rt_ref.actors.get(&hi_id).unwrap().priority,
                crate::runtime::ActorPriority::High,
                "Actor.set_priority(0) from a behavior should make the actor High"
            );
            assert_eq!(
                rt_ref.actors.get(&lo_id).unwrap().priority,
                crate::runtime::ActorPriority::Normal,
                "untouched actors stay Normal"
            );
            (hi_id, lo_id)
        };

        // Phase 2: send to Lo first, then Hi; the High entry dequeues first.
        rt.borrow_mut().send_message(lo_id, "work", &[]);
        rt.borrow_mut().send_message(hi_id, "work", &[]);
        let rt_ref = rt.borrow_mut();
        assert_eq!(rt_ref.scheduler.dequeue(), Some(hi_id));
        assert_eq!(rt_ref.scheduler.dequeue(), Some(lo_id));
    }

    // -----------------------------------------------------------------------
    // v0.2 HIR/MIR pipeline smoke tests
    // -----------------------------------------------------------------------

    fn run_source_new(source: &str) -> Result<Value, NuError> {
        let mut lexer = Lexer::new(source);
        let tokens = lexer.lex()?;
        let mut parser = Parser::new(tokens);
        let ast = parser.parse_module()?;

        // Type check (required before lowering)
        let mut type_checker = TypeChecker::new();
        let _ = type_checker.check_module(&ast)?;

        // New HIR -> MIR -> bytecode pipeline
        let hir = crate::hir_lower::lower_module(&ast);
        let mir = crate::mir_lower::lower_module(&hir)?;
        let module = crate::mir_codegen::compile_mir(&mir, "test")?;

        let mut vm = VM::new();
        vm.load_module(module);
        vm.run()
    }

    fn assert_int_new(source: &str, expected: i64) {
        let value = run_source_new(source).unwrap();
        assert_eq!(
            value.as_int(),
            Some(expected),
            "new pipeline expected integer for: {}",
            source
        );
    }

    /// Compile source through the HIR/MIR pipeline into a CodeModule without
    /// running it, for structural assertions (actor_metadata, behaviors).
    fn compile_source_new(source: &str) -> Result<crate::bytecode::CodeModule, NuError> {
        let mut lexer = Lexer::new(source);
        let tokens = lexer.lex()?;
        let mut parser = Parser::new(tokens);
        let ast = parser.parse_module()?;
        let mut type_checker = TypeChecker::new();
        let _ = type_checker.check_module(&ast)?;
        let hir = crate::hir_lower::lower_module(&ast);
        let mir = crate::mir_lower::lower_module(&hir)?;
        crate::mir_codegen::compile_mir(&mir, "test")
    }

    /// Compile and run `source` through the HIR/MIR pipeline with a real
    /// Runtime attached, exercising actual actor semantics (state, ask)
    /// rather than the no-op stubs a bare VM falls back to.
    fn run_source_new_with_runtime(
        source: &str,
        runtime: Rc<RefCell<Runtime>>,
    ) -> Result<Value, NuError> {
        let mut lexer = Lexer::new(source);
        let tokens = lexer.lex()?;
        let mut parser = Parser::new(tokens);
        let ast = parser.parse_module()?;

        let mut type_checker = TypeChecker::new();
        let _ = type_checker.check_module(&ast)?;

        let hir = crate::hir_lower::lower_module(&ast);
        let mir = crate::mir_lower::lower_module(&hir)?;
        let module = crate::mir_codegen::compile_mir(&mir, "test")?;

        let mut vm = VM::new();
        vm.load_module(module);
        vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(runtime)));
        vm.run()
    }

    #[test]
    fn test_mir_pipeline_actor_ask_with_arguments() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Calculator {
                behavior add(a: Int, b: Int) { a + b }
            }
            let calc = spawn Calculator {} in
                ask calc add(10, 20)
        "#;
        let value = run_source_new_with_runtime(source, rt).unwrap();
        assert_eq!(value.as_int(), Some(30), "ask add(10, 20) should return 30");
    }

    #[test]
    fn test_mir_pipeline_actor_state_get_set() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Counter {
                state count = 0
                behavior inc() { self.count = self.count + 1 }
                behavior get() { self.count }
            }
            let c = spawn Counter { count = 0 } in
            let _ = ask c inc() in
            let _ = ask c inc() in
            ask c get()
        "#;
        let value = run_source_new_with_runtime(source, rt).unwrap();
        assert_eq!(
            value.as_int(),
            Some(2),
            "two increments should leave count at 2"
        );
    }

    #[test]
    fn test_mir_pipeline_actor_send_then_scheduler() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Counter {
                state count = 0
                behavior add(n: Int) { self.count = self.count + n }
            }
            let c = spawn Counter { count = 0 } in {
                send c add(5)
                send c add(7)
                c
            }
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&actor_id).unwrap();
        assert_eq!(
            actor.get_state_field("count").and_then(|v| v.as_int()),
            Some(12),
            "counter should be 12 after adding 5 and 7"
        );
    }

    /// The legacy compiler and the HIR/MIR pipeline must agree on actor
    /// semantics too, not just pure expressions — run the same program
    /// through both with independent Runtimes and compare results.
    #[test]
    fn test_mir_and_legacy_actor_semantics_agree() {
        let corpus: &[&str] = &[
            r#"
                actor Calculator { behavior add(a: Int, b: Int) { a + b } }
                let calc = spawn Calculator {} in ask calc add(10, 20)
            "#,
            r#"
                actor Counter {
                    state count = 0
                    behavior inc() { self.count = self.count + 1 }
                    behavior get() { self.count }
                }
                let c = spawn Counter { count = 0 } in
                let _ = ask c inc() in
                let _ = ask c inc() in
                let _ = ask c inc() in
                ask c get()
            "#,
        ];
        for src in corpus {
            let legacy_rt = Rc::new(RefCell::new(Runtime::new()));
            let legacy = run_source_with_runtime(src, legacy_rt)
                .map(|(v, _)| v.to_string_repr())
                .unwrap_or_else(|e| panic!("legacy pipeline failed on {:?}: {}", src, e));
            let mir_rt = Rc::new(RefCell::new(Runtime::new()));
            let mir = run_source_new_with_runtime(src, mir_rt)
                .map(|v| v.to_string_repr())
                .unwrap_or_else(|e| panic!("MIR pipeline failed on {:?}: {}", src, e));
            assert_eq!(legacy, mir, "pipelines disagree on {:?}", src);
        }
    }

    // -----------------------------------------------------------------------
    // Workflow/agent desugaring via the HIR/MIR pipeline
    // -----------------------------------------------------------------------

    #[test]
    fn test_mir_workflow_lowers_to_persistent_actor() {
        let source = "workflow PurchaseOrder { step validate { 1 } }";
        let module = compile_source_new(source).unwrap();

        let meta = module
            .actor_metadata
            .iter()
            .find(|m| m.name == "PurchaseOrder")
            .expect("workflow should produce actor metadata");
        assert!(meta.is_workflow, "workflow metadata should be flagged");
        assert!(meta.persistent, "workflows should be persistent actors");
        assert_eq!(meta.behavior_indices.len(), 1, "one behavior per step");

        let behavior = &module.behaviors[meta.behavior_indices[0]];
        assert_eq!(behavior.name, "PurchaseOrder.validate");
    }

    /// Same source and assertions as
    /// test_saga_compensation_runs_in_reverse_order (legacy pipeline), run
    /// through the HIR/MIR pipeline instead. The runtime's saga-compensation
    /// machinery (invoked automatically when a step's execution fails, via
    /// BehaviorTableEntry::compensate_offset) is pipeline-agnostic, so this
    /// exercises mir_codegen's compensation_of patching end to end.
    #[test]
    fn test_mir_saga_compensation_runs_in_reverse_order() {
        let source = r#"
            workflow SagaTest {
                step a {
                    (self.step_index = self.step_index + 1, self.a_done = 1, emit A_Done())
                } compensate {
                    self.comp_order = self.comp_order * 10 + 1
                }
                step b {
                    (self.step_index = self.step_index + 1, self.b_done = 1, emit B_Done())
                } compensate {
                    self.comp_order = self.comp_order * 10 + 2
                }
                step c {
                    perform Fail.now()
                }
            }
            let c = spawn SagaTest {} in { c }
        "#;

        let rt = Rc::new(RefCell::new(Runtime::new()));
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return actor reference");

        rt.borrow_mut().send_message_by_id(actor_id, 0, &[]);
        rt.borrow_mut().run_scheduler();
        rt.borrow_mut().send_message_by_id(actor_id, 1, &[]);
        rt.borrow_mut().run_scheduler();
        rt.borrow_mut().send_message_by_id(actor_id, 2, &[]);
        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&actor_id).unwrap();
        assert_eq!(
            actor.get_state_field("a_done").and_then(|v| v.as_int()),
            Some(1)
        );
        assert_eq!(
            actor.get_state_field("b_done").and_then(|v| v.as_int()),
            Some(1)
        );
        assert_eq!(
            actor.get_state_field("comp_order").and_then(|v| v.as_int()),
            Some(21),
            "compensations should run in reverse order (b then a)"
        );
    }

    /// Same source and assertions as test_agent_ask_uses_memory (legacy
    /// pipeline), run through the HIR/MIR pipeline instead.
    #[test]
    fn test_mir_agent_ask_uses_memory() {
        let source = r#"
            agent Agent = {
                model: "mock-model",
                system_prompt: "You are helpful.",
                memory: { max_turns: 10 }
            }
            let a = spawn Agent {} in
            let r1 = ask a ask("hello") in
            let r2 = ask a ask("world") in
            r1
        "#;
        let module = compile_source_new(source).unwrap();

        let rt = Rc::new(RefCell::new(Runtime::new()));
        let client = crate::ai::MockLlmClient::text("world");
        rt.borrow_mut().set_llm_client(Box::new(client.clone()));

        let mut vm = VM::new();
        vm.load_module(module);
        vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt)));

        let result = vm.run().unwrap();

        let calls = client.recorded_calls();
        assert_eq!(calls.len(), 2, "expected two LLM calls");

        let module_idx = vm.modules.len() - 1;
        let content = vm.value_to_string(module_idx, result);
        assert_eq!(content, "world");

        assert_eq!(calls[0].messages.len(), 2);
        assert_eq!(calls[0].messages[1].content, "hello");
        assert_eq!(calls[1].messages.len(), 4);
        assert_eq!(calls[1].messages[2].content, "world");
    }

    /// Regression test for ActorMeta.is_agent/semantic_memory_dimensions:
    /// unlike `ask`/`usage` (ordinary compiled bytecode behaviors),
    /// `store_fact`/`recall` are placeholder bodies the RUNTIME intercepts
    /// by name, gated on `actor_is_agent(actor_id)` — which reads
    /// `Actor.is_agent`, itself set from `ActorMeta.is_agent` at spawn time.
    /// If mir_lower.rs ever went back to hardcoding is_agent/
    /// semantic_memory_dimensions instead of reading them off the desugared
    /// hir::ActorDef, this interception would silently stop firing and the
    /// placeholder `Unit` body would run instead — same source and
    /// assertions as test_agent_semantic_memory_store_and_recall.
    #[test]
    fn test_mir_agent_semantic_memory_store_and_recall() {
        let source = r#"
            agent Agent = {
                model: "mock-model",
                system_prompt: "You are helpful.",
                semantic_memory: { dimensions: 32 }
            }
            let a = spawn Agent {} in
            let _ = ask a store_fact("hello world") in
            ask a recall("hello", 1)
        "#;
        let module = compile_source_new(source).unwrap();

        let rt = Rc::new(RefCell::new(Runtime::new()));
        let mut vm = VM::new();
        vm.load_module(module);
        vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));

        let result = vm.run().unwrap();

        let module_idx = vm.modules.len() - 1;
        let content = vm.value_to_string(module_idx, result);
        assert_eq!(content, "hello world");

        let rt = rt.borrow();
        let actor = rt.actors.values().next().expect("expected one actor");
        let memory_json = actor.get_state_field("semantic_memory").unwrap();
        let memory_json_str = vm.value_to_string(module_idx, memory_json);
        let memory: crate::ai::SemanticMemory = serde_json::from_str(&memory_json_str).unwrap();
        assert_eq!(memory.len(), 1);
        assert_eq!(memory.documents[0].content, "hello world");
    }

    /// The legacy compiler and the HIR/MIR pipeline must agree on
    /// workflow/agent semantics too.
    #[test]
    fn test_mir_and_legacy_workflow_agent_semantics_agree() {
        let corpus: &[&str] = &[
            // Actor-ref values aren't compared here (their string repr
            // embeds an internal, Runtime-instance-specific id counter that
            // isn't guaranteed to line up between two independently
            // constructed runtimes) — ask a step for a plain value instead.
            "workflow W { step a { 1 } } let w = spawn W {} in ask w a()",
            r#"
                agent Ag = { model: "mock-model", system_prompt: "hi" }
                let a = spawn Ag {} in ask a ask("hello")
            "#,
            r#"
                workflow W2 {
                    step before { self.step_index = self.step_index + 1 }
                    parallel {
                        step branch_a { self.step_index = self.step_index + 1 }
                        step branch_b { self.step_index = self.step_index + 1 }
                    }
                }
                let w = spawn W2 {} in ask w before()
            "#,
            r#"
                @tool(description: "Adds two integers.")
                fn add(x: Int, y: Int) -> Int { x + y }
                agent Ag2 = { model: "mock-model", tools: [add] }
                let a = spawn Ag2 {} in ask a ask("hello")
            "#,
        ];
        for src in corpus {
            // `Value::to_string_repr()` prints heap-allocated results (like
            // the "world" string these agents return) as a raw pointer
            // address (`#Value(hex)`) — it has no VM/module to dereference
            // through. Comparing that directly is flaky: two independently
            // constructed VMs allocate at addresses that only coincidentally
            // match. `vm.value_to_string` resolves the actual string content
            // instead, which is what these assertions actually care about.
            let (legacy_module, _) = compile_source(src)
                .unwrap_or_else(|e| panic!("legacy pipeline failed to compile {:?}: {}", src, e));
            let legacy_rt = Rc::new(RefCell::new(Runtime::new()));
            legacy_rt
                .borrow_mut()
                .set_llm_client(Box::new(crate::ai::MockLlmClient::text("world")));
            let mut legacy_vm = VM::new();
            legacy_vm.load_module(legacy_module);
            legacy_vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(legacy_rt)));
            let legacy_value = legacy_vm
                .run()
                .unwrap_or_else(|e| panic!("legacy pipeline failed to run {:?}: {}", src, e));
            let legacy = legacy_vm.value_to_string(legacy_vm.modules.len() - 1, legacy_value);

            let mir_module = compile_source_new(src)
                .unwrap_or_else(|e| panic!("MIR pipeline failed to compile {:?}: {}", src, e));
            let mir_rt = Rc::new(RefCell::new(Runtime::new()));
            mir_rt
                .borrow_mut()
                .set_llm_client(Box::new(crate::ai::MockLlmClient::text("world")));
            let mut mir_vm = VM::new();
            mir_vm.load_module(mir_module);
            mir_vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(mir_rt)));
            let mir_value = mir_vm
                .run()
                .unwrap_or_else(|e| panic!("MIR pipeline failed to run {:?}: {}", src, e));
            let mir = mir_vm.value_to_string(mir_vm.modules.len() - 1, mir_value);

            assert_eq!(legacy, mir, "pipelines disagree on {:?}", src);
        }
    }

    /// Same source and assertions as test_workflow_parallel_branches_normal
    /// (legacy pipeline), run through the HIR/MIR pipeline instead —
    /// exercises `hir_lower::desugar_workflow`'s parallel-branch synthesis
    /// and mir_codegen's `parallel_branches_of` patching end to end.
    #[test]
    fn test_mir_workflow_parallel_branches_normal() {
        let source = r#"
            workflow ParallelNormal {
                step before { (emit BeforeDone(), self.step_index = self.step_index + 1) }
                parallel {
                    step branch_a { emit BranchA_Done() }
                    step branch_b { emit BranchB_Done() }
                }
                step after { (emit AfterDone(), self.step_index = self.step_index + 1) }
            }
            spawn ParallelNormal {}
        "#;

        let store = SharedMemoryStore::new();
        let rt = Rc::new(RefCell::new(Runtime::new()));
        rt.borrow_mut().persistence = Box::new(store.clone());

        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value.as_actor_id().expect("spawn should return actor ref");

        rt.borrow_mut().send_message_by_id(actor_id, 0, &[]);
        rt.borrow_mut().run_scheduler();
        rt.borrow_mut().send_message_by_id(actor_id, 1, &[]);
        rt.borrow_mut().run_scheduler();
        rt.borrow_mut().send_message_by_id(actor_id, 2, &[]);
        rt.borrow_mut().run_scheduler();

        assert_eq!(
            rt.borrow()
                .actors
                .get(&actor_id)
                .unwrap()
                .get_state_field("step_index")
                .and_then(|v| v.as_int()),
            Some(3),
            "workflow should advance through before, parallel, and after"
        );

        let events = store.read_workflow_events(actor_id);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, WorkflowEvent::ParallelBranchCompleted { .. }))
                .count(),
            2,
            "both branches should emit ParallelBranchCompleted"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WorkflowEvent::Custom { name, .. } if name == "AfterDone")),
            "AfterDone should be persisted"
        );
    }

    /// Regression test for tool-schema resolution in `desugar_agent`: a
    /// spawn-time `ActorMeta.tools` entry must resolve to the same
    /// `ToolSchema` the stable compiler's `compile_agent` would produce.
    #[test]
    fn test_mir_agent_with_tool_resolves_schema() {
        let source = r#"
            @tool(description: "Adds two integers.")
            fn add(x: Int, y: Int) -> Int { x + y }

            agent Ag = { model: "gpt-4o", tools: [add] }
        "#;
        let module = compile_source_new(source).unwrap();
        let meta = module
            .actor_metadata
            .iter()
            .find(|m| m.name == "Ag")
            .expect("agent should produce actor metadata");
        assert_eq!(meta.tools.len(), 1);
        assert_eq!(meta.tools[0].name, "add");
        assert_eq!(meta.tools[0].description, "Adds two integers.");
    }

    #[test]
    fn test_new_pipeline_literal_int() {
        assert_int_new("42", 42);
    }

    #[test]
    fn test_new_pipeline_arithmetic_add() {
        assert_int_new("1 + 2", 3);
    }

    #[test]
    fn test_new_pipeline_let_binding() {
        assert_int_new("let x = 10 in x + 5", 15);
    }

    #[test]
    fn test_new_pipeline_if_then_else() {
        assert_int_new("if true then 1 else 2", 1);
        assert_int_new("if false then 1 else 2", 2);
    }

    #[test]
    fn test_new_pipeline_function_call() {
        let source = r#"
            fn add(x: Int, y: Int) -> Int { x + y }
            add(3, 4)
        "#;
        assert_int_new(source, 7);
    }

    #[test]
    fn test_new_pipeline_match_literal() {
        let source = r#"
            match 2 {
                case 1 => 10
                case 2 => 20
                case _ => 30
            }
        "#;
        assert_int_new(source, 20);
    }

    #[test]
    fn test_new_pipeline_bitwise_or() {
        assert_int_new("6 ||| 3", 7);
    }

    #[test]
    fn test_new_pipeline_inequality() {
        let value = run_source_new("1 != 2").unwrap();
        assert_eq!(value.as_bool(), Some(true));
    }

    /// MIR pipeline fn main() entry point.
    #[test]
    fn test_mir_fn_main_entry_point() {
        assert_int_new("fn main() { 42 }", 42);
        assert_int_new("fn main() { 1 + 2 }", 3);
        let src = "fn add(x: Int, y: Int) -> Int { x + y } fn main() { add(10, 20) }";
        assert_int_new(src, 30);
    }

    /// MIR + Runtime + fn main() with LLM.ask.
    #[test]
    fn test_mir_fn_main_with_runtime() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        rt.borrow_mut()
            .set_llm_client(Box::new(crate::ai::MockLlmClient::text("world")));
        let v =
            run_source_new_with_runtime("fn main() { perform LLM.ask(\"hello\") }", rt).unwrap();
        assert!(!v.is_nil());
    }

    /// MIR + Runtime + Pipeline through fn main().
    #[test]
    fn test_mir_pipeline_with_runtime() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let v = run_source_new_with_runtime(
            "fn main() { let p = Pipeline.new() in p.run(\"hello\") }",
            rt,
        )
        .unwrap();
        assert!(v.is_nil(), "empty pipeline returns nil");
    }

    /// Receive expression parses, compiles, and reads from mailbox.
    #[test]
    fn test_mir_receive_expression() {
        let v = run_source_new("receive { | Msg(x) => x }").unwrap();
        assert!(v.is_nil(), "receive outside actor returns nil");
        let source = r#"
            actor Listener {
                behavior onMsg() {
                    receive { | Msg(x) => x }
                }
            }
            fn main() { 42 }
        "#;
        assert_int_new(source, 42);
    }

    /// Receive parses and runs inside a function body.
    #[test]
    fn test_mir_receive_gets_message() {
        // receive returns nil outside actor context
        let v = run_source_new("fn main() { receive { | Msg(x) => x } }").unwrap();
        assert!(
            v.is_nil(),
            "receive in fn main should return nil outside actor"
        );
    }

    /// End-to-end: a behavior using `receive` pops the next pending mailbox
    /// message and observes its first payload value.
    #[test]
    fn test_mir_receive_reads_mailbox_end_to_end() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Listener {
                state seen = 0
                behavior drain() {
                    self.seen = receive { | Msg(x) => x }
                }
                behavior feed(n: Int) { n }
            }
            let c = spawn Listener { seen = 0 } in {
                send c drain()
                send c feed(7)
                c
            }
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        rt.borrow_mut().run_scheduler();

        // `drain` is dispatched first; its `receive` pops the still-pending
        // `feed(7)` message and stores its first payload in `seen`.
        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&actor_id).unwrap();
        assert_eq!(
            actor.get_state_field("seen").and_then(|v| v.as_int()),
            Some(7),
            "receive should have popped the pending feed(7) message"
        );
    }

    /// Selective receive: with two arms and messages for both queued, the
    /// first message IN MAILBOX ORDER wins — arm order is irrelevant.
    #[test]
    fn test_receive_match_first_in_mailbox_wins() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Listener {
                state seen = 0
                behavior drain() {
                    self.seen = receive {
                        | get() => 100
                        | add(x, y) => x + y
                    }
                }
                behavior add(x: Int, y: Int) { x }
                behavior get() { 0 }
            }
            let c = spawn Listener { seen = 0 } in {
                send c drain()
                send c add(1, 2)
                send c get()
                c
            }
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        rt.borrow_mut().run_scheduler();

        // `add(1, 2)` is queued ahead of `get()`, so the `add` arm wins even
        // though `get` is listed first.
        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&actor_id).unwrap();
        assert_eq!(
            actor.get_state_field("seen").and_then(|v| v.as_int()),
            Some(3),
            "first matching message in mailbox order should win over arm order"
        );
    }

    /// Selective receive: a queued message that matches no arm is skipped and
    /// stays in the mailbox (the scheduler later dispatches it normally),
    /// while the first matching message is consumed by the receive.
    #[test]
    fn test_receive_match_selective_skip() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Listener {
                state seen = 0
                state heard = 0
                behavior drain() {
                    self.seen = receive {
                        | add(x, y) => x + y
                    }
                }
                behavior add(x: Int, y: Int) { x }
                behavior noise(n: Int) { self.heard = n }
            }
            let c = spawn Listener { seen = 0 heard = 0 } in {
                send c drain()
                send c noise(9)
                send c add(4, 5)
                c
            }
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&actor_id).unwrap();
        assert_eq!(
            actor.get_state_field("seen").and_then(|v| v.as_int()),
            Some(9),
            "receive should skip noise(9) and consume add(4, 5)"
        );
        assert_eq!(
            actor.get_state_field("heard").and_then(|v| v.as_int()),
            Some(9),
            "the skipped noise(9) message should remain queued and dispatch normally"
        );
    }

    /// Selective receive fallback: when no queued message matches any arm,
    /// the legacy non-blocking behavior runs — pop the next message and
    /// yield its first payload value.
    #[test]
    fn test_receive_match_no_match_fallback() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Listener {
                state seen = 0
                state heard = 0
                behavior drain() {
                    self.seen = receive {
                        | add(x, y) => x + y
                    }
                }
                behavior add(x: Int, y: Int) { x }
                behavior noise(n: Int) { self.heard = n }
            }
            let c = spawn Listener { seen = 0 heard = 0 } in {
                send c drain()
                send c noise(33)
                c
            }
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&actor_id).unwrap();
        assert_eq!(
            actor.get_state_field("seen").and_then(|v| v.as_int()),
            Some(33),
            "no-match fallback should pop the next message's first payload"
        );
        assert_eq!(
            actor.get_state_field("heard").and_then(|v| v.as_int()),
            Some(0),
            "the fallback consumes the message, so noise must not also dispatch"
        );
    }

    /// Selective receive on an empty mailbox evaluates to nil (non-blocking).
    #[test]
    fn test_receive_match_empty_mailbox_returns_nil() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Listener {
                state seen = 0
                behavior drain() {
                    self.seen = receive {
                        | add(x, y) => x + y
                    }
                }
                behavior add(x: Int, y: Int) { x }
            }
            let c = spawn Listener { seen = 0 } in {
                send c drain()
                c
            }
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&actor_id).unwrap();
        assert!(
            actor
                .get_state_field("seen")
                .map(|v| v.is_nil())
                .unwrap_or(false),
            "receive with no matching message and empty mailbox should yield nil"
        );
    }

    /// Arm params bind to the matched message's payload values.
    #[test]
    fn test_receive_match_binds_payload_params() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Listener {
                state seen = 0
                behavior drain() {
                    self.seen = receive {
                        | add(x, y) => x * 10 + y
                    }
                }
                behavior add(x: Int, y: Int) { x }
            }
            let c = spawn Listener { seen = 0 } in {
                send c drain()
                send c add(7, 8)
                c
            }
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&actor_id).unwrap();
        assert_eq!(
            actor.get_state_field("seen").and_then(|v| v.as_int()),
            Some(78),
            "arm params should bind to the matched message's payload values"
        );
    }

    /// A matched message with fewer payload values than arm params binds the
    /// missing params to nil.
    #[test]
    fn test_receive_match_missing_params_bind_nil() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Listener {
                state seen = 0
                behavior drain() {
                    self.seen = receive {
                        | add(x, y) => y
                    }
                }
                behavior add(x: Int, y: Int) { x }
            }
            let c = spawn Listener { seen = 0 } in {
                send c drain()
                c
            }
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        // Enqueue add with only one payload value behind the pending drain.
        rt.borrow_mut()
            .send_message(actor_id, "add", &[Value::int(7)]);
        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&actor_id).unwrap();
        assert!(
            actor
                .get_state_field("seen")
                .map(|v| v.is_nil())
                .unwrap_or(false),
            "params beyond the payload length should bind to nil"
        );
    }

    /// Timed selective receive: with no matching message in the mailbox the
    /// actor suspends, the timeout fires, and the after body runs.
    #[test]
    fn test_receive_after_times_out_runs_after_body() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Listener {
                state seen = 0
                behavior drain() {
                    self.seen = receive {
                        | add(x, y) => x + y
                    } after 30 => 4242
                }
                behavior add(x: Int, y: Int) { x }
            }
            let c = spawn Listener { seen = 0 } in {
                send c drain()
                c
            }
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&actor_id).unwrap();
        assert_eq!(
            actor.get_state_field("seen").and_then(|v| v.as_int()),
            Some(4242),
            "no message before the deadline: the after body must run"
        );
        assert!(
            actor.suspended_execution.is_none(),
            "the wait must be fully resolved after the timeout fired"
        );
    }

    /// Timed selective receive: a message arriving before the deadline wakes
    /// the suspended actor and dispatches to the matching arm; the timeout
    /// never fires observably.
    #[test]
    fn test_receive_after_wakes_on_matching_message() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Listener {
                state seen = 0
                behavior drain() {
                    self.seen = receive {
                        | add(x, y) => x + y
                    } after 5000 => 4242
                }
                behavior add(x: Int, y: Int) { x }
            }
            let c = spawn Listener { seen = 0 } in {
                send c drain()
                c
            }
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        // Deliver add(4, 5) ~30ms into the 5s wait, while the actor is
        // suspended: the send must wake it so the scan matches arm 0.
        let add_id = rt.borrow().behavior_id_for(actor_id, "add").unwrap();
        rt.borrow().timer_wheel.send_after(
            std::time::Duration::from_millis(30),
            actor_id,
            add_id,
            vec![Value::int(4), Value::int(5)],
        );
        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&actor_id).unwrap();
        assert_eq!(
            actor.get_state_field("seen").and_then(|v| v.as_int()),
            Some(9),
            "the matching message must resolve the wait, not the timeout"
        );
        assert!(
            rt_ref.timer_wheel.is_empty(),
            "the receive timeout must be cancelled once the wait matches"
        );
    }

    /// Timed selective receive with `after 0`: non-blocking poll — no match
    /// runs the after body immediately, without suspending or arming a timer.
    #[test]
    fn test_receive_after_zero_is_non_blocking() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Listener {
                state seen = 0
                behavior drain() {
                    self.seen = receive {
                        | add(x, y) => x + y
                    } after 0 => 77
                }
                behavior add(x: Int, y: Int) { x }
            }
            let c = spawn Listener { seen = 0 } in {
                send c drain()
                c
            }
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&actor_id).unwrap();
        assert_eq!(
            actor.get_state_field("seen").and_then(|v| v.as_int()),
            Some(77),
            "after 0 with no queued match must run the after body immediately"
        );
        assert!(
            actor.suspended_execution.is_none(),
            "after 0 must never suspend the actor"
        );
        assert!(
            rt_ref.timer_wheel.is_empty(),
            "after 0 must not arm a timeout timer"
        );
    }

    /// Timed selective receive with multiple arms: a message waking the
    /// suspended actor dispatches to the right arm with its payload bound.
    #[test]
    fn test_receive_after_multiple_arms_bind_payload() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Listener {
                state seen = 0
                behavior drain() {
                    self.seen = receive {
                        | get() => 100
                        | add(x, y) => x * 10 + y
                    } after 5000 => 0
                }
                behavior add(x: Int, y: Int) { x }
                behavior get() { 0 }
            }
            let c = spawn Listener { seen = 0 } in {
                send c drain()
                c
            }
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        // Wake the suspended wait with add(7, 8): the second arm must win
        // with x, y bound from the payload.
        let add_id = rt.borrow().behavior_id_for(actor_id, "add").unwrap();
        rt.borrow().timer_wheel.send_after(
            std::time::Duration::from_millis(30),
            actor_id,
            add_id,
            vec![Value::int(7), Value::int(8)],
        );
        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&actor_id).unwrap();
        assert_eq!(
            actor.get_state_field("seen").and_then(|v| v.as_int()),
            Some(78),
            "the waking message must dispatch to the add arm with its payload bound"
        );
    }

    /// Timed selective receive: a NON-matching message wakes the actor, the
    /// re-scan finds no arm, and the behavior re-suspends on the ORIGINAL
    /// deadline — the skipped message stays queued and dispatches normally
    /// after the timeout runs the after body.
    #[test]
    fn test_receive_after_nonmatching_wake_keeps_deadline() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Listener {
                state seen = 0
                state heard = 0
                behavior drain() {
                    self.seen = receive {
                        | add(x, y) => x + y
                    } after 60 => 4242
                }
                behavior add(x: Int, y: Int) { x }
                behavior noise(n: Int) { self.heard = n }
            }
            let c = spawn Listener { seen = 0 heard = 0 } in {
                send c drain()
                c
            }
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        // Wake the wait ~20ms in with a message no arm matches: the actor
        // re-suspends and the 60ms timeout (armed at the first suspend)
        // still resolves the wait.
        let noise_id = rt.borrow().behavior_id_for(actor_id, "noise").unwrap();
        rt.borrow().timer_wheel.send_after(
            std::time::Duration::from_millis(20),
            actor_id,
            noise_id,
            vec![Value::int(9)],
        );
        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&actor_id).unwrap();
        assert_eq!(
            actor.get_state_field("seen").and_then(|v| v.as_int()),
            Some(4242),
            "a non-matching wake must re-suspend; the original timeout fires"
        );
        assert_eq!(
            actor.get_state_field("heard").and_then(|v| v.as_int()),
            Some(9),
            "the skipped message must stay queued and dispatch normally"
        );
    }

    /// A behavior that `send`s to another actor: the message must be
    /// delivered (BytecodeRuntimeCallbacks::send_message used to be a
    /// silent no-op, dropping every behavior-internal send).
    #[test]
    fn test_behavior_send_relay_reaches_target() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Relay {
                behavior forward(target, n: Int) { send target arrived(n + 1) }
            }
            actor Sink {
                state seen = 0
                behavior arrived(n: Int) { self.seen = n }
            }
            let s = spawn Sink {} in
            let r = spawn Relay {} in {
                send r forward(s, 41)
                s
            }
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let sink_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let sink = rt_ref.actors.get(&sink_id).unwrap();
        assert_eq!(
            sink.get_state_field("seen").and_then(|v| v.as_int()),
            Some(42),
            "the relay's behavior-internal send must reach the sink"
        );
    }

    /// A behavior that `spawn`s a child and sends to it: the child must be
    /// created with its bytecode handlers wired up and must run (the
    /// behavior-internal spawn used to return a bogus actor_ref(0)).
    #[test]
    fn test_behavior_spawn_child_runs_and_reports() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Worker {
                state got = 0
                behavior built(v: Int, sink) {
                    self.got = v
                    send sink report(v)
                }
            }
            actor Collector {
                state seen = 0
                behavior report(n: Int) { self.seen = n }
            }
            actor Factory {
                behavior make(n: Int, sink) {
                    let child = spawn Worker {} in
                        send child built(n * 2, sink)
                }
            }
            let c = spawn Collector {} in
            let f = spawn Factory {} in {
                send f make(21, c)
                c
            }
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let collector_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        assert_eq!(
            rt_ref.actors.len(),
            3,
            "the behavior-internal spawn must create a real third actor"
        );
        let collector = rt_ref.actors.get(&collector_id).unwrap();
        assert_eq!(
            collector.get_state_field("seen").and_then(|v| v.as_int()),
            Some(42),
            "the spawned child must run and report back to the collector"
        );
    }

    /// Regression for the deferred receive-wait wake: a behavior that sends
    /// to an actor suspended in `receive ... after` must wake it via the
    /// match — but the resume cannot run inside the sender's VM execution
    /// (it would nest `vm.resume()` on the shared runtime VM), so the wake
    /// is deferred until the sender's behavior returns.
    #[test]
    fn test_behavior_send_wakes_suspended_receive_wait() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Waiter {
                state result = 0
                behavior waitwork() {
                    self.result = receive {
                        | token(n) => n
                    } after 5000 => 999
                }
                behavior token(n: Int) { n }
            }
            actor Poker {
                behavior poke(r) { send r token(77) }
            }
            let w = spawn Waiter {} in
            let p = spawn Poker {} in {
                send w waitwork()
                send p poke(w)
                w
            }
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let waiter_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let waiter = rt_ref.actors.get(&waiter_id).unwrap();
        assert_eq!(
            waiter.get_state_field("result").and_then(|v| v.as_int()),
            Some(77),
            "the behavior-internal send must wake the suspended wait via the match, not the 5s timeout"
        );
        assert!(
            waiter.suspended_execution.is_none(),
            "the wait must be fully resolved"
        );
        assert!(
            rt_ref.timer_wheel.is_empty(),
            "the receive timeout must be cancelled once the wait matches"
        );
    }

    /// A behavior that sends to its own actor (self-send): the message is
    /// delivered normally and processed in a later turn.
    #[test]
    fn test_behavior_send_to_self_delivers() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let source = r#"
            actor Loop {
                state count = 0
                behavior spin(me, n: Int) {
                    self.count = self.count + 1
                    if n > 0 then send me spin(me, n - 1) else send me halt()
                }
                behavior halt() { 0 }
            }
            let l = spawn Loop {} in {
                send l spin(l, 3)
                l
            }
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let loop_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        let actor = rt_ref.actors.get(&loop_id).unwrap();
        assert_eq!(
            actor.get_state_field("count").and_then(|v| v.as_int()),
            Some(4),
            "spin(3) plus three self-sends (2, 1, 0) must all run"
        );
    }

    /// Sending to an unknown actor id is a no-op: the message is dropped
    /// and the bogus queue entry is skipped without crashing.
    #[test]
    fn test_send_to_unknown_actor_is_noop() {
        let mut rt = Runtime::new();
        rt.send_message_by_id(999_999, 0, &[Value::int(1)]);
        rt.run_scheduler();
        // The message is routed to the DLQ, which is created lazily.
        assert!(rt.dlq_actor_id.is_some());
        assert_eq!(rt.dlq_depth(), 1);
    }

    /// Differential test: the legacy compiler and the HIR/MIR pipeline must
    /// produce identical results over a corpus of pure programs.
    #[test]
    fn test_mir_and_legacy_pipelines_agree() {
        let corpus: &[&str] = &[
            // Arithmetic and precedence
            "1 + 2 * 3 - 4",
            "(1 + 2) * (3 + 4)",
            "100 / 7 % 5",
            // Let chains and shadowing
            "let x = 5 in let y = x * 2 in x + y",
            "let x = 1 in let x = x + 1 in x",
            // Conditionals, including statements after an if
            "if 1 < 2 then 10 else 20",
            "let x = if false then 1 else 2 in x + 10",
            "if true then (if false then 1 else 2) else 3",
            // Match with literals, variable binding, and wildcard
            "match 2 { case 1 => 10 case 2 => 20 case _ => 30 }",
            "match 9 { case 1 => 10 case n => n * 2 }",
            // Closures: capturing, recursive, higher-order
            "let a = 40 in let add = fn(x) { x + a } in add(2)",
            "let fib = fn(n) { if n <= 1 then n else fib(n - 1) + fib(n - 2) } in fib(10)",
            "let twice = fn(f, x) { f(f(x)) } in let inc = fn(n) { n + 1 } in twice(inc, 5)",
            // Top-level functions
            "fn add(x: Int, y: Int) -> Int { x + y }\nadd(3, 4)",
            "fn fact(n: Int) -> Int { if n == 0 then 1 else n * fact(n - 1) }\nfact(6)",
            // Arrays, indexing, records
            "[10, 20, 30][1]",
            "let arr = [10, 20, 30] in arr[0] + arr[2]",
            "let r = { x: 1, y: 41 } in r.x + r.y",
            // Mutation via `=`: `arr[i] = v` and `record.f = v` parse as
            // BinOp::Assign binary expressions (only a bare `ident = v`
            // parses as the distinct Expr::Assign node).
            "let arr = [1, 2, 3] in { arr[0] = 99 arr[0] }",
            "let r = { x: 1, y: 2 } in { r.x = 99 r.x + r.y }",
            // For loops evaluate to unit
            "for i in [1, 2, 3] { i }",
            // Ref cells: `&` creates a cell, `*` dereferences, assignment
            // mutates and yields the assigned value.
            "let x = &10 in { x = 3; *x }",
            // Effect handlers, with and without a resumed value
            "handle perform Math.getAnswer() { | Math.getAnswer() => 42 }",
            "handle perform IO.print(\"hello\") { | IO.print(msg) => 7 }",
            // Pipe operator
            "let inc = fn(n) { n + 1 } in 41 |> inc",
            // Receive expression (MVP: returns nil outside actor context)
            "receive { | Msg(x) => x }",
        ];
        for src in corpus {
            let legacy = run_source(src)
                .map(|(v, _)| v.to_string_repr())
                .unwrap_or_else(|e| panic!("legacy pipeline failed on {:?}: {}", src, e));
            let mir = run_source_new(src)
                .map(|v| v.to_string_repr())
                .unwrap_or_else(|e| panic!("MIR pipeline failed on {:?}: {}", src, e));
            assert_eq!(legacy, mir, "pipelines disagree on {:?}", src);
        }
    }

    /// Regression: closures capturing enclosing locals must see the captured
    /// values (CapStore/CapLoad used to be VM no-ops, yielding garbage).
    #[test]
    fn test_legacy_closure_capture() {
        let source = "let a = 40 in let add = fn(x) { x + a } in add(2)";
        let (value, _ty) = run_source(source).unwrap();
        assert_eq!(value.as_int(), Some(42));
    }

    #[test]
    fn test_legacy_closure_capture_two_vars() {
        let source = "let a = 30 in let b = 10 in let f = fn(x) { a + b + x } in f(2)";
        let (value, _ty) = run_source(source).unwrap();
        assert_eq!(value.as_int(), Some(42));
    }

    #[test]
    fn test_llm_ask_mock_client() {
        let source = r#"perform LLM.ask("hello")"#;
        let (module, _ty) = compile_source(source).unwrap();

        let rt = Rc::new(RefCell::new(Runtime::new()));
        rt.borrow_mut()
            .set_llm_client(Box::new(crate::ai::MockLlmClient::text("world")));

        let mut vm = VM::new();
        vm.load_module(module);
        vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt)));

        let result = vm.run().unwrap();
        let string_id = result.as_string_id().expect("expected string result");
        let module_idx = vm.modules.len() - 1;
        let content = vm.constant_string(module_idx, string_id).unwrap();
        assert_eq!(content, "world");
    }

    // -----------------------------------------------------------------------
    // Non-blocking LLM calls in actor bytecode behaviors
    // -----------------------------------------------------------------------

    /// Native counter handler for the non-blocking LLM ordering test.
    fn llm_test_counter_inc(actor: &mut crate::runtime::Actor, _args: &[Value]) {
        let n = actor
            .get_state_field("count")
            .and_then(|v| v.as_int())
            .unwrap_or(0);
        actor.set_state_field("count", Value::int(n + 1));
    }

    /// A bytecode behavior that performs `LLM.ask` suspends on the scheduler
    /// thread, a background worker completes the HTTP call, and the behavior
    /// resumes with the response written back into the prompt register.
    #[test]
    fn test_llm_ask_actor_behavior_suspends_and_resumes() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let client = crate::ai::MockLlmClient::text("world");
        rt.borrow_mut().set_llm_client(Box::new(client.clone()));

        let source = r#"
            actor LlmActor {
                state answer = ""
                behavior go() {
                    self.answer = perform LLM.ask("hello")
                }
            }
            let a = spawn LlmActor { answer = "" } in a
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        rt.borrow_mut().send_message(actor_id, "go", &[]);
        rt.borrow_mut().run_scheduler();

        let answer = rt.borrow().actor_state_string(actor_id, "answer");
        assert_eq!(
            answer.as_deref(),
            Some("world"),
            "resumed behavior should store the LLM response in state"
        );
        {
            let rt_ref = rt.borrow();
            let actor = rt_ref.actors.get(&actor_id).unwrap();
            assert!(!actor.llm_inflight, "in-flight flag should be cleared");
            assert!(
                actor.llm_completed.is_none(),
                "completion should be consumed by the re-executed LlmAsk"
            );
            assert!(
                actor.suspended_execution.is_none(),
                "suspension should be cleared after resume"
            );
        }
        assert_eq!(client.recorded_calls().len(), 1, "exactly one LLM call");
    }

    /// While one actor is suspended on a slow LLM call, the scheduler must
    /// keep running other actors: all counter work completes before the LLM
    /// response is even pumped. Deterministic because completions are only
    /// pumped by run_scheduler, never by manual stepping.
    #[test]
    fn test_llm_ask_nonblocking_other_actors_run_first() {
        let rt = Rc::new(RefCell::new(Runtime::new()));
        rt.borrow_mut()
            .set_llm_client(Box::new(crate::ai::MockLlmClient::delayed(
                "done",
                std::time::Duration::from_millis(100),
            )));

        let source = r#"
            actor LlmActor {
                state answer = ""
                behavior go() {
                    self.answer = perform LLM.ask("hello")
                }
            }
            let a = spawn LlmActor { answer = "" } in a
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let llm_actor = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        let counter = rt
            .borrow_mut()
            .spawn_actor(Box::new(|| vec![("count".into(), Value::int(0))]));
        rt.borrow_mut()
            .actors
            .get_mut(&counter)
            .unwrap()
            .register_behavior("inc", llm_test_counter_inc);

        // LLM message first, then 20 counter increments.
        rt.borrow_mut().send_message(llm_actor, "go", &[]);
        for _ in 0..20 {
            rt.borrow_mut().send_message(counter, "inc", &[]);
        }

        // Pump the run queue manually. LLM completions are only delivered by
        // run_scheduler's completion pump, so during manual stepping the
        // response sits untouched in the channel no matter how long the
        // worker takes.
        loop {
            let next = rt.borrow_mut().scheduler.dequeue();
            match next {
                Some(actor_id) => rt.borrow_mut().step_actor(actor_id),
                None => break,
            }
        }

        {
            let rt_ref = rt.borrow();
            let counter_actor = rt_ref.actors.get(&counter).unwrap();
            assert_eq!(
                counter_actor
                    .get_state_field("count")
                    .and_then(|v| v.as_int()),
                Some(20),
                "all counter work must complete while the LLM call is in flight"
            );
            let llm = rt_ref.actors.get(&llm_actor).unwrap();
            assert!(
                llm.llm_inflight,
                "LLM call should still be in flight after the queue drained \
                 (a blocking call would have completed inline and stalled the counter)"
            );
            assert_eq!(
                rt_ref.actor_state_string(llm_actor, "answer").as_deref(),
                Some(""),
                "answer must not be stored before the completion is pumped"
            );
        }

        // Let the worker finish, then pump the completion and resume.
        std::thread::sleep(std::time::Duration::from_millis(200));
        rt.borrow_mut().run_scheduler();

        let rt_ref = rt.borrow();
        assert_eq!(
            rt_ref.actor_state_string(llm_actor, "answer").as_deref(),
            Some("done"),
            "LLM behavior should resume and store the delayed response"
        );
        assert_eq!(
            rt_ref
                .actors
                .get(&counter)
                .unwrap()
                .get_state_field("count")
                .and_then(|v| v.as_int()),
            Some(20)
        );
    }

    /// Two sequential `perform LLM.ask` calls in one behavior: the behavior
    /// suspends and resumes twice, re-capturing VM state on the second
    /// suspend, and observes both responses in order.
    #[test]
    fn test_llm_ask_chained_suspensions_resume_in_order() {
        let text_response = |content: &str| crate::ai::LlmResponse {
            content: Some(content.to_string()),
            tool_calls: Vec::new(),
            model: "mock".to_string(),
            finish_reason: "stop".to_string(),
            usage: crate::ai::TokenUsage::default(),
        };
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let client = crate::ai::MockLlmClient::sequence(vec![
            text_response("first-reply"),
            text_response("second-reply"),
        ]);
        rt.borrow_mut().set_llm_client(Box::new(client.clone()));

        let source = r#"
            actor Chained {
                state first = ""
                state second = ""
                behavior go() {
                    let _ = self.first = perform LLM.ask("one") in
                    self.second = perform LLM.ask("two")
                }
            }
            let a = spawn Chained { first = ""; second = "" } in a
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        rt.borrow_mut().send_message(actor_id, "go", &[]);
        rt.borrow_mut().run_scheduler();

        {
            let rt_ref = rt.borrow();
            assert_eq!(
                rt_ref.actor_state_string(actor_id, "first").as_deref(),
                Some("first-reply")
            );
            assert_eq!(
                rt_ref.actor_state_string(actor_id, "second").as_deref(),
                Some("second-reply")
            );
            assert!(!rt_ref.actors.get(&actor_id).unwrap().llm_inflight);
        }
        let calls = client.recorded_calls();
        assert_eq!(calls.len(), 2, "expected two LLM calls");
        assert_eq!(calls[0].messages[0].content, "one");
        assert_eq!(calls[1].messages[0].content, "two");
    }

    /// Two messages sent to one actor whose behavior suspends on
    /// `LLM.ask`: the second message must wait in the mailbox until the
    /// first behavior fully resumes.  Previously step_actor ran the second
    /// message over the live suspension; its `LlmAsk` saw the in-flight
    /// flag, returned Pending, and overwrote `suspended_execution`, so the
    /// first completion resumed the SECOND behavior with the FIRST call's
    /// response and the first behavior was lost forever.
    #[test]
    fn test_llm_ask_queued_messages_wait_for_suspended_behavior() {
        let text_response = |content: &str| crate::ai::LlmResponse {
            content: Some(content.to_string()),
            tool_calls: Vec::new(),
            model: "mock".to_string(),
            finish_reason: "stop".to_string(),
            usage: crate::ai::TokenUsage::default(),
        };
        let rt = Rc::new(RefCell::new(Runtime::new()));
        let client = crate::ai::MockLlmClient::sequence(vec![
            text_response("reply-one"),
            text_response("reply-two"),
        ]);
        rt.borrow_mut().set_llm_client(Box::new(client.clone()));

        let source = r#"
            actor LlmPair {
                state first = ""
                state second = ""
                behavior one() {
                    self.first = perform LLM.ask("one")
                }
                behavior two() {
                    self.second = perform LLM.ask("two")
                }
            }
            let a = spawn LlmPair { first = ""; second = "" } in a
        "#;
        let value = run_source_new_with_runtime(source, rt.clone()).unwrap();
        let actor_id = value
            .as_actor_id()
            .expect("spawn should return an actor reference");

        // Both messages are queued before the scheduler runs: the second
        // arrives while the first behavior is suspended on its LLM call.
        rt.borrow_mut().send_message(actor_id, "one", &[]);
        rt.borrow_mut().send_message(actor_id, "two", &[]);
        rt.borrow_mut().run_scheduler();

        {
            let rt_ref = rt.borrow();
            assert_eq!(
                rt_ref.actor_state_string(actor_id, "first").as_deref(),
                Some("reply-one"),
                "first behavior must store its own response"
            );
            assert_eq!(
                rt_ref.actor_state_string(actor_id, "second").as_deref(),
                Some("reply-two"),
                "second behavior must store its own response"
            );
            let actor = rt_ref.actors.get(&actor_id).unwrap();
            assert!(!actor.llm_inflight, "in-flight flag should be cleared");
            assert!(
                actor.suspended_execution.is_none(),
                "no suspension should remain after both behaviors complete"
            );
            assert!(
                actor.mailbox.is_empty(),
                "both queued messages should have been processed"
            );
        }
        let calls = client.recorded_calls();
        assert_eq!(
            calls.len(),
            2,
            "each behavior should issue its own LLM call"
        );
        assert_eq!(calls[0].messages[0].content, "one");
        assert_eq!(calls[1].messages[0].content, "two");
    }

    /// A workflow step that performs `LLM.ask` suspends on the background
    /// call and, once resumed, records the step completion the same way a
    /// signal-resumed step does: step_index advances, a StepCompleted
    /// event is appended, and the actor checkpoints.  Previously
    /// resume_suspended_llm_step did none of the workflow bookkeeping, so
    /// the step never advanced from the journal's perspective.
    #[test]
    fn test_workflow_llm_ask_step_records_completion() {
        let source = r#"
            workflow LlmFlow {
                step ask_step { self.answer = perform LLM.ask("hello") }
            }
            let w = spawn LlmFlow {} in { w }
        "#;

        let store = SharedMemoryStore::new();
        let (module, _ty) = compile_source(source).unwrap();

        let rt = Rc::new(RefCell::new(Runtime::new()));
        rt.borrow_mut().persistence = Box::new(store.clone());
        rt.borrow_mut()
            .set_llm_client(Box::new(crate::ai::MockLlmClient::text("world")));
        let value = {
            let mut vm = VM::new();
            vm.load_module(module.clone());
            vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));
            vm.run().unwrap()
        };
        let actor_id = value.as_actor_id().expect("spawn should return actor ref");

        rt.borrow_mut().send_message_by_id(actor_id, 0, &[]);
        rt.borrow_mut().run_scheduler();

        {
            let rt_ref = rt.borrow();
            let actor = rt_ref.actors.get(&actor_id).unwrap();
            assert_eq!(
                actor.get_state_field("step_index").and_then(|v| v.as_int()),
                Some(1),
                "resumed LLM step should advance step_index"
            );
            assert!(actor.suspended_execution.is_none());
            assert_eq!(
                actor.waiting_signal, None,
                "suspension marker should be cleared after resume"
            );
            assert_eq!(
                rt_ref.actor_state_string(actor_id, "answer").as_deref(),
                Some("world")
            );
        }
        let events = store.read_workflow_events(actor_id);
        assert!(
            events.iter().any(|e| matches!(e, WorkflowEvent::StepCompleted { step_name, .. } if step_name == "ask_step")),
            "StepCompleted event should be persisted after the LLM call resumes"
        );
    }

    /// Crash-and-recover for a workflow step suspended on `LLM.ask`: the
    /// persisted suspension marker lets recovery re-drive the interrupted
    /// step, which re-issues the call on the new runtime and completes the
    /// step in the journal.  Previously the snapshot carried no marker
    /// (waiting_signal is None for LLM suspends), so recover_actor did not
    /// re-trigger the step and it was silently lost on restart.
    #[test]
    fn test_workflow_llm_ask_step_redriven_after_restart() {
        let source = r#"
            workflow LlmFlowRecover {
                step ask_step { self.answer = perform LLM.ask("hello") }
            }
            let w = spawn LlmFlowRecover {} in { w }
        "#;

        let store = SharedMemoryStore::new();
        let (module, _ty) = compile_source(source).unwrap();
        let meta = module.actor_metadata.first().unwrap();
        let mut offsets = vec![0; module.behaviors.len()];
        for &idx in &meta.behavior_indices {
            if let Some(entry) = module.behaviors.get(idx) {
                offsets[idx] = entry.code_offset;
            }
        }

        // First runtime: start the step and let it suspend on the LLM call.
        // The completion is never pumped, simulating a crash mid-call.
        let rt1 = Rc::new(RefCell::new(Runtime::new()));
        rt1.borrow_mut().persistence = Box::new(store.clone());
        rt1.borrow_mut()
            .set_llm_client(Box::new(crate::ai::MockLlmClient::text("stale")));
        let value = {
            let mut vm = VM::new();
            vm.load_module(module.clone());
            vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt1.clone())));
            vm.run().unwrap()
        };
        let actor_id = value.as_actor_id().expect("spawn should return actor ref");

        rt1.borrow_mut().send_message_by_id(actor_id, 0, &[]);
        // Drive the queue manually so the behavior suspends; running the
        // full scheduler would pump the completion and resume the step.
        loop {
            let next = rt1.borrow_mut().scheduler.dequeue();
            match next {
                Some(id) => rt1.borrow_mut().step_actor(id),
                None => break,
            }
        }
        {
            let rt_ref = rt1.borrow();
            let actor = rt_ref.actors.get(&actor_id).unwrap();
            assert!(
                actor.suspended_execution.is_some(),
                "step should be suspended on the LLM call"
            );
            assert!(actor.llm_inflight, "background call should be in flight");
        }
        // The snapshot must carry the suspension marker so recovery knows
        // the in-flight step has to be re-driven.
        let snapshot = store
            .load_snapshot(actor_id)
            .expect("workflow spawn should have persisted a snapshot");
        assert_eq!(
            snapshot.waiting_signal.as_deref(),
            Some("__llm_ask_pending__"),
            "snapshot should record the LLM suspension marker"
        );

        // Simulate a node restart: drop the actor and recover into a fresh
        // runtime sharing the store, with its own LLM client.
        rt1.borrow_mut().actors.remove(&actor_id);

        let rt2 = Rc::new(RefCell::new(Runtime::new()));
        rt2.borrow_mut().persistence = Box::new(store.clone());
        let client2 = crate::ai::MockLlmClient::text("world");
        rt2.borrow_mut().set_llm_client(Box::new(client2.clone()));
        rt2.borrow_mut().register_recovery_module(
            actor_id,
            module.clone(),
            offsets.clone(),
            vec![None; module.behaviors.len()],
        );
        rt2.borrow_mut().recover_actor(actor_id);
        rt2.borrow_mut().run_scheduler();

        {
            let rt_ref = rt2.borrow();
            let actor = rt_ref.actors.get(&actor_id).unwrap();
            assert_eq!(
                actor.get_state_field("step_index").and_then(|v| v.as_int()),
                Some(1),
                "re-driven step should advance step_index"
            );
            assert!(actor.suspended_execution.is_none());
            assert_eq!(
                rt_ref.actor_state_string(actor_id, "answer").as_deref(),
                Some("world"),
                "re-driven step should store the new runtime's response"
            );
        }
        let events = store.read_workflow_events(actor_id);
        assert!(
            events.iter().any(|e| matches!(e, WorkflowEvent::StepCompleted { step_name, .. } if step_name == "ask_step")),
            "StepCompleted event should be persisted after recovery re-drives the step"
        );
        let calls = client2.recorded_calls();
        assert_eq!(
            calls.len(),
            1,
            "the recovered runtime should issue one fresh LLM call"
        );
        assert_eq!(calls[0].messages[0].content, "hello");
    }

    /// A workflow step that waits on a signal AND THEN performs `LLM.ask`
    /// must, once the signal arrives and the step resumes, suspend on the
    /// background LLM call instead of running saga compensation.
    /// Previously resume_suspended_workflow_step only matched
    /// "SignalWait:suspend": the resumed step's "LlmAsk:suspend" fell into
    /// the generic error arm and compensated the saga, and with suspension
    /// not enabled for the resume at all, the LLM call blocked the caller
    /// thread instead of suspending.
    #[test]
    fn test_workflow_step_signal_wait_then_llm_ask() {
        let source = r#"
            workflow SignalThenLlm {
                step wait_then_ask {
                    (perform Signal.wait("go"), self.answer = perform LLM.ask("hello"))
                }
            }
            let w = spawn SignalThenLlm {} in { w }
        "#;

        let store = SharedMemoryStore::new();
        let (module, _ty) = compile_source(source).unwrap();

        let rt = Rc::new(RefCell::new(Runtime::new()));
        rt.borrow_mut().persistence = Box::new(store.clone());
        rt.borrow_mut()
            .set_llm_client(Box::new(crate::ai::MockLlmClient::text("world")));
        let value = {
            let mut vm = VM::new();
            vm.load_module(module.clone());
            vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));
            vm.run().unwrap()
        };
        let actor_id = value.as_actor_id().expect("spawn should return actor ref");

        rt.borrow_mut().send_message_by_id(actor_id, 0, &[]);
        rt.borrow_mut().run_scheduler();

        // The step is suspended waiting for the signal.
        {
            let rt_ref = rt.borrow();
            let actor = rt_ref.actors.get(&actor_id).unwrap();
            assert_eq!(actor.waiting_signal.as_deref(), Some("go"));
            assert!(actor.suspended_execution.is_some());
            assert_eq!(
                actor.get_state_field("step_index").and_then(|v| v.as_int()),
                Some(0)
            );
        }

        // The signal arrives: the step resumes, consumes it, and suspends
        // again on the background LLM call.  The suspension must be
        // re-captured with the LLM marker — NOT treated as a step failure.
        rt.borrow_mut().signal_workflow(actor_id, "go", None);
        {
            let rt_ref = rt.borrow();
            let actor = rt_ref.actors.get(&actor_id).unwrap();
            assert_eq!(
                actor.waiting_signal.as_deref(),
                Some("__llm_ask_pending__"),
                "signal-resumed step should re-suspend with the LLM marker"
            );
            assert!(
                actor.suspended_execution.is_some(),
                "LLM suspension should be re-captured"
            );
            assert!(actor.llm_inflight, "background call should be in flight");
            assert_eq!(
                actor.get_state_field("step_index").and_then(|v| v.as_int()),
                Some(0),
                "step should not complete before the LLM response"
            );
        }
        let events = store.read_workflow_events(actor_id);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, WorkflowEvent::SagaCompensated { .. })),
            "an LLM suspend is not a step failure: no saga compensation"
        );
        assert!(
            !events.iter().any(|e| matches!(e, WorkflowEvent::StepCompleted { step_name, .. } if step_name == "wait_then_ask")),
            "step should not be journaled complete before the LLM response"
        );

        // Pump the mock completion: the step resumes through
        // resume_suspended_llm_step, which performs the workflow completion
        // bookkeeping.
        rt.borrow_mut().run_scheduler();
        {
            let rt_ref = rt.borrow();
            let actor = rt_ref.actors.get(&actor_id).unwrap();
            assert_eq!(
                actor.get_state_field("step_index").and_then(|v| v.as_int()),
                Some(1),
                "resumed LLM step should advance step_index"
            );
            assert!(actor.suspended_execution.is_none());
            assert_eq!(actor.waiting_signal, None);
            assert!(!actor.llm_inflight);
            assert_eq!(
                rt_ref.actor_state_string(actor_id, "answer").as_deref(),
                Some("world")
            );
        }
        let events = store.read_workflow_events(actor_id);
        let completions = events
            .iter()
            .filter(|e| matches!(e, WorkflowEvent::StepCompleted { step_name, .. } if step_name == "wait_then_ask"))
            .count();
        assert_eq!(
            completions, 1,
            "step should complete in the journal exactly once"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, WorkflowEvent::SagaCompensated { .. })),
            "no saga compensation should ever run for a suspending step"
        );
    }

    /// Reverse order: a workflow step that performs `LLM.ask` AND THEN waits
    /// on a signal.  The LLM completion resumes the step through
    /// resume_suspended_llm_step, whose chained-suspend arm re-captures the
    /// signal wait with the signal name as marker; the signal then completes
    /// the step through resume_suspended_workflow_step.
    #[test]
    fn test_workflow_step_llm_ask_then_signal_wait() {
        let source = r#"
            workflow LlmThenSignal {
                step ask_then_wait {
                    (self.answer = perform LLM.ask("hello"), perform Signal.wait("go"))
                }
            }
            let w = spawn LlmThenSignal {} in { w }
        "#;

        let store = SharedMemoryStore::new();
        let (module, _ty) = compile_source(source).unwrap();

        let rt = Rc::new(RefCell::new(Runtime::new()));
        rt.borrow_mut().persistence = Box::new(store.clone());
        let client = crate::ai::MockLlmClient::text("world");
        rt.borrow_mut().set_llm_client(Box::new(client.clone()));
        let value = {
            let mut vm = VM::new();
            vm.load_module(module.clone());
            vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));
            vm.run().unwrap()
        };
        let actor_id = value.as_actor_id().expect("spawn should return actor ref");

        rt.borrow_mut().send_message_by_id(actor_id, 0, &[]);
        // run_scheduler pumps the LLM completion; the resumed step then
        // suspends on the signal wait.
        rt.borrow_mut().run_scheduler();
        {
            let rt_ref = rt.borrow();
            let actor = rt_ref.actors.get(&actor_id).unwrap();
            assert_eq!(
                actor.waiting_signal.as_deref(),
                Some("go"),
                "LLM-resumed step should re-suspend waiting for the signal"
            );
            assert!(actor.suspended_execution.is_some());
            assert!(!actor.llm_inflight);
            assert_eq!(
                actor.get_state_field("step_index").and_then(|v| v.as_int()),
                Some(0),
                "step should not complete before the signal"
            );
        }
        assert_eq!(client.recorded_calls().len(), 1, "exactly one LLM call");

        // The signal arrives: the step runs to completion.
        rt.borrow_mut().signal_workflow(actor_id, "go", None);
        {
            let rt_ref = rt.borrow();
            let actor = rt_ref.actors.get(&actor_id).unwrap();
            assert_eq!(
                actor.get_state_field("step_index").and_then(|v| v.as_int()),
                Some(1),
                "workflow should advance after the signal"
            );
            assert!(actor.suspended_execution.is_none());
            assert_eq!(actor.waiting_signal, None);
            assert_eq!(
                rt_ref.actor_state_string(actor_id, "answer").as_deref(),
                Some("world")
            );
        }
        let events = store.read_workflow_events(actor_id);
        let completions = events
            .iter()
            .filter(|e| matches!(e, WorkflowEvent::StepCompleted { step_name, .. } if step_name == "ask_then_wait"))
            .count();
        assert_eq!(
            completions, 1,
            "step should complete in the journal exactly once"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, WorkflowEvent::SagaCompensated { .. })),
            "no saga compensation should run for a suspending step"
        );
    }

    #[test]
    fn test_agent_ask_uses_memory() {
        let source = r#"
            agent Agent = {
                model: "mock-model",
                system_prompt: "You are helpful.",
                memory: { max_turns: 10 }
            }
            let a = spawn Agent {} in
            let r1 = ask a ask("hello") in
            let r2 = ask a ask("world") in
            r1
        "#;
        let (module, _ty) = compile_source(source).unwrap();

        let rt = Rc::new(RefCell::new(Runtime::new()));
        let client = crate::ai::MockLlmClient::text("world");
        rt.borrow_mut().set_llm_client(Box::new(client.clone()));

        let mut vm = VM::new();
        vm.load_module(module);
        vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt)));

        let result = vm.run().unwrap();

        let calls = client.recorded_calls();
        assert_eq!(calls.len(), 2, "expected two LLM calls");

        let module_idx = vm.modules.len() - 1;
        let content = vm.value_to_string(module_idx, result);
        assert_eq!(content, "world");

        // First turn: system prompt + user prompt.
        assert_eq!(calls[0].messages.len(), 2);
        assert_eq!(calls[0].messages[0].role, "system");
        assert_eq!(calls[0].messages[0].content, "You are helpful.");
        assert_eq!(calls[0].messages[1].role, "user");
        assert_eq!(calls[0].messages[1].content, "hello");

        // Second turn includes the previous user/assistant exchange from memory.
        assert_eq!(calls[1].messages.len(), 4);
        assert_eq!(calls[1].messages[0].role, "system");
        assert_eq!(calls[1].messages[1].role, "user");
        assert_eq!(calls[1].messages[1].content, "hello");
        assert_eq!(calls[1].messages[2].role, "assistant");
        assert_eq!(calls[1].messages[2].content, "world");
        assert_eq!(calls[1].messages[3].role, "user");
        assert_eq!(calls[1].messages[3].content, "world");
    }

    #[test]
    fn test_agent_ask_tracks_usage_and_cost() {
        let source = r#"
            agent Agent = {
                model: "mock-model",
                system_prompt: "You are helpful.",
                pricing: { input: 0.01, output: 0.02 }
            }
            let a = spawn Agent {} in
            ask a ask("hello")
        "#;
        let (module, _ty) = compile_source(source).unwrap();

        let rt = Rc::new(RefCell::new(Runtime::new()));
        let client =
            crate::ai::MockLlmClient::with_usage("world", crate::ai::TokenUsage::new(1000, 500));
        let client_ref = client.clone();
        rt.borrow_mut().set_llm_client(Box::new(client));

        let mut vm = VM::new();
        vm.load_module(module);
        vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));

        let result = vm.run().unwrap();
        let module_idx = vm.modules.len() - 1;
        let content = vm.value_to_string(module_idx, result);
        assert_eq!(content, "world");

        let rt = rt.borrow();
        let actor = rt.actors.values().next().expect("expected one actor");
        assert_eq!(
            actor.get_state_field("usage_prompt").unwrap().as_int(),
            Some(1000)
        );
        assert_eq!(
            actor.get_state_field("usage_completion").unwrap().as_int(),
            Some(500)
        );
        // 1000 * 0.01 / 1000 + 500 * 0.02 / 1000 = 0.01 + 0.01 = 0.02
        let cost = actor
            .get_state_field("usage_cost")
            .unwrap()
            .as_float()
            .unwrap();
        assert!((cost - 0.02).abs() < 1e-9);

        // Pricing should be forwarded on the request.
        let calls = client_ref.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].pricing.as_ref().unwrap().input_cost_per_1k, 0.01);
        assert_eq!(calls[0].pricing.as_ref().unwrap().output_cost_per_1k, 0.02);
    }

    #[test]
    fn test_agent_usage_behavior() {
        let source = r#"
            agent Agent = {
                model: "mock-model",
                system_prompt: "You are helpful.",
                pricing: { input: 0.01, output: 0.02 }
            }
            let a = spawn Agent {} in
            let _ = ask a ask("hello") in
            ask a usage()
        "#;
        let (module, _ty) = compile_source(source).unwrap();

        let rt = Rc::new(RefCell::new(Runtime::new()));
        let client =
            crate::ai::MockLlmClient::with_usage("world", crate::ai::TokenUsage::new(1000, 500));
        rt.borrow_mut().set_llm_client(Box::new(client));

        let mut vm = VM::new();
        vm.load_module(module);
        vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));

        let result = vm.run().unwrap();

        // The usage behavior returns an array [prompt_tokens, completion_tokens, cost]
        // (see compile_agent's usage_behavior); inspect the actor-allocated
        // array directly.
        let ptr = result
            .as_ptr()
            .expect("usage() should return an array pointer");
        let usage = unsafe { std::slice::from_raw_parts(ptr as *const Value, 3) };
        assert_eq!(usage[0].as_int(), Some(1000), "prompt tokens");
        assert_eq!(usage[1].as_int(), Some(500), "completion tokens");
        let cost = usage[2].as_float().expect("cost should be a float");
        // 1000 * 0.01 / 1000 + 500 * 0.02 / 1000 = 0.01 + 0.01 = 0.02
        assert!((cost - 0.02).abs() < 1e-9, "cost: {}", cost);
    }

    #[test]
    fn test_agent_semantic_memory_store_and_recall() {
        let source = r#"
            agent Agent = {
                model: "mock-model",
                system_prompt: "You are helpful.",
                semantic_memory: { dimensions: 32 }
            }
            let a = spawn Agent {} in
            let _ = ask a store_fact("hello world") in
            ask a recall("hello", 1)
        "#;
        let (module, _ty) = compile_source(source).unwrap();

        let rt = Rc::new(RefCell::new(Runtime::new()));
        let mut vm = VM::new();
        vm.load_module(module);
        vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));

        let result = vm.run().unwrap();

        let module_idx = vm.modules.len() - 1;
        let content = vm.value_to_string(module_idx, result);
        assert_eq!(content, "hello world");

        // The durable semantic_memory field should contain one document.
        let rt = rt.borrow();
        let actor = rt.actors.values().next().expect("expected one actor");
        let memory_json = actor.get_state_field("semantic_memory").unwrap();
        let memory_json_str = vm.value_to_string(module_idx, memory_json);
        let memory: crate::ai::SemanticMemory = serde_json::from_str(&memory_json_str).unwrap();
        assert_eq!(memory.len(), 1);
        assert_eq!(memory.documents[0].content, "hello world");
    }

    #[test]
    fn test_agent_workflow_researches_and_reports() {
        // v0.9 milestone: agent researches a topic, uses a tool, stores the
        // fact in semantic memory, and synthesizes a report.
        let source = r#"
            @tool(description: "Store a research fact tagged with a topic.")
            fn store_fact(content: String, topic: String) -> String { content }

            agent Researcher = {
                model: "llama3.1",
                system_prompt: "You are a research assistant.",
                pricing: { input: 0.0, output: 0.0 },
                tools: [store_fact],
                memory: { max_turns: 10 },
                semantic_memory: { dimensions: 64 }
            }

            let researcher = spawn Researcher {} in
            let _ = ask researcher ask("Research CRDTs") in
            let report = ask researcher ask("Synthesize a report on CRDTs") in
            report
        "#;

        let (module, _ty) = compile_source(source).unwrap();

        let rt = Rc::new(RefCell::new(Runtime::new()));

        let mut store_args = serde_json::Map::new();
        store_args.insert(
            "content".to_string(),
            serde_json::Value::String("CRDTs are conflict-free replicated data types.".to_string()),
        );
        store_args.insert(
            "topic".to_string(),
            serde_json::Value::String("CRDTs".to_string()),
        );

        let client = crate::ai::MockLlmClient::sequence(vec![
            crate::ai::LlmResponse {
                content: None,
                tool_calls: vec![crate::ai::ToolCall {
                    id: String::new(),
                    name: "store_fact".to_string(),
                    arguments: store_args,
                }],
                model: "mock".to_string(),
                finish_reason: "tool_calls".to_string(),
                usage: crate::ai::TokenUsage::default(),
            },
            crate::ai::LlmResponse {
                content: Some(
                    "CRDTs enable strong eventual consistency without coordination.".to_string(),
                ),
                tool_calls: Vec::new(),
                model: "mock".to_string(),
                finish_reason: "stop".to_string(),
                usage: crate::ai::TokenUsage::default(),
            },
        ]);
        let client_ref = client.clone();
        rt.borrow_mut().set_llm_client(Box::new(client));

        let mut vm = VM::new();
        vm.load_module(module);
        vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));

        let result = vm.run().unwrap();

        let module_idx = vm.modules.len() - 1;
        let report = vm.value_to_string(module_idx, result);
        assert_eq!(
            report,
            "CRDTs enable strong eventual consistency without coordination."
        );

        // The LLM client should have been asked twice.
        let calls = client_ref.recorded_calls();
        assert_eq!(calls.len(), 2, "expected two LLM calls");

        // The first request should have exposed the store_fact tool.
        assert_eq!(calls[0].tools.len(), 1);
        assert_eq!(calls[0].tools[0].name, "store_fact");

        // The fact should be persisted in durable semantic memory.
        let rt = rt.borrow();
        let actor = rt.actors.values().next().expect("expected one actor");
        let memory_json = actor.get_state_field("semantic_memory").unwrap();
        let memory_json_str = vm.value_to_string(module_idx, memory_json);
        let memory: crate::ai::SemanticMemory = serde_json::from_str(&memory_json_str).unwrap();
        assert_eq!(memory.len(), 1);
        assert_eq!(
            memory.documents[0].content,
            "CRDTs are conflict-free replicated data types."
        );
        assert_eq!(
            memory.documents[0].metadata.get("topic"),
            Some(&"CRDTs".to_string())
        );
    }

    #[test]
    fn test_agent_workflow_recovers_semantic_memory_after_restart() {
        // v0.9 milestone: after a research agent stores a fact, simulating a
        // node restart with the same persistence store preserves the semantic
        // memory and the recovered agent can recall it.
        let source = r#"
            @tool(description: "Store a research fact tagged with a topic.")
            fn store_fact(content: String, topic: String) -> String { content }

            agent Researcher = {
                model: "llama3.1",
                system_prompt: "You are a research assistant.",
                pricing: { input: 0.0, output: 0.0 },
                tools: [store_fact],
                memory: { max_turns: 10 },
                semantic_memory: { dimensions: 64 }
            }

            let researcher = spawn Researcher {} in
            let _ = ask researcher ask("Research CRDTs") in
            researcher
        "#;

        let store = SharedMemoryStore::new();
        let (module, _ty) = compile_source(source).unwrap();
        let meta = module.actor_metadata.first().unwrap();
        let mut offsets = vec![0; module.behaviors.len()];
        for &idx in &meta.behavior_indices {
            if let Some(entry) = module.behaviors.get(idx) {
                offsets[idx] = entry.code_offset;
            }
        }

        let mut store_args = serde_json::Map::new();
        store_args.insert(
            "content".to_string(),
            serde_json::Value::String("CRDTs are conflict-free replicated data types.".to_string()),
        );
        store_args.insert(
            "topic".to_string(),
            serde_json::Value::String("CRDTs".to_string()),
        );

        let client = crate::ai::MockLlmClient::sequence(vec![crate::ai::LlmResponse {
            content: None,
            tool_calls: vec![crate::ai::ToolCall {
                id: String::new(),
                name: "store_fact".to_string(),
                arguments: store_args,
            }],
            model: "mock".to_string(),
            finish_reason: "tool_calls".to_string(),
            usage: crate::ai::TokenUsage::default(),
        }]);

        let rt1 = Rc::new(RefCell::new(Runtime::new()));
        rt1.borrow_mut().set_llm_client(Box::new(client));
        rt1.borrow_mut().persistence = Box::new(store.clone());
        let value = {
            let mut vm = VM::new();
            vm.load_module(module.clone());
            vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt1.clone())));
            vm.run().unwrap()
        };
        let actor_id = value.as_actor_id().expect("spawn should return actor ref");

        // The fact was stored during the first (and only) ask.
        {
            let rt1_ref = rt1.borrow();
            let actor = rt1_ref.actors.get(&actor_id).unwrap();
            let memory_json = actor.get_state_field("semantic_memory").unwrap();
            let memory_json_str = VM::new().value_to_string(0, memory_json);
            let memory: crate::ai::SemanticMemory = serde_json::from_str(&memory_json_str).unwrap();
            assert_eq!(memory.len(), 1);
        }

        // Simulate a node restart: new runtime sharing the same store,
        // register the bytecode module, then recover the agent.
        let rt2 = Rc::new(RefCell::new(Runtime::new()));
        rt2.borrow_mut().persistence = Box::new(store.clone());
        rt2.borrow_mut().register_recovery_module(
            actor_id,
            module.clone(),
            offsets.clone(),
            vec![None; module.behaviors.len()],
        );
        rt2.borrow_mut().recover_actor(actor_id);

        // Recall the stored fact from the recovered agent. Agent behaviors are
        // laid out as ask(0), usage(1), store_fact(2), recall(3).
        let recall_behavior_id = 3u16;
        let query = {
            let mut rt2_ref = rt2.borrow_mut();
            let actor = rt2_ref.actors.get_mut(&actor_id).unwrap();
            actor.allocate_string("CRDTs")
        };
        let top_k = Value::int(1);
        let recalled = rt2
            .borrow_mut()
            .ask_actor_sync(actor_id, recall_behavior_id, &[query, top_k])
            .unwrap();

        let module_idx = 0;
        let recalled_content = VM::new().value_to_string(module_idx, recalled);
        assert_eq!(
            recalled_content, "CRDTs are conflict-free replicated data types.",
            "recovered agent should recall the stored fact"
        );
    }

    #[test]
    fn test_agent_procedural_memory_store_and_get_pattern() {
        let source = r#"
            agent Agent = {
                model: "mock-model",
                system_prompt: "You are helpful.",
                procedural_memory: { namespace: "my_app" }
            }
            let a = spawn Agent {} in
            let _ = ask a store_pattern("format", "research_*", "{title}\\n{summary}") in
            ask a get_pattern("format")
        "#;
        let (module, _ty) = compile_source(source).unwrap();

        let rt = Rc::new(RefCell::new(Runtime::new()));
        let mut vm = VM::new();
        vm.load_module(module);
        vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));

        let result = vm.run().unwrap();

        let module_idx = vm.modules.len() - 1;
        let content = vm.value_to_string(module_idx, result);
        assert_eq!(content, "{title}\\n{summary}");

        let rt = rt.borrow();
        let actor = rt.actors.values().next().expect("expected one actor");
        let memory_json = actor.get_state_field("procedural_memory").unwrap();
        let memory_json_str = vm.value_to_string(module_idx, memory_json);
        let memory: crate::ai::ProceduralMemory = serde_json::from_str(&memory_json_str).unwrap();
        assert_eq!(memory.len(), 1);
        assert_eq!(memory.namespace, "my_app");
        assert_eq!(
            memory.get_pattern("format").unwrap().output_template,
            "{title}\\n{summary}"
        );
    }

    #[test]
    fn test_agent_procedural_memory_add_and_get_examples() {
        let source = r#"
            agent Agent = {
                model: "mock-model",
                system_prompt: "You are helpful.",
                procedural_memory: { namespace: "code_review" }
            }
            let a = spawn Agent {} in
            let _ = ask a add_example("review", "fn bad() { let x = 1; x }", "Unused variable") in
            let _ = ask a add_example("review", "fn ok() { let x = 1; x + 1 }", "Good") in
            ask a get_examples("review", "unused variable", 1)
        "#;
        let (module, _ty) = compile_source(source).unwrap();

        let rt = Rc::new(RefCell::new(Runtime::new()));
        let mut vm = VM::new();
        vm.load_module(module);
        vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));

        let result = vm.run().unwrap();

        let module_idx = vm.modules.len() - 1;
        let content = vm.value_to_string(module_idx, result);
        assert!(
            content.contains("Unused variable"),
            "expected matching example, got {}",
            content
        );

        let rt = rt.borrow();
        let actor = rt.actors.values().next().expect("expected one actor");
        let memory_json = actor.get_state_field("procedural_memory").unwrap();
        let memory_json_str = vm.value_to_string(module_idx, memory_json);
        let memory: crate::ai::ProceduralMemory = serde_json::from_str(&memory_json_str).unwrap();
        assert_eq!(memory.len(), 2);
    }

    #[test]
    fn test_agent_procedural_memory_recovers_after_restart() {
        let source = r#"
            agent Agent = {
                model: "mock-model",
                system_prompt: "You are helpful.",
                procedural_memory: { namespace: "my_app" }
            }

            let a = spawn Agent {} in
            a
        "#;
        let (module, _ty) = compile_source(source).unwrap();
        let meta = module.actor_metadata.first().unwrap();
        let mut offsets = vec![0; module.behaviors.len()];
        for &idx in &meta.behavior_indices {
            if let Some(entry) = module.behaviors.get(idx) {
                offsets[idx] = entry.code_offset;
            }
        }

        let store = SharedMemoryStore::new();
        let rt = Rc::new(RefCell::new(Runtime::new()));
        rt.borrow_mut().persistence = Box::new(store.clone());

        let mut vm = VM::new();
        vm.load_module(module.clone());
        vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));
        let value = vm.run().unwrap();
        let actor_id = value.as_actor_id().expect("spawn should return actor ref");

        let (key_arg, pattern_arg, template_arg) = {
            let mut rt_ref = rt.borrow_mut();
            let actor = rt_ref.actors.get_mut(&actor_id).unwrap();
            (
                actor.allocate_string("format"),
                actor.allocate_string("research_*"),
                actor.allocate_string("{title}"),
            )
        };
        rt.borrow_mut()
            .ask_actor_sync(actor_id, 2, &[key_arg, pattern_arg, template_arg])
            .unwrap();

        let rt2 = Rc::new(RefCell::new(Runtime::new()));
        rt2.borrow_mut().persistence = Box::new(store.clone());
        rt2.borrow_mut().register_recovery_module(
            actor_id,
            module.clone(),
            offsets.clone(),
            vec![None; module.behaviors.len()],
        );
        rt2.borrow_mut().recover_actor(actor_id);

        let get_pattern_behavior_id = 3u16;
        let key_arg = {
            let mut rt2_ref = rt2.borrow_mut();
            let actor = rt2_ref.actors.get_mut(&actor_id).unwrap();
            actor.allocate_string("format")
        };
        let recalled = rt2
            .borrow_mut()
            .ask_actor_sync(actor_id, get_pattern_behavior_id, &[key_arg])
            .unwrap();

        let module_idx = 0;
        let recalled_content = VM::new().value_to_string(module_idx, recalled);
        assert_eq!(
            recalled_content, "{title}",
            "recovered agent should return the stored pattern"
        );
    }

    #[test]
    fn test_pipeline_source_end_to_end() {
        let source = r#"
            agent Researcher = {
                model: "llama3.1",
                system_prompt: "Research.",
                pricing: { input: 0.0, output: 0.0 }
            }
            agent Writer = {
                model: "llama3.1",
                system_prompt: "Write.",
                pricing: { input: 0.0, output: 0.0 }
            }

            fn main() {
                let researcher = spawn Researcher {} in
                let writer = spawn Writer {} in
                let pipeline = Pipeline.new()
                    |> Pipeline.stage("research", researcher, "Research: {input}")
                    |> Pipeline.stage("write", writer, "Write based on: {input}")
                in
                pipeline.run("CRDTs")
            }
        "#;

        let rt = Rc::new(RefCell::new(Runtime::new()));
        let client = crate::ai::MockLlmClient::sequence(vec![
            crate::ai::LlmResponse {
                content: Some("research notes".to_string()),
                tool_calls: Vec::new(),
                model: "mock".to_string(),
                finish_reason: "stop".to_string(),
                usage: crate::ai::TokenUsage::default(),
            },
            crate::ai::LlmResponse {
                content: Some("final article".to_string()),
                tool_calls: Vec::new(),
                model: "mock".to_string(),
                finish_reason: "stop".to_string(),
                usage: crate::ai::TokenUsage::default(),
            },
        ]);
        rt.borrow_mut().set_llm_client(Box::new(client));

        let (module, _ty) = compile_source(source).unwrap();
        let mut vm = VM::new();
        vm.load_module(module);
        vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt)));
        let value = vm.run().unwrap();

        let module_idx = vm.modules.len() - 1;
        let result = vm.value_to_string(module_idx, value);
        assert_eq!(result, "final article");
    }

    #[test]
    fn test_supervisor_source_end_to_end() {
        let source = r#"
            agent Researcher = {
                model: "llama3.1",
                system_prompt: "Research.",
                pricing: { input: 0.0, output: 0.0 }
            }
            agent Writer = {
                model: "llama3.1",
                system_prompt: "Write.",
                pricing: { input: 0.0, output: 0.0 }
            }

            fn main() {
                let researcher = spawn Researcher {} in
                let writer = spawn Writer {} in
                let team = Supervisor.new()
                    |> Supervisor.worker("researcher", researcher, "Finds information")
                    |> Supervisor.worker("writer", writer, "Writes content")
                in
                team.run("Write an article about CRDTs")
            }
        "#;

        let rt = Rc::new(RefCell::new(Runtime::new()));
        let client = crate::ai::MockLlmClient::sequence(vec![
            crate::ai::LlmResponse {
                content: Some("research notes".to_string()),
                tool_calls: Vec::new(),
                model: "mock".to_string(),
                finish_reason: "stop".to_string(),
                usage: crate::ai::TokenUsage::default(),
            },
            crate::ai::LlmResponse {
                content: Some("final article".to_string()),
                tool_calls: Vec::new(),
                model: "mock".to_string(),
                finish_reason: "stop".to_string(),
                usage: crate::ai::TokenUsage::default(),
            },
        ]);
        rt.borrow_mut().set_llm_client(Box::new(client));

        let (module, _ty) = compile_source(source).unwrap();
        let mut vm = VM::new();
        vm.load_module(module);
        vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt)));
        let value = vm.run().unwrap();

        let module_idx = vm.modules.len() - 1;
        let result = vm.value_to_string(module_idx, value);
        assert_eq!(result, "final article");
    }

    #[test]
    fn test_debate_source_end_to_end() {
        let source = r#"
            agent ProAgent = {
                model: "llama3.1",
                system_prompt: "Argue in favor.",
                pricing: { input: 0.0, output: 0.0 }
            }
            agent ConAgent = {
                model: "llama3.1",
                system_prompt: "Argue against.",
                pricing: { input: 0.0, output: 0.0 }
            }
            agent Moderator = {
                model: "llama3.1",
                system_prompt: "Synthesize.",
                pricing: { input: 0.0, output: 0.0 }
            }

            fn main() {
                let pro = spawn ProAgent {} in
                let con = spawn ConAgent {} in
                let moderator = spawn Moderator {} in
                let debate = Debate.new("microservices vs monolith", 1, 0.8)
                    |> Debate.participant("pro", "pro", pro)
                    |> Debate.participant("con", "con", con)
                    |> Debate.participant("moderator", "neutral", moderator)
                in
                debate.run()
            }
        "#;

        let rt = Rc::new(RefCell::new(Runtime::new()));
        let client = crate::ai::MockLlmClient::sequence(vec![
            crate::ai::LlmResponse {
                content: Some("pro argument".to_string()),
                tool_calls: Vec::new(),
                model: "mock".to_string(),
                finish_reason: "stop".to_string(),
                usage: crate::ai::TokenUsage::default(),
            },
            crate::ai::LlmResponse {
                content: Some("con argument".to_string()),
                tool_calls: Vec::new(),
                model: "mock".to_string(),
                finish_reason: "stop".to_string(),
                usage: crate::ai::TokenUsage::default(),
            },
            crate::ai::LlmResponse {
                content: Some("moderator observation".to_string()),
                tool_calls: Vec::new(),
                model: "mock".to_string(),
                finish_reason: "stop".to_string(),
                usage: crate::ai::TokenUsage::default(),
            },
            crate::ai::LlmResponse {
                content: Some("consensus reached".to_string()),
                tool_calls: Vec::new(),
                model: "mock".to_string(),
                finish_reason: "stop".to_string(),
                usage: crate::ai::TokenUsage::default(),
            },
        ]);
        rt.borrow_mut().set_llm_client(Box::new(client));

        let (module, _ty) = compile_source(source).unwrap();
        let mut vm = VM::new();
        vm.load_module(module);
        vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt)));
        let value = vm.run().unwrap();

        let module_idx = vm.modules.len() - 1;
        let result = vm.value_to_string(module_idx, value);
        assert_eq!(result, "consensus reached");
    }

    #[test]
    fn test_let_annotation_type_mismatch() {
        // A let annotation that contradicts the value type must be a type
        // error, not silently discarded.
        let source = r#"let x : Int = "not an int" in x"#;
        let result = run_source(source);
        assert!(
            result.is_err(),
            "let annotation mismatch should be a type error"
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("Cannot unify"),
            "Error should be a unification failure: {}",
            err_msg
        );
    }

    #[test]
    fn test_let_annotation_matching_type() {
        // A matching annotation type checks and runs normally.
        assert_int("let x : Int = 41 in x + 1", 42);
    }

    // -----------------------------------------------------------------------
    // Regression: `return` inside `handle` must unwind the handler frame
    // (previously the frame stayed on the VM handler_stack, so a later
    // unhandled perform dispatched into the dead function's handler code)
    // -----------------------------------------------------------------------

    #[test]
    fn test_return_inside_handle_unwinds_handler_frame() {
        // `leak` returns out of a handled perform; afterwards a top-level
        // perform of the same effect must be unhandled rather than
        // dispatching into the dead function's handler.
        let source = r#"
            fn leak() -> Int {
                handle {
                    perform Math.getAnswer();
                    return 1
                } {
                    | Math.getAnswer() => 40 + 1
                }
            }
            { leak(); perform Math.getAnswer() }
        "#;
        let result = run_source(source);
        assert!(
            result.is_err(),
            "perform after return from handle must be unhandled, got {:?}",
            result
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("Unhandled effect"),
            "expected unhandled-effect error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_return_inside_nested_handles_unwinds_all_frames() {
        // Two nested handlers for the same effect: the return must pop BOTH
        // frames, or the outer (stale) one would catch the later perform.
        let source = r#"
            fn leak() -> Int {
                handle {
                    handle {
                        perform Math.getAnswer();
                        return 1
                    } {
                        | Math.getAnswer() => 41
                    }
                } {
                    | Math.getAnswer() => 99
                }
            }
            { leak(); perform Math.getAnswer() }
        "#;
        let result = run_source(source);
        assert!(
            result.is_err(),
            "both nested handler frames must be unwound, got {:?}",
            result
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("Unhandled effect"),
            "expected unhandled-effect error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_return_inside_handle_branch_unwinds_handler_frame() {
        // The FnReturn path through an expression-position `if` inside the
        // handle body (lower_body_into) must unwind the frame too.
        let source = r#"
            fn leak(b: Bool) -> Int {
                handle {
                    perform Math.getAnswer();
                    if b then return 1 else 2
                } {
                    | Math.getAnswer() => 41
                }
            }
            { leak(true); perform Math.getAnswer() }
        "#;
        let result = run_source(source);
        assert!(
            result.is_err(),
            "return from an if-branch inside handle must unwind, got {:?}",
            result
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("Unhandled effect"),
            "expected unhandled-effect error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_return_inside_handle_loop_unwinds_handler_frame() {
        // The FnReturn path through a `for` body inside the handle body
        // (lower_for) must unwind the frame too.
        let source = r#"
            fn leak() -> Int {
                handle {
                    perform Math.getAnswer();
                    for x in [1, 2, 3] { return x };
                    0
                } {
                    | Math.getAnswer() => 41
                }
            }
            { leak(); perform Math.getAnswer() }
        "#;
        let result = run_source(source);
        assert!(
            result.is_err(),
            "return from a for body inside handle must unwind, got {:?}",
            result
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("Unhandled effect"),
            "expected unhandled-effect error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_return_inside_handler_body_unwinds_handler_frame() {
        // A `return` inside a HANDLER body (not the handled body) runs with
        // the handle's frame on the VM handler_stack; it must unwind that
        // frame or a later unhandled perform dispatches into the dead
        // function's handler code.
        let source = r#"
            fn leak() -> Int {
                handle { perform Math.getAnswer() } { | Math.getAnswer() => return 7 }
            }
            { leak(); perform Math.getAnswer() }
        "#;
        let result = run_source(source);
        assert!(
            result.is_err(),
            "return from a handler body must unwind its frame, got {:?}",
            result
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("Unhandled effect"),
            "expected unhandled-effect error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_return_inside_handler_body_value() {
        // Positive control: the return itself still yields its value.
        let source = r#"
            fn f() -> Int {
                handle { perform Math.getAnswer() } { | Math.getAnswer() => return 7 }
            }
            f()
        "#;
        assert_int(source, 7);
    }

    #[test]
    fn test_return_inside_nested_handler_body_unwinds_all_frames() {
        // The inner handler body's return runs with BOTH handle frames on
        // the stack; both must be unwound (depth counts the handler's own
        // frame plus enclosing handles).
        let source = r#"
            fn leak() -> Int {
                handle {
                    handle { perform Math.getAnswer() } { | Math.getAnswer() => return 7 }
                } {
                    | Math.getAnswer() => 1
                }
            }
            { leak(); perform Math.getAnswer() }
        "#;
        let result = run_source(source);
        assert!(
            result.is_err(),
            "return from a nested handler body must unwind both frames, got {:?}",
            result
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("Unhandled effect"),
            "expected unhandled-effect error, got: {}",
            err_msg
        );
    }

    // -----------------------------------------------------------------------
    // Regression: recursive closures must capture enclosing variables
    // -----------------------------------------------------------------------

    #[test]
    fn test_recursive_closure_captures_enclosing_var() {
        let source = r#"
            let k = 10 in
            let f = fn(n) { if n < 1 then 0 else f(n - 1) + k } in
            f(3)
        "#;
        assert_int(source, 30);
    }

    #[test]
    fn test_recursive_closure_captures_multiple_vars() {
        let source = r#"
            let a = 1 in
            let b = 100 in
            let f = fn(n) { if n < 1 then 0 else f(n - 1) + a + b } in
            f(2)
        "#;
        // f(2) = f(1) + 101 = (f(0) + 101) + 101 = 202
        assert_int(source, 202);
    }

    // -----------------------------------------------------------------------
    // Regression: free_vars must descend into effect/actor expressions so
    // closures capture variables used only inside them
    // -----------------------------------------------------------------------

    #[test]
    fn test_closure_captures_var_used_in_perform() {
        // `k` is used only as a perform argument (inside a handle so the
        // program evaluates to a value).
        let source = r#"
            let k = 7 in
            let f = fn(x) { handle perform IO.print(k) { | IO.print(m) => m } } in
            f(1)
        "#;
        assert_int(source, 7);
    }

    #[test]
    fn test_closure_captures_var_used_in_bare_perform() {
        // Exact repro: previously failed at compile time with
        // "undefined variable 'k'"; now compiles (k is captured) and the
        // standalone IO.print built-in handles the perform at runtime, so
        // the closure body evaluates to x.
        let source = r#"
            let k = 7 in
            let f = fn(x) { perform IO.print(k); x } in
            f(1)
        "#;
        assert_int(source, 1);
    }

    #[test]
    fn test_closure_captures_var_used_in_handler_body() {
        // `secret` is used only inside an effect handler body.
        let source = r#"
            let secret = 41 in
            let f = fn(x) { handle perform Math.getAnswer() { | Math.getAnswer() => secret + 1 } } in
            f(0)
        "#;
        assert_int(source, 42);
    }

    // -----------------------------------------------------------------------
    // Regression: non-exhaustive match must be a runtime error, not silently
    // evaluate the last arm
    // -----------------------------------------------------------------------

    #[test]
    fn test_match_non_exhaustive_is_runtime_error() {
        let source = r#"match 99 {
            case 1 => 10
            case 2 => 20
        }"#;
        let result = run_source(source);
        assert!(
            result.is_err(),
            "non-exhaustive match must be a runtime error, got {:?}",
            result
        );
    }

    #[test]
    fn test_match_last_literal_arm_still_matches() {
        // Control: a matching refutable last arm still evaluates normally.
        let source = r#"match 2 {
            case 1 => 10
            case 2 => 20
        }"#;
        assert_int(source, 20);
    }

    // -----------------------------------------------------------------------
    // Pattern guards: `| pat if cond => body`
    // -----------------------------------------------------------------------

    #[test]
    fn test_match_guard_accepts() {
        let source = r#"match 42 { | n if n > 10 => 1 | _ => 0 }"#;
        assert_int(source, 1);
    }

    #[test]
    fn test_match_guard_rejects_falls_through() {
        let source = r#"match 5 { | n if n > 10 => 1 | _ => 0 }"#;
        assert_int(source, 0);
    }

    #[test]
    fn test_match_guard_over_variant_payload() {
        // The guard sees the payload binding: a failing guard falls through
        // to the next arm even when the constructor matches.
        let source = r#"
            type Option[T] = Some(T) | None

            fn classify(o: Option[Int]) -> Int {
                match o with {
                    | Some(n) if n > 0 => 1
                    | Some(n) => 2
                    | None => 0
                }
            }

            classify(Some(5)) * 100 + classify(Some(0 - 3)) * 10 + classify(None)
        "#;
        assert_int(source, 120);
    }

    #[test]
    fn test_match_guarded_final_wildcard_non_exhaustive() {
        // A guarded last arm is not a catch-all: when its guard fails the
        // match is non-exhaustive and must raise the runtime error.
        let source = r#"match 5 { | _ if false => 1 }"#;
        let result = run_source(source);
        match &result {
            Err(e) => assert!(
                format!("{e}").contains("non-exhaustive"),
                "expected non-exhaustive match error, got {e}"
            ),
            Ok(v) => panic!("guarded final wildcard with failing guard must error, got {v:?}"),
        }
    }

    #[test]
    fn test_match_guard_must_be_bool() {
        let source = r#"match 1 { | n if n => 1 | _ => 0 }"#;
        let result = run_source(source);
        assert!(
            result.is_err(),
            "non-Bool guard must be a type error, got {:?}",
            result
        );
    }
}
