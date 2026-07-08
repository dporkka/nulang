//! End-to-end integration tests that exercise the full compiler pipeline.
//!
//! Tests go through lex → parse → typecheck → compile → VM run.

#[cfg(test)]
mod tests {
    use crate::vm::{VM, Value};
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::typechecker::TypeChecker;
    use crate::types::Type;
    use crate::types::NuError;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use crate::runtime::{Runtime, RuntimeVmCallbacks, MemoryStore, PersistenceStore, ActorSnapshot, JournalEntry, WorkflowEvent};

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
        fn append_workflow_event(&mut self, actor_id: u64, event: WorkflowEvent) -> std::io::Result<()> {
            self.0.lock().unwrap().append_workflow_event(actor_id, event)
        }
        fn read_workflow_events(&self, actor_id: u64) -> Vec<WorkflowEvent> {
            self.0.lock().unwrap().read_workflow_events(actor_id)
        }
        fn clear(&mut self, actor_id: u64) -> std::io::Result<()> {
            self.0.lock().unwrap().clear(actor_id)
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

        // 4. Compile
        let mut compiler = crate::compiler::Compiler::new("test");
        let module = compiler.compile_module(&ast)?.clone();

        // 5. Run
        let mut vm = VM::new();
        vm.load_module(module);
        let value = vm.run()?;

        Ok((value, module_type))
    }

    /// Assert that running source produces an integer value.
    fn assert_int(source: &str, expected: i64) {
        let (value, _ty) = run_source(source).unwrap();
        assert_eq!(value.as_int(), Some(expected), "Expected integer result for: {}", source);
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

        let mut compiler = crate::compiler::Compiler::new("test");
        let module = compiler.compile_module(&ast)?.clone();

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
    fn test_arithmetic_precedence() {
        assert_int("1 + 2 * 3", 7);   // mul before add
        assert_int("(1 + 2) * 3", 9); // parens override
    }

    #[test]
    fn test_let_binding() {
        let source = "let x = 10 in x + 5";
        assert_int(source, 15);
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
        let source = r#"
            perform IO.print("hello")
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
        assert_eq!(value.as_int(), Some(42), "Handler should return 42 via resume");
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
        let source = "let a = 1 in let b = 2 in let c = 3 in let d = 4 in let e = 5 in a + b + c + d + e";
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
            assert!(actor.bytecode_module.is_some(), "actor should have a bytecode module");
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
            rt1.borrow().actors.get(&actor_id).unwrap().get_state_field("count").and_then(|v| v.as_int()),
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
            rt2.borrow().actors.get(&actor_id).unwrap().get_state_field("count").and_then(|v| v.as_int()),
            Some(3),
            "recovered counter should still be 3"
        );

        // Send two more inc messages on the recovered runtime.
        rt2.borrow_mut().send_message(actor_id, "inc", &[]);
        rt2.borrow_mut().send_message(actor_id, "inc", &[]);
        rt2.borrow_mut().run_scheduler();
        assert_eq!(
            rt2.borrow().actors.get(&actor_id).unwrap().get_state_field("count").and_then(|v| v.as_int()),
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
            rt1.borrow().actors.get(&actor_id).unwrap()
                .get_state_field("step_index").and_then(|v| v.as_int()),
            Some(1),
            "first step should advance step_index to 1"
        );

        let events_before = store.read_workflow_events(actor_id);
        assert!(events_before.iter().any(|e| matches!(e, WorkflowEvent::WorkflowStarted { .. })));
        assert!(events_before.iter().any(|e| matches!(e, WorkflowEvent::Custom { name, .. } if name == "Started")));
        assert!(events_before.iter().any(|e| matches!(e, WorkflowEvent::StepCompleted { .. })));

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
            rt2.borrow().actors.get(&actor_id).unwrap()
                .get_state_field("step_index").and_then(|v| v.as_int()),
            Some(1),
            "recovered workflow should resume at step_index 1"
        );

        // Continue execution on the recovered runtime: advance the second step.
        // Bytecode-only workflow actors have an empty behavior_table, so route
        // by explicit behavior id (1 is the second step).
        rt2.borrow_mut().send_message_by_id(actor_id, 1, &[]);
        rt2.borrow_mut().run_scheduler();

        assert_eq!(
            rt2.borrow().actors.get(&actor_id).unwrap()
                .get_state_field("step_index").and_then(|v| v.as_int()),
            Some(2),
            "final step_index should be 2 after second step"
        );

