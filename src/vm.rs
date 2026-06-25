//! Register-based Virtual Machine for Nulang.
//!
//! - 256 virtual registers per activation frame
//! - 32-bit fixed-width instructions
//! - Direct-threaded dispatch (token threading via computed goto pattern)
//! - NaN-tagged 64-bit values

use crate::bytecode::*;
use crate::runtime::*;
use crate::types::NuResult;
use crate::types::NuError;

// ---------------------------------------------------------------------------
// Value Representation (NaN Tagging)
// ---------------------------------------------------------------------------

/// NaN-tagged 64-bit value.
/// - Positive/negative integers: use NaN payload with tag bits
/// - Floats: regular IEEE 754, NaN payload used for tagging
/// - Heap pointers: NaN payload contains pointer
/// - Special values (unit, true, false): dedicated NaN payloads
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Value(pub u64);

const TAG_MASK: u64 = 0xFFFF000000000000;
const TAG_INT: u64 = 0x7FF9000000000000;    // quiet NaN, int tag
const TAG_PTR: u64 = 0x7FFA000000000000;    // heap pointer
const TAG_ACTOR: u64 = 0x7FFB000000000000;  // actor reference
const TAG_SPECIAL: u64 = 0x7FFC000000000000; // true, false, unit, nil
const TAG_STRING: u64 = 0x7FFD000000000000; // interned string

const SPECIAL_UNIT: u64 = 0;
const SPECIAL_TRUE: u64 = 1;
const SPECIAL_FALSE: u64 = 2;
const SPECIAL_NIL: u64 = 3;

impl Value {
    pub fn int(n: i64) -> Value {
        let bits = (n as u64) & 0x0000FFFFFFFFFFFF;
        Value(TAG_INT | bits)
    }
    pub fn float(f: f64) -> Value {
        Value(f.to_bits())
    }
    pub fn bool(b: bool) -> Value {
        let s = if b { SPECIAL_TRUE } else { SPECIAL_FALSE };
        Value(TAG_SPECIAL | s)
    }
    pub fn unit() -> Value {
        Value(TAG_SPECIAL | SPECIAL_UNIT)
    }
    pub fn nil() -> Value {
        Value(TAG_SPECIAL | SPECIAL_NIL)
    }
    pub fn ptr(addr: *mut u8) -> Value {
        let bits = (addr as u64) & 0x0000FFFFFFFFFFFF;
        Value(TAG_PTR | bits)
    }
    pub fn actor_ref(id: u64) -> Value {
        Value(TAG_ACTOR | (id & 0x0000FFFFFFFFFFFF))
    }
    pub fn string(id: u32) -> Value {
        Value(TAG_STRING | (id as u64))
    }

    pub fn as_int(&self) -> Option<i64> {
        if self.0 & TAG_MASK == TAG_INT {
            let bits = self.0 & 0x0000FFFFFFFFFFFF;
            // Sign extend from 48 bits
            Some(if bits & 0x0000800000000000 != 0 {
                (bits | 0xFFFF000000000000) as i64
            } else {
                bits as i64
            })
        } else {
            None
        }
    }
    pub fn as_float(&self) -> Option<f64> {
        let f = f64::from_bits(self.0);
        if f.is_nan() { None } else { Some(f) }
    }
    pub fn as_bool(&self) -> Option<bool> {
        if self.0 & TAG_MASK == TAG_SPECIAL {
            let s = self.0 & 0xFFFF;
            match s {
                SPECIAL_TRUE => Some(true),
                SPECIAL_FALSE => Some(false),
                _ => None,
            }
        } else {
            None
        }
    }
    pub fn is_unit(&self) -> bool {
        self.0 == (TAG_SPECIAL | SPECIAL_UNIT)
    }
    pub fn is_nil(&self) -> bool {
        self.0 == (TAG_SPECIAL | SPECIAL_NIL)
    }
    pub fn as_ptr<T>(&self) -> Option<*mut T> {
        if self.0 & TAG_MASK == TAG_PTR {
            Some((self.0 & 0x0000FFFFFFFFFFFF) as *mut T)
        } else {
            None
        }
    }
    pub fn as_actor_id(&self) -> Option<u64> {
        if self.0 & TAG_MASK == TAG_ACTOR {
            Some(self.0 & 0x0000FFFFFFFFFFFF)
        } else {
            None
        }
    }
    pub fn is_truthy(&self) -> bool {
        !self.is_nil() && self.as_bool() != Some(false) && self.as_int() != Some(0)
    }
    pub fn to_string_repr(&self) -> String {
        if let Some(n) = self.as_int() { return format!("{}", n); }
        if let Some(f) = self.as_float() { return format!("{}", f); }
        if let Some(b) = self.as_bool() { return format!("{}", b); }
        if self.is_unit() { return "unit".to_string(); }
        if self.is_nil() { return "nil".to_string(); }
        if let Some(id) = self.as_actor_id() { return format!("<actor:{}>", id); }
        format!("<value:0x{:016X}>", self.0)
    }
}

// ---------------------------------------------------------------------------
// Call Frame
// ---------------------------------------------------------------------------

/// Activation frame: 256 registers + metadata.
pub struct Frame {
    pub regs: [Value; 256],
    pub pc: usize,              // Program counter
    pub closure: Option<Value>, // Closure value (for captures, or return dst)
    pub caller: Option<Box<Frame>>, // Linked list of frames
    pub module_idx: usize,      // Which module this frame is executing
}

impl Frame {
    pub fn new(caller: Option<Box<Frame>>, module_idx: usize) -> Self {
        Frame {
            regs: [Value::nil(); 256],
            pc: 0,
            closure: None,
            caller,
            module_idx,
        }
    }
}

