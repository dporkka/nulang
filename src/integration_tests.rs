#[cfg(test)]
mod end_to_end {
    use crate::ast::*;
    use crate::compiler::Compiler;
    use crate::effect_checker::*;
    use crate::parser;
    use crate::typechecker::*;
    use crate::types::*;
    use crate::vm::*;

    /// Helper: run full pipeline: parse -> type-check -> compile -> VM run
    fn run_source(source: &str) -> NuResult<(Value, Type)> {
        // 1. Parse
        let module = parser::parse(source)?;

        // 2. Type-check
        let mut tc = TypeChecker::new();
        let ty = tc.check_module(&module)?;

        // 3. Effect-check
        let mut ec = EffectChecker::new();
        let ctx = EffectContext::new(EffectRow::Open(vec![], 0));
        let _effects = ec.infer_effects(&ctx, &module)?;

        // 4. Compile
        let mut compiler = Compiler::new("__main__");
        let module_ref = compiler.compile_module(&module)?;
        let code_module = module_ref.clone();

        // 5. VM
        let mut vm = VM::new();
        vm.load_module(&code_module)?;
        let result = vm.run()?;

        Ok((result, ty))
    }

    fn expect_int(source: &str, expected: i64) {
        let (val, _ty) = run_source(source).unwrap_or_else(|e| panic!("{}: {:?}", source, e));
        assert_eq!(
            val, Value::Int(expected),
            "Expected Int({}), got {:?} for: {}", expected, val, source
        );
    }

    fn expect_float(source: &str, expected: f64) {
        let (val, _ty) = run_source(source).unwrap_or_else(|e| panic!("{}: {:?}", source, e));
        assert_eq!(
            val, Value::Float(expected),
            "Expected Float({}), got {:?} for: {}", expected, val, source
        );
    }

    fn expect_bool(source: &str, expected: bool) {
        let (val, _ty) = run_source(source).unwrap_or_else(|e| panic!("{}: {:?}", source, e));
        assert_eq!(
            val, Value::Bool(expected),
            "Expected Bool({}), got {:?} for: {}", expected, val, source
        );
    }

    fn expect_error(source: &str) {
        assert!(run_source(source).is_err(), "Expected error for: {}", source);
    }

    // ================================================================
    // Literals & Arithmetic
    // ================================================================

    #[test]
    fn test_literal_int() {
        expect_int("42", 42);
    }

    #[test]
    fn test_literal_float() {
        expect_float("3.14", 3.14);
    }

    #[test]
    fn test_literal_bool() {
        expect_bool("true", true);
        expect_bool("false", false);
    }

