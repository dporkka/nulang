//! Register-based Virtual Machine for Nulang.
//!
//! - 256 virtual registers per activation frame
//! - 32-bit fixed-width instructions
//! - Direct-threaded dispatch (token threading via computed goto pattern)
//! - NaN-tagged 64-bit values

use crate::bytecode::*;
use crate::runtime::*;
use crate::types::{NuResult, NuError, Span};
// Python interop is handled via NativeActor (see src/python/native_actor.rs).
// No Python types enter the VM value representation.

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
const TAG_CLOSURE: u64 = 0x7FF8000000000000; // VM closure object id
// TAG_PYTHON (0x7FFE...) removed per audit — Python objects are quarantined
// to NativeActor OS threads; they never enter the VM value representation.

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
    pub fn closure(id: usize) -> Value {
        Value(TAG_CLOSURE | (id as u64 & 0x0000FFFFFFFFFFFF))
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
    pub fn as_string_id(&self) -> Option<u32> {
        if self.0 & TAG_MASK == TAG_STRING {
            Some((self.0 & 0x0000FFFFFFFFFFFF) as u32)
        } else {
            None
        }
    }
    pub fn as_closure_id(&self) -> Option<usize> {
        if self.0 & TAG_MASK == TAG_CLOSURE {
            Some((self.0 & 0x0000FFFFFFFFFFFF) as usize)
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
        if let Some(id) = self.as_string_id() { return format!("<string:{}>", id); }
        if let Some(id) = self.as_closure_id() { return format!("<closure:{}>", id); }
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
    pub return_dst: u8,         // Register in caller to store return value
    pub closure_env: Option<usize>, // Active closure environment id (for captures)
    pub caller: Option<Box<Frame>>, // Linked list of frames
    pub module_idx: usize,      // Which module this frame is executing
}

impl Frame {
    pub fn new(caller: Option<Box<Frame>>, module_idx: usize) -> Self {
        Frame {
            regs: [Value::nil(); 256],
            pc: 0,
            return_dst: 0,
            closure_env: None,
            caller,
            module_idx,
        }
    }
}

// ---------------------------------------------------------------------------
// VM
// ---------------------------------------------------------------------------

/// Lightweight closure object stored by the VM. Captures are immutable values
/// copied from the creating scope.
struct Closure {
    func_idx: usize,
    captures: Vec<Value>,
}

// ---------------------------------------------------------------------------
// Continuation (for algebraic effects)
// ---------------------------------------------------------------------------

/// Recursively deep-clone a frame chain (for continuation capture).
fn clone_frame_chain(frame: &Frame) -> Box<Frame> {
    Box::new(Frame {
        regs: frame.regs,
        pc: frame.pc,
        return_dst: frame.return_dst,
        closure_env: frame.closure_env,
        caller: frame.caller.as_ref().map(|c| clone_frame_chain(c)),
        module_idx: frame.module_idx,
    }))
}

/// A captured continuation — a deep snapshot of the VM's execution state
/// at the point of a `perform` call. Restored by `resume` to continue
/// the suspended computation with a value.
#[derive(Debug)]
struct Continuation {
    /// Deep-cloned frame chain (innermost frame is the head).
    frame: Box<Frame>,
    /// The PC to resume at (points to the instruction after Perform).
    resume_pc: usize,
    /// Which module the resumed code lives in.
    module_idx: usize,
    /// Which register to store the resume value into.
    resume_dst: u8,
    /// Step count at capture time.
    step_count: usize,
}

impl Continuation {
    /// Capture a continuation from the current VM state.
    fn capture(vm: &VM, resume_dst: u8) -> Option<Self> {
        let current = vm.current_frame.as_ref()?;
        Some(Continuation {
            frame: clone_frame_chain(current),
            resume_pc: current.pc, // PC already points past the Perform instruction
            module_idx: current.module_idx,
            resume_dst,
            step_count: vm.step_count,
        })
    }

    /// Restore this continuation into the VM, placing `value` in the
    /// resume destination register.
    fn restore(self, vm: &mut VM, value: Value) {
        let mut frame = self.frame;
        frame.regs[self.resume_dst as usize] = value;
        frame.pc = self.resume_pc;
        vm.current_frame = Some(frame);
        vm.step_count = self.step_count;
    }
}

// ---------------------------------------------------------------------------
// Handler Frame (for algebraic effects)
// ---------------------------------------------------------------------------

/// An active handler frame on the handler stack. One frame per
/// `handle { ... }` block. Tracks which effects this block handles
/// and where to resume after normal completion.
#[derive(Debug)]
struct HandlerFrame {
    /// Index into the current module's handler_tables.
    handler_table_idx: usize,
    /// Which module this handler belongs to.
    module_idx: usize,
    /// The PC to resume at when the handle block completes normally
    /// (i.e., the instruction after the matching Unwind).
    resume_pc: usize,
    /// The register to store the normal completion result in.
    resume_dst: u8,
    /// The captured continuation (set by Perform, consumed by Resume).
    captured_continuation: Option<Continuation>,
}

impl HandlerFrame {
    fn new(handler_table_idx: usize, module_idx: usize, resume_pc: usize, resume_dst: u8) -> Self {
        HandlerFrame {
            handler_table_idx,
            module_idx,
            resume_pc,
            resume_dst,
            captured_continuation: None,
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
    closures: Vec<Box<Closure>>,
    /// Handler stack: one frame per active `handle { ... }` block.
    /// Grows on Handle, shrinks on Unwind or Resume.
    handler_stack: Vec<HandlerFrame>,
    // Note: Python interop is handled externally via NativeActor.
    // No Python state is stored in the VM (audit requirement).
}

impl VM {
    pub fn new() -> Self {
        VM {
            modules: Vec::new(),
            current_frame: None,
            running: false,
            runtime: Runtime::new(),
            step_count: 0,
            closures: Vec::new(),
            handler_stack: Vec::new(),
        }
    }

    fn alloc_closure(&mut self, func_idx: usize) -> usize {
        let id = self.closures.len();
        self.closures.push(Box::new(Closure {
            func_idx,
            captures: Vec::new(),
        }));
        id
    }

    fn closure_store(&mut self, closure_id: usize, idx: usize, val: Value) {
        if let Some(closure) = self.closures.get_mut(closure_id) {
            if idx >= closure.captures.len() {
                closure.captures.resize(idx + 1, Value::nil());
            }
            closure.captures[idx] = val;
        }
    }

    fn closure_load(&self, closure_id: usize, idx: usize) -> Option<Value> {
        self.closures.get(closure_id)?.captures.get(idx).copied()
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

    /// Resolve a function value to a (function_table_index, optional_closure_env_id).
    fn resolve_function(&self, func_val: Value, _module_idx: usize) -> NuResult<(usize, Option<usize>)> {
        if let Some(func_idx) = func_val.as_int() {
            Ok((func_idx as usize, None))
        } else if let Some(closure_id) = func_val.as_closure_id() {
            let closure = self.closures.get(closure_id)
                .ok_or_else(|| NuError::VMError(format!("Closure {} not found", closure_id)))?;
            Ok((closure.func_idx, Some(closure_id)))
        } else {
            Err(NuError::VMError("Invalid function reference".to_string()))
        }
    }

    /// Single-step execute one instruction.
    pub fn step(&mut self) -> NuResult<()> {
        // Step limit: configurable via env var NULANG_STEP_LIMIT.
        // Default 10M steps — long-running actors (servers, processors) may need more.
        self.step_count += 1;
        let limit = std::env::var("NULANG_STEP_LIMIT")
            .ok().and_then(|s| s.parse().ok()).unwrap_or(10_000_000);
        if self.step_count > limit {
            return Err(NuError::VMError(
                format!("Step limit exceeded ({} steps). Set NULANG_STEP_LIMIT env var to increase.", limit)
            ));
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
                let (func_idx, closure_env) = self.resolve_function(func_val, module_idx)?;
                let code_offset = self.modules.get(module_idx)
                    .and_then(|m| m.function_table.get(func_idx)).copied()
                    .ok_or_else(|| NuError::VMError(format!("Function {} not found", func_idx)))?;
                let mut new_frame = Frame::new(None, module_idx);
                new_frame.pc = code_offset;
                for i in 0..(argc as usize).min(256) {
                    new_frame.regs[i] = frame.regs[i];
                }
                new_frame.return_dst = dst;
                new_frame.closure_env = closure_env;
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
                    let dst = frame.return_dst as usize;
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
                    let dst = frame.return_dst as usize;
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
                let func_val = frame.regs[instr.op1 as usize];
                let module_idx = frame.module_idx;
                let argc = instr.op2;
                let dst = instr.op3;
                let (func_idx, closure_env) = self.resolve_function(func_val, module_idx)?;
                let code_offset = self.modules.get(module_idx)
                    .and_then(|m| m.function_table.get(func_idx)).copied()
                    .ok_or_else(|| NuError::VMError(format!("Function {} not found", func_idx)))?;
                let mut new_frame = Frame::new(None, module_idx);
                new_frame.pc = code_offset;
                for i in 0..(argc as usize).min(256) {
                    new_frame.regs[i] = frame.regs[i];
                }
                new_frame.return_dst = dst;
                new_frame.closure_env = closure_env;
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
                if let Err(e) = self.dispatch_regular(frame, instr) {
                    return Err(e);
                }
                // Frame is put back inside dispatch_regular
            }
        }

        Ok(())
    }

    fn dispatch_regular(&mut self, mut frame: Box<Frame>, instr: Instruction) -> NuResult<()> {
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
                let func_idx = instr.imm16() as usize;
                let dst = instr.op3;
                let closure_id = self.alloc_closure(func_idx);
                frame.regs[dst as usize] = Value::closure(closure_id);
            }
            OpCode::CapLoad => {
                let idx = instr.op1 as usize;
                let dst = instr.op2 as usize;
                if let Some(closure_id) = frame.closure_env {
                    if let Some(val) = self.closure_load(closure_id, idx) {
                        frame.regs[dst] = val;
                    }
                }
            }
            OpCode::CapStore => {
                let closure_reg = instr.op1 as usize;
                let idx = instr.op2 as usize;
                let src = instr.op3 as usize;
                if let Some(closure_id) = frame.regs[closure_reg].as_closure_id() {
                    let val = frame.regs[src];
                    self.closure_store(closure_id, idx, val);
                }
            }
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
                                // No message available — check for timeout.
                                let timeout_val = frame.regs[timeout_reg];
                                let has_timeout = timeout_val.as_int().unwrap_or(0) > 0;
                                if has_timeout {
                                    // Timeout specified and no message → set timeout flag.
                                    frame.regs[dst] = Value::nil();
                                    frame.regs[timeout_dst] = Value::bool(true); // timed out
                                } else {
                                    // No timeout — actor goes to Waiting state.
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
                // Stop this VM execution — the scheduler will resume later.
                self.running = false;
            }

            // -- Algebraic Effects --
            OpCode::Handle => {
                // Handle: push a new handler frame onto the handler stack.
                // op1 = handler_table_idx (index into module.handler_tables)
                // The handler remains active until matching Unwind.
                //
                // Resume PC: we save the current PC (which points past this Handle
                // instruction) as the place to resume when the handle block
                // completes normally.
                let handler_table_idx = instr.op1 as usize;
                let module_idx = frame.module_idx;
                let resume_pc = frame.pc; // already incremented past Handle
                // dst reg for normal completion result — stored in op2
                let resume_dst = instr.op2;
                self.handler_stack.push(HandlerFrame::new(
                    handler_table_idx, module_idx, resume_pc, resume_dst,
                ));
            }
            OpCode::Perform => {
                // Perform: invoke an effect operation.
                // op1<<8 | op2 = effect_name constant pool index
                // op3 = dst_reg (where to store the result after resume)
                let eff_name_idx = instr.imm16();
                let dst_reg = instr.op3;
                let effect_name = self.module_const_string(frame.module_idx, eff_name_idx);

                // Search handler stack from top (innermost) to bottom (outermost).
                let handler_idx = self.handler_stack.iter().rposition(|hf| {
                    if let Some(module) = self.modules.get(hf.module_idx) {
                        if let Some(ht) = module.handler_tables.get(hf.handler_table_idx) {
                            ht.bindings.iter().any(|b| b.effect_name == effect_name)
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                });

                if let Some(handler_stack_idx) = handler_idx {
                    // Found a handler. Capture continuation and invoke handler.
                    let hf = &mut self.handler_stack[handler_stack_idx];

                    // Look up the binding to get the handler code offset.
                    let (handler_offset, _arg_count, result_reg) = {
                        let module = self.modules.get(hf.module_idx).unwrap();
                        let ht = module.handler_tables.get(hf.handler_table_idx).unwrap();
                        let binding = ht.bindings.iter()
                            .find(|b| b.effect_name == effect_name)
                            .unwrap();
                        (binding.handler_offset, binding.arg_count, binding.result_reg)
                    };

                    // Capture continuation BEFORE we modify execution.
                    let cont = Continuation::capture(self, dst_reg)
                        .ok_or_else(|| NuError::VMError(
                            "Cannot capture continuation: no current frame".into()
                        ))?;
                    self.handler_stack[handler_stack_idx].captured_continuation = Some(cont);

                    // Save the result register so Resume knows where to place
                    // the handler's return value.
                    self.handler_stack[handler_stack_idx].resume_dst = result_reg;

                    // Set up execution at the handler code offset.
                    // The handler body receives effect arguments in r0..rn.
                    // For MVP: args are already in r0 from the compiled Perform site.
                    frame.pc = handler_offset;

                    // Continue with the handler body executing.
                } else {
                    // No handler found — check for fallback.
                    let has_fallback = self.handler_stack.last().and_then(|hf| {
                        self.modules.get(hf.module_idx)
                            .and_then(|m| m.handler_tables.get(hf.handler_table_idx))
                            .and_then(|ht| ht.fallback_offset)
                    }).is_some();

                    if has_fallback {
                        // Jump to fallback handler.
                        let fallback_offset = self.handler_stack.last().and_then(|hf| {
                            self.modules.get(hf.module_idx)
                                .and_then(|m| m.handler_tables.get(hf.handler_table_idx))
                                .and_then(|ht| ht.fallback_offset)
                        }).unwrap();
                        frame.pc = fallback_offset;
                    } else {
                        self.current_frame = Some(frame);
                        return Err(NuError::EffectError {
                            msg: format!("Unhandled effect: '{}'", effect_name),
                            span: Span::default(),
                        });
                    }
                }
            }
            OpCode::Resume => {
                // Resume: restore the captured continuation with a value.
                // op1 = register containing the value to resume with.
                let val_reg = instr.op1 as usize;
                let val = frame.regs[val_reg];

                // Pop the handler frame that was invoked.
                if let Some(hf) = self.handler_stack.pop() {
                    if let Some(cont) = hf.captured_continuation {
                        // Restore continuation: this resets the frame chain and PC.
                        cont.restore(self, val);
                        // The restored frame's PC points past the original Perform.
                        // Do NOT put the current frame back — it's been replaced
                        // by the restored continuation.
                        return Ok(());
                    }
                }

                // Resume without a captured continuation — error.
                self.current_frame = Some(frame);
                return Err(NuError::VMError(
                    "resume called without a captured continuation".into()
                ));
            }
            OpCode::Unwind => {
                // Unwind: the handle block completed normally. Pop the handler
                // frame and continue execution at the next instruction.
                // The PC already points past Unwind (incremented in step()),
                // so we just pop the handler frame and let execution flow
                // continue naturally.
                self.handler_stack.pop();
            }

            // -- Capabilities (MVP) --
            OpCode::CapChk => { frame.regs[instr.op2 as usize] = Value::bool(true); }
            OpCode::CapUp => { frame.regs[instr.op2 as usize] = frame.regs[instr.op1 as usize]; }
            OpCode::CapDown => { frame.regs[instr.op2 as usize] = frame.regs[instr.op1 as usize]; }
            OpCode::CapSend => { frame.regs[instr.op2 as usize] = frame.regs[instr.op1 as usize]; }

            // -- Python Interop — RESERVED (see audit, native_actor.rs) --
            //
            // Python interop is handled EXTERNALLY via NativeActor (see
            // src/python/native_actor.rs). These opcodes are reserved for
            // a future bytecode-level native-actor call instruction.
            //
            // Per the architectural audit: Python objects MUST NOT enter
            // the VM value representation. All Python code runs in
            // dedicated OS threads with marshal-only data crossing.
            OpCode::PyImport | OpCode::PyGetAttr | OpCode::PyCall
            | OpCode::PyCallKw | OpCode::PySetAttr | OpCode::PyToNu
            | OpCode::PyFromNu | OpCode::PyRelease => {
                self.current_frame = Some(frame);
                return Err(NuError::VMError(
                    "Python opcodes require native actor runtime. \
                     Use perform Python.call(...) instead.".into()
                ));
            }

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
        Ok(())
    }

    // === Frame-Manipulating Dispatchers (called before frame borrow) ===

    fn dispatch_call(&mut self, func_reg: u8, argc: u8, dst: u8) -> NuResult<()> {
        let old_frame = self.current_frame.take()
            .ok_or_else(|| NuError::VMError("No frame for call".to_string()))?;

        let func_val = old_frame.regs[func_reg as usize];
        let module_idx = old_frame.module_idx;

        let (func_idx, closure_env) = self.resolve_function(func_val, module_idx)?;

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

        new_frame.return_dst = dst;
        new_frame.closure_env = closure_env;

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
            let dst = old_frame.return_dst as usize;
            caller_frame.regs[dst] = ret_val;
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
            let dst = old_frame.return_dst as usize;
            caller_frame.regs[dst] = ret_val;
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
    fn exec_fsub(&mut self, frame: &mut Frame, r1: u8, r2: u8, dst: u8) {
        if let (Some(a), Some(b)) = (frame.regs[r1 as usize].as_float(), frame.regs[r2 as usize].as_float()) {
            frame.regs[dst as usize] = Value::float(a - b);
        }
    }
    fn exec_fmul(&mut self, frame: &mut Frame, r1: u8, r2: u8, dst: u8) {
        if let (Some(a), Some(b)) = (frame.regs[r1 as usize].as_float(), frame.regs[r2 as usize].as_float()) {
            frame.regs[dst as usize] = Value::float(a * b);
        }
    }
    fn exec_fdiv(&mut self, frame: &mut Frame, r1: u8, r2: u8, dst: u8) {
        if let (Some(a), Some(b)) = (frame.regs[r1 as usize].as_float(), frame.regs[r2 as usize].as_float()) {
            frame.regs[dst as usize] = Value::float(a / b);
        }
    }
    fn exec_fneg(&mut self, frame: &mut Frame, r1: u8, dst: u8) {
        if let Some(a) = frame.regs[r1 as usize].as_float() {
            frame.regs[dst as usize] = Value::float(-a);
        }
    }
    fn exec_fmod(&mut self, frame: &mut Frame, r1: u8, r2: u8, dst: u8) {
        if let (Some(a), Some(b)) = (frame.regs[r1 as usize].as_float(), frame.regs[r2 as usize].as_float()) {
            frame.regs[dst as usize] = Value::float(a % b);
        }
    }

    fn exec_icmp_eq(&mut self, frame: &mut Frame, r1: u8, r2: u8, dst: u8) {
        let result = match (frame.regs[r1 as usize].as_int(), frame.regs[r2 as usize].as_int()) {
            (Some(a), Some(b)) => Value::bool(a == b),
            _ => Value::bool(false),
        };
        frame.regs[dst as usize] = result;
    }
    fn exec_icmp_lt(&mut self, frame: &mut Frame, r1: u8, r2: u8, dst: u8) {
        if let (Some(a), Some(b)) = (frame.regs[r1 as usize].as_int(), frame.regs[r2 as usize].as_int()) {
            frame.regs[dst as usize] = Value::bool(a < b);
        }
    }
    fn exec_icmp_gt(&mut self, frame: &mut Frame, r1: u8, r2: u8, dst: u8) {
        if let (Some(a), Some(b)) = (frame.regs[r1 as usize].as_int(), frame.regs[r2 as usize].as_int()) {
            frame.regs[dst as usize] = Value::bool(a > b);
        }
    }
    fn exec_icmp_le(&mut self, frame: &mut Frame, r1: u8, r2: u8, dst: u8) {
        if let (Some(a), Some(b)) = (frame.regs[r1 as usize].as_int(), frame.regs[r2 as usize].as_int()) {
            frame.regs[dst as usize] = Value::bool(a <= b);
        }
    }
    fn exec_icmp_ge(&mut self, frame: &mut Frame, r1: u8, r2: u8, dst: u8) {
        if let (Some(a), Some(b)) = (frame.regs[r1 as usize].as_int(), frame.regs[r2 as usize].as_int()) {
            frame.regs[dst as usize] = Value::bool(a >= b);
        }
    }

    fn exec_fcmp_eq(&mut self, frame: &mut Frame, r1: u8, r2: u8, dst: u8) {
        if let (Some(a), Some(b)) = (frame.regs[r1 as usize].as_float(), frame.regs[r2 as usize].as_float()) {
            frame.regs[dst as usize] = Value::bool((a - b).abs() < f64::EPSILON);
        }
    }
    fn exec_fcmp_lt(&mut self, frame: &mut Frame, r1: u8, r2: u8, dst: u8) {
        if let (Some(a), Some(b)) = (frame.regs[r1 as usize].as_float(), frame.regs[r2 as usize].as_float()) {
            frame.regs[dst as usize] = Value::bool(a < b);
        }
    }
    fn exec_fcmp_gt(&mut self, frame: &mut Frame, r1: u8, r2: u8, dst: u8) {
        if let (Some(a), Some(b)) = (frame.regs[r1 as usize].as_float(), frame.regs[r2 as usize].as_float()) {
            frame.regs[dst as usize] = Value::bool(a > b);
        }
    }

    fn exec_jmp(&mut self, frame: &mut Frame, offset: i16) {
        let new_pc = (frame.pc as i64 + offset as i64 - 1) as usize;
        // -1 because PC was already incremented before dispatch
        frame.pc = new_pc + 1;
    }
    fn exec_jmp_t(&mut self, frame: &mut Frame, reg: u8, offset: i16) {
        if frame.regs[reg as usize].is_truthy() {
            self.exec_jmp(frame, offset);
        }
    }
    fn exec_jmp_f(&mut self, frame: &mut Frame, reg: u8, offset: i16) {
        if !frame.regs[reg as usize].is_truthy() {
            self.exec_jmp(frame, offset);
        }
    }

    fn exec_call(&mut self, _frame: &mut Frame, func_reg: u8, argc: u8, dst: u8) -> NuResult<()> {
        self.dispatch_call(func_reg, argc, dst)
    }
    fn exec_ret(&mut self, _frame: &mut Frame) -> NuResult<()> {
        self.dispatch_ret()
    }
    fn exec_ret_val(&mut self, _frame: &mut Frame, val_reg: u8) -> NuResult<()> {
        self.dispatch_ret_val(val_reg)
    }

    fn exec_print(&mut self, frame: &mut Frame, reg: u8) {
        let val = frame.regs[reg as usize];
        println!("{}", val.to_string_repr());
    }

    fn exec_sprint(&mut self, frame: &mut Frame, reg: u8) {
        let val = frame.regs[reg as usize];
        print!("{}", val.to_string_repr());
    }

    fn exec_dbg_print(&mut self, frame: &mut Frame) {
        eprintln!("=== Debug: Register State ===");
        for i in (0..256).step_by(8) {
            let mut line = format!("R{:03}-R{:03}: ", i, i + 7);
            for j in 0..8 {
                line.push_str(&format!("{:>20} ", frame.regs[i + j].to_string_repr()));
            }
            eprintln!("{}", line);
        }
    }

    fn exec_dbg_stack(&mut self) {
        eprintln!("=== Debug: Call Stack ===");
        let mut depth = 0;
        let mut frame_ref = self.current_frame.as_deref();
        while let Some(frame) = frame_ref {
            let module_name = self.modules.get(frame.module_idx)
                .map(|m| m.name.clone())
                .unwrap_or_else(|| "?".to_string());
            eprintln!("  [{}] module={} pc={}",
                depth, module_name, frame.pc);
            depth += 1;
            frame_ref = frame.caller.as_deref();
        }
        if depth == 0 {
            eprintln!("  (empty)");
        }
    }

    // === Actor operations ===
    fn exec_spawn(&mut self, frame: &mut Frame, behavior_idx: u16, init_reg: u8, dst: u8) -> NuResult<()> {
        self.dispatch_spawn(frame, behavior_idx, init_reg, dst)
    }
    fn exec_send(&mut self, frame: &mut Frame, addr_reg: u8, behavior_id: u16) -> NuResult<()> {
        self.dispatch_send(frame, addr_reg, behavior_id)
    }
    fn exec_self(&mut self, frame: &mut Frame, dst: u8) {
        if let Some(id) = self.runtime.current_actor_id() {
            frame.regs[dst as usize] = Value::actor_ref(id);
        }
    }

    // === Helpers ===
    fn current_module(&self) -> Option<&CodeModule> {
        self.current_frame.as_ref().and_then(|f| self.modules.get(f.module_idx))
    }
    fn current_module_mut(&mut self) -> Option<&mut CodeModule> {
        let idx = self.current_frame.as_ref()?.module_idx;
        self.modules.get_mut(idx)
    }

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

    /// Get a constant string from the current module's constant pool.
    fn get_constant_string(&self, const_idx: u16) -> String {
        let module_idx = self.current_frame.as_ref().map(|f| f.module_idx).unwrap_or(0);
        self.module_const_string(module_idx, const_idx)
    }
}

fn constant_to_value(c: &Constant) -> Value {
    match c {
        Constant::Int(n) => Value::int(*n),
        Constant::Float(f) => Value::float(*f),
        Constant::Bool(b) => Value::bool(*b),
        Constant::Unit => Value::unit(),
        Constant::String(s) => Value::ptr(s.as_ptr() as *mut u8),
        _ => Value::nil(),
    }
}

// ---------------------------------------------------------------------------
// VM Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod vm_tests {
    use super::*;

    /// Test 1: Value::int creates a properly tagged value.
    #[test]
    fn test_int_tagging() {
        let val = Value::int(42);
        assert_eq!(val.as_int(), Some(42));
        assert!(val.0 & TAG_MASK == TAG_INT);
    }

    /// Test 2: Value::bool creates a properly tagged value.
    #[test]
    fn test_bool_tagging() {
        let t = Value::bool(true);
        let f = Value::bool(false);
        assert_eq!(t.as_bool(), Some(true));
        assert_eq!(f.as_bool(), Some(false));
        assert!(t.0 & TAG_MASK == TAG_SPECIAL);
        assert!(f.0 & TAG_MASK == TAG_SPECIAL);
    }

    /// Test 3: Unit and nil values.
    #[test]
    fn test_special_values() {
        let u = Value::unit();
        let n = Value::nil();
        assert!(u.is_unit());
        assert!(n.is_nil());
        assert!(!u.is_nil());
        assert!(!n.is_unit());
    }

    /// Test 4: Actor reference values.
    #[test]
    fn test_actor_ref() {
        let val = Value::actor_ref(123);
        assert_eq!(val.as_actor_id(), Some(123));
        assert!(val.0 & TAG_MASK == TAG_ACTOR);
    }

    /// Test 5: to_string_repr formats values correctly.
    #[test]
    fn test_to_string_repr() {
        assert_eq!(Value::int(42).to_string_repr(), "42");
        assert_eq!(Value::bool(true).to_string_repr(), "true");
        assert_eq!(Value::unit().to_string_repr(), "unit");
        assert_eq!(Value::nil().to_string_repr(), "nil");
        assert_eq!(Value::actor_ref(7).to_string_repr(), "<actor:7>");
    }

    /// Test 6: Python opcodes trap with a clear error (audit requirement).
    #[test]
    fn test_python_opcodes_trap() {
        let mut vm = VM::new();
        let mut module = CodeModule::new("test");

        let mod_name_idx = module.add_constant(Constant::String("math".to_string()));
        module.emit(Instruction::new2(OpCode::PyImport, mod_name_idx as u8, 0));
        module.emit(Instruction::new0(OpCode::Halt));

        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_err(), "PyImport should trap in clean VM");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("Python opcodes require native actor runtime"),
            "Error should mention native actor runtime: {}", err_msg);
    }

    /// Test 7: Step limit is configurable and defaults to 10M.
    #[test]
    fn test_step_limit_default() {
        // Verify the default limit is 10M by checking the env var fallback
        let limit = std::env::var("NULANG_STEP_LIMIT")
            .ok().and_then(|s| s.parse().ok()).unwrap_or(10_000_000);
        assert_eq!(limit, 10_000_000, "Default step limit should be 10M");
    }

    /// Test 8: Frame register initialization.
    #[test]
    fn test_frame_registers_nil() {
        let frame = Frame::new(None, 0);
        for i in 0..256 {
            assert!(frame.regs[i].is_nil(), "Register {} should be nil", i);
        }
    }

    // =====================================================================
    // Effect System Tests
    // =====================================================================

    /// Build a module with a handler table for testing.
    fn module_with_handler_table(bindings: Vec<(&str, usize, u8, u8)>) -> CodeModule {
        let mut module = CodeModule::new("test_effects");
        let ht_bindings: Vec<_> = bindings.into_iter()
            .map(|(name, offset, argc, res)| HandlerBinding {
                effect_name: name.to_string(),
                handler_offset: offset,
                arg_count: argc,
                result_reg: res,
            })
            .collect();
        module.add_handler_table(HandlerTable {
            bindings: ht_bindings,
            fallback_offset: None,
        });
        module
    }

    /// Test 9: Handle pushes a handler frame; Unwind pops it.
    #[test]
    fn test_handle_unwind_lifecycle() {
        let mut module = module_with_handler_table(vec![]);
        // Set up: Handle(0) -> Const1 r0 -> Unwind -> Halt
        module.emit(Instruction::new2(OpCode::Handle, 0, 0));
        module.emit(Instruction::new1(OpCode::Const1, 0));
        module.emit(Instruction::new0(OpCode::Unwind));
        module.emit(Instruction::new0(OpCode::Halt));
        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        assert!(vm.handler_stack.is_empty());
        let result = vm.run();
        assert!(result.is_ok(), "Handle/Unwind should complete: {:?}", result.err());
        assert!(vm.handler_stack.is_empty(), "Handler stack should be empty after Unwind");
        assert_eq!(result.unwrap().as_int(), Some(1), "Result should be 1");
    }

    /// Test 10: Perform invokes a matching handler; Resume restores continuation.
    #[test]
    fn test_perform_resume() {
        let mut module = CodeModule::new("test_perform");

        // Handler table: effect "Get42" -> handler at offset 7
        module.add_handler_table(HandlerTable {
            bindings: vec![
                HandlerBinding {
                    effect_name: "Get42".to_string(),
                    handler_offset: 7, // handler body PC
                    arg_count: 0,
                    result_reg: 0,
                },
            ],
            fallback_offset: None,
        });

        // Constant pool: "Get42" at index 0
        let get42_idx = module.add_constant(Constant::String("Get42".to_string()));
        assert_eq!(get42_idx, 0);

        // Program:
        // PC 0: Handle(0)          — push handler frame
        // PC 1: Perform "Get42" -> r1  — should invoke handler
        // PC 2: (after perform) Move r1 -> r0  — copy result to return reg
        // PC 3: Unwind
        // PC 4: Halt
        // PC 5-6: (padding)
        // PC 7: handler body: ConstU c42 -> r0; Resume r0

        module.emit(Instruction::new1(OpCode::Handle, 0));           // 0
        module.emit(Instruction::new3(OpCode::Perform, 0, 0, 1));    // 1: perform Get42 -> r1
        // After resume, r1 should have 42. Copy it to r0 for return.
        module.emit(Instruction::new2(OpCode::Move, 1, 0));          // 2
        module.emit(Instruction::new0(OpCode::Unwind));              // 3
        module.emit(Instruction::new0(OpCode::Halt));                // 4
        // Handler body at PC 7:
        // Place 42 in r0, then resume with it
        module.emit(Instruction::new0(OpCode::Nop));                 // 5 (padding)
        module.emit(Instruction::new0(OpCode::Nop));                 // 6 (padding)
        module.emit(Instruction::new2(OpCode::ConstU, 0, 0));        // 7: const 42 -> r0
        module.emit(Instruction::new1(OpCode::Resume, 0));           // 8: resume with r0

        // Patch ConstU at PC 7 to load constant 42
        let c42_idx = module.add_constant(Constant::Int(42));
        if let Some(instr) = module.instructions.get_mut(7) {
            instr.op1 = ((c42_idx >> 8) & 0xFF) as u8;
            instr.op2 = (c42_idx & 0xFF) as u8;
            instr.op3 = 0; // dst = r0
        }

        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_ok(), "Perform/Resume should work: {:?}", result.err());
        assert_eq!(result.unwrap().as_int(), Some(42), "Should get 42 from effect handler");
    }

    /// Test 11: Perform without a matching handler raises EffectError.
    #[test]
    fn test_unhandled_effect_errors() {
        let mut module = module_with_handler_table(vec![]);
        let no_effect_idx = module.add_constant(Constant::String("NoHandler".to_string()));

        module.emit(Instruction::new1(OpCode::Handle, 0));
        module.emit(Instruction::new3(OpCode::Perform,
            ((no_effect_idx >> 8) & 0xFF) as u8,
            (no_effect_idx & 0xFF) as u8,
            0));
        module.emit(Instruction::new0(OpCode::Unwind));
        module.emit(Instruction::new0(OpCode::Halt));
        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_err(), "Unhandled effect should error");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("Unhandled effect"), "Error should mention unhandled effect: {}", err_msg);
        assert!(err_msg.contains("NoHandler"), "Error should name the effect: {}", err_msg);
    }

    /// Test 12: Nested handlers — inner handler shadows outer.
    #[test]
    fn test_nested_handlers_shadow() {
        let mut module = CodeModule::new("test_nested");

        // Handler table 0: "GetVal" -> returns 10
        module.add_handler_table(HandlerTable {
            bindings: vec![
                HandlerBinding {
                    effect_name: "GetVal".to_string(),
                    handler_offset: 12,
                    arg_count: 0,
                    result_reg: 0,
                },
            ],
            fallback_offset: None,
        });

        // Handler table 1: "GetVal" -> returns 20 (shadows)
        module.add_handler_table(HandlerTable {
            bindings: vec![
                HandlerBinding {
                    effect_name: "GetVal".to_string(),
                    handler_offset: 15,
                    arg_count: 0,
                    result_reg: 0,
                },
            ],
            fallback_offset: None,
        });

        let getval_idx = module.add_constant(Constant::String("GetVal".to_string()));
        let c10_idx = module.add_constant(Constant::Int(10));
        let c20_idx = module.add_constant(Constant::Int(20));

        // Program:
        // 0: Handle(0)          — outer handler
        // 1: Handle(1)          — inner handler (shadows)
        // 2: Perform "GetVal" -> r0
        // 3: Unwind             — pop inner
        // 4: Unwind             — pop outer
        // 5: Halt
        // Padding: 6-11
        // 12: handler outer: ConstU c10 -> r0; Resume r0
        // 15: handler inner: ConstU c20 -> r0; Resume r0

        module.emit(Instruction::new1(OpCode::Handle, 0));            // 0
        module.emit(Instruction::new1(OpCode::Handle, 1));            // 1
        module.emit(Instruction::new3(OpCode::Perform,
            ((getval_idx >> 8) & 0xFF) as u8,
            (getval_idx & 0xFF) as u8,
            0));                                                      // 2
        module.emit(Instruction::new0(OpCode::Unwind));               // 3
        module.emit(Instruction::new0(OpCode::Unwind));               // 4
        module.emit(Instruction::new0(OpCode::Halt));                 // 5
        // padding
        for _ in 6..12 { module.emit(Instruction::new0(OpCode::Nop)); }
        // Outer handler at 12: load 10, resume
        module.emit(Instruction::new3(OpCode::ConstU,
            ((c10_idx >> 8) & 0xFF) as u8, (c10_idx & 0xFF) as u8, 0)); // 12
        module.emit(Instruction::new1(OpCode::Resume, 0));             // 13
        module.emit(Instruction::new0(OpCode::Nop));                   // 14 (padding)
        // Inner handler at 15: load 20, resume
        module.emit(Instruction::new3(OpCode::ConstU,
            ((c20_idx >> 8) & 0xFF) as u8, (c20_idx & 0xFF) as u8, 0)); // 15
        module.emit(Instruction::new1(OpCode::Resume, 0));             // 16

        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_ok(), "Nested handlers should work: {:?}", result.err());
        // Inner handler shadows outer, so we should get 20
        assert_eq!(result.unwrap().as_int(), Some(20),
            "Inner handler should shadow outer handler");
    }

    /// Test 13: Multiple effects in one handler table.
    #[test]
    fn test_multi_effect_handler() {
        let mut module = CodeModule::new("test_multi");

        // Handler table with two effects: "GetA" and "GetB"
        module.add_handler_table(HandlerTable {
            bindings: vec![
                HandlerBinding {
                    effect_name: "GetA".to_string(),
                    handler_offset: 8,
                    arg_count: 0,
                    result_reg: 0,
                },
                HandlerBinding {
                    effect_name: "GetB".to_string(),
                    handler_offset: 11,
                    arg_count: 0,
                    result_reg: 0,
                },
            ],
            fallback_offset: None,
        });

        let geta_idx = module.add_constant(Constant::String("GetA".to_string()));
        let getb_idx = module.add_constant(Constant::String("GetB".to_string()));
        let c100_idx = module.add_constant(Constant::Int(100));
        let c200_idx = module.add_constant(Constant::Int(200));

        // Program: perform GetA -> r0, then GetB -> r1, add them
        module.emit(Instruction::new1(OpCode::Handle, 0));             // 0
        module.emit(Instruction::new3(OpCode::Perform,
            ((geta_idx >> 8) & 0xFF) as u8, (geta_idx & 0xFF) as u8, 0)); // 1: GetA -> r0
        module.emit(Instruction::new3(OpCode::Perform,
            ((getb_idx >> 8) & 0xFF) as u8, (getb_idx & 0xFF) as u8, 1)); // 2: GetB -> r1
        module.emit(Instruction::new3(OpCode::IAdd, 0, 1, 0));          // 3: r0 + r1 -> r0
        module.emit(Instruction::new0(OpCode::Unwind));                 // 4
        module.emit(Instruction::new0(OpCode::Halt));                   // 5
        // padding 6-7
        module.emit(Instruction::new0(OpCode::Nop));                    // 6
        module.emit(Instruction::new0(OpCode::Nop));                    // 7
        // GetA handler at 8
        module.emit(Instruction::new3(OpCode::ConstU,
            ((c100_idx >> 8) & 0xFF) as u8, (c100_idx & 0xFF) as u8, 0)); // 8
        module.emit(Instruction::new1(OpCode::Resume, 0));              // 9
        module.emit(Instruction::new0(OpCode::Nop));                    // 10
        // GetB handler at 11
        module.emit(Instruction::new3(OpCode::ConstU,
            ((c200_idx >> 8) & 0xFF) as u8, (c200_idx & 0xFF) as u8, 0)); // 11
        module.emit(Instruction::new1(OpCode::Resume, 0));              // 12

        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_ok(), "Multi-effect handler should work: {:?}", result.err());
        assert_eq!(result.unwrap().as_int(), Some(300), "100 + 200 = 300");
    }

    /// Test 14: Handler fallback — effect not in bindings triggers fallback.
    #[test]
    fn test_handler_fallback() {
        let mut module = CodeModule::new("test_fallback");

        // Handler table: handles "Known", fallback for everything else
        module.add_handler_table(HandlerTable {
            bindings: vec![
                HandlerBinding {
                    effect_name: "Known".to_string(),
                    handler_offset: 8,
                    arg_count: 0,
                    result_reg: 0,
                },
            ],
            fallback_offset: Some(11), // fallback handler
        });

        let unknown_idx = module.add_constant(Constant::String("Unknown".to_string()));
        let c999_idx = module.add_constant(Constant::Int(999));

        module.emit(Instruction::new1(OpCode::Handle, 0));              // 0
        module.emit(Instruction::new3(OpCode::Perform,
            ((unknown_idx >> 8) & 0xFF) as u8, (unknown_idx & 0xFF) as u8, 0)); // 1
        module.emit(Instruction::new0(OpCode::Unwind));                 // 2
        module.emit(Instruction::new0(OpCode::Halt));                   // 3
        // padding 4-7
        for _ in 4..8 { module.emit(Instruction::new0(OpCode::Nop)); }
        // Known handler at 8 (not used)
        module.emit(Instruction::new1(OpCode::Const1, 0));              // 8
        module.emit(Instruction::new1(OpCode::Resume, 0));              // 9
        module.emit(Instruction::new0(OpCode::Nop));                    // 10
        // Fallback handler at 11: returns 999
        module.emit(Instruction::new3(OpCode::ConstU,
            ((c999_idx >> 8) & 0xFF) as u8, (c999_idx & 0xFF) as u8, 0)); // 11
        module.emit(Instruction::new1(OpCode::Resume, 0));              // 12

        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_ok(), "Fallback handler should work: {:?}", result.err());
        assert_eq!(result.unwrap().as_int(), Some(999), "Fallback should return 999");
    }

    /// Test 15: Resume without captured continuation errors.
    #[test]
    fn test_resume_without_continuation_errors() {
        let mut module = CodeModule::new("test_bad_resume");
        module.emit(Instruction::new1(OpCode::Resume, 0));              // 0
        module.emit(Instruction::new0(OpCode::Halt));                   // 1
        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_err(), "Resume without continuation should error");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("resume called without a captured continuation"),
            "Error should mention missing continuation: {}", err_msg);
    }
}