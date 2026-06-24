//! Register-based virtual machine with token-threaded dispatch.

use crate::bytecode::*;
use crate::types::{NuError, NuResult, Span, Value};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// VM Error
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
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
// Call Frame
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CallFrame {
    module_idx: usize,
    behavior_idx: usize,
    pc: usize,
    base_reg: usize,
    return_reg: u8,
}

// ---------------------------------------------------------------------------
// VM
// ---------------------------------------------------------------------------

const STACK_SIZE: usize = 64 * 1024;
const REG_COUNT: usize = 256;

pub struct VM {
    registers: [Value; REG_COUNT],
    frames: Vec<CallFrame>,
    pc: usize,
    modules: Vec<CodeModule>,
    function_table: HashMap<String, (usize, usize)>,
    heap: Vec<u8>,
    heap_ptr: usize,
    strings: Vec<String>,
    output: Vec<String>,
}

impl VM {
    pub fn new() -> Self {
        VM {
            registers: [Value::Unit; REG_COUNT],
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

    pub fn load_module(&mut self, module: &CodeModule) -> NuResult<()> {
        let mod_idx = self.modules.len();

        // Register all behaviors in the function table
        for (beh_idx, beh) in module.behaviors.iter().enumerate() {
            self.function_table.insert(
                beh.name.clone(),
                (mod_idx, beh_idx),
            );
        }

        // Copy string constants
        for c in &module.constants {
            if let Constant::String(s) = c {
                self.strings.push(s.clone());
            }
        }

        self.modules.push(module.clone());
        Ok(())
    }

    pub fn call_function(&mut self, name: &str, args: &[Value]) -> NuResult<Value> {
        let (mod_idx, beh_idx) = self.function_table.get(name)
            .copied()
            .ok_or_else(|| NuError::RuntimeError(format!("Unknown function: {}", name)))?;

        let entry = self.modules[mod_idx].behaviors[beh_idx].code_offset;

        // Set up arguments in registers r1, r2, ...
        for (i, &arg) in args.iter().enumerate() {
            if i + 1 < REG_COUNT {
                self.registers[i + 1] = arg.clone();
            }
        }

        self.pc = entry;
        self.run()?;

        Ok(self.registers[0].clone())
    }

    pub fn run(&mut self) -> NuResult<Value> {
        let max_instructions = 10_000_000;
        let mut executed = 0;

        let module_idx = self.modules.len().saturating_sub(1);

        while self.pc < self.modules[module_idx].instructions.len()
            && executed < max_instructions
        {
            let inst = self.modules[module_idx].instructions[self.pc];
            let opcode = inst.opcode.as_u8();

            match inst.opcode {
                // == Constants ==
                OpCode::Const0 => {
                    self.registers[inst.op1 as usize] = Value::Int(0);
                }
                OpCode::Const1 => {
                    self.registers[inst.op1 as usize] = Value::Int(1);
                }
                OpCode::Const2 => {
                    self.registers[inst.op1 as usize] = Value::Int(2);
                }
                OpCode::ConstM1 => {
                    self.registers[inst.op1 as usize] = Value::Int(-1);
                }
                OpCode::ConstU => {
                    let idx = inst.imm16() as usize;
                    self.registers[inst.op1 as usize] = self.load_constant(idx)?;
                }
                OpCode::LoadNull => {
                    self.registers[inst.op1 as usize] = Value::Unit;
                }
                OpCode::Move => {
                    self.registers[inst.op1 as usize] = self.registers[inst.op2 as usize].clone();
                }

                // == Arithmetic - Integer ==
                OpCode::IAdd => {
                    let a = self.registers[inst.op1 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    let b = self.registers[inst.op2 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    self.registers[inst.op3 as usize] = Value::Int(a + b);
                }
                OpCode::ISub => {
                    let a = self.registers[inst.op1 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    let b = self.registers[inst.op2 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    self.registers[inst.op3 as usize] = Value::Int(a - b);
                }
                OpCode::IMul => {
                    let a = self.registers[inst.op1 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    let b = self.registers[inst.op2 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    self.registers[inst.op3 as usize] = Value::Int(a * b);
                }
                OpCode::IDiv => {
                    let a = self.registers[inst.op1 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    let b = self.registers[inst.op2 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    if b == 0 {
                        return Err(NuError::RuntimeError("Division by zero".into()));
                    }
                    self.registers[inst.op3 as usize] = Value::Int(a / b);
                }
                OpCode::IMod => {
                    let a = self.registers[inst.op1 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    let b = self.registers[inst.op2 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    if b == 0 {
                        return Err(NuError::RuntimeError("Division by zero".into()));
                    }
                    self.registers[inst.op3 as usize] = Value::Int(a % b);
                }
                OpCode::INeg => {
                    let a = self.registers[inst.op1 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    self.registers[inst.op2 as usize] = Value::Int(-a);
                }
                OpCode::IInc => {
                    let a = self.registers[inst.op1 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    self.registers[inst.op1 as usize] = Value::Int(a + 1);
                }
                OpCode::IDec => {
                    let a = self.registers[inst.op1 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    self.registers[inst.op1 as usize] = Value::Int(a - 1);
                }

                // == Arithmetic - Float ==
                OpCode::FAdd => {
                    let a = self.registers[inst.op1 as usize].float_val();
                    let b = self.registers[inst.op2 as usize].float_val();
                    self.registers[inst.op3 as usize] = Value::Float(f64::to_bits(a + b));
                }
                OpCode::FSub => {
                    let a = self.registers[inst.op1 as usize].float_val();
                    let b = self.registers[inst.op2 as usize].float_val();
                    self.registers[inst.op3 as usize] = Value::Float(f64::to_bits(a - b));
                }
                OpCode::FMul => {
                    let a = self.registers[inst.op1 as usize].float_val();
                    let b = self.registers[inst.op2 as usize].float_val();
                    self.registers[inst.op3 as usize] = Value::Float(f64::to_bits(a * b));
                }
                OpCode::FDiv => {
                    let a = self.registers[inst.op1 as usize].float_val();
                    let b = self.registers[inst.op2 as usize].float_val();
                    if b == 0.0 {
                        return Err(NuError::RuntimeError("Division by zero".into()));
                    }
                    self.registers[inst.op3 as usize] = Value::Float(f64::to_bits(a / b));
                }
                OpCode::FNeg => {
                    let a = self.registers[inst.op1 as usize].float_val();
                    self.registers[inst.op2 as usize] = Value::Float(f64::to_bits(-a));
                }
                OpCode::IToF => {
                    let a = self.registers[inst.op1 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    self.registers[inst.op2 as usize] = Value::Float(f64::to_bits(a as f64));
                }
                OpCode::FToI => {
                    let a = self.registers[inst.op1 as usize].float_val();
                    self.registers[inst.op2 as usize] = Value::Int(a as i64);
                }

                // == Comparison ==
                OpCode::ICmpEq => {
                    let a = self.registers[inst.op1 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    let b = self.registers[inst.op2 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    self.registers[inst.op3 as usize] = Value::Bool(a == b);
                }
                OpCode::ICmpLt => {
                    let a = self.registers[inst.op1 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    let b = self.registers[inst.op2 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    self.registers[inst.op3 as usize] = Value::Bool(a < b);
                }
                OpCode::ICmpGt => {
                    let a = self.registers[inst.op1 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    let b = self.registers[inst.op2 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    self.registers[inst.op3 as usize] = Value::Bool(a > b);
                }
                OpCode::ICmpLe => {
                    let a = self.registers[inst.op1 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    let b = self.registers[inst.op2 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    self.registers[inst.op3 as usize] = Value::Bool(a <= b);
                }
                OpCode::ICmpGe => {
                    let a = self.registers[inst.op1 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    let b = self.registers[inst.op2 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int".into()))?;
                    self.registers[inst.op3 as usize] = Value::Bool(a >= b);
                }
                OpCode::FCmpEq => {
                    let a = self.registers[inst.op1 as usize].float_val();
                    let b = self.registers[inst.op2 as usize].float_val();
                    self.registers[inst.op3 as usize] = Value::Bool((a - b).abs() < f64::EPSILON);
                }
                OpCode::FCmpLt => {
                    let a = self.registers[inst.op1 as usize].float_val();
                    let b = self.registers[inst.op2 as usize].float_val();
                    self.registers[inst.op3 as usize] = Value::Bool(a < b);
                }
                OpCode::FCmpGt => {
                    let a = self.registers[inst.op1 as usize].float_val();
                    let b = self.registers[inst.op2 as usize].float_val();
                    self.registers[inst.op3 as usize] = Value::Bool(a > b);
                }
                OpCode::SCmpEq => {
                    let a = self.registers[inst.op1 as usize].as_string()
                        .ok_or_else(|| NuError::RuntimeError("Expected String".into()))?;
                    let b = self.registers[inst.op2 as usize].as_string()
                        .ok_or_else(|| NuError::RuntimeError("Expected String".into()))?;
                    self.registers[inst.op3 as usize] = Value::Bool(a == b);
                }

                // == Logic ==
                OpCode::Not => {
                    let a = self.registers[inst.op1 as usize].as_bool()
                        .ok_or_else(|| NuError::RuntimeError("Expected Bool".into()))?;
                    self.registers[inst.op2 as usize] = Value::Bool(!a);
                }
                OpCode::And => {
                    let a = self.registers[inst.op1 as usize].as_bool()
                        .ok_or_else(|| NuError::RuntimeError("Expected Bool".into()))?;
                    let b = self.registers[inst.op2 as usize].as_bool()
                        .ok_or_else(|| NuError::RuntimeError("Expected Bool".into()))?;
                    self.registers[inst.op3 as usize] = Value::Bool(a && b);
                }
                OpCode::Or => {
                    let a = self.registers[inst.op1 as usize].as_bool()
                        .ok_or_else(|| NuError::RuntimeError("Expected Bool".into()))?;
                    let b = self.registers[inst.op2 as usize].as_bool()
                        .ok_or_else(|| NuError::RuntimeError("Expected Bool".into()))?;
                    self.registers[inst.op3 as usize] = Value::Bool(a || b);
                }

                // == Control Flow ==
                OpCode::Jmp => {
                    let offset = inst.simm16();
                    if offset >= 0 {
                        self.pc += offset as usize;
                    } else {
                        self.pc = (self.pc as i64 + offset as i64) as usize;
                    }
                    // Don't increment pc at end of loop
                    continue;
                }
                OpCode::JmpT => {
                    let val = self.registers[inst.op1 as usize].as_bool()
                        .ok_or_else(|| NuError::RuntimeError("Expected Bool".into()))?;
                    if val {
                        let offset = inst.offset16();
                        if offset >= 0 {
                            self.pc += offset as usize;
                        } else {
                            self.pc = (self.pc as i64 + offset as i64) as usize;
                        }
                        continue;
                    }
                }
                OpCode::JmpF => {
                    let val = self.registers[inst.op1 as usize].as_bool()
                        .ok_or_else(|| NuError::RuntimeError("Expected Bool".into()))?;
                    if !val {
                        let offset = inst.offset16();
                        if offset >= 0 {
                            self.pc += offset as usize;
                        } else {
                            self.pc = (self.pc as i64 + offset as i64) as usize;
                        }
                        continue;
                    }
                }
                OpCode::Call => {
                    // func_reg contains behavior index, argc in op2, dst in op3
                    let func_val = &self.registers[inst.op1 as usize];
                    let func_name = self.find_function_name(func_val)?;
                    let (mod_idx, beh_idx) = self.function_table.get(&func_name)
                        .copied()
                        .ok_or_else(|| NuError::RuntimeError(format!("Unknown function: {}", func_name)))?;

                    let entry = self.modules[mod_idx].behaviors[beh_idx].code_offset;

                    self.frames.push(CallFrame {
                        module_idx: mod_idx,
                        behavior_idx: beh_idx,
                        pc: self.pc,
                        base_reg: inst.op3 as usize,
                        return_reg: inst.op3,
                    });

                    self.pc = entry;
                    continue;
                }
                OpCode::TailCall => {
                    // Tail call optimization: reuse current frame
                    let func_val = &self.registers[inst.op1 as usize];
                    let func_name = self.find_function_name(func_val)?;
                    let (mod_idx, beh_idx) = self.function_table.get(&func_name)
                        .copied()
                        .ok_or_else(|| NuError::RuntimeError(format!("Unknown function: {}", func_name)))?;

                    let entry = self.modules[mod_idx].behaviors[beh_idx].code_offset;
                    self.pc = entry;
                    continue;
                }
                OpCode::Ret => {
                    let val = self.registers[inst.op1 as usize].clone();
                    if let Some(frame) = self.frames.pop() {
                        self.pc = frame.pc;
                        self.registers[frame.return_reg as usize] = val;
                    } else {
                        // Top-level return
                        self.registers[0] = val;
                        break;
                    }
                }
                OpCode::RetVal => {
                    let val = self.registers[0].clone();
                    if let Some(frame) = self.frames.pop() {
                        self.pc = frame.pc;
                        self.registers[frame.return_reg as usize] = val;
                    } else {
                        self.registers[0] = val;
                        break;
                    }
                }

                // == Tuples ==
                OpCode::TupleMk => {
                    let count = inst.op2 as usize;
                    let elems: Vec<Value> = (0..count)
                        .map(|i| self.registers[(inst.op1 as usize + i) % REG_COUNT].clone())
                        .collect();
                    self.registers[inst.op3 as usize] = Value::Tuple(elems);
                }
                OpCode::TupleL => {
                    let tuple = self.registers[inst.op1 as usize].clone();
                    let idx = inst.op2 as usize;
                    match tuple {
                        Value::Tuple(elems) if idx < elems.len() => {
                            self.registers[inst.op3 as usize] = elems[idx].clone();
                        }
                        _ => return Err(NuError::RuntimeError("Tuple index out of bounds".into())),
                    }
                }

                // == Records ==
                OpCode::RecMk => {
                    let field_count = inst.op2 as usize;
                    // Field values are in consecutive registers starting at op1
                    let mut fields = Vec::new();
                    for i in 0..field_count {
                        let reg = (inst.op1 as usize + i) % REG_COUNT;
                        fields.push((format!("f{}", i), self.registers[reg].clone()));
                    }
                    self.registers[inst.op3 as usize] = Value::Record(fields);
                }
                OpCode::RecL => {
                    let record = self.registers[inst.op1 as usize].clone();
                    let field_idx = inst.imm16() as usize;
                    match record {
                        Value::Record(fields) if field_idx < fields.len() => {
                            self.registers[inst.op3 as usize] = fields[field_idx].1.clone();
                        }
                        _ => return Err(NuError::RuntimeError("Record field not found".into())),
                    }
                }
                OpCode::RecS => {
                    let mut record = self.registers[inst.op1 as usize].clone();
                    let field_idx = inst.imm16() as usize;
                    match &mut record {
                        Value::Record(fields) if field_idx < fields.len() => {
                            fields[field_idx].1 = self.registers[inst.op3 as usize].clone();
                            self.registers[inst.op1 as usize] = record;
                        }
                        _ => return Err(NuError::RuntimeError("Record field not found".into())),
                    }
                }

                // == Arrays ==
                OpCode::ArrAlloc => {
                    let len = self.registers[inst.op1 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int for array length".into()))?;
                    let elems = vec![Value::Unit; len as usize];
                    self.registers[inst.op3 as usize] = Value::Array(elems);
                }
                OpCode::ArrLoad => {
                    let arr = self.registers[inst.op1 as usize].clone();
                    let idx = self.registers[inst.op2 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int for array index".into()))?;
                    match arr {
                        Value::Array(elems) if (idx as usize) < elems.len() => {
                            self.registers[inst.op3 as usize] = elems[idx as usize].clone();
                        }
                        _ => return Err(NuError::RuntimeError("Array index out of bounds".into())),
                    }
                }
                OpCode::ArrStore => {
                    let mut arr = self.registers[inst.op1 as usize].clone();
                    let idx = self.registers[inst.op2 as usize].as_int()
                        .ok_or_else(|| NuError::RuntimeError("Expected Int for array index".into()))?;
                    let val = self.registers[inst.op3 as usize].clone();
                    match &mut arr {
                        Value::Array(elems) if (idx as usize) < elems.len() => {
                            elems[idx as usize] = val;
                            self.registers[inst.op1 as usize] = arr;
                        }
                        _ => return Err(NuError::RuntimeError("Array index out of bounds".into())),
                    }
                }
                OpCode::ArrLen => {
                    let arr = self.registers[inst.op1 as usize].clone();
                    match arr {
                        Value::Array(elems) => {
                            self.registers[inst.op2 as usize] = Value::Int(elems.len() as i64);
                        }
                        _ => return Err(NuError::RuntimeError("Expected Array".into())),
                    }
                }

                // == String ==
                OpCode::SConcat => {
                    let a = self.registers[inst.op1 as usize].as_string()
                        .ok_or_else(|| NuError::RuntimeError("Expected String".into()))?.to_string();
                    let b = self.registers[inst.op2 as usize].as_string()
                        .ok_or_else(|| NuError::RuntimeError("Expected String".into()))?.to_string();
                    self.registers[inst.op3 as usize] = Value::String(a + &b);
                }
                OpCode::SPrint => {
                    let val = self.registers[inst.op1 as usize].clone();
                    let s = match &val {
                        Value::String(s) => s.clone(),
                        other => format!("{:?}", other),
                    };
                    self.output.push(s.clone());
                    println!("{}", s);
                }
                OpCode::Print => {
                    let val = self.registers[inst.op1 as usize].clone();
                    let s = format!("{:?}", val);
                    self.output.push(s.clone());
                    println!("{}", s);
                }

                // == Actor operations (placeholders) ==
                OpCode::Spawn => {
                    // Placeholder: return a dummy actor address
                    self.registers[inst.op3 as usize] = Value::IntAddr(inst.op2 as u64);
                }
                OpCode::Send => {
                    // Placeholder: async send does not return a value
                }
                OpCode::Ask => {
                    // Placeholder: return Unit
                    self.registers[inst.op3 as usize] = Value::Unit;
                }
                OpCode::Receive => {
                    // Placeholder: return Unit
                    self.registers[inst.op3 as usize] = Value::Unit;
                }
                OpCode::SelfOp => {
                    // Placeholder: return current actor address
                    self.registers[inst.op1 as usize] = Value::IntAddr(0);
                }
                OpCode::Monitor | OpCode::Demon | OpCode::Link | OpCode::Unlink | OpCode::Exit => {
                    // Placeholder: no-op for MVP
                }

                // == Effect operations (placeholders) ==
                OpCode::Perform | OpCode::Handle | OpCode::Resume | OpCode::Unwind => {
                    // Placeholder: effects not yet implemented in VM
                    self.registers[inst.op3 as usize] = Value::Unit;
                }

                // == Capability operations (placeholders) ==
                OpCode::CapChk | OpCode::CapUp | OpCode::CapDown | OpCode::CapSend => {
                    // Placeholder: capability checks not yet in VM
                }

                // == Distribution (placeholders) ==
                OpCode::NodeId | OpCode::Migrate | OpCode::RSend | OpCode::RAsk | OpCode::RSpawn | OpCode::Gossip => {
                    // Placeholder: distribution not yet implemented
                    self.registers[inst.op3 as usize] = Value::Unit;
                }

                // == Debug ==
                OpCode::DbgPrint => {
                    println!("[VM Debug] Registers: {:?}", &self.registers[0..16]);
                }
                OpCode::DbgStack => {
                    println!("[VM Debug] Call stack depth: {}", self.frames.len());
                }

                // == Meta ==
                OpCode::MetaType => {
                    let val = self.registers[inst.op1 as usize].clone();
                    let type_str = val.type_of().display();
                    self.registers[inst.op2 as usize] = Value::String(type_str);
                }

                // == Fallback ==
                _ => {
                    // For any unimplemented opcode, just skip
                }
            }

            self.pc += 1;
            executed += 1;
        }

        if executed >= max_instructions {
            return Err(NuError::RuntimeError("Max instructions exceeded (possible infinite loop)".into()));
        }

        Ok(self.registers[0].clone())
    }

    fn load_constant(&self, idx: usize) -> NuResult<Value> {
        let module_idx = self.modules.len().saturating_sub(1);
        let module = &self.modules[module_idx];
        module.constants.get(idx)
            .cloned()
            .map(|c| match c {
                Constant::Int(n) => Value::Int(n),
                Constant::Float(f) => Value::Float(f64::to_bits(f)),
                Constant::Bool(b) => Value::Bool(b),
                Constant::String(s) => Value::String(s),
                Constant::Unit => Value::Unit,
                _ => Value::Unit,
            })
            .ok_or_else(|| NuError::RuntimeError(format!("Invalid constant index: {}", idx)))
    }

    fn find_function_name(&self, val: &Value) -> NuResult<String> {
        match val {
            Value::String(s) => Ok(s.clone()),
            _ => Ok("__main".to_string()),
        }
    }

    pub fn output(&self) -> &[String] {
        &self.output
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;
    use crate::bytecode::{BehaviorTableEntry, CodeModule, Constant, Instruction, OpCode};
    use crate::compiler::Compiler;
    use crate::parser;
    use crate::types::Span;

    fn s() -> Span { Span::default() }

    #[test]
    fn test_vm_arithmetic() {
        // Build a simple module that does: 1 + 2 * 3 = 7
        let mut module = CodeModule::new("test");

        // Emit bytecode: r1 = 1, r2 = 2, r3 = 3, r2 = r2 * r3, r1 = r1 + r2
        module.emit(Instruction::new2(OpCode::ConstU, 1, 0)); // const 1 -> r1
        module.emit(Instruction::new2(OpCode::ConstU, 2, 0)); // const 2 -> r2
        module.emit(Instruction::new2(OpCode::ConstU, 3, 0)); // const 3 -> r3
        module.emit(Instruction::new3(OpCode::IMul, 2, 3, 2)); // r2 = r2 * r3
        module.emit(Instruction::new3(OpCode::IAdd, 1, 2, 1)); // r1 = r1 + r2
        module.emit(Instruction::new1(OpCode::Ret, 1));

        // Add constants
        module.add_constant(Constant::Int(1));
        module.add_constant(Constant::Int(2));
        module.add_constant(Constant::Int(3));

        // Add behavior entry
        module.add_behavior(BehaviorTableEntry {
            name: "__main".into(),
            param_count: 0,
            code_offset: 0,
            local_count: 4,
            effect_mask: 0,
        });

        let mut vm = VM::new();
        vm.load_module(&module).unwrap();

        // Set up constants
        vm.registers[1] = Value::Int(1);
        vm.registers[2] = Value::Int(2);
        vm.registers[3] = Value::Int(3);

        let result = vm.run().unwrap();
        assert_eq!(result, Value::Int(7));
    }

    #[test]
    fn test_vm_comparison() {
        let mut vm = VM::new();

        // Test: 1 < 2 should be true
        vm.registers[1] = Value::Int(1);
        vm.registers[2] = Value::Int(2);

        let mut module = CodeModule::new("test");
        module.emit(Instruction::new3(OpCode::ICmpLt, 1, 2, 0));
        module.emit(Instruction::new1(OpCode::Ret, 0));
        module.add_behavior(BehaviorTableEntry {
            name: "__main".into(),
            param_count: 0,
            code_offset: 0,
            local_count: 3,
            effect_mask: 0,
        });

        vm.load_module(&module).unwrap();
        let result = vm.run().unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_vm_function_call() {
        // Build module: add(x, y) = x + y; main = add(3, 4)
        let mut module = CodeModule::new("test");

        // add behavior at offset 0
        module.emit(Instruction::new3(OpCode::IAdd, 1, 2, 0));
        module.emit(Instruction::new1(OpCode::Ret, 0));

        // main behavior at offset 2
        let main_offset = module.current_offset();
        module.emit(Instruction::new2(OpCode::ConstU, 1, 0)); // const 3 -> r1
        module.emit(Instruction::new2(OpCode::ConstU, 2, 0)); // const 4 -> r2
        module.emit(Instruction::new3(OpCode::Call, 1, 2, 0)); // call add(r1, r2) -> r0
        module.emit(Instruction::new1(OpCode::Ret, 0));

        module.add_constant(Constant::Int(3));
        module.add_constant(Constant::Int(4));

        module.add_behavior(BehaviorTableEntry {
            name: "add".into(),
            param_count: 2,
            code_offset: 0,
            local_count: 3,
            effect_mask: 0,
        });
        module.add_behavior(BehaviorTableEntry {
            name: "__main".into(),
            param_count: 0,
            code_offset: main_offset,
            local_count: 3,
            effect_mask: 0,
        });

        let mut vm = VM::new();
        vm.load_module(&module).unwrap();

        // Set up args
        vm.registers[1] = Value::Int(3);
        vm.registers[2] = Value::Int(4);

        let result = vm.run().unwrap();
        assert_eq!(result, Value::Int(7));
    }

    #[test]
    fn test_vm_nested_function() {
        let input = r#"
fun outer(x) = x + 1
fun main() = outer(5)
"#;
        let ast = parser::parse(input).unwrap();
        let mut compiler = Compiler::new("test");
        let module_ref = compiler.compile_module(&ast).unwrap();
        let code_module = module_ref.clone();

        let mut vm = VM::new();
        vm.load_module(&code_module).unwrap();
        let result = vm.run().unwrap();
        assert_eq!(result, Value::Int(6));
    }

    #[test]
    fn test_vm_conditionals() {
        let input = r#"
fun max(x, y) = if x > y then x else y
fun main() = max(3, 7)
"#;
        let ast = parser::parse(input).unwrap();
        let mut compiler = Compiler::new("test");
        let module_ref = compiler.compile_module(&ast).unwrap();
        let code_module = module_ref.clone();

        let mut vm = VM::new();
        vm.load_module(&code_module).unwrap();
        let result = vm.run().unwrap();
        assert_eq!(result, Value::Int(7));
    }
}