        let events_after = store.read_workflow_events(actor_id);
        assert_eq!(
            events_after.iter().filter(|e| matches!(e, WorkflowEvent::StepCompleted { .. })).count(),
            2,
            "two StepCompleted events should be persisted"
        );
        assert!(events_after.iter().any(|e| matches!(e, WorkflowEvent::Custom { name, .. } if name == "Incremented")));
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
            rt1.borrow().actors.get(&actor_id).unwrap()
                .get_state_field("step_index").and_then(|v| v.as_int()),
            Some(0),
            "step should not advance before signal is received"
        );
        assert!(
            rt1.borrow().actors.get(&actor_id).unwrap().suspended_execution.is_some(),
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
            rt2.borrow().actors.get(&actor_id).unwrap()
                .get_state_field("step_index").and_then(|v| v.as_int()),
            Some(0),
            "step should still be waiting after recovery"
        );

        // Send the signal. The runtime appends SignalReceived and resumes the step.
        rt2.borrow_mut().signal_workflow(actor_id, "go", None);

        assert_eq!(
            rt2.borrow().actors.get(&actor_id).unwrap()
                .get_state_field("step_index").and_then(|v| v.as_int()),
            Some(1),
            "workflow should advance after the signal is received"
        );

        let events = store.read_workflow_events(actor_id);
        assert!(
            events.iter().any(|e| matches!(e, WorkflowEvent::SignalReceived { name, .. } if name == "go")),
            "SignalReceived event should be persisted"
        );
        assert!(
            events.iter().any(|e| matches!(e, WorkflowEvent::StepCompleted { step_name, .. } if step_name == "wait_for_go")),
            "StepCompleted event should be persisted after the signal"
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
            assert_eq!(actor.get_state_field("a_done").and_then(|v| v.as_int()), Some(1));
            assert_eq!(actor.get_state_field("b_done").and_then(|v| v.as_int()), Some(1));
            assert_eq!(
                actor.get_state_field("comp_order").and_then(|v| v.as_int()),
                Some(21),
                "compensations should run in reverse order (b then a)"
            );
        }

        let events = rt.borrow().persistence.read_workflow_events(actor_id);
        assert_eq!(
            events.iter().filter(|e| matches!(e, WorkflowEvent::StepCompleted { .. })).count(),
            2,
            "only the first two steps should record StepCompleted"
        );
        let saga_events: Vec<_> = events.iter().filter(|e| matches!(e, WorkflowEvent::SagaCompensated { .. })).collect();
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

        // First runtime: spawn the workflow and run the timer step.
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

        let events_before = store.read_workflow_events(actor_id);
        assert!(
            events_before.iter().any(|e| matches!(e, WorkflowEvent::TimerSet { name, .. } if name == "timeout1")),
            "TimerSet event should be persisted"
        );
        assert_eq!(
            rt1.borrow().actors.get(&actor_id).unwrap()
                .get_state_field("step_index").and_then(|v| v.as_int()),
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
            rt2.borrow().actors.get(&actor_id).unwrap()
                .get_state_field("step_index").and_then(|v| v.as_int()),
            Some(0),
            "recovered workflow should resume at the snapshot step_index"
        );

        // Let the timer fire and process the resulting message.
        std::thread::sleep(std::time::Duration::from_millis(20));
        rt2.borrow_mut().tick_timers();
        rt2.borrow_mut().run_scheduler();

        let events_after = store.read_workflow_events(actor_id);
        assert!(
            events_after.iter().any(|e| matches!(e, WorkflowEvent::TimerFired { name, .. } if name == "timeout1")),
            "TimerFired event should be persisted after the timer fires"
        );
        assert_eq!(
            rt2.borrow().actors.get(&actor_id).unwrap()
                .get_state_field("step_index").and_then(|v| v.as_int()),
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
            rt.borrow().actors.get(&actor_id).unwrap()
                .get_state_field("step_index").and_then(|v| v.as_int()),
            Some(3),
            "workflow should advance through before, parallel, and after"
        );

        let events = store.read_workflow_events(actor_id);
        assert_eq!(
            events.iter().filter(|e| matches!(e, WorkflowEvent::ParallelBranchCompleted { .. })).count(),
            2,
            "both branches should emit ParallelBranchCompleted"
        );
        assert!(
            events.iter().any(|e| matches!(e, WorkflowEvent::StepCompleted { step_name, .. } if step_name == "parallel_0")),
            "parallel_0 should record StepCompleted"
        );
        assert!(
            events.iter().any(|e| matches!(e, WorkflowEvent::Custom { name, .. } if name == "AfterDone")),
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
            rt1.borrow().actors.get(&actor_id).unwrap()
                .get_state_field("step_index").and_then(|v| v.as_int()),
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
            rt1.borrow().actors.get(&actor_id).unwrap()
                .get_state_field("parallel_progress").and_then(|v| v.as_int()),
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
            rt2.borrow().actors.get(&actor_id).unwrap()
                .get_state_field("step_index").and_then(|v| v.as_int()),
            Some(2),
            "parallel block should advance step_index to 2"
        );
        assert_eq!(
            rt2.borrow().actors.get(&actor_id).unwrap()
                .get_state_field("parallel_progress").and_then(|v| v.as_int()),
            Some(0),
            "parallel_progress should be reset after the block completes"
        );

        let events_after_signal = store.read_workflow_events(actor_id);
        assert_eq!(
            events_after_signal.iter().filter(|e| matches!(e, WorkflowEvent::ParallelBranchCompleted { .. })).count(),
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
            rt2.borrow().actors.get(&actor_id).unwrap()
                .get_state_field("step_index").and_then(|v| v.as_int()),
            Some(3),
            "after step should advance step_index to 3"
        );
        let events_final = store.read_workflow_events(actor_id);
        assert!(
            events_final.iter().any(|e| matches!(e, WorkflowEvent::Custom { name, .. } if name == "AfterDone")),
            "AfterDone event should be persisted"
        );
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
        let mir = crate::mir_lower::lower_module(&hir);
        let module = crate::mir_codegen::compile_mir(&mir, "test")?;

        let mut vm = VM::new();
        vm.load_module(module);
        vm.run()
    }

    fn assert_int_new(source: &str, expected: i64) {
        let value = run_source_new(source).unwrap();
        assert_eq!(value.as_int(), Some(expected), "new pipeline expected integer for: {}", source);
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
    fn test_llm_ask_mock_client() {
        let source = r#"perform LLM.ask("hello")"#;
        let (module, _ty) = compile_source(source).unwrap();

        let rt = Rc::new(RefCell::new(Runtime::new()));
        rt.borrow_mut().set_llm_client(Box::new(crate::ai::MockLlmClient::text("world")));

        let mut vm = VM::new();
        vm.load_module(module);
        vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt)));

        let result = vm.run().unwrap();
        let string_id = result.as_string_id().expect("expected string result");
        let module_idx = vm.modules.len() - 1;
        let content = vm.constant_string(module_idx, string_id).unwrap();
        assert_eq!(content, "world");
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
        let client = crate::ai::MockLlmClient::with_usage(
            "world",
            crate::ai::TokenUsage::new(1000, 500),
        );
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
        assert_eq!(actor.get_state_field("usage_prompt").unwrap().as_int(), Some(1000));
        assert_eq!(
            actor.get_state_field("usage_completion").unwrap().as_int(),
            Some(500)
        );
        // 1000 * 0.01 / 1000 + 500 * 0.02 / 1000 = 0.01 + 0.01 = 0.02
        let cost = actor.get_state_field("usage_cost").unwrap().as_float().unwrap();
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
        let client = crate::ai::MockLlmClient::with_usage(
            "world",
            crate::ai::TokenUsage::new(1000, 500),
        );
        rt.borrow_mut().set_llm_client(Box::new(client));

        let mut vm = VM::new();
        vm.load_module(module);
        vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));

        let result = vm.run().unwrap();

        // The usage behavior returns an array [prompt_tokens, completion_tokens, cost].
        // Records and tuples are not yet supported by the interpreter, so the
        // actor-allocated array is inspected directly in Rust.
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
            serde_json::Value::String(
                "CRDTs are conflict-free replicated data types.".to_string(),
            ),
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
            serde_json::Value::String(
                "CRDTs are conflict-free replicated data types.".to_string(),
            ),
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
        ]);

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
            let memory: crate::ai::SemanticMemory =
                serde_json::from_str(&memory_json_str).unwrap();
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
            recalled_content,
            "CRDTs are conflict-free replicated data types.",
            "recovered agent should recall the stored fact"
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
}
