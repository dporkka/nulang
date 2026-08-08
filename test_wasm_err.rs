fn main() {
    let source = "let x = 42; match x { 00 => false, _ => true }";
    let ast = nulang::parser::Parser::new(nulang::lexer::Lexer::new(source).lex().unwrap()).parse_module().unwrap();
    let hir = nulang::hir_lower::lower_module(&ast);
    let mir = nulang::mir_lower::lower_module(&hir).unwrap();
    let mut backend = nulang::backends::DefaultWasmBackend;
    let bytes = nulang::backends::WasmBackend::compile(&mut backend, &mir, "main").unwrap();
    match nulang::backends::WasmBackend::run(&mut backend, &bytes) {
        Err(e) => {
            println!("Error: {}", e);
            let mut current: Option<&dyn std::error::Error> = std::error::Error::source(&e);
            while let Some(cause) = current {
                println!("Caused by: {}", cause);
                current = cause.source();
            }
            if let nulang::types::NuError::VMError { msg, .. } = e {
                println!("Msg: {}", msg);
            }
        },
        Ok(v) => println!("Ok: {:?}", v),
    }
}