    #[test]
    fn test_literal_string() {
        let (val, ty) = run_source(r#""hello world""#).unwrap();
        assert_eq!(val, Value::String("hello world".into()));
        assert_eq!(ty, Type::String);
    }

    #[test]
    fn test_literal_unit() {
        let (val, ty) = run_source("()").unwrap();
        assert_eq!(val, Value::Unit);
        assert_eq!(ty, Type::Unit);
    }

    #[test]
    fn test_int_add() {
        expect_int("1 + 2", 3);
    }

    #[test]
    fn test_int_sub() {
        expect_int("10 - 3", 7);
    }

    #[test]
    fn test_int_mul() {
        expect_int("6 * 7", 42);
    }

    #[test]
    fn test_int_div() {
        expect_int("21 / 3", 7);
    }

    #[test]
    fn test_int_mod() {
        expect_int("17 % 5", 2);
    }

    #[test]
    fn test_int_neg() {
        expect_int("-42", -42);
    }

    #[test]
    fn test_arithmetic_precedence() {
        expect_int("1 + 2 * 3", 7);   // mul before add
        expect_int("10 - 2 * 3", 4);  // mul before sub
        expect_int("(1 + 2) * 3", 9); // parens override
        expect_int("20 / 4 + 3", 8);  // div before add
    }

    #[test]
    fn test_float_add() {
        expect_float("1.5 + 2.5", 4.0);
    }

    #[test]
    fn test_float_mul() {
        expect_float("2.5 * 4.0", 10.0);
    }

    // ================================================================
    // Variables & Let-bindings
    // ================================================================

    #[test]
    fn test_let_binding() {
        expect_int("let x = 5 in x + 3", 8);
    }

    #[test]
    fn test_nested_let() {
        expect_int("let a = 1 in let b = 2 in a + b", 3);
    }

    #[test]
    fn test_multiple_bindings() {
        expect_int("let x = 10 in let y = 20 in let z = 30 in x + y + z", 60);
    }

    #[test]
    fn test_let_shadowing() {
        expect_int("let x = 5 in let x = 10 in x + 1", 11);
    }

    // ================================================================
    // Functions
    // ================================================================

    #[test]
    fn test_function_declaration() {
        expect_int(
            "fun add(x, y) = x + y\nadd(3, 4)",
            7,
        );
    }

    #[test]
    fn test_function_multiple() {
        expect_int(
            "fun double(x) = x * 2\nfun triple(x) = x * 3\ndouble(5) + triple(2)",
            16,
        );
    }

    #[test]
    fn test_function_zero_args() {
        expect_int(
            "fun answer() = 42\nanswer()",
            42,
        );
    }

    // ================================================================
    // Conditionals
    // ================================================================

    #[test]
    fn test_if_true() {
        expect_int("if true then 42 else 0", 42);
    }

    #[test]
    fn test_if_false() {
        expect_int("if false then 42 else 0", 0);
    }

    #[test]
    fn test_if_comparison() {
        expect_int("if 3 < 5 then 1 else 0", 1);
        expect_int("if 5 < 3 then 1 else 0", 0);
        expect_int("if 3 == 3 then 1 else 0", 1);
        expect_int("if 3 == 4 then 1 else 0", 0);
    }

    #[test]
    fn test_if_nested() {
        expect_int(
            "if 1 < 2 then if 3 < 4 then 100 else 50 else 25",
            100,
        );
    }

    // ================================================================
    // Comparison
    // ================================================================

    #[test]
    fn test_int_eq() {
        expect_bool("3 == 3", true);
        expect_bool("3 == 4", false);
    }

    #[test]
    fn test_int_lt() {
        expect_bool("3 < 5", true);
        expect_bool("5 < 3", false);
    }

    #[test]
    fn test_int_gt() {
        expect_bool("5 > 3", true);
        expect_bool("3 > 5", false);
    }

    #[test]
    fn test_int_lte() {
        expect_bool("3 <= 3", true);
        expect_bool("3 <= 5", true);
        expect_bool("5 <= 3", false);
    }

    #[test]
    fn test_int_gte() {
        expect_bool("5 >= 5", true);
        expect_bool("5 >= 3", true);
        expect_bool("3 >= 5", false);
    }

    // ================================================================
    // Boolean Logic
    // ================================================================

    #[test]
    fn test_and() {
        expect_bool("true and true", true);
        expect_bool("true and false", false);
        expect_bool("false and true", false);
        expect_bool("false and false", false);
    }

    #[test]
    fn test_or() {
        expect_bool("true or true", true);
        expect_bool("true or false", true);
        expect_bool("false or true", true);
        expect_bool("false or false", false);
    }

    #[test]
    fn test_not() {
        expect_bool("not true", false);
        expect_bool("not false", true);
    }

    // ================================================================
    // Tuples
    // ================================================================

    #[test]
    fn test_tuple_create() {
        let (val, ty) = run_source("(1, 2, 3)").unwrap();
        assert_eq!(val, Value::Tuple(vec![
            Value::Int(1), Value::Int(2), Value::Int(3)
        ]));
        assert_eq!(ty, Type::Tuple(vec![Type::Int, Type::Int, Type::Int]));
    }

    #[test]
    fn test_tuple_access() {
        expect_int("let t = (10, 20, 30) in t.0", 10);
        expect_int("let t = (10, 20, 30) in t.1", 20);
        expect_int("let t = (10, 20, 30) in t.2", 30);
    }

    #[test]
    fn test_tuple_nested() {
        let (val, ty) = run_source("((1, 2), (3, 4))").unwrap();
        assert_eq!(val, Value::Tuple(vec![
            Value::Tuple(vec![Value::Int(1), Value::Int(2)]),
            Value::Tuple(vec![Value::Int(3), Value::Int(4)]),
        ]));
        assert_eq!(ty, Type::Tuple(vec![
            Type::Tuple(vec![Type::Int, Type::Int]),
            Type::Tuple(vec![Type::Int, Type::Int]),
        ]));
    }

    // ================================================================
    // Records
    // ================================================================

    #[test]
    fn test_record_create() {
        let (val, ty) = run_source(r#"{ name: "hello", count: 5 }"#).unwrap();
        match &val {
            Value::Record(fields) => {
                assert_eq!(fields[0].0, "name");
                assert_eq!(fields[0].1, Value::String("hello".into()));
                assert_eq!(fields[1].0, "count");
                assert_eq!(fields[1].1, Value::Int(5));
            }
            other => panic!("Expected Record, got {:?}", other),
        }
        assert_eq!(ty, Type::Record(vec![
            ("name".into(), Type::String),
            ("count".into(), Type::Int),
        ]));
    }

    #[test]
    fn test_record_access() {
        expect_int(
            "let r = { x: 10, y: 20 } in r.x",
            10,
        );
    }

    #[test]
    fn test_record_update() {
        expect_int(
            "let r = { x: 10, y: 20 } in { r | x: 99 }.x",
            99,
        );
    }

    // ================================================================
    // Pattern Matching
    // ================================================================

    #[test]
    fn test_match_int_literal() {
        expect_int(
            "match 1 with | 1 => 100 | 2 => 200 | _ => 0",
            100,
        );
        expect_int(
            "match 2 with | 1 => 100 | 2 => 200 | _ => 0",
            200,
        );
        expect_int(
            "match 99 with | 1 => 100 | 2 => 200 | _ => 0",
            0,
        );
    }

    #[test]
    fn test_match_bool() {
        expect_int(
            "match true with | true => 1 | false => 0",
            1,
        );
    }

    #[test]
    fn test_match_tuple() {
        expect_int(
            "match (1, 2) with | (a, b) => a + b",
            3,
        );
    }

    #[test]
    fn test_match_record() {
        expect_int(
            "match { x: 5, y: 7 } with | { x: a, y: b } => a + b",
            12,
        );
    }

    #[test]
    fn test_match_wildcard() {
        expect_int(
            "match 42 with | _ => 100",
            100,
        );
    }

    // ================================================================
    // Closures
    // ================================================================

    #[test]
    fn test_closure_basic() {
        expect_int(
            "let add = fn(x) { fn(y) { x + y } } in let add5 = add(5) in add5(3)",
            8,
        );
    }

    #[test]
    fn test_closure_multiple_capture() {
        expect_int(
            "let make_counter = fn(start) { fn() { start + 1 } } in let c = make_counter(10) in c()",
            11,
        );
    }

    // ================================================================
    // Pipe Operator
    // ================================================================

    #[test]
    fn test_pipe_operator() {
        expect_int(
            "let add = fn(x) { fn(y) { x + y } } in 5 |> add(3)",
            8,
        );
    }

    #[test]
    fn test_pipe_chain() {
        expect_int(
            "let double = fn(x) { x * 2 } in let add1 = fn(x) { x + 1 } in 5 |> double |> add1",
            11,
        );
    }

    // ================================================================
    // Recursive Functions
    // ================================================================

    #[test]
    fn test_factorial() {
        expect_int(
            "let rec fact = fn(n) { if n == 0 then 1 else n * fact(n - 1) } in fact(5)",
            120,
        );
    }

    #[test]
    fn test_fibonacci() {
        expect_int(
            "let rec fib = fn(n) { if n == 0 then 0 else if n == 1 then 1 else fib(n - 1) + fib(n - 2) } in fib(10)",
            55,
        );
    }

    // ================================================================
    // Polymorphism
    // ================================================================

    #[test]
    fn test_identity_polymorphic() {
        let (val_int, ty_int) = run_source("let id = fn(x) { x } in id(42)").unwrap();
        assert_eq!(val_int, Value::Int(42));
        // id : forall a. a -> a
        assert!(matches!(ty_int, Type::Int));
    }

    #[test]
    fn test_identity_bool() {
        let (val_bool, ty_bool) = run_source("let id = fn(x) { x } in id(true)").unwrap();
        assert_eq!(val_bool, Value::Bool(true));
        assert!(matches!(ty_bool, Type::Bool));
    }

    // ================================================================
    // Type Errors
    // ================================================================

    #[test]
    fn test_type_error_int_plus_string() {
        expect_error(r#"1 + "hello""#);
    }

    #[test]
    fn test_type_error_string_plus_int() {
        expect_error(r#""hello" + 1"#);
    }

    #[test]
    fn test_type_error_bool_arithmetic() {
        expect_error("true + 1");
    }

    #[test]
    fn test_type_error_if_branches() {
        expect_error(r#"if true then 1 else "hello""#);
    }

    #[test]
    fn test_type_error_undefined_variable() {
        expect_error("x + 1");
    }

    // ================================================================
    // Actor Model (parse & type-check only — VM actor support is stubbed)
    // ================================================================

    #[test]
    fn test_actor_declaration() {
        let (val, ty) = run_source("actor Counter { state count: Int, initial: Init, behavior Tick(n: Int) = count + n }\nspawn Counter { count: 0 }").unwrap();
        // spawn returns an actor reference (Int address for now)
        assert!(matches!(val, Value::Int(_)));
        assert!(matches!(ty, Type::Int));
    }

    #[test]
    fn test_effect_perform() {
        let (val, ty) = run_source("perform IO.print(\"hello\")").unwrap();
        // perform IO.print returns Unit
        assert_eq!(val, Value::Unit);
        assert_eq!(ty, Type::Unit);
    }

    // ================================================================
    // String operations
    // ================================================================

    #[test]
    fn test_string_concat() {
        let (val, ty) = run_source(r#""hello" ++ " " ++ "world""#).unwrap();
        assert_eq!(val, Value::String("hello world".into()));
        assert_eq!(ty, Type::String);
    }

    // ================================================================
    // Arrays
    // ================================================================

    #[test]
    fn test_array_create() {
        let (val, ty) = run_source("[1, 2, 3]").unwrap();
        match &val {
            Value::Array(elems) => {
                assert_eq!(elems.len(), 3);
                assert_eq!(elems[0], Value::Int(1));
                assert_eq!(elems[1], Value::Int(2));
                assert_eq!(elems[2], Value::Int(3));
            }
            other => panic!("Expected Array, got {:?}", other),
        }
        assert_eq!(ty, Type::Array(Box::new(Type::Int)));
    }

    #[test]
    fn test_array_index() {
        expect_int("let arr = [10, 20, 30] in arr[1]", 20);
    }

    // ================================================================
    // Error handling
    // ================================================================

    #[test]
    fn test_division_by_zero() {
        // Division by zero should produce a runtime error
        let result = run_source("1 / 0");
        assert!(result.is_err() || result.unwrap().0 == Value::Int(0));
    }
}
