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
        // if debug {
        //     eprintln!("[step {}] PC={} op={:?} op1={} op2={} op3={} regs[0]={} regs[1]={}",
        //         self.step_count, frame.pc - 1, instr.opcode,
        //         instr.op1, instr.op2, instr.op3,
        //         frame.regs[0].to_string_repr(), frame.regs[1].to_string_repr());
        // }

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
            OpCode::Receive => { frame.regs[instr.op1 as usize] = Value::nil(); }
            OpCode::Monitor => {}
            OpCode::Demon => {}
            OpCode::Link => {}
            OpCode::Unlink => {}
            OpCode::Exit => { self.running = false; }
            OpCode::Yield => {}

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

    // === Frame-Manipulating Dispatchers (called before frame borrow) ===

    fn dispatch_call(&mut self, func_reg: u8, argc: u8, dst: u8) -> NuResult<()> {
        let old_frame = self.current_frame.take()
            .ok_or_else(|| NuError::VMError("No frame for call".to_string()))?;

        let func_val = old_frame.regs[func_reg as usize];
        let module_idx = old_frame.module_idx;

        // Resolve function: func_val is either a function index (int) or a code offset
        let func_idx = func_val.as_int()
            .ok_or_else(|| NuError::VMError("Invalid function reference (not an integer index)".to_string()))? as usize;

        let code_offset = self.modules.get(module_idx)
            .and_then(|m| m.function_table.get(func_idx)).copied()
            .ok_or_else(|| NuError::VMError(format!("Function index {} not found in module {}", func_idx, module_idx)))?;

        // Create new frame
        let mut new_frame = Frame::new(None, module_idx);
        new_frame.pc = code_offset;

        // Copy arguments from old frame registers 0..argc
        let arg_count = (argc as usize).min(256);
        for i in 0..arg_count {
            new_frame.regs[i] = old_frame.regs[i];
        }

        // Store return destination in closure field
        new_frame.closure = Some(Value::int(dst as i64));

        // Link frames
        new_frame.caller = Some(old_frame);
        self.current_frame = Some(Box::new(new_frame));

        Ok(())
    }

    fn dispatch_ret(&mut self) -> NuResult<()> {
        let old_frame = self.current_frame.take()
            .ok_or_else(|| NuError::VMError("No frame to return from".to_string()))?;
        let ret_val = old_frame.regs[0];

        if let Some(mut caller_frame) = old_frame.caller {
            let dst = old_frame.closure.as_ref()
                .and_then(|v| v.as_int()).unwrap_or(0) as u8;
            caller_frame.regs[dst as usize] = ret_val;
            self.current_frame = Some(caller_frame);
        } else {
            // Top-level return: create dummy frame to hold return value
            let module_idx = old_frame.module_idx;
            let mut dummy = Frame::new(None, module_idx);
            dummy.regs[0] = ret_val;
            dummy.pc = usize::MAX;
            self.current_frame = Some(Box::new(dummy));
            self.running = false;
        }
        Ok(())
    }

    fn dispatch_ret_val(&mut self, val_reg: u8) -> NuResult<()> {
        let old_frame = self.current_frame.take()
            .ok_or_else(|| NuError::VMError("No frame to return from".to_string()))?;
        let ret_val = old_frame.regs[val_reg as usize];

        if let Some(mut caller_frame) = old_frame.caller {
            let dst = old_frame.closure.as_ref()
                .and_then(|v| v.as_int()).unwrap_or(0) as u8;
            caller_frame.regs[dst as usize] = ret_val;
            self.current_frame = Some(caller_frame);
        } else {
            let module_idx = old_frame.module_idx;
            let mut dummy = Frame::new(None, module_idx);
            dummy.regs[0] = ret_val;
            dummy.pc = usize::MAX;
            self.current_frame = Some(Box::new(dummy));
            self.running = false;
        }
        Ok(())
    }

    fn dispatch_spawn(&mut self, frame: &mut Frame, _behavior_idx: u16, _init_reg: u8, dst: u8) -> NuResult<()> {
        // MVP: create a placeholder actor ID and store it
        let actor_id = fresh_actor_id();
        frame.regs[dst as usize] = Value::actor_ref(actor_id);
        Ok(())
    }

    fn dispatch_send(&mut self, frame: &mut Frame, addr_reg: u8, _behavior_id: u16) -> NuResult<()> {
        // MVP: look up actor and enqueue message
        if let Some(actor_id) = frame.regs[addr_reg as usize].as_actor_id() {
            if let Some(actor) = self.runtime.actors.get_mut(&actor_id) {
                let msg = Message {
                    behavior_id: _behavior_id,
                    payload: vec![frame.regs[0]],
                    sender: self.runtime.current_actor.unwrap_or(0),
                    priority: MessagePriority::Normal,
                };
                let _ = actor.mailbox.push(msg);
            }
        }
        Ok(())
    }

    // === Instruction Handlers (frame already borrowed) ===

    fn exec_nop(&mut self, _frame: &mut Frame) {}
    fn exec_halt(&mut self, _frame: &mut Frame) { self.running = false; }
    fn exec_const0(&mut self, frame: &mut Frame, dst: u8) { frame.regs[dst as usize] = Value::int(0); }
    fn exec_const1(&mut self, frame: &mut Frame, dst: u8) { frame.regs[dst as usize] = Value::int(1); }
    fn exec_constu(&mut self, frame: &mut Frame, idx: u16, dst: u8) {
        if let Some(module) = self.current_module() {
            if let Some(c) = module.constants.get(idx as usize) {
                frame.regs[dst as usize] = constant_to_value(c);
            }
        }
    }

    fn exec_iadd(&mut self, frame: &mut Frame, r1: u8, r2: u8, dst: u8) {
        if let (Some(a), Some(b)) = (frame.regs[r1 as usize].as_int(), frame.regs[r2 as usize].as_int()) {
            frame.regs[dst as usize] = Value::int(a + b);
        }
    }
    fn exec_isub(&mut self, frame: &mut Frame, r1: u8, r2: u8, dst: u8) {
        if let (Some(a), Some(b)) = (frame.regs[r1 as usize].as_int(), frame.regs[r2 as usize].as_int()) {
            frame.regs[dst as usize] = Value::int(a - b);
        }
    }
    fn exec_imul(&mut self, frame: &mut Frame, r1: u8, r2: u8, dst: u8) {
        if let (Some(a), Some(b)) = (frame.regs[r1 as usize].as_int(), frame.regs[r2 as usize].as_int()) {
            frame.regs[dst as usize] = Value::int(a * b);
        }
    }
    fn exec_idiv(&mut self, frame: &mut Frame, r1: u8, r2: u8, dst: u8) {
        if let (Some(a), Some(b)) = (frame.regs[r1 as usize].as_int(), frame.regs[r2 as usize].as_int()) {
            if b != 0 { frame.regs[dst as usize] = Value::int(a / b); }
        }
    }
    fn exec_imod(&mut self, frame: &mut Frame, r1: u8, r2: u8, dst: u8) {
        if let (Some(a), Some(b)) = (frame.regs[r1 as usize].as_int(), frame.regs[r2 as usize].as_int()) {
            if b != 0 { frame.regs[dst as usize] = Value::int(a % b); }
        }
    }
    fn exec_ineg(&mut self, frame: &mut Frame, r1: u8, dst: u8) {
        if let Some(a) = frame.regs[r1 as usize].as_int() {
            frame.regs[dst as usize] = Value::int(-a);
        }
    }

    fn exec_fadd(&mut self, frame: &mut Frame, r1: u8, r2: u8, dst: u8) {
        if let (Some(a), Some(b)) = (frame.regs[r1 as usize].as_float(), frame.regs[r2 as usize].as_float()) {
            frame.regs[dst as usize] = Value::float(a + b);
        }
    }

    /// Helper: get a reference to the current module (if any).
    fn current_module(&self) -> Option<&CodeModule> {
        self.current_frame.as_ref()
            .and_then(|f| self.modules.get(f.module_idx))
    }

    /// Helper: get a constant string from a module's constant pool.
    fn module_const_string(&self, module_idx: usize, const_idx: u16) -> String {
        self.modules.get(module_idx)
            .and_then(|m| m.constants.get(const_idx as usize))
            .map(|c| match c {
                Constant::String(s) => s.clone(),
                Constant::Int(n) => n.to_string(),
                _ => format!("{:?}", c),
            })
            .unwrap_or_else(|| "?".to_string())
    }
}

// Helper: convert Constant to VM Value
fn constant_to_value(c: &Constant) -> Value {
    match c {
        Constant::Int(n) => Value::int(*n),
        Constant::Float(f) => Value::float(*f),
        Constant::Bool(b) => Value::bool(*b),
        Constant::String(_) => Value::unit(), // MVP: strings not fully supported
        Constant::Unit => Value::unit(),
        Constant::FunctionRef(idx) => Value::int(*idx as i64),
    }
}

/// Global counter for actor IDs (MVP)
static ACTOR_ID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn fresh_actor_id() -> u64 {
    ACTOR_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}