// ---------------------------------------------------------------------------
// VM
// ---------------------------------------------------------------------------

pub struct VM {
    modules: Vec<CodeModule>,
    current_frame: Option<Box<Frame>>,
    running: bool,
    pub runtime: Runtime,
    step_count: usize,
}

impl VM {
    pub fn new() -> Self {
        VM {
            modules: Vec::new(),
            current_frame: None,
            running: false,
            runtime: Runtime::new(),
            step_count: 0,
        }
    }

    pub fn load_module(&mut self, module: CodeModule) -> usize {
        let idx = self.modules.len();
        self.modules.push(module);
        idx
    }

    /// Execute the most recently loaded module.
    /// Uses entry_point if __main was compiled inline, otherwise starts at PC=0.
    pub fn run(&mut self) -> NuResult<Value> {
        if self.modules.is_empty() {
            return Err(NuError::VMError("No modules loaded".to_string()));
        }
        let module_idx = self.modules.len() - 1;
        let start_pc = self.modules[module_idx].entry_point.unwrap_or(0);
        let mut frame = Frame::new(None, module_idx);
        frame.pc = start_pc;
        self.current_frame = Some(Box::new(frame));
        self.running = true;
        while self.running {
            if let Err(e) = self.step() {
                self.running = false;
                return Err(e);
            }
        }
        // Return value from register 0
        Ok(self.current_frame.as_ref().map(|f| f.regs[0]).unwrap_or(Value::unit()))
    }

    /// Call a specific function by module + function index.
    pub fn call_function(&mut self, module_idx: usize, func_idx: usize, args: &[Value]) -> NuResult<Value> {
        let code_offset = self.modules.get(module_idx)
            .and_then(|m| m.function_table.get(func_idx)).copied()
            .ok_or_else(|| NuError::VMError(format!("Function {} not found in module {}", func_idx, module_idx)))?;

        let mut frame = Frame::new(None, module_idx);
        frame.pc = code_offset;
        for (i, arg) in args.iter().enumerate().take(256) {
            frame.regs[i] = *arg;
        }

        // If there's a current frame, link it as caller
        if let Some(old_frame) = self.current_frame.take() {
            frame.caller = Some(old_frame);
        }

        self.current_frame = Some(Box::new(frame));
        self.running = true;

        while self.running {
            if let Err(e) = self.step() {
                self.running = false;
                return Err(e);
            }
        }

        // Return value is in register 0 of the current (possibly dummy) frame
        Ok(self.current_frame.as_ref().map(|f| f.regs[0]).unwrap_or(Value::unit()))
    }

