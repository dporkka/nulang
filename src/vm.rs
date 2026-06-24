//! Register-based VM with token-threaded dispatch.

use crate::bytecode::*;
use crate::types::Value;

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// VM state
// ---------------------------------------------------------------------------

const STACK_SIZE: usize = 64 * 1024;
const REG_COUNT: usize = 256;

pub struct VM {
    // Register file (sliced by call frame)
    registers: [Value; REG_COUNT],
    // Call stack
    frames: Vec<CallFrame>,
    // Program counter (index into current module's instructions)
    pc: usize,
    // Loaded modules
    modules: Vec<Module>,
    // Function name -> (module_idx, behavior_idx)
    function_table: HashMap<String, (usize, usize)>,
    // Heap (simple bump allocator for MVP)
    heap: Vec<u8>,
    heap_ptr: usize,
    // String constants
    strings: Vec<String>,
    // Output buffer (for print operations)
    output: Vec<String>,
}

#[derive(Debug, Clone)]
struct CallFrame {
    module_idx: usize,
    behavior_idx: usize,
    pc: usize,
    base_reg: usize,
    return_reg: u8,
}

#[derive(Debug)]
pub enum VMError {
    UnknownFunction(String),
    InvalidOpcode(u8),
    TypeMismatch { expected: String, got: String },
    DivisionByZero,
    StackOverflow,
    OutOfBounds,
    ModuleNotLoaded,
    InvalidConstant(u16),
    MissingHandler(String),
}

impl std::fmt::Display for VMError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VMError::UnknownFunction(name) => write!(f, "Unknown function: {}", name),
            VMError::InvalidOpcode(op) => write!(f, "Invalid opcode: {}", op),
            VMError::TypeMismatch { expected, got } => write!(f, "Type mismatch: expected {}, got {}", expected, got),
            VMError::DivisionByZero => write!(f, "Division by zero"),
            VMError::StackOverflow => write!(f, "Stack overflow"),
            VMError::OutOfBounds => write!(f, "Out of bounds"),
            VMError::ModuleNotLoaded => write!(f, "Module not loaded"),
            VMError::InvalidConstant(idx) => write!(f, "Invalid constant index: {}", idx),
            VMError::MissingHandler(name) => write!(f, "Missing effect handler: {}", name),
        }
    }
}

impl std::error::Error for VMError {}

// ---------------------------------------------------------------------------
// Dispatch table
// ---------------------------------------------------------------------------

