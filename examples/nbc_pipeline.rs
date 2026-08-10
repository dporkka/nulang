// examples/nbc_pipeline.rs — Demonstrates the full .nbc pipeline programmatically.
//
// This example:
// 1. Compiles a Nulang Core program to bytecode
// 2. Exports it to .nbc binary
// 3. Loads the .nbc artifact
// 4. Executes it and prints the result
//
// Build: cargo build --example nbc_pipeline
// Run:   cargo run --example nbc_pipeline

use nulang::bytecode::{CodeModule, Constant, Instruction};

fn main() {
    // Program: 1 + 2 * 3 = 7
    // Bytecode:
    //   ConstU 0→r0 (2)
    //   ConstU 1→r1 (3)
    //   IMul r0,r1,r0   (r0 = 2 * 3 = 6)
    //   ConstU 2→r2 (1)
    //   IAdd r0,r2,r0   (r0 = 6 + 1 = 7)
    //   RetVal r0
    let instrs = [
        0x07000000, 0x07000101, 0x22000100, 0x07000202, 0x20000200, 0x57000000,
    ];
    let mut module = CodeModule::new("example");
    module.constants.push(Constant::Int(2));
    module.constants.push(Constant::Int(3));
    module.constants.push(Constant::Int(1));
    for &w in &instrs {
        module.instructions.push(Instruction::decode(w).unwrap());
    }
    module.entry_point = Some(0);

    // Export to .nbc
    let source_hash = *blake3::hash(b"1 + 2 * 3").as_bytes();
    let nbc_bytes = module.to_nbc(Some(source_hash)).expect("to_nbc");
    println!("Exported .nbc: {} bytes (format v1)", nbc_bytes.len());

    // Load .nbc artifact
    let artifact = CodeModule::from_nbc(&nbc_bytes).expect("from_nbc");
    assert_eq!(artifact.format_version, 1);
    assert_eq!(artifact.source_hash, Some(source_hash));

    // Execute
    let mut vm = nulang::vm::VM::new();
    vm.load_module(artifact.module);
    let result = vm.run().expect("execute");
    println!("Result: {} (expected 7)", result.as_int().unwrap());
    assert_eq!(result.as_int(), Some(7));
    println!("Pipeline verified: compile → .nbc → load → execute ✓");
}