    /// Single-step execute one instruction.
    pub fn step(&mut self) -> NuResult<()> {
        // Safety limit to prevent infinite loops
        self.step_count += 1;
        if self.step_count > 100000 {
            return Err(NuError::VMError(format!("Step limit exceeded at step {}, possible infinite loop", self.step_count)));
        }
        // Debug: print first 50 steps
        let debug = self.step_count <= 50;
        // Fetch instruction
        let instr = {
            let frame = self.current_frame.as_ref()
                .ok_or_else(|| NuError::VMError("No current frame".to_string()))?;
            let module = self.modules.get(frame.module_idx)
                .ok_or_else(|| NuError::VMError(format!("Module {} not found", frame.module_idx)))?;
            let pc = frame.pc;
            *module.instructions.get(pc)
                .ok_or_else(|| NuError::VMError(format!("PC {} out of bounds in module {}", pc, frame.module_idx)))?
        };

        // Take ownership of the current frame to eliminate borrow-checker issues.
        // Call/Ret/TailCall/ClosureCall consume the frame; all other opcodes put it back.
        let mut frame = self.current_frame.take()
            .ok_or_else(|| NuError::VMError("No current frame".to_string()))?;

        // Increment PC before execution
        frame.pc += 1;

        let _ = debug; // silence unused warning when debug is off

        match instr.opcode {
            // -- Frame-manipulating opcodes (consume frame) --
            OpCode::Call => {
                let func_val = frame.regs[instr.op1 as usize];
                let module_idx = frame.module_idx;
                let argc = instr.op2;
                let dst = instr.op3;
                // Save old frame as caller in a new frame
                let func_idx = func_val.as_int()
                    .ok_or_else(|| NuError::VMError("Invalid function reference".to_string()))? as usize;
                let code_offset = self.modules.get(module_idx)
                    .and_then(|m| m.function_table.get(func_idx)).copied()
                    .ok_or_else(|| NuError::VMError(format!("Function {} not found", func_idx)))?;
                let mut new_frame = Frame::new(None, module_idx);
                new_frame.pc = code_offset;
                for i in 0..(argc as usize).min(256) {
                    new_frame.regs[i] = frame.regs[i];
                }
                new_frame.closure = Some(Value::int(dst as i64));
                new_frame.caller = Some(frame);
                self.current_frame = Some(Box::new(new_frame));
                return Ok(());
            }
            OpCode::TailCall => {
                let func_val = frame.regs[instr.op1 as usize];
                let module_idx = frame.module_idx;
                let func_idx = func_val.as_int()
                    .ok_or_else(|| NuError::VMError("Invalid function reference".to_string()))? as usize;
                let code_offset = self.modules.get(module_idx)
                    .and_then(|m| m.function_table.get(func_idx)).copied()
                    .ok_or_else(|| NuError::VMError(format!("Function {} not found", func_idx)))?;
                frame.pc = code_offset;
                self.current_frame = Some(frame);
                return Ok(());
            }
            OpCode::Ret => {
                let ret_val = frame.regs[0];
                if let Some(mut caller_frame) = frame.caller {
                    let dst = frame.closure.as_ref()
                        .and_then(|v| v.as_int()).unwrap_or(0) as usize;
                    caller_frame.regs[dst] = ret_val;
                    self.current_frame = Some(caller_frame);
                } else {
                    let module_idx = frame.module_idx;
                    let mut dummy = Frame::new(None, module_idx);
                    dummy.regs[0] = ret_val;
                    dummy.pc = usize::MAX;
                    self.current_frame = Some(Box::new(dummy));
                    self.running = false;
                }
                return Ok(());
            }
            OpCode::RetVal => {
                let val_reg = instr.op1 as usize;
                let ret_val = frame.regs[val_reg];
                if let Some(mut caller_frame) = frame.caller {
                    let dst = frame.closure.as_ref()
                        .and_then(|v| v.as_int()).unwrap_or(0) as usize;
                    caller_frame.regs[dst] = ret_val;
                    self.current_frame = Some(caller_frame);
                } else {
                    let module_idx = frame.module_idx;
                    let mut dummy = Frame::new(None, module_idx);
                    dummy.regs[0] = ret_val;
                    dummy.pc = usize::MAX;
                    self.current_frame = Some(Box::new(dummy));
                    self.running = false;
                }
                return Ok(());
            }
            OpCode::ClosureCall => {
                // MVP: treat same as regular call
                let func_val = frame.regs[instr.op1 as usize];
                let module_idx = frame.module_idx;
                let argc = instr.op2;
                let dst = instr.op3;
                let func_idx = func_val.as_int()
                    .ok_or_else(|| NuError::VMError("Invalid function reference".to_string()))? as usize;
                let code_offset = self.modules.get(module_idx)
                    .and_then(|m| m.function_table.get(func_idx)).copied()
                    .ok_or_else(|| NuError::VMError(format!("Function {} not found", func_idx)))?;
                let mut new_frame = Frame::new(None, module_idx);
                new_frame.pc = code_offset;
                for i in 0..(argc as usize).min(256) {
                    new_frame.regs[i] = frame.regs[i];
                }
                new_frame.closure = Some(Value::int(dst as i64));
                new_frame.caller = Some(frame);
                self.current_frame = Some(Box::new(new_frame));
                return Ok(());
            }
            OpCode::Panic => {
                let msg_idx = instr.imm16();
                let msg = self.module_const_string(frame.module_idx, msg_idx);
                // Put frame back before returning error
                self.current_frame = Some(frame);
                return Err(NuError::VMError(format!("Panic: {}", msg)));
            }

            // -- Actor opcodes that need runtime access (consume frame, put back) --
            OpCode::Spawn => {
                let behavior_idx = instr.imm16();
                let _init_reg = instr.op2;
                let dst = instr.op3;
                let actor_id = fresh_actor_id();
                frame.regs[dst as usize] = Value::actor_ref(actor_id);
                self.current_frame = Some(frame);
                return Ok(());
            }
            OpCode::Send => {
                let addr_reg = instr.op1;
                let behavior_id = instr.imm16();
                if let Some(actor_id) = frame.regs[addr_reg as usize].as_actor_id() {
                    if let Some(actor) = self.runtime.actors.get_mut(&actor_id) {
                        let msg = Message {
                            behavior_id,
                            payload: vec![frame.regs[0]],
                            sender: self.runtime.current_actor.unwrap_or(0),
                            priority: MessagePriority::Normal,
                        };
                        let _ = actor.mailbox.push(msg);
                    }
                }
                self.current_frame = Some(frame);
                return Ok(());
            }
            OpCode::Ask => {
                let addr_reg = instr.op1;
                let behavior_id = instr.imm16();
                if let Some(actor_id) = frame.regs[addr_reg as usize].as_actor_id() {
                    if let Some(actor) = self.runtime.actors.get_mut(&actor_id) {
                        let msg = Message {
                            behavior_id,
                            payload: vec![frame.regs[0]],
                            sender: self.runtime.current_actor.unwrap_or(0),
                            priority: MessagePriority::Normal,
                        };
                        let _ = actor.mailbox.push(msg);
                    }
                }
                self.current_frame = Some(frame);
                return Ok(());
            }
            OpCode::RSend => {
                let addr_reg = instr.op1;
                let behavior_id = instr.imm16();
                if let Some(actor_id) = frame.regs[addr_reg as usize].as_actor_id() {
                    if let Some(actor) = self.runtime.actors.get_mut(&actor_id) {
                        let msg = Message {
                            behavior_id,
                            payload: vec![frame.regs[0]],
                            sender: self.runtime.current_actor.unwrap_or(0),
                            priority: MessagePriority::Normal,
                        };
                        let _ = actor.mailbox.push(msg);
                    }
                }
                self.current_frame = Some(frame);
                return Ok(());
            }
            OpCode::RSpawn => {
                let _init_reg = instr.op2;
                let dst = instr.op3;
                let actor_id = fresh_actor_id();
                frame.regs[dst as usize] = Value::actor_ref(actor_id);
                self.current_frame = Some(frame);
                return Ok(());
            }

            // -- All other opcodes (operate on owned frame, then put back) --
            _ => {
                self.dispatch_regular(frame, instr);
                // Frame is put back inside dispatch_regular
            }
        }

        Ok(())
    }