macro_rules! dispatch {
    ($vm:ident, $inst:ident, $body:block) => {
        {
            let opcode = OpCode::from_u8($inst.opcode).ok_or(VMError::InvalidOpcode($inst.opcode))?;
            match opcode {
                OpCode::LoadConst => {
                    let idx = (($inst.b as u16) << 8) | ($inst.c as u16);
                    $vm.registers[$inst.a as usize] = $vm.load_constant(idx)?;
                }
                OpCode::LoadNull => {
                    $vm.registers[$inst.a as usize] = Value::null();
                }
                OpCode::Move => {
                    $vm.registers[$inst.a as usize] = $vm.registers[$inst.b as usize];
                }
                OpCode::Add => {
                    let lhs = $vm.registers[$inst.b as usize];
                    let rhs = $vm.registers[$inst.c as usize];
                    $vm.registers[$inst.a as usize] = if lhs.is_int() && rhs.is_int() {
                        Value::int(lhs.as_int().unwrap() + rhs.as_int().unwrap())
                    } else if let (Some(l), Some(r)) = (lhs.as_float(), rhs.as_float()) {
                        Value::float(l + r)
                    } else {
                        return Err(VMError::TypeMismatch { expected: "number".to_string(), got: "other".to_string() });
                    };
                }
                OpCode::Sub => {
                    let lhs = $vm.registers[$inst.b as usize];
                    let rhs = $vm.registers[$inst.c as usize];
                    $vm.registers[$inst.a as usize] = if lhs.is_int() && rhs.is_int() {
                        Value::int(lhs.as_int().unwrap() - rhs.as_int().unwrap())
                    } else if let (Some(l), Some(r)) = (lhs.as_float(), rhs.as_float()) {
                        Value::float(l - r)
                    } else {
                        return Err(VMError::TypeMismatch { expected: "number".to_string(), got: "other".to_string() });
                    };
                }
                OpCode::Mul => {
                    let lhs = $vm.registers[$inst.b as usize];
                    let rhs = $vm.registers[$inst.c as usize];
                    $vm.registers[$inst.a as usize] = if lhs.is_int() && rhs.is_int() {
                        Value::int(lhs.as_int().unwrap() * rhs.as_int().unwrap())
                    } else if let (Some(l), Some(r)) = (lhs.as_float(), rhs.as_float()) {
                        Value::float(l * r)
                    } else {
                        return Err(VMError::TypeMismatch { expected: "number".to_string(), got: "other".to_string() });
                    };
                }
                OpCode::Div => {
                    let lhs = $vm.registers[$inst.b as usize];
                    let rhs = $vm.registers[$inst.c as usize];
                    if rhs.is_int() && rhs.as_int().unwrap() == 0 {
                        return Err(VMError::DivisionByZero);
                    }
                    $vm.registers[$inst.a as usize] = if lhs.is_int() && rhs.is_int() {
                        Value::int(lhs.as_int().unwrap() / rhs.as_int().unwrap())
                    } else if let (Some(l), Some(r)) = (lhs.as_float(), rhs.as_float()) {
                        if r == 0.0 {
                            return Err(VMError::DivisionByZero);
                        }
                        Value::float(l / r)
                    } else {
                        return Err(VMError::TypeMismatch { expected: "number".to_string(), got: "other".to_string() });
                    };
                }
                OpCode::Mod => {
                    let lhs = $vm.registers[$inst.b as usize];
                    let rhs = $vm.registers[$inst.c as usize];
                    $vm.registers[$inst.a as usize] = if lhs.is_int() && rhs.is_int() {
                        Value::int(lhs.as_int().unwrap() % rhs.as_int().unwrap())
                    } else {
                        return Err(VMError::TypeMismatch { expected: "int".to_string(), got: "other".to_string() });
                    };
                }
                OpCode::Neg => {
                    let val = $vm.registers[$inst.b as usize];
                    $vm.registers[$inst.a as usize] = if val.is_int() {
                        Value::int(-val.as_int().unwrap())
                    } else if let Some(f) = val.as_float() {
                        Value::float(-f)
                    } else {
                        return Err(VMError::TypeMismatch { expected: "number".to_string(), got: "other".to_string() });
                    };
                }
                OpCode::Eq => {
                    let lhs = $vm.registers[$inst.b as usize];
                    let rhs = $vm.registers[$inst.c as usize];
                    $vm.registers[$inst.a as usize] = Value::bool(lhs == rhs);
                }
                OpCode::Ne => {
                    let lhs = $vm.registers[$inst.b as usize];
                    let rhs = $vm.registers[$inst.c as usize];
                    $vm.registers[$inst.a as usize] = Value::bool(lhs != rhs);
                }
                OpCode::Lt => {
                    let lhs = $vm.registers[$inst.b as usize];
                    let rhs = $vm.registers[$inst.c as usize];
                    let result = if let (Some(l), Some(r)) = (lhs.as_int(), rhs.as_int()) {
                        l < r
                    } else if let (Some(l), Some(r)) = (lhs.as_float(), rhs.as_float()) {
                        l < r
                    } else {
                        false
                    };
                    $vm.registers[$inst.a as usize] = Value::bool(result);
                }
                OpCode::Le => {
                    let lhs = $vm.registers[$inst.b as usize];
                    let rhs = $vm.registers[$inst.c as usize];
                    let result = if let (Some(l), Some(r)) = (lhs.as_int(), rhs.as_int()) {
                        l <= r
                    } else if let (Some(l), Some(r)) = (lhs.as_float(), rhs.as_float()) {
                        l <= r
                    } else {
                        false
                    };
                    $vm.registers[$inst.a as usize] = Value::bool(result);
                }
                OpCode::Gt => {
                    let lhs = $vm.registers[$inst.b as usize];
                    let rhs = $vm.registers[$inst.c as usize];
                    let result = if let (Some(l), Some(r)) = (lhs.as_int(), rhs.as_int()) {
                        l > r
                    } else if let (Some(l), Some(r)) = (lhs.as_float(), rhs.as_float()) {
                        l > r
                    } else {
                        false
                    };
                    $vm.registers[$inst.a as usize] = Value::bool(result);
                }
                OpCode::Ge => {
                    let lhs = $vm.registers[$inst.b as usize];
                    let rhs = $vm.registers[$inst.c as usize];
                    let result = if let (Some(l), Some(r)) = (lhs.as_int(), rhs.as_int()) {
                        l >= r
                    } else if let (Some(l), Some(r)) = (lhs.as_float(), rhs.as_float()) {
                        l >= r
                    } else {
                        false
                    };
                    $vm.registers[$inst.a as usize] = Value::bool(result);
                }
                OpCode::And => {
                    let lhs = $vm.registers[$inst.b as usize];
                    let rhs = $vm.registers[$inst.c as usize];
                    $vm.registers[$inst.a as usize] = Value::bool(
                        lhs.as_bool().unwrap_or(false) && rhs.as_bool().unwrap_or(false)
                    );
                }
                OpCode::Or => {
                    let lhs = $vm.registers[$inst.b as usize];
                    let rhs = $vm.registers[$inst.c as usize];
                    $vm.registers[$inst.a as usize] = Value::bool(
                        lhs.as_bool().unwrap_or(false) || rhs.as_bool().unwrap_or(false)
                    );
                }
                OpCode::Not => {
                    let val = $vm.registers[$inst.b as usize];
                    $vm.registers[$inst.a as usize] = Value::bool(!val.as_bool().unwrap_or(false));
                }
                OpCode::Jump => {
                    let offset = (($inst.b as i16) << 8) | ($inst.c as i16);
                    if offset >= 0 {
                        $vm.pc += offset as usize;
                    } else {
                        $vm.pc = ($vm.pc as i64 + offset as i64) as usize;
                    }
                    // The increment at the end of the loop will add 1, so subtract 1 here
                    $vm.pc = $vm.pc.saturating_sub(1);
                }
                OpCode::JumpIf => {
                    let val = $vm.registers[$inst.a as usize];
                    if val.as_bool().unwrap_or(false) {
                        let offset = (($inst.b as i16) << 8) | ($inst.c as i16);
                        if offset >= 0 {
                            $vm.pc += offset as usize;
                        } else {
                            $vm.pc = ($vm.pc as i64 + offset as i64) as usize;
                        }
                        $vm.pc = $vm.pc.saturating_sub(1);
                    }
                }
                OpCode::JumpIfNot => {
                    let val = $vm.registers[$inst.a as usize];
                    if !val.as_bool().unwrap_or(true) {
                        let offset = (($inst.b as i16) << 8) | ($inst.c as i16);
                        if offset >= 0 {
                            $vm.pc += offset as usize;
                        } else {
                            $vm.pc = ($vm.pc as i64 + offset as i64) as usize;
                        }
                        $vm.pc = $vm.pc.saturating_sub(1);
                    }
                }
                OpCode::Call => {
                    let func_val = $vm.registers[$inst.b as usize];
                    // Look up function in function table
                    let func_name = $vm.find_function_name(func_val)?;
                    let (mod_idx, beh_idx) = $vm.function_table.get(&func_name)
                        .copied()
                        .ok_or(VMError::UnknownFunction(func_name.clone()))?;
                    let entry = $vm.modules[mod_idx].behavior_table[*beh_idx].entry_point as usize;
                    $vm.frames.push(CallFrame {
                        module_idx: *mod_idx,
                        behavior_idx: *beh_idx,
                        pc: $vm.pc,
                        base_reg: $inst.a as usize,
                        return_reg: $inst.a,
                    });
                    $vm.pc = entry;
                    // Continue without incrementing pc
                    continue;
                }
                OpCode::Ret => {
                    let val = $vm.registers[$inst.a as usize];
                    if let Some(frame) = $vm.frames.pop() {
                        $vm.pc = frame.pc;
                        $vm.registers[frame.return_reg as usize] = val;
                    } else {
                        // Top-level return
                        $vm.registers[0] = val;
                        break;
                    }
                }
                OpCode::Halt => {
                    break;
                }
                OpCode::NewTuple => {
                    // For now, just store the first element
                    if $inst.c > $inst.b {
                        $vm.registers[$inst.a as usize] = $vm.registers[$inst.b as usize];
                    } else {
                        $vm.registers[$inst.a as usize] = Value::null();
                    }
                }
                OpCode::FieldGet => {
                    // Placeholder: just return the object
                    $vm.registers[$inst.a as usize] = $vm.registers[$inst.b as usize];
                }
                OpCode::FieldSet => {
                    // Placeholder
                }
                OpCode::NewArray => {
                    // Placeholder
                    $vm.registers[$inst.a as usize] = Value::null();
                }
                OpCode::ArrayGet => {
                    $vm.registers[$inst.a as usize] = Value::null();
                }
                OpCode::ArraySet => {
                    // Placeholder
                }
                OpCode::Cons => {
                    // Placeholder
                    $vm.registers[$inst.a as usize] = $vm.registers[$inst.b as usize];
                }
                OpCode::TestTag => {
                    // Placeholder
                    $vm.registers[$inst.a as usize] = Value::bool(true);
                }
                OpCode::TestTupleLen => {
                    // Placeholder
                    $vm.registers[$inst.a as usize] = Value::bool(true);
                }
                OpCode::Destructure => {
                    // Placeholder
                }
                OpCode::Spawn => {
                    // Placeholder: return a dummy actor ref
                    $vm.registers[$inst.a as usize] = Value::actor_ref(0, 0, 1);
                }
                OpCode::Send => {
                    // Placeholder
                }
                OpCode::Ask => {
                    // Placeholder: return null
                    $vm.registers[$inst.a as usize] = Value::null();
                }
                OpCode::Receive => {
                    // Placeholder
                    $vm.registers[$inst.a as usize] = Value::null();
                }
                OpCode::SelfAddr => {
                    $vm.registers[$inst.a as usize] = Value::actor_ref(0, 0, 1);
                }
                OpCode::Perform => {
                    // Placeholder
                    $vm.registers[$inst.a as usize] = Value::null();
                }
                OpCode::Handle => {
                    // Placeholder
                }
                OpCode::PopHandler => {
                    // Placeholder
                }
                OpCode::Migrate => {
                    // Placeholder
                    $vm.registers[$inst.a as usize] = $vm.registers[$inst.b as usize];
                }
                OpCode::NewRecord => {
                    // Placeholder
                    $vm.registers[$inst.a as usize] = Value::null();
                }
                OpCode::TailCall => {
                    // Placeholder
                    break;
                }
            }
            $vm.pc += 1;
        }
    };
}

