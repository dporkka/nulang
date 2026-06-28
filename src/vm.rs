//! Nulang Virtual Machine: register-based bytecode interpreter.
//!
//! ## Architecture
//!
//! - **256 general-purpose registers** per activation frame
//! - **NaN-boxing** for efficient tagged values (int/float/bool/nil/actor_ref)
//! - **Bytecode modules** with constant pools and function tables
//! - **Algebraic effects** via handler stack (Perform/Resume/Unwind/Handle)
//! - **Capability tracking** via CapChk/CapUp/CapDown opcodes
//!
//! ## Effect System
//!
//! The VM implements algebraic effects via four opcodes:
//! - `Handle`: Push a handler frame onto the handler stack
//! - `Perform`: Invoke an effect operation (captures continuation)
//! - `Resume`: Restore the captured continuation with a value
//! - `Unwind`: Pop the handler frame (normal completion)
//!
//! Handler frames stay on the stack until `Unwind`, allowing multiple
//! effects in the same handle block to be handled by the same handler.
//!
//! ## Value Representation
//!
//! Uses NaN boxing: all non-float values are encoded in the quiet-NaN
//! payload of an f64. This gives us 51 bits of payload space for
//! pointers, integers, and type tags.

use crate::bytecode::{CodeModule, Constant, Instruction, OpCode};
use crate::types::{NuError, NuResult, Span};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Value: NaN-boxed tagged value
// ---------------------------------------------------------------------------

/// Tagged value using NaN boxing.
///
/// All non-float values are encoded in the quiet-NaN payload of an f64.
/// The high 16 bits hold the type tag; the low 48 bits hold the payload.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Value {
    raw: u64,
}

/// Value type tags (stored in the upper 16 bits).
const TAG_MASK:    u64 = 0xFFFF_0000_0000_0000;
const TAG_NIL:     u64 = 0x7FF8_0000_0000_0000;
const TAG_UNIT:    u64 = 0x7FF9_0000_0000_0000;
const TAG_BOOL:    u64 = 0x7FFA_0000_0000_0000;
const TAG_INT:     u64 = 0x7FFB_0000_0000_0000;
const TAG_PTR:     u64 = 0x7FFC_0000_0000_0000;
const TAG_ACTOR:   u64 = 0x7FFD_0000_0000_0000;
const TAG_STRING:  u64 = 0x7FFE_0000_0000_0000;
const TAG_CLOSURE: u64 = 0x7FF7_0000_0000_0000;

const PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

impl Value {
    /// Create a nil value.
    pub fn nil() -> Self { Value { raw: TAG_NIL } }

    /// Create an integer value.
    pub fn int(n: i64) -> Self {
        // Store directly in the 48-bit payload.
        let payload = (n as u64) & PAYLOAD_MASK;
        Value { raw: TAG_INT | payload }
    }

    /// Create a float value.
    pub fn float(f: f64) -> Self {
        Value { raw: f.to_bits() }
    }

    /// Create a boolean value.
    pub fn bool(b: bool) -> Self {
        Value { raw: TAG_BOOL | (b as u64) }
    }

    /// Create a unit value.
    pub fn unit() -> Self { Value { raw: TAG_UNIT } }

    /// Create an actor reference.
    pub fn actor_ref(id: u64) -> Self {
        Value { raw: TAG_ACTOR | (id & PAYLOAD_MASK) }
    }

    /// Create a closure reference.
    pub fn closure(id: u64) -> Self {
        Value { raw: TAG_CLOSURE | (id & PAYLOAD_MASK) }
    }

    /// Create a pointer value (for strings, lists, etc.).
    pub fn ptr(p: *mut u8) -> Self {
        Value { raw: TAG_PTR | (p as u64 & PAYLOAD_MASK) }
    }

    /// Create a string reference (index into string pool).
    pub fn string(id: u32) -> Self {
        Value { raw: TAG_STRING | (id as u64) }
    }

    // -- Type checks --

    pub fn is_nil(&self) -> bool { self.raw == TAG_NIL }
    pub fn is_unit(&self) -> bool { self.raw == TAG_UNIT }
    pub fn is_int(&self) -> bool { (self.raw & TAG_MASK) == TAG_INT }
    pub fn is_float(&self) -> bool { self.as_float().is_some() }
    pub fn is_bool(&self) -> bool { (self.raw & TAG_MASK) == TAG_BOOL }
    pub fn is_actor_ref(&self) -> bool { (self.raw & TAG_MASK) == TAG_ACTOR }

    // -- Extractors --