    fn dispatch_regular(&mut self, mut frame: Box<Frame>, instr: Instruction) {
        match instr.opcode {
            // -- Special --
            OpCode::Nop => {}
            OpCode::Halt => self.running = false,
            OpCode::Const0 => frame.regs[instr.op1 as usize] = Value::int(0),
            OpCode::Const1 => frame.regs[instr.op1 as usize] = Value::int(1),
            OpCode::Const2 => frame.regs[instr.op1 as usize] = Value::int(2),
            OpCode::ConstM1 => frame.regs[instr.op1 as usize] = Value::int(-1),
            OpCode::ConstU => {
                let idx = instr.imm16();
                let dst = instr.op3;
                if let Some(module) = self.modules.get(frame.module_idx) {
                    if let Some(c) = module.constants.get(idx as usize) {
                        frame.regs[dst as usize] = constant_to_value(c);
                    }
                }
            }
            OpCode::ConstL => {
                let idx = ((instr.op1 as u32) << 16) | ((instr.op2 as u32) << 8) | (instr.op3 as u32);
                let dst = 0;
                if let Some(module) = self.modules.get(frame.module_idx) {
                    if let Some(c) = module.constants.get(idx as usize) {
                        frame.regs[dst as usize] = constant_to_value(c);
                    }
                }
            }

            // -- Stack & Locals --
            OpCode::Load | OpCode::Store | OpCode::Move | OpCode::Dup => {
                frame.regs[instr.op2 as usize] = frame.regs[instr.op1 as usize];
            }
            OpCode::Pop => {}
            OpCode::Swap => {
                let r1 = instr.op1 as usize;
                let r2 = instr.op2 as usize;
                let tmp = frame.regs[r1];
                frame.regs[r1] = frame.regs[r2];
                frame.regs[r2] = tmp;
            }

            // -- Integer Arithmetic --
            OpCode::IAdd => { let (r1,r2,dst) = (instr.op1,instr.op2,instr.op3);
                if let (Some(a),Some(b)) = (frame.regs[r1 as usize].as_int(), frame.regs[r2 as usize].as_int()) {
                    frame.regs[dst as usize] = Value::int(a + b);
                }
            }
            OpCode::ISub => { let (r1,r2,dst) = (instr.op1,instr.op2,instr.op3);
                if let (Some(a),Some(b)) = (frame.regs[r1 as usize].as_int(), frame.regs[r2 as usize].as_int()) {
                    frame.regs[dst as usize] = Value::int(a - b);
                }
            }
            OpCode::IMul => { let (r1,r2,dst) = (instr.op1,instr.op2,instr.op3);
                if let (Some(a),Some(b)) = (frame.regs[r1 as usize].as_int(), frame.regs[r2 as usize].as_int()) {
                    frame.regs[dst as usize] = Value::int(a * b);
                }
            }
            OpCode::IDiv => { let (r1,r2,dst) = (instr.op1,instr.op2,instr.op3);
                if let (Some(a),Some(b)) = (frame.regs[r1 as usize].as_int(), frame.regs[r2 as usize].as_int()) {
                    if b != 0 { frame.regs[dst as usize] = Value::int(a / b); }
                }
            }
            OpCode::IMod => {
                if let (Some(a), Some(b)) = (frame.regs[instr.op1 as usize].as_int(), frame.regs[instr.op2 as usize].as_int()) {
                    if b != 0 { frame.regs[instr.op3 as usize] = Value::int(a % b); }
                }
            }
            OpCode::INeg => {
                if let Some(a) = frame.regs[instr.op1 as usize].as_int() {
                    frame.regs[instr.op2 as usize] = Value::int(-a);
                }
            }
            OpCode::IInc => {
                if let Some(a) = frame.regs[instr.op1 as usize].as_int() {
                    frame.regs[instr.op1 as usize] = Value::int(a + 1);
                }
            }
            OpCode::IDec => {
                if let Some(a) = frame.regs[instr.op1 as usize].as_int() {
                    frame.regs[instr.op1 as usize] = Value::int(a - 1);
                }
            }
            OpCode::IPow => {
                if let (Some(a), Some(b)) = (frame.regs[instr.op1 as usize].as_int(), frame.regs[instr.op2 as usize].as_int()) {
                    let exp = if b < 0 { 0 } else { b as u32 };
                    frame.regs[instr.op3 as usize] = Value::int(a.pow(exp));
                }
            }

            // -- Float Arithmetic --
            OpCode::FAdd => { let (r1,r2,dst) = (instr.op1,instr.op2,instr.op3);
                if let (Some(a),Some(b)) = (frame.regs[r1 as usize].as_float(), frame.regs[r2 as usize].as_float()) {
                    frame.regs[dst as usize] = Value::float(a + b);
                }
            }
            OpCode::FSub => { let (r1,r2,dst) = (instr.op1,instr.op2,instr.op3);
                if let (Some(a),Some(b)) = (frame.regs[r1 as usize].as_float(), frame.regs[r2 as usize].as_float()) {
                    frame.regs[dst as usize] = Value::float(a - b);
                }
            }
            OpCode::FMul => { let (r1,r2,dst) = (instr.op1,instr.op2,instr.op3);
                if let (Some(a),Some(b)) = (frame.regs[r1 as usize].as_float(), frame.regs[r2 as usize].as_float()) {
                    frame.regs[dst as usize] = Value::float(a * b);
                }
            }
            OpCode::FDiv => { let (r1,r2,dst) = (instr.op1,instr.op2,instr.op3);
                if let (Some(a),Some(b)) = (frame.regs[r1 as usize].as_float(), frame.regs[r2 as usize].as_float()) {
                    frame.regs[dst as usize] = Value::float(a / b);
                }
            }
            OpCode::FNeg => {
                if let Some(a) = frame.regs[instr.op1 as usize].as_float() {
                    frame.regs[instr.op2 as usize] = Value::float(-a);
                }
            }
            OpCode::FMod => {
                if let (Some(a), Some(b)) = (frame.regs[instr.op1 as usize].as_float(), frame.regs[instr.op2 as usize].as_float()) {
                    frame.regs[instr.op3 as usize] = Value::float(a % b);
                }
            }
            OpCode::IToF => {
                if let Some(a) = frame.regs[instr.op1 as usize].as_int() {
                    frame.regs[instr.op2 as usize] = Value::float(a as f64);
                }
            }
            OpCode::FToI => {
                if let Some(a) = frame.regs[instr.op1 as usize].as_float() {
                    frame.regs[instr.op2 as usize] = Value::int(a as i64);
                }
            }
            OpCode::FToS => {
                let s = frame.regs[instr.op1 as usize].to_string_repr();
                frame.regs[instr.op2 as usize] = Value::ptr(s.as_bytes().as_ptr() as *mut u8);
            }

            // -- Comparison & Logic --
            OpCode::ICmpEq => { let (r1,r2,dst) = (instr.op1,instr.op2,instr.op3);
                let result = match (frame.regs[r1 as usize].as_int(), frame.regs[r2 as usize].as_int()) {
                    (Some(a), Some(b)) => Value::bool(a == b), _ => Value::bool(false),
                };
                frame.regs[dst as usize] = result;
            }
            OpCode::ICmpLt => { let (r1,r2,dst) = (instr.op1,instr.op2,instr.op3);
                if let (Some(a),Some(b)) = (frame.regs[r1 as usize].as_int(), frame.regs[r2 as usize].as_int()) {
                    frame.regs[dst as usize] = Value::bool(a < b);
                }
            }
            OpCode::ICmpGt => {
                if let (Some(a), Some(b)) = (frame.regs[instr.op1 as usize].as_int(), frame.regs[instr.op2 as usize].as_int()) {
                    frame.regs[instr.op3 as usize] = Value::bool(a > b);
                }
            }
            OpCode::ICmpLe => {
                if let (Some(a), Some(b)) = (frame.regs[instr.op1 as usize].as_int(), frame.regs[instr.op2 as usize].as_int()) {
                    frame.regs[instr.op3 as usize] = Value::bool(a <= b);
                }
            }
            OpCode::ICmpGe => {
                if let (Some(a), Some(b)) = (frame.regs[instr.op1 as usize].as_int(), frame.regs[instr.op2 as usize].as_int()) {
                    frame.regs[instr.op3 as usize] = Value::bool(a >= b);
                }
            }
            OpCode::FCmpEq => {
                if let (Some(a), Some(b)) = (frame.regs[instr.op1 as usize].as_float(), frame.regs[instr.op2 as usize].as_float()) {
                    frame.regs[instr.op3 as usize] = Value::bool((a - b).abs() < f64::EPSILON);
                }
            }
            OpCode::FCmpLt => {
                if let (Some(a), Some(b)) = (frame.regs[instr.op1 as usize].as_float(), frame.regs[instr.op2 as usize].as_float()) {
                    frame.regs[instr.op3 as usize] = Value::bool(a < b);
                }
            }
            OpCode::FCmpGt => {
                if let (Some(a), Some(b)) = (frame.regs[instr.op1 as usize].as_float(), frame.regs[instr.op2 as usize].as_float()) {
                    frame.regs[instr.op3 as usize] = Value::bool(a > b);
                }
            }
            OpCode::SCmpEq => {
                frame.regs[instr.op3 as usize] = Value::bool(frame.regs[instr.op1 as usize].0 == frame.regs[instr.op2 as usize].0);
            }
            OpCode::Not => {
                if let Some(b) = frame.regs[instr.op1 as usize].as_bool() {
                    frame.regs[instr.op2 as usize] = Value::bool(!b);
                } else {
                    frame.regs[instr.op2 as usize] = Value::bool(!frame.regs[instr.op1 as usize].is_truthy());
                }
            }
            OpCode::And => {
                let a = frame.regs[instr.op1 as usize].is_truthy();
                let b = frame.regs[instr.op2 as usize].is_truthy();
                frame.regs[instr.op3 as usize] = Value::bool(a && b);
            }
            OpCode::Or => {
                let a = frame.regs[instr.op1 as usize].is_truthy();
                let b = frame.regs[instr.op2 as usize].is_truthy();
                frame.regs[instr.op3 as usize] = Value::bool(a || b);
            }

            // -- Control Flow --
            OpCode::Jmp => { let off = instr.simm16(); frame.pc = (frame.pc as i64 + off as i64 - 1) as usize; }
            OpCode::JmpT => { if frame.regs[instr.op1 as usize].is_truthy() { let off = instr.offset16(); frame.pc = (frame.pc as i64 + off as i64 - 1) as usize; } }
            OpCode::JmpF => { if !frame.regs[instr.op1 as usize].is_truthy() { let off = instr.offset16(); frame.pc = (frame.pc as i64 + off as i64 - 1) as usize; } }
            OpCode::Switch => {
                let val = frame.regs[instr.op1 as usize];
                let table_idx = instr.imm16();
                if let Some(case) = val.as_int() {
                    if let Some(module) = self.modules.get(frame.module_idx) {
                        if let Some(Constant::Int(offset)) = module.constants.get(table_idx as usize + case as usize) {
                            frame.pc = (frame.pc as i64 + *offset as i64 - 1) as usize;
                        }
                    }
                }
            }

            // -- Closures (MVP) --
            OpCode::Closure => {
                let func_idx = instr.imm16();
                let dst = instr.op3;
                frame.regs[dst as usize] = Value::int(func_idx as i64);
            }
            OpCode::CapLoad => {}
            OpCode::CapStore => {}
            OpCode::FreeVar => {}

            // -- Memory & Objects --
            OpCode::Alloc => {
                let size = frame.regs[instr.op1 as usize].as_int().unwrap_or(0) as usize;
                let dst = instr.op3;
                if size > 0 && size <= 256 {
                    let layout = std::alloc::Layout::from_size_align(size * std::mem::size_of::<Value>(), 8).unwrap();
                    let ptr = unsafe { std::alloc::alloc(layout) };
                    if !ptr.is_null() { frame.regs[dst as usize] = Value::ptr(ptr); }
                }
            }
            OpCode::FieldL => {
                let (obj, fld, dst) = (instr.op1 as usize, instr.op2 as usize, instr.op3 as usize);
                if let Some(ptr) = frame.regs[obj].as_ptr::<Value>() { unsafe { frame.regs[dst] = *ptr.add(fld); } }
            }
            OpCode::FieldS => {
                let (obj, fld, src) = (instr.op1 as usize, instr.op2 as usize, instr.op3 as usize);
                if let Some(ptr) = frame.regs[obj].as_ptr::<Value>() { unsafe { *ptr.add(fld) = frame.regs[src]; } }
            }
            OpCode::ArrAlloc => {
                let len = frame.regs[instr.op1 as usize].as_int().unwrap_or(0) as usize;
                let dst = instr.op3 as usize;
                if len > 0 {
                    let layout = std::alloc::Layout::from_size_align((len + 1) * std::mem::size_of::<Value>(), 8).unwrap();
                    let ptr = unsafe { std::alloc::alloc(layout) } as *mut Value;
                    if !ptr.is_null() {
                        unsafe { *ptr = Value::int(len as i64); for i in 0..len { *ptr.add(i + 1) = Value::nil(); } }
                        frame.regs[dst] = Value::ptr(ptr as *mut u8);
                    }
                } else { frame.regs[dst] = Value::nil(); }
            }
            OpCode::ArrLoad => {
                let (arr, idx, dst) = (instr.op1 as usize, instr.op2 as usize, instr.op3 as usize);
                if let Some(ptr) = frame.regs[arr].as_ptr::<Value>() {
                    let i = frame.regs[idx].as_int().unwrap_or(0) as usize;
                    unsafe { let len = (*ptr).as_int().unwrap_or(0) as usize; if i < len { frame.regs[dst] = *ptr.add(i + 1); } }
                }
            }
            OpCode::ArrStore => {
                let (arr, idx, src) = (instr.op1 as usize, instr.op2 as usize, instr.op3 as usize);
                if let Some(ptr) = frame.regs[arr].as_ptr::<Value>() {
                    let i = frame.regs[idx].as_int().unwrap_or(0) as usize;
                    unsafe { let len = (*ptr).as_int().unwrap_or(0) as usize; if i < len { *ptr.add(i + 1) = frame.regs[src]; } }
                }
            }
            OpCode::ArrLen => {
                let (arr, dst) = (instr.op1 as usize, instr.op2 as usize);
                if let Some(ptr) = frame.regs[arr].as_ptr::<Value>() { unsafe { frame.regs[dst] = Value::int((*ptr).as_int().unwrap_or(0)); } }
                else { frame.regs[dst] = Value::int(0); }
            }
            OpCode::TupleMk => {
                let count = instr.op1 as usize;
                let dst = instr.op2 as usize;
                if count > 0 {
                    let layout = std::alloc::Layout::from_size_align(count * std::mem::size_of::<Value>(), 8).unwrap();
                    let ptr = unsafe { std::alloc::alloc(layout) } as *mut Value;
                    if !ptr.is_null() { for i in 0..count { unsafe { *ptr.add(i) = frame.regs[i]; } } frame.regs[dst] = Value::ptr(ptr as *mut u8); }
                }
            }
            OpCode::TupleL => {
                let (tup, fld, dst) = (instr.op1 as usize, instr.op2 as usize, instr.op3 as usize);
                if let Some(ptr) = frame.regs[tup].as_ptr::<Value>() { unsafe { frame.regs[dst] = *ptr.add(fld); } }
            }
            OpCode::RecMk => {
                let fc = instr.op1 as usize;
                let dst = instr.op2 as usize;
                if fc > 0 {
                    let layout = std::alloc::Layout::from_size_align(fc * std::mem::size_of::<Value>(), 8).unwrap();
                    let ptr = unsafe { std::alloc::alloc(layout) } as *mut Value;
                    if !ptr.is_null() { for i in 0..fc { unsafe { *ptr.add(i) = frame.regs[i]; } } frame.regs[dst] = Value::ptr(ptr as *mut u8); }
                }
            }
            OpCode::RecL => {
                let (obj, fld, dst) = (instr.op1 as usize, instr.op2 as usize, instr.op3 as usize);
                if let Some(ptr) = frame.regs[obj].as_ptr::<Value>() { unsafe { frame.regs[dst] = *ptr.add(fld); } }
            }
            OpCode::RecS => {
                let (obj, fld, src) = (instr.op1 as usize, instr.op2 as usize, instr.op3 as usize);
                if let Some(ptr) = frame.regs[obj].as_ptr::<Value>() { unsafe { *ptr.add(fld) = frame.regs[src]; } }
            }
            OpCode::IsTag => {
                let matches = frame.regs[instr.op1 as usize].as_int().map(|v| v as u64 == instr.op2 as u64).unwrap_or(false);
                frame.regs[instr.op3 as usize] = Value::bool(matches);
            }
            OpCode::Unpack => {
                frame.regs[instr.op2 as usize] = frame.regs[instr.op1 as usize];
            }
            OpCode::Copy => { frame.regs[instr.op3 as usize] = frame.regs[instr.op2 as usize]; }
            OpCode::Drop => { frame.regs[instr.op1 as usize] = Value::nil(); }

            // -- Actor (simple) --
            OpCode::SelfOp => {
                if let Some(id) = self.runtime.current_actor_id() { frame.regs[instr.op1 as usize] = Value::actor_ref(id); }
            }
            /// Receive a message from the current actor's mailbox.
            ///
            /// Operands:
            /// - `op1` = destination register for the received message (behavior_id as int)
            /// - `op2` = timeout register (0 = no timeout, >0 = timeout in ms)
            /// - `op3` = destination register for timeout flag (true = timed out)
            ///
            /// MVP: Pattern matching happens at a higher level. The VM simply
            /// pops the next message from the mailbox and stores its behavior_id.
            /// If no message is available and a timeout is set, the timeout flag
            /// is set to true. If no timeout, the receive returns nil (actor
            /// will be re-scheduled and can try again).
            OpCode::Receive => {
                let dst = instr.op1 as usize;
                let timeout_reg = instr.op2 as usize;
                let timeout_dst = instr.op3 as usize;

                if let Some(actor_id) = self.runtime.current_actor {
                    if let Some(actor) = self.runtime.actors.get_mut(&actor_id) {
                        match actor.mailbox.pop() {
                            Some(msg) => {
                                // Store behavior_id as the received value (MVP).
                                // The higher-level runtime matches on this to route
                                // to the correct behavior handler.
                                frame.regs[dst] = Value::int(msg.behavior_id as i64);
                                frame.regs[timeout_dst] = Value::bool(false); // not timed out
                            }
                            None => {
                                // No message available -- check for timeout.
                                let timeout_val = frame.regs[timeout_reg];
                                let has_timeout = timeout_val.as_int().unwrap_or(0) > 0;
                                if has_timeout {
                                    // Timeout specified and no message -> set timeout flag.
                                    frame.regs[dst] = Value::nil();
                                    frame.regs[timeout_dst] = Value::bool(true); // timed out
                                } else {
                                    // No timeout -- actor goes to Waiting state.
                                    // Store nil result; scheduler will re-enqueue later.
                                    frame.regs[dst] = Value::nil();
                                    frame.regs[timeout_dst] = Value::bool(false);
                                }
                            }
                        }
                    }
                }
            }
            /// Monitor an actor for exit signals.
            ///
            /// Operands:
            /// - `op1` = register containing the target actor reference to monitor
            /// - `op2` = destination register for the MonitorRef (target ID in MVP)
            ///
            /// When the target actor exits, the watcher (current actor) receives
            /// a DOWN message in its mailbox. Monitors are unidirectional and
            /// automatically removed when the target exits.
            OpCode::Monitor => {
                let target_reg = instr.op1 as usize;
                let dst = instr.op2 as usize;

                if let (Some(target_id), Some(watcher_id)) = (
                    frame.regs[target_reg].as_actor_id(),
                    self.runtime.current_actor,
                ) {
                    self.runtime.monitor(watcher_id, target_id);
                    // In MVP, the MonitorRef is just the target actor ID.
                    frame.regs[dst] = Value::int(target_id as i64);
                }
            }
            /// Demonitor an actor.
            ///
            /// Operands:
            /// - `op1` = register containing the watcher actor reference
            ///           (falls back to current_actor if not an actor ref)
            /// - `op2` = register containing the monitor ref (target actor ID in MVP)
            ///
            /// Removes the watcher from the target's monitor list. If either
            /// actor does not exist, the operation is a no-op.
            OpCode::Demon => {
                let watcher_reg = instr.op1 as usize;
                let target_reg = instr.op2 as usize;

                if let (Some(watcher_id), Some(target_id)) = (
                    frame.regs[watcher_reg].as_actor_id().or(self.runtime.current_actor),
                    frame.regs[target_reg].as_actor_id(),
                ) {
                    self.runtime.demonitor(watcher_id, target_id);
                }
            }
            /// Link two actors bidirectionally.
            ///
            /// Operands:
            /// - `op1` = register containing the other actor reference to link with
            ///
            /// If either actor exits abnormally, the other will also exit
            /// (unless it traps exits). Links are symmetric.
            OpCode::Link => {
                let other_reg = instr.op1 as usize;

                if let (Some(other_id), Some(self_id)) = (
                    frame.regs[other_reg].as_actor_id(),
                    self.runtime.current_actor,
                ) {
                    self.runtime.link_actors(self_id, other_id);
                }
            }
            /// Unlink two actors.
            ///
            /// Operands:
            /// - `op1` = register containing the other actor reference to unlink from
            ///
            /// Removes the bidirectional link between the current actor and
            /// the specified actor. If either actor does not exist, the
            /// operation is a no-op.
            OpCode::Unlink => {
                let other_reg = instr.op1 as usize;

                if let (Some(other_id), Some(self_id)) = (
                    frame.regs[other_reg].as_actor_id(),
                    self.runtime.current_actor,
                ) {
                    self.runtime.unlink_actors(self_id, other_id);
                }
            }
            /// Exit the current actor.
            ///
            /// Operands:
            /// - `op1` = register containing the exit reason (unused in MVP,
            ///           all exits are treated as Normal)
            ///
            /// Marks the current actor as Terminated and stops VM execution
            /// for this actor. The runtime's exit handling (notifying monitors,
            /// linked actors, supervisor) happens separately through the
            /// `exit_actor` / `handle_actor_exit` path.
            OpCode::Exit => {
                if let Some(actor_id) = self.runtime.current_actor {
                    // Mark actor as terminated in the actors map.
                    if let Some(actor) = self.runtime.actors.get_mut(&actor_id) {
                        actor.state = crate::runtime::ActorState::Terminated;
                    }
                    // Stop VM execution for this actor. The scheduler will
                    // handle cleanup (monitors, links, supervisor) via the
                    // runtime's exit signal path.
                    self.running = false;
                }
            }
            /// Yield execution back to the scheduler.
            ///
            /// No operands. Resets the current actor's reduction count and
            /// re-enqueues it in the scheduler, then stops VM execution.
            /// The scheduler will pick another actor and eventually re-schedule
            /// this one.
            OpCode::Yield => {
                if let Some(actor_id) = self.runtime.current_actor {
                    if let Some(actor) = self.runtime.actors.get_mut(&actor_id) {
                        actor.reset_reductions();
                    }
                    self.runtime.scheduler.enqueue(actor_id);
                }
                // Stop this VM execution -- the scheduler will resume later.
                self.running = false;
            }

            // -- Effects (MVP) --
            OpCode::Perform => { frame.regs[instr.op3 as usize] = Value::unit(); }
            OpCode::Handle => {}
            OpCode::Resume => {}
            OpCode::Unwind => {}

            // -- Capabilities (MVP) --
            OpCode::CapChk => { frame.regs[instr.op2 as usize] = Value::bool(true); }
            OpCode::CapUp => { frame.regs[instr.op2 as usize] = frame.regs[instr.op1 as usize]; }
            OpCode::CapDown => { frame.regs[instr.op2 as usize] = frame.regs[instr.op1 as usize]; }
            OpCode::CapSend => { frame.regs[instr.op2 as usize] = frame.regs[instr.op1 as usize]; }

            // -- Distribution (MVP) --
            OpCode::NodeId => { frame.regs[instr.op1 as usize] = Value::int(0); }
            OpCode::Migrate => {}
            OpCode::RAsk => { frame.regs[instr.op3 as usize] = Value::nil(); }
            OpCode::Gossip => {}

            // -- String & IO --
            OpCode::SConcat => {
                let s1 = frame.regs[instr.op1 as usize].to_string_repr();
                let s2 = frame.regs[instr.op2 as usize].to_string_repr();
                let result = format!("{}{}", s1, s2);
                frame.regs[instr.op3 as usize] = Value::ptr(result.into_bytes().leak().as_mut_ptr());
            }
            OpCode::SPrint => { print!("{}", frame.regs[instr.op1 as usize].to_string_repr()); }
            OpCode::SRead => {
                let mut input = String::new();
                frame.regs[instr.op1 as usize] = if std::io::stdin().read_line(&mut input).is_ok() {
                    Value::ptr(input.into_bytes().leak().as_mut_ptr())
                } else { Value::nil() };
            }
            OpCode::FOpen => { frame.regs[instr.op2 as usize] = Value::nil(); }
            OpCode::FRead => { frame.regs[instr.op2 as usize] = Value::nil(); }
            OpCode::FWrite => {}
            OpCode::FClose => {}
            OpCode::Print => { println!("{}", frame.regs[instr.op1 as usize].to_string_repr()); }

            // -- Debug & Meta --
            OpCode::DbgBreak => {}
            OpCode::DbgPrint => {
                eprintln!("=== Debug: Register State ===");
                for i in (0..256).step_by(8) {
                    let mut line = format!("R{:03}-R{:03}: ", i, i + 7);
                    for j in 0..8 { line.push_str(&format!("{:>20} ", frame.regs[i + j].to_string_repr())); }
                    eprintln!("{}", line);
                }
            }
            OpCode::DbgStack => {
                eprintln!("=== Debug: Call Stack ===");
                let mut depth = 0;
                let mut fref: Option<&Frame> = Some(&frame);
                while let Some(fr) = fref {
                    let mname = self.modules.get(fr.module_idx).map(|m| m.name.as_str()).unwrap_or("?");
                    eprintln!("  [{}] module={} pc={}", depth, mname, fr.pc);
                    depth += 1;
                    fref = fr.caller.as_deref();
                }
                if depth == 0 { eprintln!("  (empty)"); }
            }
            OpCode::MetaType => { frame.regs[instr.op2 as usize] = Value::int(0); }
            OpCode::MetaCap => { frame.regs[instr.op2 as usize] = Value::int(0); }

            // Frame-manipulating opcodes handled above; unreachable but needed for exhaustiveness
            OpCode::Call | OpCode::TailCall | OpCode::Ret | OpCode::RetVal |
            OpCode::ClosureCall | OpCode::Panic |
            OpCode::Spawn | OpCode::Send | OpCode::Ask |
            OpCode::RSend | OpCode::RSpawn => {}
        }
        self.current_frame = Some(frame);
    }

    fn module_const_string(&self, module_idx: usize, idx: u16) -> String {
        self.modules.get(module_idx)
            .and_then(|m| m.constants.get(idx as usize))
            .map(|c| format!("{:?}", c))
            .unwrap_or_default()
    }
}

impl Default for VM {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn constant_to_value(c: &Constant) -> Value {
    match c {
        Constant::Int(n) => Value::int(*n),
        Constant::Float(f) => Value::float(*f),
        Constant::Bool(b) => Value::bool(*b),
        Constant::Unit => Value::unit(),
    }
}