// ---------------------------------------------------------------------------
// VM methods
// ---------------------------------------------------------------------------

impl VM {
    pub fn new() -> Self {
        VM {
            registers: [Value::null(); REG_COUNT],
            frames: Vec::with_capacity(1024),
            pc: 0,
            modules: Vec::new(),
            function_table: HashMap::new(),
            heap: vec![0; 1024 * 1024], // 1MB heap
            heap_ptr: 0,
            strings: Vec::new(),
            output: Vec::new(),
        }
    }

    pub fn load_module(&mut self, module: &Module) -> Result<(), VMError> {
        let mod_idx = self.modules.len();
        // Register all functions in the module
        for (beh_idx, beh) in module.behavior_table.iter().enumerate() {
            self.function_table.insert(
                beh.name.clone(),
                (mod_idx, beh_idx),
            );
        }
        // Copy constants
        for c in &module.constants {
            if let Constant::String(s) = c {
                self.strings.push(s.clone());
            }
        }
        self.modules.push(module.clone());
        Ok(())
    }

    pub fn call_function(&mut self, name: &str, args: &[Value]) -> Result<Value, VMError> {
        let (mod_idx, beh_idx) = self.function_table.get(name)
            .copied()
            .ok_or_else(|| VMError::UnknownFunction(name.to_string()))?;

        let entry = self.modules[mod_idx].behavior_table[beh_idx].entry_point as usize;

        // Set up arguments in registers
        for (i, &arg) in args.iter().enumerate() {
            self.registers[i + 1] = arg;
        }

        self.pc = entry;
        self.run()?;

        Ok(self.registers[0])
    }