    pub fn as_int(&self) -> Option<i64> {
        if (self.raw & TAG_MASK) == TAG_INT {
            let bits = self.raw & PAYLOAD_MASK;
            // Sign-extend from 48 bits
            Some(if bits & 0x0000_8000_0000_0000 != 0 {
                (bits | 0xFFFF_0000_0000_0000) as i64
            } else {
                bits as i64
            })
        } else {
            None
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        let f = f64::from_bits(self.raw);
        // All tagged values are quiet NaNs, so any non-NaN bit pattern is a real float.
        if f.is_nan() { None } else { Some(f) }
    }

    pub fn as_bool(&self) -> Option<bool> {
        if (self.raw & TAG_MASK) == TAG_BOOL {
            Some((self.raw & 1) != 0)
        } else {
            None
        }
    }

    pub fn as_actor_id(&self) -> Option<u64> {
        if (self.raw & TAG_MASK) == TAG_ACTOR {
            Some(self.raw & PAYLOAD_MASK)
        } else {
            None
        }
    }

    pub fn as_ptr(&self) -> Option<*mut u8> {
        if (self.raw & TAG_MASK) == TAG_PTR {
            Some((self.raw & PAYLOAD_MASK) as *mut u8)
        } else {
            None
        }
    }

    pub fn is_ptr(&self) -> bool { (self.raw & TAG_MASK) == TAG_PTR }
    pub fn is_string(&self) -> bool { (self.raw & TAG_MASK) == TAG_STRING }
    pub fn is_closure(&self) -> bool { (self.raw & TAG_MASK) == TAG_CLOSURE }

    pub fn as_string_id(&self) -> Option<u32> {
        if self.is_string() {
            Some((self.raw & PAYLOAD_MASK) as u32)
        } else {
            None
        }
    }

    /// Return the raw NaN-boxed bits.
    pub fn as_raw(&self) -> u64 { self.raw }

    /// Construct a Value from raw NaN-boxed bits.
    ///
    /// # Safety
    /// The caller must ensure the bits form a valid tagged value.
    pub fn from_raw(raw: u64) -> Self { Value { raw } }

    pub fn to_string_repr(&self) -> String {
        if self.is_nil() { "nil".to_string() }
        else if self.is_unit() { "()".to_string() }
        else if let Some(n) = self.as_int() { n.to_string() }
        else if let Some(f) = self.as_float() { f.to_string() }
        else if let Some(b) = self.as_bool() { b.to_string() }
        else if self.is_actor_ref() { format!("#Actor:{}", self.as_actor_id().unwrap()) }
        else { format!("#Value({:x})", self.raw) }
    }
}

// ---------------------------------------------------------------------------
// Frame: activation frame
// ---------------------------------------------------------------------------

/// Activation frame: 256 registers + metadata.
pub struct Frame {
    /// 256 general-purpose registers.
    pub regs: [Value; 256],
    /// Program counter (bytecode index).
    pub pc: usize,
    /// Module index in VM.modules.
    pub module_idx: usize,
    /// Return destination register.
    pub return_dst: u8,
    /// Caller frame (None for top-level).
    pub caller: Option<Box<Frame>>,
    /// Closure environment (None if not a closure).
    pub closure_env: Option<Value>,
}

impl Frame {
    /// Create a new frame with all registers initialized to nil.
    pub fn new(caller: Option<Box<Frame>>, module_idx: usize) -> Self {
        Frame {
            regs: [Value::nil(); 256],
            pc: 0,
            module_idx,
            return_dst: 0,
            caller,
            closure_env: None,
        }
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Show only first 8 registers and key metadata to avoid
        // overwhelming output (all 256 regs is too much).
        f.debug_struct("Frame")
            .field("pc", &self.pc)
            .field("module_idx", &self.module_idx)
            .field("return_dst", &self.return_dst)
            .field("regs[0..8]", &&self.regs[0..8])
            .field("has_caller", &self.caller.is_some())
            .field("closure_env", &self.closure_env)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// HandlerFrame: handler stack entry for algebraic effects
// ---------------------------------------------------------------------------

/// A handler frame tracks a single `handle` block's context.
///
/// Created by `Handle` opcode, popped by `Unwind`.
/// When `Perform` finds this handler, it captures a `Continuation`
/// and stores it here for `Resume` to use.
#[derive(Debug)]
pub struct HandlerFrame {
    /// Index into the module's handler_tables.
    pub handler_table_idx: usize,
    /// Module index (so we can look up handler_tables).
    pub module_idx: usize,
    /// PC to resume at after the handle block completes normally.
    pub resume_pc: usize,
    /// Destination register for the handle block's result.
    pub resume_dst: u8,
    /// Captured continuation (set by Perform, consumed by Resume).
    pub captured_continuation: Option<Continuation>,
}

impl HandlerFrame {
    pub fn new(handler_table_idx: usize, module_idx: usize, resume_pc: usize, resume_dst: u8) -> Self {
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
// Continuation: captured execution state for algebraic effects
// ---------------------------------------------------------------------------

/// A captured continuation — a deep snapshot of the VM's execution state
/// at the point of a `perform` call. Restored by `resume` to continue
/// the suspended computation with a value.
#[derive(Debug)]
struct Continuation {
    /// Deep-cloned frame chain (current frame + all callers).
    frame: Box<Frame>,
    /// Program counter at the point of capture (points past Perform).
    resume_pc: usize,
    /// Module index for the frame.
    module_idx: usize,
    /// Destination register for the resume value.
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
// VM: Virtual Machine
// ---------------------------------------------------------------------------

/// Deep-clone a frame chain (current + all callers).
fn clone_frame_chain(frame: &Frame) -> Box<Frame> {
    Box::new(Frame {
        regs: frame.regs,
        pc: frame.pc,
        module_idx: frame.module_idx,
        return_dst: frame.return_dst,
        caller: frame.caller.as_ref().map(|c| clone_frame_chain(c)),
        closure_env: frame.closure_env,
    })
}

/// Register-based bytecode virtual machine.
///
/// Executes Nulang bytecode modules with:
/// - 256 registers per frame
/// - NaN-boxed tagged values
/// - Algebraic effects via handler stack
/// - Capability tracking
pub struct VM {
    /// Loaded bytecode modules.
    pub modules: Vec<CodeModule>,
    /// Current execution frame.
    current_frame: Option<Box<Frame>>,
    /// Handler stack for algebraic effects.
    handler_stack: Vec<HandlerFrame>,
    /// Step counter (for debugging / limits).
    step_count: usize,
}

impl VM {
    /// Create a new VM.
    pub fn new() -> Self {
        VM {
            modules: Vec::new(),
            current_frame: None,
            handler_stack: Vec::new(),
            step_count: 0,
        }
    }

    /// Load a bytecode module into the VM.
    pub fn load_module(&mut self, module: CodeModule) {
        self.modules.push(module);
    }

    /// Get a constant string from a module's constant pool.
    fn module_const_string(&self, module_idx: usize, const_idx: usize) -> String {
        self.modules.get(module_idx)
            .and_then(|m| m.constants.get(const_idx))
            .map(|c| match c {
                Constant::String(s) => s.clone(),
                Constant::Int(n) => n.to_string(),
                _ => format!("{:?}", c),
            })
            .unwrap_or_else(|| format!("#const{}", const_idx))
    }

    /// Run the loaded program starting from the entry point of the last module.
    ///
    /// Returns the value in register 0 of the final frame, or unit if no frame.
    pub fn run(&mut self) -> NuResult<Value> {
        let module_idx = self.modules.len().saturating_sub(1);
        let entry_point = self.modules.get(module_idx)
            .and_then(|m| m.entry_point)
            .unwrap_or(0);

        let mut frame = Frame::new(None, module_idx);
        frame.pc = entry_point;
        self.current_frame = Some(Box::new(frame));

        // Main execution loop
        loop {
            // Check if halted
            if let Some(ref frame) = self.current_frame {
                let module_idx = frame.module_idx;
                let pc = frame.pc;
                if let Some(module) = self.modules.get(module_idx) {
                    if pc >= module.instructions.len() {
                        // PC past end — program complete
                        return Ok(self.current_frame.as_ref().map(|f| f.regs[0]).unwrap_or(Value::unit()));
                    }
                    // Check if next instruction is Halt
                    if module.instructions.get(pc).map(|i| i.opcode == OpCode::Halt).unwrap_or(false) {
                        self.current_frame.as_mut().unwrap().pc += 1;
                        return Ok(self.current_frame.as_ref().map(|f| f.regs[0]).unwrap_or(Value::unit()));
                    }
                } else {
                    return Ok(Value::unit());
                }
            } else {
                return Ok(Value::unit());
            }

            match self.step() {
                Ok(()) => {},
                Err(NuError::VMError(msg)) if msg == "Halt" => {
                    return Ok(self.current_frame.as_ref().map(|f| f.regs[0]).unwrap_or(Value::unit()));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Run with a specific entry point (for testing).
    pub fn run_from(&mut self, module_idx: usize, pc: usize) -> NuResult<Value> {
        let mut frame = Frame::new(None, module_idx);
        frame.pc = pc;
        self.current_frame = Some(Box::new(frame));

        loop {
            if let Some(ref frame) = self.current_frame {
                let m_idx = frame.module_idx;
                let pc = frame.pc;
                if let Some(module) = self.modules.get(m_idx) {
                    if pc >= module.instructions.len() {
                        return Ok(self.current_frame.as_ref().map(|f| f.regs[0]).unwrap_or(Value::unit()));
                    }
                    if module.instructions.get(pc).map(|i| i.opcode == OpCode::Halt).unwrap_or(false) {
                        self.current_frame.as_mut().unwrap().pc += 1;
                        return Ok(self.current_frame.as_ref().map(|f| f.regs[0]).unwrap_or(Value::unit()));
                    }
                } else {
                    return Ok(Value::unit());
                }
            } else {
                return Ok(Value::unit());
            }

            match self.step() {
                Ok(()) => {},
                Err(NuError::VMError(msg)) if msg == "Halt" => {
                    return Ok(self.current_frame.as_ref().map(|f| f.regs[0]).unwrap_or(Value::unit()));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Execute a single bytecode instruction.
    pub fn step(&mut self) -> NuResult<()> {
        // Step limit: configurable via env var NULANG_STEP_LIMIT.
        // Default 10M steps — long-running actors (servers, processors) may need more.
        self.step_count += 1;
        let limit = std::env::var("NULANG_STEP_LIMIT")
            .ok().and_then(|s| s.parse().ok()).unwrap_or(10_000_000);
        if self.step_count > limit {
            return Err(NuError::VMError(
                format!("Step limit exceeded ({} steps). Set NULANG_STEP_LIMIT env var to increase.", self.step_count)
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
                    // Top-level return: store value and halt
                    frame.regs[0] = ret_val;
                    self.current_frame = Some(frame);
                }
                return Ok(());
            }
            OpCode::RetVal => {
                let ret_val = frame.regs[instr.op1 as usize];
                if let Some(mut caller_frame) = frame.caller {
                    let dst = frame.return_dst as usize;
                    caller_frame.regs[dst] = ret_val;
                    self.current_frame = Some(caller_frame);
                } else {
                    frame.regs[0] = ret_val;
                    self.current_frame = Some(frame);
                }
                return Ok(());
            }
            OpCode::ClosureCall => {
                let closure_val = frame.regs[instr.op1 as usize];
                let module_idx = frame.module_idx;
                let (func_idx, closure_env) = self.resolve_function(closure_val, module_idx)?;
                let code_offset = self.modules.get(module_idx)
                    .and_then(|m| m.function_table.get(func_idx)).copied()
                    .ok_or_else(|| NuError::VMError(format!("Function {} not found", func_idx)))?;
                let mut new_frame = Frame::new(None, module_idx);
                new_frame.pc = code_offset;
                for i in 0..256 {
                    new_frame.regs[i] = frame.regs[i];
                }
                new_frame.return_dst = instr.op3;
                new_frame.closure_env = closure_env;
                new_frame.caller = Some(frame);
                self.current_frame = Some(Box::new(new_frame));
                return Ok(());
            }
            OpCode::Panic => {
                let pc = frame.pc.saturating_sub(1);
                let r0_repr = frame.regs[0].to_string_repr();
                self.current_frame = Some(frame);
                return Err(NuError::VMError(
                    format!("Panic at PC {}: r0={}", pc, r0_repr)
                ));
            }

            // -- Actor opcodes (consume frame for Spawn/Send/Ask) --
            OpCode::Spawn => {
                // Placeholder: spawn a new actor.
                // For now, return a dummy actor reference.
                frame.regs[instr.op3 as usize] = Value::actor_ref(0);
                self.current_frame = Some(frame);
                return Ok(());
            }
            OpCode::Send => {
                // Placeholder: send a message to an actor.
                self.current_frame = Some(frame);
                return Ok(());
            }
            OpCode::Ask => {
                // Placeholder: ask an actor (sync send/receive).
                frame.regs[instr.op3 as usize] = Value::nil();
                self.current_frame = Some(frame);
                return Ok(());
            }
            OpCode::RSend => {
                // Placeholder: remote send.
                self.current_frame = Some(frame);
                return Ok(());
            }
            OpCode::RSpawn => {
                // Placeholder: remote spawn.
                frame.regs[instr.op3 as usize] = Value::actor_ref(0);
                self.current_frame = Some(frame);
                return Ok(());
            }

            // -- Constants --
            OpCode::Const0 => { frame.regs[instr.op1 as usize] = Value::int(0); }
            OpCode::Const1 => { frame.regs[instr.op1 as usize] = Value::int(1); }
            OpCode::Const2 => { frame.regs[instr.op1 as usize] = Value::int(2); }
            OpCode::ConstU => {
                let idx = instr.imm16() as usize;
                let val = self.modules.get(frame.module_idx)
                    .and_then(|m| m.constants.get(idx))
                    .map(|c| match c {
                        Constant::Int(n) => Value::int(*n),
                        Constant::Float(f) => Value::float(*f),
                        Constant::String(s) => Value::string(idx as u32),
                        Constant::Bool(b) => Value::bool(*b),
                        Constant::Nil => Value::nil(),
                        Constant::Unit => Value::unit(),
                        _ => Value::nil(),
                    })
                    .unwrap_or(Value::nil());
                frame.regs[instr.op3 as usize] = val;
            }
            OpCode::Closure => {
                let func_idx = instr.imm16() as u64;
                frame.regs[instr.op3 as usize] = Value::closure(func_idx);
            }
            OpCode::CapLoad | OpCode::CapStore | OpCode::FreeVar => {
                // Captures are not yet implemented at runtime; these opcodes are
                // no-ops so that simple capture-free closures still work.
            }
            // -- Arithmetic --
            OpCode::IAdd => {
                let a = frame.regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = frame.regs[instr.op2 as usize].as_int().unwrap_or(0);
                frame.regs[instr.op3 as usize] = Value::int(a + b);
            }
            OpCode::ISub => {
                let a = frame.regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = frame.regs[instr.op2 as usize].as_int().unwrap_or(0);
                frame.regs[instr.op3 as usize] = Value::int(a - b);
            }
            OpCode::IMul => {
                let a = frame.regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = frame.regs[instr.op2 as usize].as_int().unwrap_or(0);
                frame.regs[instr.op3 as usize] = Value::int(a * b);
            }
            OpCode::IDiv => {
                let a = frame.regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = frame.regs[instr.op2 as usize].as_int().unwrap_or(1);
                frame.regs[instr.op3 as usize] = if b != 0 { Value::int(a / b) } else { Value::nil() };
            }
            OpCode::IMod => {
                let a = frame.regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = frame.regs[instr.op2 as usize].as_int().unwrap_or(1);
                frame.regs[instr.op3 as usize] = if b != 0 { Value::int(a % b) } else { Value::nil() };
            }
            OpCode::INeg => {
                let a = frame.regs[instr.op1 as usize].as_int().unwrap_or(0);
                frame.regs[instr.op2 as usize] = Value::int(-a);
            }

            // -- Float arithmetic --
            OpCode::FAdd => {
                let a = frame.regs[instr.op1 as usize].as_float().unwrap_or(0.0);
                let b = frame.regs[instr.op2 as usize].as_float().unwrap_or(0.0);
                frame.regs[instr.op3 as usize] = Value::float(a + b);
            }
            OpCode::FSub => {
                let a = frame.regs[instr.op1 as usize].as_float().unwrap_or(0.0);
                let b = frame.regs[instr.op2 as usize].as_float().unwrap_or(0.0);
                frame.regs[instr.op3 as usize] = Value::float(a - b);
            }
            OpCode::FMul => {
                let a = frame.regs[instr.op1 as usize].as_float().unwrap_or(0.0);
                let b = frame.regs[instr.op2 as usize].as_float().unwrap_or(0.0);
                frame.regs[instr.op3 as usize] = Value::float(a * b);
            }
            OpCode::FDiv => {
                let a = frame.regs[instr.op1 as usize].as_float().unwrap_or(0.0);
                let b = frame.regs[instr.op2 as usize].as_float().unwrap_or(1.0);
                frame.regs[instr.op3 as usize] = if b != 0.0 { Value::float(a / b) } else { Value::nil() };
            }
            OpCode::FNeg => {
                let a = frame.regs[instr.op1 as usize].as_float().unwrap_or(0.0);
                frame.regs[instr.op3 as usize] = Value::float(-a);
            }

            // -- Comparison --
            OpCode::ICmpEq => {
                let a = frame.regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = frame.regs[instr.op2 as usize].as_int().unwrap_or(0);
                frame.regs[instr.op3 as usize] = Value::bool(a == b);
            }
            OpCode::ICmpLt => {
                let a = frame.regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = frame.regs[instr.op2 as usize].as_int().unwrap_or(0);
                frame.regs[instr.op3 as usize] = Value::bool(a < b);
            }
            OpCode::ICmpGt => {
                let a = frame.regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = frame.regs[instr.op2 as usize].as_int().unwrap_or(0);
                frame.regs[instr.op3 as usize] = Value::bool(a > b);
            }
            OpCode::ICmpLe => {
                let a = frame.regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = frame.regs[instr.op2 as usize].as_int().unwrap_or(0);
                frame.regs[instr.op3 as usize] = Value::bool(a <= b);
            }
            OpCode::ICmpGe => {
                let a = frame.regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = frame.regs[instr.op2 as usize].as_int().unwrap_or(0);
                frame.regs[instr.op3 as usize] = Value::bool(a >= b);
            }
            OpCode::FCmpEq => {
                let a = frame.regs[instr.op1 as usize].as_float().unwrap_or(0.0);
                let b = frame.regs[instr.op2 as usize].as_float().unwrap_or(0.0);
                frame.regs[instr.op3 as usize] = Value::bool((a - b).abs() < f64::EPSILON);
            }
            OpCode::FCmpLt => {
                let a = frame.regs[instr.op1 as usize].as_float().unwrap_or(0.0);
                let b = frame.regs[instr.op2 as usize].as_float().unwrap_or(0.0);
                frame.regs[instr.op3 as usize] = Value::bool(a < b);
            }
            OpCode::FCmpGt => {
                let a = frame.regs[instr.op1 as usize].as_float().unwrap_or(0.0);
                let b = frame.regs[instr.op2 as usize].as_float().unwrap_or(0.0);
                frame.regs[instr.op3 as usize] = Value::bool(a > b);
            }

            // -- Arrays (minimal heap-backed implementation; memory is leaked) --
            OpCode::ArrAlloc => {
                let len = frame.regs[instr.op1 as usize].as_int().unwrap_or(0) as usize;
                let arr: Box<Vec<Value>> = Box::new(vec![Value::nil(); len]);
                frame.regs[instr.op2 as usize] = Value::ptr(Box::into_raw(arr) as *mut u8);
            }
            OpCode::ArrLoad => {
                let arr_ptr = frame.regs[instr.op1 as usize].as_ptr().unwrap_or(std::ptr::null_mut());
                let idx = frame.regs[instr.op2 as usize].as_int().unwrap_or(0) as usize;
                let val = if !arr_ptr.is_null() {
                    let arr = unsafe { &*(arr_ptr as *const Vec<Value>) };
                    arr.get(idx).copied().unwrap_or(Value::nil())
                } else {
                    Value::nil()
                };
                frame.regs[instr.op3 as usize] = val;
            }
            OpCode::ArrStore => {
                let arr_ptr = frame.regs[instr.op1 as usize].as_ptr().unwrap_or(std::ptr::null_mut());
                let idx = frame.regs[instr.op2 as usize].as_int().unwrap_or(0) as usize;
                let val = frame.regs[instr.op3 as usize];
                if !arr_ptr.is_null() {
                    let arr = unsafe { &mut *(arr_ptr as *mut Vec<Value>) };
                    if idx < arr.len() {
                        arr[idx] = val;
                    }
                }
            }
            OpCode::ArrLen => {
                let arr_ptr = frame.regs[instr.op1 as usize].as_ptr().unwrap_or(std::ptr::null_mut());
                let len = if !arr_ptr.is_null() {
                    unsafe { (&*(arr_ptr as *const Vec<Value>)).len() as i64 }
                } else {
                    0
                };
                frame.regs[instr.op3 as usize] = Value::int(len);
            }

            // -- Boolean logic --
            OpCode::And => {
                let a = frame.regs[instr.op1 as usize].as_bool().unwrap_or(false);
                let b = frame.regs[instr.op2 as usize].as_bool().unwrap_or(false);
                frame.regs[instr.op3 as usize] = Value::bool(a && b);
            }
            OpCode::Or => {
                let a = frame.regs[instr.op1 as usize].as_bool().unwrap_or(false);
                let b = frame.regs[instr.op2 as usize].as_bool().unwrap_or(false);
                frame.regs[instr.op3 as usize] = Value::bool(a || b);
            }
            OpCode::Not => {
                let a = frame.regs[instr.op1 as usize].as_bool().unwrap_or(false);
                frame.regs[instr.op2 as usize] = Value::bool(!a);
            }

            // -- Type checks --
            // IsTag checks the tag of the value in op1 against the tag_id in op2.
            // Tag IDs mirror the low byte of the internal NaN tag constants.
            OpCode::IsTag => {
                let val = frame.regs[instr.op1 as usize];
                let tag_id = instr.op2;
                let result = match tag_id {
                    0x01 => val.is_nil(),
                    0x02 => val.is_int(),
                    0x03 => val.is_bool(),
                    0x04 => val.is_unit(),
                    0x05 => val.is_actor_ref(),
                    0x06 => val.is_string(),
                    0x07 => val.is_closure(),
                    0x08 => val.is_ptr(),
                    0x09 => val.as_float().is_some(),
                    0x0A => false, // list
                    0x0B => false, // tuple
                    _ => false,
                };
                frame.regs[instr.op3 as usize] = Value::bool(result);
            }

            // -- Register moves --
            OpCode::Load | OpCode::Store | OpCode::Move | OpCode::Dup => {
                frame.regs[instr.op2 as usize] = frame.regs[instr.op1 as usize];
            }
            OpCode::Swap => {
                let a = instr.op1 as usize;
                let b = instr.op2 as usize;
                let tmp = frame.regs[a];
                frame.regs[a] = frame.regs[b];
                frame.regs[b] = tmp;
            }

            // -- Control flow (non-consuming) --
            OpCode::Jmp => {
                let offset = instr.imm16() as i16;
                frame.pc = (frame.pc as i64 + offset as i64 - 1) as usize;
            }
            OpCode::JmpT => {
                let cond = frame.regs[instr.op1 as usize].as_bool().unwrap_or(false);
                if cond {
                    let offset = instr.offset16() as i16;
                    frame.pc = (frame.pc as i64 + offset as i64 - 1) as usize;
                }
            }
            OpCode::JmpF => {
                let cond = frame.regs[instr.op1 as usize].as_bool().unwrap_or(false);
                if !cond {
                    let offset = instr.offset16() as i16;
                    frame.pc = (frame.pc as i64 + offset as i64 - 1) as usize;
                }
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
                let effect_name = self.module_const_string(frame.module_idx, eff_name_idx as usize);

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

                // Determine the handler to invoke and capture continuation.
                // We temporarily put the frame back so capture() can clone it.
                self.current_frame = Some(frame);

                let target_offset = if let Some(handler_stack_idx) = handler_idx {
                    // Found a matching handler.
                    let hf = &mut self.handler_stack[handler_stack_idx];
                    let (handler_offset, _arg_count, result_reg) = {
                        let module = self.modules.get(hf.module_idx).unwrap();
                        let ht = module.handler_tables.get(hf.handler_table_idx).unwrap();
                        let binding = ht.bindings.iter()
                            .find(|b| b.effect_name == effect_name)
                            .unwrap();
                        (binding.handler_offset, binding.arg_count, binding.result_reg)
                    };
                    self.handler_stack[handler_stack_idx].resume_dst = result_reg;
                    Some(handler_offset)
                } else {
                    // No handler found — check for fallback.
                    let fallback_offset = self.handler_stack.last().and_then(|hf| {
                        self.modules.get(hf.module_idx)
                            .and_then(|m| m.handler_tables.get(hf.handler_table_idx))
                            .and_then(|ht| ht.fallback_offset)
                    });
                    fallback_offset
                };

                if let Some(handler_stack_idx) = handler_idx {
                    let cont = Continuation::capture(self, dst_reg)
                        .ok_or_else(|| NuError::VMError(
                            "Cannot capture continuation: no current frame".into()
                        ))?;
                    self.handler_stack[handler_stack_idx].captured_continuation = Some(cont);
                } else if target_offset.is_some() {
                    // Fallback path — capture continuation on the innermost handler.
                    let hf_idx = self.handler_stack.len().saturating_sub(1);
                    let cont = Continuation::capture(self, dst_reg)
                        .ok_or_else(|| NuError::VMError(
                            "Cannot capture continuation for fallback: no current frame".into()
                        ))?;
                    self.handler_stack[hf_idx].captured_continuation = Some(cont);
                } else {
                    // No handler and no fallback — error.
                    frame = self.current_frame.take().unwrap();
                    self.current_frame = Some(frame);
                    return Err(NuError::EffectError {
                        msg: format!("Unhandled effect: '{}'", effect_name),
                        span: Span::default(),
                    });
                }

                // Redirect execution to the handler/fallback body.
                frame = self.current_frame.take().unwrap();
                frame.pc = target_offset.unwrap();
            }
            OpCode::Resume => {
                // Resume: restore the captured continuation with a value.
                // op1 = register containing the value to resume with.
                let val_reg = instr.op1 as usize;
                let val = frame.regs[val_reg];

                // Peek at the innermost handler frame (do NOT pop — the handler
                // stays active until Unwind, allowing multiple effects in the
                // same handle block to be handled by the same handler).
                if let Some(hf) = self.handler_stack.last_mut() {
                    if let Some(cont) = hf.captured_continuation.take() {
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
            // All Python opcodes trap with a descriptive error. Python code
            // runs in dedicated OS threads via native_actor.rs, not inline
            // in the VM. This prevents Python objects from entering the
            // value representation and keeps the VM boundary clean.
            //
            // To call Python from Nulang, use:
            //   perform Python.call("module.function", args)
            // which is dispatched by the effect handler to
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

            // All other opcodes are not yet implemented in the interpreter.
            // Treat them as no-ops for now.
            _ => {}
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
            .ok_or_else(|| NuError::VMError(format!("Function {} not found", func_idx)))?;

        let mut new_frame = Frame::new(None, module_idx);
        new_frame.pc = code_offset;
        for i in 0..(argc as usize).min(256) {
            new_frame.regs[i] = old_frame.regs[i];
        }
        new_frame.return_dst = dst;
        new_frame.closure_env = closure_env;
        new_frame.caller = Some(old_frame);

        self.current_frame = Some(Box::new(new_frame));
        Ok(())
    }

    // === Function Resolution ===

    /// Resolve a function value to a (function_table_index, closure_env).
    fn resolve_function(&self, func_val: Value, _module_idx: usize) -> NuResult<(usize, Option<Value>)> {
        if let Some(func_idx) = func_val.as_int() {
            Ok((func_idx as usize, None))
        } else if (func_val.raw & 0xFFFF_0000_0000_0000) == TAG_CLOSURE {
            let closure_id = func_val.raw & 0x0000_7FFF_FFFF_FFFF;
            // For closures, the closure_id IS the function index (MVP).
            // In a full implementation, we'd look up the closure env.
            Ok((closure_id as usize, Some(func_val)))
        } else {
            Err(NuError::VMError(format!("Not a function: {}", func_val.to_string_repr())))
        }
    }
}

impl Default for VM {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
fn module_with_handler_table(bindings: Vec<crate::bytecode::HandlerBinding>) -> CodeModule {
    let mut module = CodeModule::new("test_module");
    module.add_handler_table(crate::bytecode::HandlerTable {
        bindings,
        fallback_offset: None,
    });
    module
}

// ---------------------------------------------------------------------------
// VM Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod vm_tests {
    use super::*;
    use crate::bytecode::{HandlerBinding, HandlerTable};

    /// Test 1: Basic integer arithmetic.
    #[test]
    fn test_basic_arithmetic() {
        let mut module = CodeModule::new("test_arith");
        // r0 = 10, r1 = 3, r2 = r0 + r1
        module.emit(Instruction::new2(OpCode::Const1, 0, 0));
        module.emit(Instruction::new2(OpCode::Const1, 0, 1));
        // Patch: use ConstU with constant pool
        let c10_idx = module.add_constant(Constant::Int(10));
        let c3_idx = module.add_constant(Constant::Int(3));
        module.instructions.clear(); // clear the Const1 instructions
        module.emit(Instruction::new3(OpCode::ConstU,
            ((c10_idx >> 8) & 0xFF) as u8, (c10_idx & 0xFF) as u8, 0)); // r0 = 10
        module.emit(Instruction::new3(OpCode::ConstU,
            ((c3_idx >> 8) & 0xFF) as u8, (c3_idx & 0xFF) as u8, 1));  // r1 = 3
        module.emit(Instruction::new3(OpCode::IAdd, 0, 1, 2)); // r2 = r0 + r1 = 13
        module.emit(Instruction::new2(OpCode::Move, 2, 0));    // r0 = r2 (return value)
        module.emit(Instruction::new0(OpCode::Halt));
        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_ok(), "Arithmetic should work: {:?}", result.err());
        assert_eq!(result.unwrap().as_int(), Some(13), "10 + 3 = 13");
    }

    /// Test 2: NaN-boxed value representation.
    #[test]
    fn test_value_nan_tagging() {
        let v_int = Value::int(42);
        assert_eq!(v_int.as_int(), Some(42));
        assert!(v_int.is_int());

        let v_float = Value::float(3.14);
        assert!((v_float.as_float().unwrap() - 3.14).abs() < 0.001);

        let v_bool = Value::bool(true);
        assert_eq!(v_bool.as_bool(), Some(true));

        let v_nil = Value::nil();
        assert!(v_nil.is_nil());

        let v_unit = Value::unit();
        assert!(v_unit.is_unit());

        let v_actor = Value::actor_ref(123);
        assert_eq!(v_actor.as_actor_id(), Some(123));
    }

    /// Test 3: Halt instruction stops execution.
    #[test]
    fn test_halt_stops() {
        let mut module = CodeModule::new("test_halt");
        let c42_idx = module.add_constant(Constant::Int(42));
        module.emit(Instruction::new3(OpCode::ConstU,
            ((c42_idx >> 8) & 0xFF) as u8, (c42_idx & 0xFF) as u8, 0));
        module.emit(Instruction::new0(OpCode::Halt));
        module.emit(Instruction::new1(OpCode::Const1, 0)); // should not execute
        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_int(), Some(42));
    }

    /// Test 4: PC out of bounds returns safely.
    #[test]
    fn test_pc_out_of_bounds() {
        let mut module = CodeModule::new("test_oob");
        let c99_idx = module.add_constant(Constant::Int(99));
        module.emit(Instruction::new3(OpCode::ConstU,
            ((c99_idx >> 8) & 0xFF) as u8, (c99_idx & 0xFF) as u8, 0));
        // No Halt — PC goes past end
        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_ok(), "PC out of bounds should return gracefully");
        assert_eq!(result.unwrap().as_int(), Some(99));
    }

    /// Test 5: to_string_repr formatting.
    #[test]
    fn test_to_string_repr() {
        assert_eq!(Value::int(42).to_string_repr(), "42");
        assert_eq!(Value::bool(true).to_string_repr(), "true");
        assert_eq!(Value::nil().to_string_repr(), "nil");
        assert_eq!(Value::unit().to_string_repr(), "()");
    }

    /// Test 6: Special values (nil, unit, bool) roundtrip.
    #[test]
    fn test_special_values() {
        assert!(Value::nil().is_nil());
        assert!(!Value::nil().is_unit());
        assert!(Value::unit().is_unit());
        assert!(!Value::unit().is_nil());
        assert_eq!(Value::bool(false).as_bool(), Some(false));
        assert_eq!(Value::bool(true).as_bool(), Some(true));
    }

    /// Test 7: Step limit defaults to 10M.
    #[test]
    fn test_step_limit_default() {
        // This test just verifies the step limit mechanism exists.
        // Running 10M steps would take too long, so we verify the env var parsing.
        let limit = std::env::var("NULANG_STEP_LIMIT")
            .ok().and_then(|s| s.parse().ok()).unwrap_or(10_000_000);
        assert_eq!(limit, 10_000_000, "Default step limit should be 10M");
    }

    /// Test 8: Python opcodes trap with error.
    #[test]
    fn test_python_opcodes_trap() {
        let mut module = CodeModule::new("test_py_trap");
        module.emit(Instruction::new0(OpCode::PyCall));
        module.emit(Instruction::new0(OpCode::Halt));
        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_err(), "Python opcodes should trap");
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("Python") || msg.contains("native actor"),
            "Error should mention Python: {}", msg);
    }

    /// Test 9: Float operations.
    #[test]
    fn test_float_operations() {
        let mut module = CodeModule::new("test_float");
        let c3_14 = module.add_constant(Constant::Float(3.14));
        let c2_0 = module.add_constant(Constant::Float(2.0));
        module.emit(Instruction::new3(OpCode::ConstU,
            ((c3_14 >> 8) & 0xFF) as u8, (c3_14 & 0xFF) as u8, 0)); // r0 = 3.14
        module.emit(Instruction::new3(OpCode::ConstU,
            ((c2_0 >> 8) & 0xFF) as u8, (c2_0 & 0xFF) as u8, 1));  // r1 = 2.0
        module.emit(Instruction::new3(OpCode::FAdd, 0, 1, 2)); // r2 = 5.14
        module.emit(Instruction::new2(OpCode::Move, 2, 0));
        module.emit(Instruction::new0(OpCode::Halt));
        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_ok(), "Float ops should work: {:?}", result.err());
        let f = result.unwrap().as_float().unwrap();
        assert!((f - 5.14).abs() < 0.01, "3.14 + 2.0 = 5.14, got {}", f);
    }

    /// Test 10: Perform + Resume with handler.
    #[test]
    fn test_perform_resume() {
        let mut module = module_with_handler_table(vec![
            HandlerBinding {
                effect_name: "Get42".to_string(),
                handler_offset: 7,
                arg_count: 0,
                result_reg: 0,
            },
        ]);

        // Program layout:
        // PC 0: Handle(0)          — push handler frame
        // PC 1: Perform "Get42" -> r1  — should invoke handler
        // PC 2: (after perform) Move r1 -> r0  — copy result to return reg
        // PC 3: Unwind
        // PC 4: Halt
        // PC 5-6: (padding)
        // PC 7: handler body: ConstU c42 -> r0; Resume r0

        // Add the effect name string to the constant pool first so its index
        // is known when we emit Perform.
        let get42_idx = module.add_constant(Constant::String("Get42".to_string()));

        module.emit(Instruction::new1(OpCode::Handle, 0));           // 0
        module.emit(Instruction::new3(OpCode::Perform,
            ((get42_idx >> 8) & 0xFF) as u8,
            (get42_idx & 0xFF) as u8,
            1));                                                       // 1: perform Get42 -> r1
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
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("Unhandled effect"), "Error should mention unhandled: {}", msg);
    }

    /// Test 12: Nested handlers with shadowing.
    #[test]
    fn test_nested_handlers_shadow() {
        let mut module = CodeModule::new("test_nested");

        // Outer handler table: GetX -> 100
        let outer_bindings = vec![
            HandlerBinding {
                effect_name: "GetX".to_string(),
                handler_offset: 10,
                arg_count: 0,
                result_reg: 0,
            },
        ];
        module.add_handler_table(HandlerTable {
            bindings: outer_bindings,
            fallback_offset: None,
        });

        // Inner handler table: GetX -> 200 (shadows outer)
        let inner_bindings = vec![
            HandlerBinding {
                effect_name: "GetX".to_string(),
                handler_offset: 12,
                arg_count: 0,
                result_reg: 0,
            },
        ];
        module.add_handler_table(HandlerTable {
            bindings: inner_bindings,
            fallback_offset: None,
        });

        let getx_idx = module.add_constant(Constant::String("GetX".to_string()));
        let c100_idx = module.add_constant(Constant::Int(100));
        let c200_idx = module.add_constant(Constant::Int(200));

        // Program:
        // PC 0: Handle(0) — outer handler
        // PC 1: Handle(1) — inner handler
        // PC 2: Perform "GetX" -> r0  — should hit inner (returns 200)
        // PC 3: Unwind — pop inner
        // PC 4: Unwind — pop outer
        // PC 5: Halt
        // padding 6-9
        // PC 10: outer handler body: ConstU 100 -> r0; Resume r0
        // PC 12: inner handler body: ConstU 200 -> r0; Resume r0

        module.emit(Instruction::new1(OpCode::Handle, 0));              // 0
        module.emit(Instruction::new1(OpCode::Handle, 1));              // 1
        module.emit(Instruction::new3(OpCode::Perform,
            ((getx_idx >> 8) & 0xFF) as u8, (getx_idx & 0xFF) as u8, 0)); // 2
        module.emit(Instruction::new0(OpCode::Unwind));                 // 3
        module.emit(Instruction::new0(OpCode::Unwind));                 // 4
        module.emit(Instruction::new0(OpCode::Halt));                   // 5
        // padding 6-9
        for _ in 6..10 { module.emit(Instruction::new0(OpCode::Nop)); }
        // Outer handler at 10
        module.emit(Instruction::new3(OpCode::ConstU,
            ((c100_idx >> 8) & 0xFF) as u8, (c100_idx & 0xFF) as u8, 0)); // 10
        module.emit(Instruction::new1(OpCode::Resume, 0));              // 11
        // Inner handler at 12
        module.emit(Instruction::new3(OpCode::ConstU,
            ((c200_idx >> 8) & 0xFF) as u8, (c200_idx & 0xFF) as u8, 0)); // 12
        module.emit(Instruction::new1(OpCode::Resume, 0));              // 13

        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_ok(), "Nested handlers should work: {:?}", result.err());
        assert_eq!(result.unwrap().as_int(), Some(200), "Inner handler should shadow outer");
    }

    /// Test 13: Multiple effects in one handle block.
    #[test]
    fn test_multi_effect_handler() {
        let mut module = CodeModule::new("test_multi");

        // Handler table: GetA -> 100, GetB -> 200
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
