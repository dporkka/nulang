use wasm_encoder::{CodeSection, Function, Instruction, Module, TypeSection, ValType, BlockType};

fn main() {
    let mut module = Module::new();

    let mut types = TypeSection::new();
    types.ty().function([], [ValType::I64]);
    module.section(&types);

    let mut funcs = wasm_encoder::FunctionSection::new();
    funcs.function(0);
    module.section(&funcs);

    let mut codes = CodeSection::new();
    let mut body = Function::new([]);
    
    body.instruction(&Instruction::Loop(BlockType::Empty));
    body.instruction(&Instruction::Block(BlockType::Empty));
    
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::BrTable(std::borrow::Cow::Owned(vec![0]), 0));
    
    // End block
    body.instruction(&Instruction::End);
    
    // Terminator
    body.instruction(&Instruction::I64Const(0));
    body.instruction(&Instruction::Return);

    // End loop
    body.instruction(&Instruction::End);
    
    // Default return
    body.instruction(&Instruction::I64Const(0));
    body.instruction(&Instruction::End); // function end
    body.instruction(&Instruction::End); // EXTRA END
    
    codes.function(&body);
    module.section(&codes);

    std::fs::write("test.wasm", module.finish()).unwrap();
}