    pub fn run(&mut self) -> Result<(), VMError> {
        let max_instructions = 10_000_000;
        let mut executed = 0;

        while self.pc < self.modules.last().map(|m| m.instructions.len()).unwrap_or(0)
            && executed < max_instructions {
            let inst = self.modules[self.frames.last().map(|f| f.module_idx).unwrap_or(self.modules.len() - 1)]
                .instructions[self.pc];
            dispatch!(self, inst, {});
            executed += 1;
        }

        if executed >= max_instructions {
            return Err(VMError::StackOverflow);
        }

        Ok(())
    }

    fn load_constant(&self, idx: u16) -> Result<Value, VMError> {
        let mod_idx = self.frames.last().map(|f| f.module_idx).unwrap_or(0);
        let module = &self.modules[mod_idx];
        module.constants.get(idx as usize)
            .map(|c| match c {
                Constant::Int(n) => Value::int(*n),
                Constant::Float(f) => Value::float(*f),
                Constant::String(s) => Value::heap_ptr(s.as_ptr() as usize),
                Constant::Bool(b) => Value::bool(*b),
                Constant::Unit => Value::null(),
                _ => Value::null(),
            })
            .ok_or(VMError::InvalidConstant(idx))
    }

    fn find_function_name(&self, _val: Value) -> Result<String, VMError> {
        // In a real implementation, this would extract the function name from a closure value
        // For now, search the function table
        self.function_table.keys().next()
            .cloned()
            .ok_or_else(|| VMError::UnknownFunction("unknown".to_string()))
    }

    pub fn output(&self) -> &[String] {
        &self.output
    }
}

impl Default for VM {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;
    use crate::compiler::compile;
    use crate::parser::parse;
    use crate::types::Span;

    fn s() -> Span { Span { start: 0, end: 0, line: 1, col: 1 } }

    #[test]
    fn test_vm_arithmetic() {
        let ast = parse("fun main() = 1 + 2 * 3\n").unwrap();
        let module = compile(&ast);
        let mut vm = VM::new();
        vm.load_module(&module).unwrap();
        let result = vm.call_function("main", &[]).unwrap();
        assert!(result.is_int());
        assert_eq!(result.as_int(), Some(7));
    }

    #[test]
    fn test_vm_comparison() {
        let ast = parse("fun main() = if 1 < 2 then 42 else 0\n").unwrap();
        let module = compile(&ast);
        let mut vm = VM::new();
        vm.load_module(&module).unwrap();
        let result = vm.call_function("main", &[]).unwrap();
        assert!(result.is_int());
        assert_eq!(result.as_int(), Some(42));
    }

    #[test]
    fn test_vm_function_call() {
        let ast = parse("fun add(x, y) = x + y\nfun main() = add(3, 4)\n").unwrap();
        let module = compile(&ast);
        let mut vm = VM::new();
        vm.load_module(&module).unwrap();
        let result = vm.call_function("main", &[]).unwrap();
        assert!(result.is_int());
        assert_eq!(result.as_int(), Some(7));
    }

    #[test]
    fn test_vm_nested_function() {
        let input = r#"
fun outer(x) =
  let y = x + 1 in
  y * 2

fun main() = outer(5)
"#;
        let ast = parse(input).unwrap();
        let module = compile(&ast);
        let mut vm = VM::new();
        vm.load_module(&module).unwrap();
        let result = vm.call_function("main", &[]).unwrap();
        assert!(result.is_int());
        assert_eq!(result.as_int(), Some(12));
    }
}
