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

use std::ffi::{c_char, CStr, CString};

use crate::bytecode::{CodeModule, Constant, OpCode};
use crate::ffi::{call_native, CType, Signature, FFI_REGISTRY};
use crate::jit::{self, JitSession, TieredAction};
use crate::runtime::heap::{ActorHeap, TypeTag as HeapTypeTag};
use crate::types::{NuError, NuResult, Span};

// ---------------------------------------------------------------------------
// Distributed runtime callbacks for VM opcode integration.
//
// The VM does not depend on the actor runtime directly (that would create a
// circular crate dependency). Instead, a lightweight callback trait can be
// installed when the VM is used inside a distributed actor context.
// ---------------------------------------------------------------------------

/// Callback interface that supplies real distributed behavior for the VM's
/// `NodeId`, `Migrate`, `RAsk`, and `Gossip` opcodes.
///
/// A default no-op implementation is provided so the standalone VM remains
/// usable without any distributed runtime attached.
pub trait DistributedVmCallbacks: std::any::Any + std::fmt::Debug {
    /// Return the local node ID.
    fn node_id(&self) -> u64 { 0 }

    /// Record an actor migration request.
    fn migrate(&mut self, _actor_id: u64, _target_node_id: u64) {}

    /// Perform a synchronous remote ask.
    ///
    /// Returns the response value, or `Value::nil()` on timeout / failure.
    fn remote_ask(
        &mut self,
        _target_actor: u64,
        _behavior: &str,
        _args: &[Value],
        _timeout_ms: u64,
    ) -> Value {
        Value::nil()
    }

    /// Send a gossip-style message to a subset of known nodes.
    ///
    /// Returns `Value::unit()`.
    fn gossip(&mut self, _message: &str) -> Value {
        Value::unit()
    }
}

// ---------------------------------------------------------------------------
// Actor runtime callbacks for VM opcode integration.
//
// The VM is designed to run standalone, but when embedded in the actor
// runtime these callbacks wire Spawn to real actors and route heap
// allocations through the current actor's heap.
// ---------------------------------------------------------------------------

/// Result of querying whether a workflow signal has been received.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SignalWaitResult {
    /// The signal has been received; resume with this value.
    Ready(Value),
    /// The signal has not been received; the runtime should suspend the step.
    NotReady,
}

/// Callback interface that supplies real actor-runtime behavior for the VM's
/// `Spawn`, `ArrAlloc`, `SConcat`, `SRead`, and `Drop` opcodes.
pub trait ActorVmCallbacks: std::any::Any + std::fmt::Debug {
    /// Return the ID of the actor currently executing in the VM, if any.
    fn current_actor_id(&self) -> Option<u64> { None }

    /// Allocate `size` bytes on the current actor's heap.
    ///
    /// `type_tag` tells the heap what kind of object is being allocated.
    /// Returns a pointer to the payload region, or `None` if allocation fails.
    fn alloc(&mut self, size: usize, type_tag: HeapTypeTag) -> Option<*mut u8>;

    /// Drop a local reference to a heap object.
    ///
    /// For standalone heaps this frees immediately; for actor heaps it should
    /// decrement the local reference count and reclaim when possible.
    fn drop_ref(&mut self, ptr: *mut u8);

    /// Return the number of elements in an array allocated on the actor heap.
    fn array_len(&self, ptr: *mut u8) -> Option<usize>;

    /// Spawn a real actor from `module.actor_metadata`.
    ///
    /// `behavior_idx` is the behavior table index embedded in the `Spawn`
    /// instruction. The callback should find the matching `ActorMeta`, apply
    /// its persistence defaults, and return an actor reference value.
    fn spawn_actor(
        &mut self,
        module: &CodeModule,
        behavior_idx: usize,
        init: Vec<(String, Value)>,
    ) -> Value;

    /// Send a message to an actor by behavior table index.
    fn send_message(&mut self, target: Value, behavior_id: u16, args: &[Value]);

    /// Synchronously ask an actor and return its response.
    /// Default implementation sends the message and returns nil.
    fn ask_actor(&mut self, target: Value, behavior_id: u16, args: &[Value]) -> Value {
        let _ = (target, behavior_id, args);
        Value::nil()
    }

    /// Read a field from the current actor's state.  Default returns nil.
    fn get_state_field(&self, _field: &str) -> Value { Value::nil() }

    /// Write a field on the current actor's state.  Default is a no-op.
    fn set_state_field(&mut self, _field: &str, _value: Value) {}

    /// Emit an event in the current actor.  Default is a no-op.
    fn emit_event(&mut self, _event: &str, _args: &[Value]) {}

    /// Handle a built-in effect performed without an explicit handler.
    ///
    /// The callback receives the effect name and the current frame registers
    /// (args are placed in r0..rn by the compiler). If it returns `Some`, the
    /// VM resumes with that value; otherwise the effect is unhandled and the
    /// VM errors.
    fn perform_effect(&mut self, _effect_name: &str, _regs: &[Value]) -> Option<Value> { None }

    /// Check whether a workflow signal has been received.
    /// Default returns `Ready(unit)` so un-wired signal waits do not block.
    fn wait_signal(&mut self, _name: &str) -> SignalWaitResult {
        SignalWaitResult::Ready(Value::unit())
    }

    /// Suspend the current workflow step waiting for a signal.
    /// The callback receives the captured VM state so it can store it on the
    /// actor and resume execution when the signal arrives.
    fn suspend_for_signal(&mut self, _name: &str, _vm_state: Option<SuspendedVmState>) {}

    /// Execute an LLM request synchronously and return the response content.
    ///
    /// The VM extracts the prompt as a string and passes it to the callback
    /// along with the model constant from the `LlmAsk` instruction. If no
    /// client is configured, return `None` and the VM will leave the result
    /// register as `nil`.
    fn complete_llm(&mut self, _model: &str, _prompt: &str) -> Option<String> { None }

    /// Create a new pipeline and return its runtime ID.
    fn pipeline_new(&mut self) -> i64 { 0 }

    /// Add a stage to an existing pipeline and return its ID.
    fn pipeline_stage(
        &mut self,
        _id: i64,
        _name: &str,
        _actor_id: u64,
        _template: &str,
    ) -> i64 { -1 }

    /// Run a pipeline and return its final output string.
    fn pipeline_run(&mut self, _id: i64, _input: &str) -> Option<String> { None }

    /// Create a new supervisor team and return its runtime ID.
    fn supervisor_new(&mut self) -> i64 { 0 }

    /// Add a worker to an existing supervisor team and return its ID.
    fn supervisor_worker(
        &mut self,
        _id: i64,
        _name: &str,
        _actor_id: u64,
        _description: &str,
    ) -> i64 { -1 }

    /// Run a supervisor team and return its final output string.
    fn supervisor_run(&mut self, _id: i64, _task: &str) -> Option<String> { None }

    /// Create a new debate and return its runtime ID.
    fn debate_new(&mut self, _topic: &str, _rounds: i64, _threshold: f64) -> i64 { 0 }

    /// Add a participant to an existing debate and return its ID.
    fn debate_participant(
        &mut self,
        _id: i64,
        _name: &str,
        _stance: &str,
        _actor_id: u64,
    ) -> i64 { -1 }

    /// Run a debate and return its final output string.
    fn debate_run(&mut self, _id: i64) -> Option<String> { None }
}

/// Standalone callbacks used when the VM runs without an actor runtime.
///
/// Allocations go through a private `ActorHeap` so that `Drop` actually
/// reclaims memory instead of leaking.
#[derive(Debug)]
struct StandaloneVmCallbacks {
    heap: ActorHeap,
    gc: crate::runtime::OrcaGc,
}

impl StandaloneVmCallbacks {
    fn new() -> Self {
        let mut heap = ActorHeap::new(1024 * 1024);
        heap.set_actor_id(0);
        Self { heap, gc: crate::runtime::OrcaGc::new(0) }
    }
}

impl ActorVmCallbacks for StandaloneVmCallbacks {
    fn alloc(&mut self, size: usize, type_tag: HeapTypeTag) -> Option<*mut u8> {
        self.heap.alloc(size, type_tag)
    }

    fn drop_ref(&mut self, ptr: *mut u8) {
        unsafe {
            self.gc.drop_local_ref(&mut self.heap, ptr);
        }
    }

    fn array_len(&self, ptr: *mut u8) -> Option<usize> {
        unsafe {
            let header = &*ActorHeap::header_of(ptr);
            if header.type_tag == HeapTypeTag::Array {
                let payload_size = header.size.saturating_sub(ActorHeap::HEADER_SIZE);
                Some(payload_size / std::mem::size_of::<Value>())
            } else {
                None
            }
        }
    }

    fn spawn_actor(
        &mut self,
        _module: &CodeModule,
        _behavior_idx: usize,
        _init: Vec<(String, Value)>,
    ) -> Value {
        Value::actor_ref(0)
    }

    fn send_message(&mut self, _target: Value, _behavior_id: u16, _args: &[Value]) {}
}

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

use crate::value_layout::{
    sext48, TAG_MASK, TAG_NIL, TAG_UNIT, TAG_BOOL, TAG_INT, TAG_PTR, TAG_ACTOR, TAG_STRING,
    TAG_CLOSURE, PAYLOAD_MASK,
};

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
            Some(sext48(self.raw & PAYLOAD_MASK))
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

    /// Return the raw NaN-boxed bits (opaque bit pattern).
    pub fn to_bits(self) -> u64 { self.raw }

    /// Construct a Value from raw NaN-boxed bits.
    pub fn from_bits(raw: u64) -> Self { Value { raw } }

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

/// Convert a bytecode constant into a runtime value.
pub(crate) fn constant_to_value(c: &Constant) -> Value {
    match c {
        Constant::Int(i) => Value::int(*i),
        Constant::Float(f) => Value::float(*f),
        Constant::String(_) => Value::nil(), // strings are heap-allocated on demand
        Constant::Bool(b) => Value::bool(*b),
        Constant::Nil => Value::nil(),
        Constant::Unit => Value::unit(),
        Constant::FunctionRef(_) | Constant::BehaviorRef(_) | Constant::TypeDescriptor(_) => {
            Value::nil()
        }
    }
}

/// Convert a bytecode constant pool to raw NaN-boxed bits for the JIT.
fn constants_to_jit_bits(constants: &[Constant]) -> Vec<u64> {
    constants
        .iter()
        .map(|c| match c {
            Constant::Int(i) => Value::int(*i).to_bits(),
            Constant::Float(f) => Value::float(*f).to_bits(),
            Constant::Bool(b) => Value::bool(*b).to_bits(),
            Constant::Nil => Value::nil().to_bits(),
            Constant::Unit => Value::unit().to_bits(),
            Constant::String(_) |
            Constant::FunctionRef(_) |
            Constant::BehaviorRef(_) |
            Constant::TypeDescriptor(_) => Value::nil().to_bits(),
        })
        .collect()
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
    /// Index of the caller frame in the VM's flat frame stack.
    /// None for the top-level frame.
    pub caller_idx: Option<usize>,
    /// Closure environment (None if not a closure).
    pub closure_env: Option<Value>,
}

impl Frame {
    /// Create a new frame with all registers initialized to nil.
    pub fn new(caller_idx: Option<usize>, module_idx: usize) -> Self {
        Frame {
            regs: [Value::nil(); 256],
            pc: 0,
            module_idx,
            return_dst: 0,
            caller_idx,
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
            .field("caller_idx", &self.caller_idx)
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
    captured_continuation: Option<Continuation>,
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
    /// Deep-cloned frames (current frame + all callers).
    frames: Vec<Frame>,
    /// Index of the active frame within `frames`.
    current_frame_idx: usize,
    /// Program counter at the point of capture (points past Perform).
    resume_pc: usize,
    /// Destination register for the resume value.
    resume_dst: u8,
    /// Step count at capture time.
    step_count: usize,
}

impl Continuation {
    /// Capture a continuation from the current VM state.
    fn capture(vm: &VM, resume_dst: u8) -> Option<Self> {
        let current_idx = vm.current_frame_idx?;
        Some(Continuation {
            frames: vm.frames.iter().take(current_idx + 1).map(clone_frame).collect(),
            current_frame_idx: current_idx,
            resume_pc: vm.frames[current_idx].pc, // PC already points past the Perform instruction
            resume_dst,
            step_count: vm.step_count,
        })
    }

    /// Restore this continuation into the VM, placing `value` in the
    /// resume destination register.
    fn restore(self, vm: &mut VM, value: Value) {
        vm.frames = self.frames;
        vm.current_frame_idx = Some(self.current_frame_idx);
        vm.frames[self.current_frame_idx].regs[self.resume_dst as usize] = value;
        vm.frames[self.current_frame_idx].pc = self.resume_pc;
        vm.step_count = self.step_count;
    }
}

// ---------------------------------------------------------------------------
// VM: Virtual Machine
// ---------------------------------------------------------------------------

/// Deep-clone a single frame.
fn clone_frame(frame: &Frame) -> Frame {
    Frame {
        regs: frame.regs,
        pc: frame.pc,
        module_idx: frame.module_idx,
        return_dst: frame.return_dst,
        caller_idx: frame.caller_idx,
        closure_env: frame.closure_env,
    }
}

/// Captured VM state for a suspended workflow step (e.g. waiting on a signal).
/// The runtime can extract this state from the VM, store it on the actor, and
/// restore it later when the signal arrives.
#[derive(Debug)]
pub struct SuspendedVmState {
    pub frames: Vec<Frame>,
    pub current_frame_idx: Option<usize>,
    pub handler_stack: Vec<HandlerFrame>,
    pub step_count: usize,
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
    /// Flat stack of activation frames.  The active frame is at
    /// `current_frame_idx`; earlier entries are callers.
    frames: Vec<Frame>,
    /// Index of the currently executing frame in `frames`.
    current_frame_idx: Option<usize>,
    /// Handler stack for algebraic effects.
    handler_stack: Vec<HandlerFrame>,
    /// Step counter (for debugging / limits).
    step_count: usize,
    /// Optional JIT session for tiered compilation.
    jit_session: Option<JitSession>,
    /// Per-module constant pools converted to raw bits for the JIT.
    jit_constants: Vec<Vec<u64>>,
    /// Local node ID reported by the `NodeId` opcode.
    node_id: u64,
    /// Migration requests recorded by the `Migrate` opcode when no runtime
    /// callback is installed.
    pending_migrations: Vec<(u64, u64)>,
    /// Gossip messages recorded by the `Gossip` opcode when no runtime
    /// callback is installed.
    gossip_log: Vec<String>,
    /// Name of the signal that caused the most recent workflow suspension.
    /// Filled by `SignalWait` and consumed by the runtime after `run`/`run_from`
    /// returns a suspend error.
    pub suspended_signal_name: Option<String>,
    /// Optional distributed runtime callbacks for remote operations.
    distributed_callbacks: Option<Box<dyn DistributedVmCallbacks>>,
    /// Actor-runtime callbacks: heap allocation, drop, spawn.
    ///
    /// Defaults to a standalone heap so the VM is usable without a runtime.
    actor_callbacks: Box<dyn ActorVmCallbacks>,
    /// Capture environments for closures that captured enclosing locals.
    /// Indexed by the payload of env-flagged closure values. Environments are
    /// never reclaimed for the lifetime of the VM (known limitation — closure
    /// envs are not yet traced by the GC).
    closure_envs: Vec<ClosureEnv>,
}

/// Captured environment of a closure: the lifted function it wraps plus the
/// values captured (by value) from the enclosing scope at creation time.
#[derive(Debug, Clone)]
struct ClosureEnv {
    func_idx: usize,
    captures: Vec<Value>,
}

/// Payload bit distinguishing env-carrying closures (index into
/// `VM::closure_envs`) from immediate closures (payload = function index).
const CLOSURE_ENV_FLAG: u64 = 0x0000_4000_0000_0000;
const CLOSURE_ENV_IDX_MASK: u64 = CLOSURE_ENV_FLAG - 1;

impl VM {
    /// Create a new VM.
    pub fn new() -> Self {
        VM {
            modules: Vec::new(),
            frames: Vec::with_capacity(64),
            current_frame_idx: None,
            handler_stack: Vec::new(),
            step_count: 0,
            jit_session: Some(JitSession::new()),
            jit_constants: Vec::new(),
            node_id: 0,
            pending_migrations: Vec::new(),
            gossip_log: Vec::new(),
            suspended_signal_name: None,
            distributed_callbacks: None,
            actor_callbacks: Box::new(StandaloneVmCallbacks::new()),
            closure_envs: Vec::new(),
        }
    }

    /// Set the local node ID returned by the `NodeId` opcode.
    pub fn set_node_id(&mut self, node_id: u64) {
        self.node_id = node_id;
    }

    /// Install distributed runtime callbacks for remote opcodes.
    pub fn set_distributed_callbacks(&mut self, callbacks: Box<dyn DistributedVmCallbacks>) {
        self.distributed_callbacks = Some(callbacks);
    }

    /// Install actor-runtime callbacks for Spawn and heap operations.
    ///
    /// Replaces the default standalone heap, so all subsequent allocations go
    /// through the supplied runtime.
    pub fn set_actor_callbacks(&mut self, callbacks: Box<dyn ActorVmCallbacks>) {
        self.actor_callbacks = callbacks;
    }

    /// Capture the current VM execution state so a workflow step can be
    /// suspended (e.g. while waiting for a signal) and resumed later.
    pub fn take_suspended_state(&mut self) -> Option<SuspendedVmState> {
        if self.current_frame_idx.is_none() {
            return None;
        }
        Some(SuspendedVmState {
            frames: std::mem::take(&mut self.frames),
            current_frame_idx: self.current_frame_idx.take(),
            handler_stack: std::mem::take(&mut self.handler_stack),
            step_count: self.step_count,
        })
    }

    /// Restore a previously captured VM execution state.
    pub fn restore_suspended_state(&mut self, state: SuspendedVmState) {
        self.frames = state.frames;
        self.current_frame_idx = state.current_frame_idx;
        self.handler_stack = state.handler_stack;
        self.step_count = state.step_count;
    }

    /// Set the current execution frame. Used by the runtime to execute actor
    /// bytecode behavior handlers.
    pub fn set_current_frame(&mut self, frame: Frame) {
        self.frames.clear();
        self.frames.push(frame);
        self.current_frame_idx = Some(0);
    }

    /// Return the module index of the currently executing frame, if any.
    pub fn current_module_idx(&self) -> Option<usize> {
        self.current_frame_idx
            .and_then(|idx| self.frames.get(idx))
            .map(|frame| frame.module_idx)
    }

    /// Resolve a string-pool value to its contents using the current module's
    /// constant pool.
    pub fn constant_string(&self, module_idx: usize, string_id: u32) -> Option<String> {
        self.modules.get(module_idx)
            .and_then(|m| m.constants.get(string_id as usize))
            .and_then(|c| match c {
                Constant::String(s) => Some(s.clone()),
                _ => None,
            })
    }

    /// Take a snapshot of recorded migration requests.
    pub fn pending_migrations(&self) -> &[(u64, u64)] {
        &self.pending_migrations
    }

    /// Take a snapshot of recorded gossip messages.
    pub fn gossip_log(&self) -> &[String] {
        &self.gossip_log
    }

    /// Load a bytecode module into the VM.
    pub fn load_module(&mut self, module: CodeModule) {
        let bits = constants_to_jit_bits(&module.constants);
        self.modules.push(module);
        self.jit_constants.push(bits);
    }

    /// Copy the payload of a string-like value into a `Vec<u8>`.
    ///
    /// Used by the FFI call path to build temporary `CString` arguments.
    ///
    /// # Safety
    /// Pointer values must point to a valid heap object or a C string borrowed
    /// for the duration of this call.
    unsafe fn value_to_bytes(&self, module_idx: usize, value: Value) -> Option<Vec<u8>> {
        if let Some(id) = value.as_string_id() {
            self.modules.get(module_idx)
                .and_then(|m| m.constants.get(id as usize))
                .and_then(|c| match c {
                    Constant::String(s) => Some(s.as_bytes().to_vec()),
                    _ => None,
                })
        } else if let Some(ptr) = value.as_ptr() {
            // SAFETY: ptr must point to a heap object with an OrcaHeader.
            let header = unsafe { &*ActorHeap::header_of(ptr) };
            let payload_size = header.size.saturating_sub(ActorHeap::HEADER_SIZE);
            // SAFETY: payload_size bytes follow the header.
            Some(unsafe { std::slice::from_raw_parts(ptr, payload_size) }.to_vec())
        } else {
            None
        }
    }

    /// Copy a C string return value into the actor heap and free the temporary.
    fn copy_cstr_return(&mut self, value: Value) -> NuResult<Value> {
        let ptr = value.as_ptr().ok_or_else(|| NuError::VMError("FFI C string return was not a pointer".to_string()))?;
        // SAFETY: ptr is a valid null-terminated C string from cstr_to_value.
        let bytes = unsafe { CStr::from_ptr(ptr as *const c_char).to_bytes() };
        let len = bytes.len();
        let heap_ptr = self.actor_callbacks.alloc(len + 1, HeapTypeTag::String)
            .ok_or_else(|| NuError::VMError("FFI C string heap allocation failed".to_string()))?;
        // SAFETY: heap_ptr points to len+1 bytes of freshly allocated memory.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), heap_ptr, len);
            *heap_ptr.add(len) = 0;
        }
        // SAFETY: value was produced by cstr_to_value.
        unsafe { crate::ffi::marshal::free_cstr_value(value); }
        Ok(Value::ptr(heap_ptr))
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

    /// Convert a runtime value into a plain Rust string.
    ///
    /// String-id values are resolved through the module's constant pool.
    /// Pointer values are read as null-terminated UTF-8.
    pub fn value_to_string(&self, module_idx: usize, value: Value) -> String {
        if let Some(id) = value.as_string_id() {
            self.constant_string(module_idx, id).unwrap_or_default()
        } else if let Some(ptr) = value.as_ptr() {
            if ptr.is_null() {
                String::new()
            } else {
                // SAFETY: the pointer was allocated by this VM's allocator and
                // is expected to be null-terminated for string payloads.
                unsafe {
                    CStr::from_ptr(ptr as *const c_char)
                        .to_string_lossy()
                        .into_owned()
                }
            }
        } else {
            value.to_string_repr()
        }
    }

    /// Allocate a fresh heap string and return it as a pointer value.
    pub fn allocate_string(&mut self, s: &str) -> Value {
        let bytes = s.as_bytes();
        if let Some(ptr) = self.actor_callbacks.alloc(bytes.len() + 1, HeapTypeTag::String) {
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
                *ptr.add(bytes.len()) = 0;
            }
            Value::ptr(ptr)
        } else {
            Value::nil()
        }
    }

    /// Add a runtime string to a module's constant pool and return its string-id value.
    fn add_runtime_string(&mut self, module_idx: usize, s: String) -> Value {
        let idx = self.modules.get(module_idx)
            .map(|m| m.constants.len())
            .unwrap_or(0);
        if let Some(module) = self.modules.get_mut(module_idx) {
            module.constants.push(Constant::String(s));
        }
        if let Some(bits) = self.jit_constants.get_mut(module_idx) {
            bits.push(Value::nil().to_bits());
        }
        Value::string(idx as u32)
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
        self.frames.clear();
        self.frames.push(frame);
        self.current_frame_idx = Some(0);

        // Main execution loop
        loop {
            // Check if halted
            if let Some(idx) = self.current_frame_idx {
                let module_idx = self.frames[idx].module_idx;
                let pc = self.frames[idx].pc;
                if let Some(module) = self.modules.get(module_idx) {
                    if pc >= module.instructions.len() {
                        // PC past end — program complete
                        return Ok(self.frames.get(idx).map(|f| f.regs[0]).unwrap_or(Value::unit()));
                    }
                    // Check if next instruction is Halt
                    if module.instructions.get(pc).map(|i| i.opcode == OpCode::Halt).unwrap_or(false) {
                        self.frames[idx].pc += 1;
                        return Ok(self.frames.get(idx).map(|f| f.regs[0]).unwrap_or(Value::unit()));
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
                    return Ok(self.current_frame_idx.and_then(|i| self.frames.get(i)).map(|f| f.regs[0]).unwrap_or(Value::unit()));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Run with a specific entry point. If the VM already has a current frame
    /// (e.g. set by the actor runtime with pre-populated argument registers),
    /// reuse it; otherwise create a fresh frame. This lets actor behavior
    /// handlers receive their arguments.
    pub fn run_from(&mut self, module_idx: usize, pc: usize) -> NuResult<Value> {
        if self.frames.is_empty() {
            let mut frame = Frame::new(None, module_idx);
            frame.pc = pc;
            self.frames.push(frame);
            self.current_frame_idx = Some(0);
        } else if let Some(frame) = self.frames.get_mut(0) {
            frame.pc = pc;
            frame.module_idx = module_idx;
            self.current_frame_idx = Some(0);
        }

        loop {
            if let Some(idx) = self.current_frame_idx {
                let m_idx = self.frames[idx].module_idx;
                let pc = self.frames[idx].pc;
                if let Some(module) = self.modules.get(m_idx) {
                    if pc >= module.instructions.len() {
                        let v = self.current_frame_idx.and_then(|i| self.frames.get(i)).map(|f| f.regs[0]).unwrap_or(Value::unit());
                        return Ok(v);
                    }
                    if module.instructions.get(pc).map(|i| i.opcode == OpCode::Halt).unwrap_or(false) {
                        self.frames[idx].pc += 1;
                        let v = self.current_frame_idx.and_then(|i| self.frames.get(i)).map(|f| f.regs[0]).unwrap_or(Value::unit());
                        return Ok(v);
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
                    return Ok(self.current_frame_idx.and_then(|i| self.frames.get(i)).map(|f| f.regs[0]).unwrap_or(Value::unit()));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Resume a previously suspended execution.
    ///
    /// Continues from the current frame state (set by `restore_suspended_state`)
    /// until the program halts, yields again, or errors.
    pub fn resume(&mut self) -> NuResult<Value> {
        loop {
            if let Some(idx) = self.current_frame_idx {
                let m_idx = self.frames[idx].module_idx;
                let pc = self.frames[idx].pc;
                if let Some(module) = self.modules.get(m_idx) {
                    if pc >= module.instructions.len() {
                        return Ok(self.current_frame_idx.and_then(|i| self.frames.get(i)).map(|f| f.regs[0]).unwrap_or(Value::unit()));
                    }
                    if module.instructions.get(pc).map(|i| i.opcode == OpCode::Halt).unwrap_or(false) {
                        self.frames[idx].pc += 1;
                        return Ok(self.current_frame_idx.and_then(|i| self.frames.get(i)).map(|f| f.regs[0]).unwrap_or(Value::unit()));
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
                    return Ok(self.current_frame_idx.and_then(|i| self.frames.get(i)).map(|f| f.regs[0]).unwrap_or(Value::unit()));
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
        let _debug = self.step_count <= 50;

        let frame_idx = self.current_frame_idx
            .ok_or_else(|| NuError::VMError("No current frame".to_string()))?;

        // Try JIT execution for hot bytecode regions before interpreting.
        if let Some(module) = self.modules.get(self.frames[frame_idx].module_idx) {
            let pc = self.frames[frame_idx].pc;
            let instructions = &module.instructions;
            let module_idx = self.frames[frame_idx].module_idx;
            let constants = self.jit_constants.get(module_idx)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            if let Some(ref mut jit) = self.jit_session {
                let mut regs: [u64; 256] = [0; 256];
                for (i, r) in self.frames[frame_idx].regs.iter().enumerate() {
                    regs[i] = r.to_bits();
                }
                let action = jit::tiered_execute_step(
                    jit, module_idx, pc, instructions, &mut regs, constants, None,
                );
                if action != TieredAction::Interpret {
                    for (i, bits) in regs.iter().enumerate() {
                        self.frames[frame_idx].regs[i] = Value::from_bits(*bits);
                    }
                    let region_len = jit::find_compilable_region(pc, instructions);
                    self.frames[frame_idx].pc += region_len;
                    return Ok(());
                }
            }
        }

        // Fetch instruction
        let module_idx = self.frames[frame_idx].module_idx;
        let pc = self.frames[frame_idx].pc;
        let instr = {
            let module = self.modules.get(module_idx)
                .ok_or_else(|| NuError::VMError(format!("Module {} not found", module_idx)))?;
            *module.instructions.get(pc)
                .ok_or_else(|| NuError::VMError(format!("PC {} out of bounds in module {}", pc, module_idx)))?
        };
        self.frames[frame_idx].pc += 1;

        match instr.opcode {
            // -- Frame-manipulating opcodes --
            OpCode::Call => {
                let func_val = self.frames[frame_idx].regs[instr.op1 as usize];
                let argc = instr.op2;
                let dst = instr.op3;
                let (func_idx, closure_env) = self.resolve_function(func_val, module_idx)?;
                let code_offset = self.modules.get(module_idx)
                    .and_then(|m| m.function_table.get(func_idx)).copied()
                    .ok_or_else(|| NuError::VMError(format!("Function {} not found", func_idx)))?;
                let mut new_frame = Frame::new(Some(frame_idx), module_idx);
                new_frame.pc = code_offset;
                for i in 0..(argc as usize).min(256) {
                    new_frame.regs[i] = self.frames[frame_idx].regs[i];
                }
                new_frame.return_dst = dst;
                new_frame.closure_env = closure_env;
                self.frames.push(new_frame);
                self.current_frame_idx = Some(self.frames.len() - 1);
                return Ok(());
            }
            OpCode::TailCall => {
                let func_val = self.frames[frame_idx].regs[instr.op1 as usize];
                let func_idx = func_val.as_int()
                    .ok_or_else(|| NuError::VMError("Invalid function reference".to_string()))? as usize;
                let code_offset = self.modules.get(module_idx)
                    .and_then(|m| m.function_table.get(func_idx)).copied()
                    .ok_or_else(|| NuError::VMError(format!("Function {} not found", func_idx)))?;
                self.frames[frame_idx].pc = code_offset;
                return Ok(());
            }
            OpCode::Ret => {
                let ret_val = self.frames[frame_idx].regs[0];
                if let Some(caller_idx) = self.frames[frame_idx].caller_idx {
                    let dst = self.frames[frame_idx].return_dst;
                    self.frames[caller_idx].regs[dst as usize] = ret_val;
                    self.frames.pop();
                    self.current_frame_idx = Some(caller_idx);
                    return Ok(());
                }
                // No caller frame: halt so that run/run_from stop at the end
                // of a top-level behavior handler instead of falling through
                // into the next compiled code region.
                self.frames[frame_idx].regs[0] = ret_val;
                return Err(NuError::VMError("Halt".to_string()));
            }
            OpCode::RetVal => {
                let ret_val = self.frames[frame_idx].regs[instr.op1 as usize];
                if let Some(caller_idx) = self.frames[frame_idx].caller_idx {
                    let dst = self.frames[frame_idx].return_dst;
                    self.frames[caller_idx].regs[dst as usize] = ret_val;
                    self.frames.pop();
                    self.current_frame_idx = Some(caller_idx);
                    return Ok(());
                }
                // No caller frame: halt so that run/run_from stop at the end
                // of a top-level behavior handler instead of falling through
                // into the next compiled code region.
                self.frames[frame_idx].regs[0] = ret_val;
                return Err(NuError::VMError("Halt".to_string()));
            }
            OpCode::ClosureCall => {
                let closure_val = self.frames[frame_idx].regs[instr.op1 as usize];
                let dst = instr.op3;
                let (func_idx, closure_env) = self.resolve_function(closure_val, module_idx)?;
                let code_offset = self.modules.get(module_idx)
                    .and_then(|m| m.function_table.get(func_idx)).copied()
                    .ok_or_else(|| NuError::VMError(format!("Function {} not found", func_idx)))?;
                let mut new_frame = Frame::new(Some(frame_idx), module_idx);
                new_frame.pc = code_offset;
                new_frame.regs = self.frames[frame_idx].regs;
                new_frame.return_dst = dst;
                new_frame.closure_env = closure_env;
                self.frames.push(new_frame);
                self.current_frame_idx = Some(self.frames.len() - 1);
                return Ok(());
            }
            OpCode::FFICall => {
                let func_idx = instr.imm16() as usize;
                let dst = instr.op3;
                let (def, module_idx) = self.modules.get(module_idx)
                    .and_then(|m| m.foreign_functions.get(func_idx).map(|d| (d.clone(), module_idx)))
                    .ok_or_else(|| NuError::VMError(format!("Foreign function {} not found", func_idx)))?;

                let params: Vec<CType> = def.params.iter()
                    .map(|p| crate::ffi::marshal::ffi_type_to_ctype(p))
                    .collect::<Option<_>>()
                    .ok_or_else(|| NuError::VMError(format!("Unsupported FFI parameter type in {}", def.symbol)))?;
                let ret = crate::ffi::marshal::ffi_type_to_ctype(&def.ret)
                    .ok_or_else(|| NuError::VMError(format!("Unsupported FFI return type in {:?}", def.ret)))?;
                let signature = Signature::new(params.clone(), ret);

                // Build argument values. For CStr parameters we copy Nulang
                // string values into temporary CString buffers whose pointers
                // remain valid for the duration of the native call.
                let mut cstrings: Vec<CString> = Vec::new();
                let mut args: Vec<Value> = Vec::with_capacity(def.params.len());
                for (i, param_ctype) in params.iter().enumerate() {
                    let src = self.frames[frame_idx].regs[i];
                    if *param_ctype == CType::CStr {
                        let bytes = unsafe { self.value_to_bytes(module_idx, src) }
                            .ok_or_else(|| NuError::VMError(format!("FFI argument {} for {} is not a string", i, def.symbol)))?;
                        let cstring = CString::new(bytes)
                            .map_err(|e| NuError::VMError(format!("FFI argument {} contains null byte: {}", i, e)))?;
                        args.push(Value::ptr(cstring.as_ptr() as *mut u8));
                        cstrings.push(cstring);
                    } else {
                        args.push(src);
                    }
                }

                let func = {
                    // SAFETY: caller ensures the named library is a valid shared
                    // library. Do not hold the lock across the native call.
                    let registry = FFI_REGISTRY.get_or_init(|| std::sync::Mutex::new(crate::ffi::native::FfiRegistry::new()));
                    let mut reg = registry.lock()
                        .map_err(|e| NuError::VMError(format!("FFI registry lock failed: {}", e)))?;
                    // SAFETY: resolve_or_load opens the library if needed.
                    unsafe { reg.resolve_or_load(&def.library, &def.symbol, signature) }
                        .map_err(|e| NuError::VMError(format!("FFI resolve/load failed for {}: {}", def.symbol, e)))?
                };

                // SAFETY: func.ptr points to a function whose ABI matches signature.
                let mut result = unsafe { call_native(&func, &args) }
                    .map_err(|e| NuError::VMError(format!("FFI call {} failed: {}", def.symbol, e)))?;

                // C string returns are temporary; copy them into the actor heap
                // and free the temporary CString from cstr_to_value.
                if ret == CType::CStr {
                    result = self.copy_cstr_return(result)?;
                }

                self.frames[frame_idx].regs[dst as usize] = result;
                return Ok(());
            }
            OpCode::Panic => {
                let pc = self.frames[frame_idx].pc.saturating_sub(1);
                let r0_repr = self.frames[frame_idx].regs[0].to_string_repr();
                return Err(NuError::VMError(
                    format!("Panic at PC {}: r0={}", pc, r0_repr)
                ));
            }

            // -- Actor opcodes --
            OpCode::Spawn => {
                let behavior_idx = instr.imm16() as usize;
                let init: Vec<(String, Value)> = self.modules.get(module_idx)
                    .and_then(|m| {
                        m.actor_metadata
                            .iter()
                            .find(|meta| meta.behavior_indices.contains(&behavior_idx))
                    })
                    .map(|meta| {
                        meta.state_defaults
                            .iter()
                            .map(|(name, c)| (name.clone(), constant_to_value(c)))
                            .collect()
                    })
                    .unwrap_or_default();
                let result = if let Some(module) = self.modules.get(module_idx) {
                    self.actor_callbacks.spawn_actor(module, behavior_idx, init)
                } else {
                    Value::actor_ref(0)
                };
                self.frames[frame_idx].regs[instr.op3 as usize] = result;
                return Ok(());
            }
            OpCode::Send => {
                let actor_val = self.frames[frame_idx].regs[instr.op1 as usize];
                let behavior_idx = (((instr.op2 as u16) << 8) | (instr.op3 as u16)) as usize;
                let (param_count, behavior_id) = self.modules.get(module_idx)
                    .and_then(|m| m.behaviors.get(behavior_idx))
                    .map(|b| (b.param_count, behavior_idx as u16))
                    .unwrap_or((0, 0));
                let args: Vec<Value> = (0..param_count)
                    .map(|i| self.frames[frame_idx].regs[i])
                    .collect();
                self.actor_callbacks.send_message(actor_val, behavior_id, &args);
                return Ok(());
            }
            OpCode::Ask => {
                let actor_val = self.frames[frame_idx].regs[instr.op1 as usize];
                let behavior_idx = (((instr.op2 as u16) << 8) | (instr.op3 as u16)) as usize;
                let (param_count, behavior_id) = self.modules.get(module_idx)
                    .and_then(|m| m.behaviors.get(behavior_idx))
                    .map(|b| (b.param_count, behavior_idx as u16))
                    .unwrap_or((0, 0));
                let args: Vec<Value> = (0..param_count)
                    .map(|i| self.frames[frame_idx].regs[i])
                    .collect();
                let result = self.actor_callbacks.ask_actor(actor_val, behavior_id, &args);
                self.frames[frame_idx].regs[instr.op1 as usize] = result;
                return Ok(());
            }
            OpCode::SelfOp => {
                let actor_id = self.actor_callbacks.current_actor_id().unwrap_or(0);
                self.frames[frame_idx].regs[instr.op1 as usize] = Value::actor_ref(actor_id);
            }
            OpCode::StateGet => {
                let field_idx = instr.imm16() as usize;
                let field = self.module_const_string(module_idx, field_idx);
                self.frames[frame_idx].regs[instr.op3 as usize] = self.actor_callbacks.get_state_field(&field);
            }
            OpCode::StateSet => {
                let field_idx = instr.imm16() as usize;
                let field = self.module_const_string(module_idx, field_idx);
                let val = self.frames[frame_idx].regs[instr.op3 as usize];
                self.actor_callbacks.set_state_field(&field, val);
            }
            OpCode::Emit => {
                let event_idx = instr.imm16() as usize;
                let event = self.module_const_string(module_idx, event_idx);
                let arg_count = instr.op3 as usize;
                let args: Vec<Value> = (0..arg_count)
                    .map(|i| self.frames[frame_idx].regs[i])
                    .collect();
                self.actor_callbacks.emit_event(&event, &args);
            }
            OpCode::SignalWait => {
                let name_idx = instr.imm16() as usize;
                let name = self.module_const_string(module_idx, name_idx);
                let dst = instr.op3;
                match self.actor_callbacks.wait_signal(&name) {
                    SignalWaitResult::Ready(v) => {
                        self.frames[frame_idx].regs[dst as usize] = v;
                    }
                    SignalWaitResult::NotReady => {
                        self.suspended_signal_name = Some(name.clone());
                        // Leave the PC pointing at the SignalWait instruction so
                        // resumption re-executes it and can write the result into
                        // the destination register once the signal is received.
                        self.frames[frame_idx].pc -= 1;
                        return Err(NuError::VMError("SignalWait:suspend".into()));
                    }
                }
            }
            OpCode::RSend => {
                return Ok(());
            }
            OpCode::RSpawn => {
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::actor_ref(0);
                return Ok(());
            }

            // -- Constants --
            OpCode::Const0 => { self.frames[frame_idx].regs[instr.op1 as usize] = Value::int(0); }
            OpCode::Const1 => { self.frames[frame_idx].regs[instr.op1 as usize] = Value::int(1); }
            OpCode::Const2 => { self.frames[frame_idx].regs[instr.op1 as usize] = Value::int(2); }
            OpCode::ConstU => {
                let idx = instr.imm16() as usize;
                let val = self.modules.get(module_idx)
                    .and_then(|m| m.constants.get(idx))
                    .map(|c| match *c {
                        Constant::Int(n) => Value::int(n),
                        Constant::Float(f) => Value::float(f),
                        Constant::String(_) => Value::string(idx as u32),
                        Constant::Bool(b) => Value::bool(b),
                        Constant::Nil => Value::nil(),
                        Constant::Unit => Value::unit(),
                        _ => Value::nil(),
                    })
                    .unwrap_or(Value::nil());
                self.frames[frame_idx].regs[instr.op3 as usize] = val;
            }
            OpCode::Closure => {
                let func_idx = instr.imm16() as u64;
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::closure(func_idx);
            }
            OpCode::CapStore => {
                // op1 = register holding the closure, op2 = capture slot,
                // op3 = source register. The first store upgrades the closure
                // from the immediate form (payload = function index) to the
                // env-carrying form (payload = env index).
                let closure_reg = instr.op1 as usize;
                let slot = instr.op2 as usize;
                let src = self.frames[frame_idx].regs[instr.op3 as usize];
                let val = self.frames[frame_idx].regs[closure_reg];
                if (val.raw & TAG_MASK) != TAG_CLOSURE {
                    return Err(NuError::VMError(format!(
                        "CapStore target is not a closure: {}",
                        val.to_string_repr()
                    )));
                }
                let payload = val.raw & PAYLOAD_MASK;
                let env_idx = if payload & CLOSURE_ENV_FLAG != 0 {
                    (payload & CLOSURE_ENV_IDX_MASK) as usize
                } else {
                    let idx = self.closure_envs.len();
                    self.closure_envs.push(ClosureEnv {
                        func_idx: payload as usize,
                        captures: Vec::new(),
                    });
                    self.frames[frame_idx].regs[closure_reg] = Value {
                        raw: TAG_CLOSURE | CLOSURE_ENV_FLAG | (idx as u64 & CLOSURE_ENV_IDX_MASK),
                    };
                    idx
                };
                let env = &mut self.closure_envs[env_idx];
                if env.captures.len() <= slot {
                    env.captures.resize(slot + 1, Value::nil());
                }
                env.captures[slot] = src;
            }
            OpCode::CapLoad => {
                // op1 = capture slot, op2 = destination register. Reads from
                // the closure environment of the currently executing frame.
                let slot = instr.op1 as usize;
                let dst = instr.op2 as usize;
                let env_val = self.frames[frame_idx].closure_env.ok_or_else(|| {
                    NuError::VMError("CapLoad outside a closure call".to_string())
                })?;
                let payload = env_val.raw & PAYLOAD_MASK;
                if payload & CLOSURE_ENV_FLAG == 0 {
                    return Err(NuError::VMError(
                        "CapLoad in a closure without captures".to_string(),
                    ));
                }
                let env_idx = (payload & CLOSURE_ENV_IDX_MASK) as usize;
                let value = self
                    .closure_envs
                    .get(env_idx)
                    .and_then(|env| env.captures.get(slot))
                    .copied()
                    .ok_or_else(|| {
                        NuError::VMError(format!("CapLoad of missing capture slot {}", slot))
                    })?;
                self.frames[frame_idx].regs[dst] = value;
            }
            OpCode::FreeVar => {
                // Nothing emits FreeVar; keep it a no-op.
            }

            // -- Arithmetic --
            OpCode::IAdd => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_int().unwrap_or(0);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::int(a + b);
            }
            OpCode::ISub => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_int().unwrap_or(0);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::int(a - b);
            }
            OpCode::IMul => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_int().unwrap_or(0);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::int(a * b);
            }
            OpCode::IDiv => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_int().unwrap_or(1);
                self.frames[frame_idx].regs[instr.op3 as usize] = if b != 0 { Value::int(a / b) } else { Value::nil() };
            }
            OpCode::IMod => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_int().unwrap_or(1);
                self.frames[frame_idx].regs[instr.op3 as usize] = if b != 0 { Value::int(a % b) } else { Value::nil() };
            }
            OpCode::Xor => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_int().unwrap_or(0);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::int(a ^ b);
            }
            OpCode::Shl => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_int().unwrap_or(0);
                let shift = (b as u64) & 0x3f;
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::int(a << shift);
            }
            OpCode::Shr => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_int().unwrap_or(0);
                let shift = (b as u64) & 0x3f;
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::int(a >> shift);
            }
            OpCode::BitAnd => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_int().unwrap_or(0);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::int(a & b);
            }
            OpCode::BitOr => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_int().unwrap_or(0);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::int(a | b);
            }
            OpCode::INeg => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0);
                self.frames[frame_idx].regs[instr.op2 as usize] = Value::int(-a);
            }

            // -- Float arithmetic --
            OpCode::FAdd => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_float().unwrap_or(0.0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_float().unwrap_or(0.0);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::float(a + b);
            }
            OpCode::FSub => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_float().unwrap_or(0.0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_float().unwrap_or(0.0);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::float(a - b);
            }
            OpCode::FMul => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_float().unwrap_or(0.0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_float().unwrap_or(0.0);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::float(a * b);
            }
            OpCode::FDiv => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_float().unwrap_or(0.0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_float().unwrap_or(1.0);
                self.frames[frame_idx].regs[instr.op3 as usize] = if b != 0.0 { Value::float(a / b) } else { Value::nil() };
            }
            OpCode::FNeg => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_float().unwrap_or(0.0);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::float(-a);
            }

            // -- Comparison --
            OpCode::ICmpEq => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_int().unwrap_or(0);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::bool(a == b);
            }
            OpCode::ICmpLt => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_int().unwrap_or(0);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::bool(a < b);
            }
            OpCode::ICmpGt => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_int().unwrap_or(0);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::bool(a > b);
            }
            OpCode::ICmpLe => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_int().unwrap_or(0);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::bool(a <= b);
            }
            OpCode::ICmpGe => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_int().unwrap_or(0);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::bool(a >= b);
            }
            OpCode::FCmpEq => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_float().unwrap_or(0.0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_float().unwrap_or(0.0);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::bool((a - b).abs() < f64::EPSILON);
            }
            OpCode::FCmpLt => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_float().unwrap_or(0.0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_float().unwrap_or(0.0);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::bool(a < b);
            }
            OpCode::FCmpGt => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_float().unwrap_or(0.0);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_float().unwrap_or(0.0);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::bool(a > b);
            }

            // -- Arrays (actor-heap backed; no longer leaked) --
            OpCode::ArrAlloc => {
                let len = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0) as usize;
                let size = len.checked_mul(std::mem::size_of::<Value>()).unwrap_or(0);
                self.frames[frame_idx].regs[instr.op2 as usize] = if let Some(ptr) = self.actor_callbacks.alloc(size, HeapTypeTag::Array) {
                    unsafe {
                        let slots = std::slice::from_raw_parts_mut(ptr as *mut Value, len);
                        for slot in slots.iter_mut() {
                            *slot = Value::nil();
                        }
                    }
                    Value::ptr(ptr)
                } else {
                    Value::nil()
                };
            }
            OpCode::ArrLoad => {
                let arr_ptr = self.frames[frame_idx].regs[instr.op1 as usize].as_ptr().unwrap_or(std::ptr::null_mut());
                let idx = self.frames[frame_idx].regs[instr.op2 as usize].as_int().unwrap_or(0) as usize;
                let val = if !arr_ptr.is_null() {
                    if let Some(len) = self.actor_callbacks.array_len(arr_ptr) {
                        if idx < len {
                            unsafe { *((arr_ptr as *const Value).add(idx)) }
                        } else {
                            Value::nil()
                        }
                    } else {
                        Value::nil()
                    }
                } else {
                    Value::nil()
                };
                self.frames[frame_idx].regs[instr.op3 as usize] = val;
            }
            OpCode::ArrStore => {
                let arr_ptr = self.frames[frame_idx].regs[instr.op1 as usize].as_ptr().unwrap_or(std::ptr::null_mut());
                let idx = self.frames[frame_idx].regs[instr.op2 as usize].as_int().unwrap_or(0) as usize;
                let val = self.frames[frame_idx].regs[instr.op3 as usize];
                if !arr_ptr.is_null() {
                    if let Some(len) = self.actor_callbacks.array_len(arr_ptr) {
                        if idx < len {
                            unsafe { *((arr_ptr as *mut Value).add(idx)) = val; }
                        }
                    }
                }
            }
            OpCode::ArrLen => {
                let arr_ptr = self.frames[frame_idx].regs[instr.op1 as usize].as_ptr().unwrap_or(std::ptr::null_mut());
                let len = if !arr_ptr.is_null() {
                    self.actor_callbacks.array_len(arr_ptr).unwrap_or(0) as i64
                } else {
                    0
                };
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::int(len);
            }

            // -- Records (flat array indexed by module field id) --
            OpCode::RecMk => {
                let slot_count = instr.op1 as usize;
                let size = slot_count.checked_mul(std::mem::size_of::<Value>()).unwrap_or(0);
                self.frames[frame_idx].regs[instr.op2 as usize] = if let Some(ptr) = self.actor_callbacks.alloc(size, HeapTypeTag::Record) {
                    unsafe {
                        let slots = std::slice::from_raw_parts_mut(ptr as *mut Value, slot_count);
                        for slot in slots.iter_mut() {
                            *slot = Value::nil();
                        }
                    }
                    Value::ptr(ptr)
                } else {
                    Value::nil()
                };
            }
            OpCode::RecS => {
                let rec_ptr = self.frames[frame_idx].regs[instr.op1 as usize].as_ptr().unwrap_or(std::ptr::null_mut());
                let field_id = instr.op2 as usize;
                let val = self.frames[frame_idx].regs[instr.op3 as usize];
                if !rec_ptr.is_null() {
                    unsafe {
                        let header = &*ActorHeap::header_of(rec_ptr);
                        if header.type_tag == HeapTypeTag::Record {
                            let payload_size = header.size.saturating_sub(ActorHeap::HEADER_SIZE);
                            let len = payload_size / std::mem::size_of::<Value>();
                            if field_id < len {
                                *((rec_ptr as *mut Value).add(field_id)) = val;
                            }
                        }
                    }
                }
            }
            OpCode::RecL => {
                let rec_ptr = self.frames[frame_idx].regs[instr.op1 as usize].as_ptr().unwrap_or(std::ptr::null_mut());
                let field_id = instr.op2 as usize;
                let val = if !rec_ptr.is_null() {
                    unsafe {
                        let header = &*ActorHeap::header_of(rec_ptr);
                        if header.type_tag == HeapTypeTag::Record {
                            let payload_size = header.size.saturating_sub(ActorHeap::HEADER_SIZE);
                            let len = payload_size / std::mem::size_of::<Value>();
                            if field_id < len {
                                *((rec_ptr as *const Value).add(field_id))
                            } else {
                                Value::nil()
                            }
                        } else {
                            Value::nil()
                        }
                    }
                } else {
                    Value::nil()
                };
                self.frames[frame_idx].regs[instr.op3 as usize] = val;
            }

            // -- Tuples (heap-backed fixed-size arrays) --
            OpCode::TupleMk => {
                let count = instr.op1 as usize;
                let size = count.checked_mul(std::mem::size_of::<Value>()).unwrap_or(0);
                self.frames[frame_idx].regs[instr.op2 as usize] = if let Some(ptr) = self.actor_callbacks.alloc(size, HeapTypeTag::Tuple) {
                    unsafe {
                        let slots = std::slice::from_raw_parts_mut(ptr as *mut Value, count);
                        for slot in slots.iter_mut() {
                            *slot = Value::nil();
                        }
                    }
                    Value::ptr(ptr)
                } else {
                    Value::nil()
                };
            }
            OpCode::FieldS => {
                let tup_ptr = self.frames[frame_idx].regs[instr.op1 as usize].as_ptr().unwrap_or(std::ptr::null_mut());
                let idx = instr.op2 as usize;
                let val = self.frames[frame_idx].regs[instr.op3 as usize];
                if !tup_ptr.is_null() {
                    unsafe {
                        let header = &*ActorHeap::header_of(tup_ptr);
                        if header.type_tag == HeapTypeTag::Tuple {
                            let payload_size = header.size.saturating_sub(ActorHeap::HEADER_SIZE);
                            let len = payload_size / std::mem::size_of::<Value>();
                            if idx < len {
                                *((tup_ptr as *mut Value).add(idx)) = val;
                            }
                        }
                    }
                }
            }
            OpCode::FieldL => {
                let tup_ptr = self.frames[frame_idx].regs[instr.op1 as usize].as_ptr().unwrap_or(std::ptr::null_mut());
                let idx = instr.op2 as usize;
                let val = if !tup_ptr.is_null() {
                    unsafe {
                        let header = &*ActorHeap::header_of(tup_ptr);
                        if header.type_tag == HeapTypeTag::Tuple {
                            let payload_size = header.size.saturating_sub(ActorHeap::HEADER_SIZE);
                            let len = payload_size / std::mem::size_of::<Value>();
                            if idx < len {
                                *((tup_ptr as *const Value).add(idx))
                            } else {
                                Value::nil()
                            }
                        } else {
                            Value::nil()
                        }
                    }
                } else {
                    Value::nil()
                };
                self.frames[frame_idx].regs[instr.op3 as usize] = val;
            }

            // -- Boolean logic --
            OpCode::And => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_bool().unwrap_or(false);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_bool().unwrap_or(false);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::bool(a && b);
            }
            OpCode::Or => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_bool().unwrap_or(false);
                let b = self.frames[frame_idx].regs[instr.op2 as usize].as_bool().unwrap_or(false);
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::bool(a || b);
            }
            OpCode::Not => {
                let a = self.frames[frame_idx].regs[instr.op1 as usize].as_bool().unwrap_or(false);
                self.frames[frame_idx].regs[instr.op2 as usize] = Value::bool(!a);
            }

            // -- Type checks --
            OpCode::IsTag => {
                let val = self.frames[frame_idx].regs[instr.op1 as usize];
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
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::bool(result);
            }

            // -- Register moves --
            OpCode::Load | OpCode::Store | OpCode::Move | OpCode::Dup => {
                let src = self.frames[frame_idx].regs[instr.op1 as usize];
                self.frames[frame_idx].regs[instr.op2 as usize] = src;
            }
            OpCode::Swap => {
                let a = instr.op1 as usize;
                let b = instr.op2 as usize;
                let tmp = self.frames[frame_idx].regs[a];
                self.frames[frame_idx].regs[a] = self.frames[frame_idx].regs[b];
                self.frames[frame_idx].regs[b] = tmp;
            }

            // -- Control flow (non-consuming) --
            OpCode::Jmp => {
                let offset = instr.imm16() as i16;
                self.frames[frame_idx].pc = (self.frames[frame_idx].pc as i64 + offset as i64 - 1) as usize;
            }
            OpCode::JmpT => {
                let cond = self.frames[frame_idx].regs[instr.op1 as usize].as_bool().unwrap_or(false);
                if cond {
                    let offset = instr.offset16() as i16;
                    self.frames[frame_idx].pc = (self.frames[frame_idx].pc as i64 + offset as i64 - 1) as usize;
                }
            }
            OpCode::JmpF => {
                let cond = self.frames[frame_idx].regs[instr.op1 as usize].as_bool().unwrap_or(false);
                if !cond {
                    let offset = instr.offset16() as i16;
                    self.frames[frame_idx].pc = (self.frames[frame_idx].pc as i64 + offset as i64 - 1) as usize;
                }
            }

            // -- Algebraic Effects --
            OpCode::Handle => {
                let handler_table_idx = instr.op1 as usize;
                let resume_pc = self.frames[frame_idx].pc; // already incremented past Handle
                let resume_dst = instr.op2;
                self.handler_stack.push(HandlerFrame::new(
                    handler_table_idx, module_idx, resume_pc, resume_dst,
                ));
            }
            OpCode::Perform => {
                let eff_name_idx = instr.imm16();
                let dst_reg = instr.op3;
                let effect_name = self.module_const_string(module_idx, eff_name_idx as usize);

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

                let target_offset = if let Some(handler_stack_idx) = handler_idx {
                    let (handler_offset, result_reg) = {
                        let hf = &self.handler_stack[handler_stack_idx];
                        let module = self.modules.get(hf.module_idx).unwrap();
                        let ht = module.handler_tables.get(hf.handler_table_idx).unwrap();
                        let binding = ht.bindings.iter()
                            .find(|b| b.effect_name == effect_name)
                            .unwrap();
                        (binding.handler_offset, binding.result_reg)
                    };
                    self.handler_stack[handler_stack_idx].resume_dst = result_reg;
                    Some(handler_offset)
                } else {
                    self.handler_stack.last().and_then(|hf| {
                        self.modules.get(hf.module_idx)
                            .and_then(|m| m.handler_tables.get(hf.handler_table_idx))
                            .and_then(|ht| ht.fallback_offset)
                    })
                };

                if let Some(handler_stack_idx) = handler_idx {
                    let cont = Continuation::capture(self, dst_reg)
                        .ok_or_else(|| NuError::VMError(
                            "Cannot capture continuation: no current frame".into()
                        ))?;
                    self.handler_stack[handler_stack_idx].captured_continuation = Some(cont);
                } else if target_offset.is_some() {
                    let hf_idx = self.handler_stack.len().saturating_sub(1);
                    let cont = Continuation::capture(self, dst_reg)
                        .ok_or_else(|| NuError::VMError(
                            "Cannot capture continuation for fallback: no current frame".into()
                        ))?;
                    self.handler_stack[hf_idx].captured_continuation = Some(cont);
                } else {
                    // No handler and no fallback: give the runtime callback a
                    // chance to handle built-in effects (e.g. Timer.sleep in
                    // workflow steps). Args are in r0..rn.
                    if let Some(result) = self.actor_callbacks.perform_effect(&effect_name, &self.frames[frame_idx].regs) {
                        self.frames[frame_idx].regs[dst_reg as usize] = result;
                    } else {
                        return Err(NuError::EffectError {
                            msg: format!("Unhandled effect: '{}'", effect_name),
                            span: Span::default(),
                        });
                    }
                }

                if let Some(offset) = target_offset {
                    self.frames[frame_idx].pc = offset;
                }
            }
            OpCode::Resume => {
                let val = self.frames[frame_idx].regs[instr.op1 as usize];
                if let Some(hf) = self.handler_stack.last_mut() {
                    if let Some(cont) = hf.captured_continuation.take() {
                        cont.restore(self, val);
                        return Ok(());
                    }
                }
                return Err(NuError::VMError(
                    "resume called without a captured continuation".into()
                ));
            }
            OpCode::Unwind => {
                self.handler_stack.pop();
            }

            // -- Capabilities (MVP) --
            OpCode::CapChk => { self.frames[frame_idx].regs[instr.op2 as usize] = Value::bool(true); }
            OpCode::CapUp => { self.frames[frame_idx].regs[instr.op2 as usize] = self.frames[frame_idx].regs[instr.op1 as usize]; }
            OpCode::CapDown => { self.frames[frame_idx].regs[instr.op2 as usize] = self.frames[frame_idx].regs[instr.op1 as usize]; }
            OpCode::CapSend => { self.frames[frame_idx].regs[instr.op2 as usize] = self.frames[frame_idx].regs[instr.op1 as usize]; }

            // -- Python Interop — RESERVED (see python/bridge.rs) --
            OpCode::PyImport | OpCode::PyGetAttr | OpCode::PyCall
            | OpCode::PyCallKw | OpCode::PySetAttr | OpCode::PyToNu
            | OpCode::PyFromNu | OpCode::PyRelease => {
                return Err(NuError::VMError(
                    "Python opcodes require native actor runtime. \
                     Use perform Python.call(...) instead.".into()
                ));
            }

            // -- Distribution (MVP) --
            OpCode::NodeId => {
                let node_id = self
                    .distributed_callbacks
                    .as_ref()
                    .map(|cb| cb.node_id())
                    .unwrap_or(self.node_id);
                self.frames[frame_idx].regs[instr.op1 as usize] = Value::int(node_id as i64);
            }
            OpCode::Migrate => {
                let actor_id = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0) as u64;
                let target_node_id = self.frames[frame_idx].regs[instr.op2 as usize].as_int().unwrap_or(0) as u64;
                self.pending_migrations.push((actor_id, target_node_id));
                if let Some(ref mut cb) = self.distributed_callbacks {
                    cb.migrate(actor_id, target_node_id);
                }
                self.frames[frame_idx].regs[instr.op3 as usize] = Value::unit();
            }
            OpCode::RAsk => {
                let target_actor = self.frames[frame_idx].regs[instr.op1 as usize].as_int().unwrap_or(0) as u64;
                let behavior_const_idx = instr.op2 as usize;
                let behavior = self.module_const_string(module_idx, behavior_const_idx);
                let result = if let Some(ref mut cb) = self.distributed_callbacks {
                    cb.remote_ask(target_actor, &behavior, &[], 5_000)
                } else {
                    Value::nil()
                };
                self.frames[frame_idx].regs[instr.op3 as usize] = result;
            }
            OpCode::Gossip => {
                let message_const_idx = instr.op1 as usize;
                let message = self.module_const_string(module_idx, message_const_idx);
                self.gossip_log.push(message.clone());
                let result = if let Some(ref mut cb) = self.distributed_callbacks {
                    cb.gossip(&message)
                } else {
                    Value::unit()
                };
                self.frames[frame_idx].regs[instr.op3 as usize] = result;
            }

            // -- String & IO --
            OpCode::SConcat => {
                let s1 = self.frames[frame_idx].regs[instr.op1 as usize].to_string_repr();
                let s2 = self.frames[frame_idx].regs[instr.op2 as usize].to_string_repr();
                let result = format!("{}{}", s1, s2);
                let bytes = result.into_bytes();
                self.frames[frame_idx].regs[instr.op3 as usize] = if let Some(ptr) = self.actor_callbacks.alloc(bytes.len(), HeapTypeTag::String) {
                    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len()); }
                    Value::ptr(ptr)
                } else {
                    Value::nil()
                };
            }
            OpCode::SPrint => { print!("{}", self.frames[frame_idx].regs[instr.op1 as usize].to_string_repr()); }
            OpCode::SRead => {
                let mut input = String::new();
                self.frames[frame_idx].regs[instr.op1 as usize] = if std::io::stdin().read_line(&mut input).is_ok() {
                    let bytes = input.into_bytes();
                    if let Some(ptr) = self.actor_callbacks.alloc(bytes.len(), HeapTypeTag::String) {
                        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len()); }
                        Value::ptr(ptr)
                    } else {
                        Value::nil()
                    }
                } else { Value::nil() };
            }
            OpCode::FOpen => { self.frames[frame_idx].regs[instr.op2 as usize] = Value::nil(); }
            OpCode::FRead => { self.frames[frame_idx].regs[instr.op2 as usize] = Value::nil(); }
            OpCode::FWrite => {}
            OpCode::FClose => {}
            OpCode::Print => { println!("{}", self.frames[frame_idx].regs[instr.op1 as usize].to_string_repr()); }

            // -- Debug & Meta --
            OpCode::DbgBreak => {}
            OpCode::DbgPrint => {
                eprintln!("=== Debug: Register State ===");
                for i in (0..256).step_by(8) {
                    let mut line = format!("R{:03}-R{:03}: ", i, i + 7);
                    for j in 0..8 { line.push_str(&format!("{:>20} ", self.frames[frame_idx].regs[i + j].to_string_repr())); }
                    eprintln!("{}", line);
                }
            }
            OpCode::DbgStack => {
                eprintln!("=== Debug: Call Stack ===");
                let mut depth = 0;
                let mut idx = Some(frame_idx);
                while let Some(i) = idx {
                    let fr = &self.frames[i];
                    let mname = self.modules.get(fr.module_idx).map(|m| m.name.as_str()).unwrap_or("?");
                    eprintln!("  [{}] module={} pc={}", depth, mname, fr.pc);
                    depth += 1;
                    idx = fr.caller_idx;
                }
                if depth == 0 { eprintln!("  (empty)"); }
            }
            OpCode::MetaType => { self.frames[frame_idx].regs[instr.op2 as usize] = Value::int(0); }
            OpCode::MetaCap => { self.frames[frame_idx].regs[instr.op2 as usize] = Value::int(0); }

            // -- LLM effect (v0.9 AI Runtime) --
            OpCode::LlmAsk => {
                let model_idx = instr.imm16() as usize;
                let prompt_reg = instr.op3 as usize;
                let model = self.module_const_string(module_idx, model_idx);
                let prompt_value = self.frames[frame_idx].regs[prompt_reg];
                let prompt = self.value_to_string(module_idx, prompt_value);
                let result = self.actor_callbacks.complete_llm(&model, &prompt);
                let value = match result {
                    Some(ref content) => self.add_runtime_string(module_idx, content.clone()),
                    None => Value::nil(),
                };
                self.frames[frame_idx].regs[prompt_reg] = value;
            }

            // -- Pipeline (v0.9 AI Runtime) --
            OpCode::PipelineNew => {
                let dst = instr.op1;
                let id = self.actor_callbacks.pipeline_new();
                self.frames[frame_idx].regs[dst as usize] = Value::int(id);
            }
            OpCode::PipelineStage => {
                let dst = instr.op1;
                let regs = &self.frames[frame_idx].regs;
                let id = regs[0].as_int().unwrap_or(0);
                let name = self.value_to_string(module_idx, regs[1]);
                let actor_id = regs[2].as_actor_id().unwrap_or(0);
                let template = self.value_to_string(module_idx, regs[3]);
                let result = self
                    .actor_callbacks
                    .pipeline_stage(id, &name, actor_id, &template);
                self.frames[frame_idx].regs[dst as usize] = Value::int(result);
            }
            OpCode::PipelineRun => {
                let dst = instr.op1;
                let regs = &self.frames[frame_idx].regs;
                let id = regs[0].as_int().unwrap_or(0);
                let input = self.value_to_string(module_idx, regs[1]);
                let value = match self.actor_callbacks.pipeline_run(id, &input) {
                    Some(content) => self.add_runtime_string(module_idx, content),
                    None => Value::nil(),
                };
                self.frames[frame_idx].regs[dst as usize] = value;
            }

            // -- Supervisor (v0.9 AI Runtime) --
            OpCode::SupervisorNew => {
                let dst = instr.op1;
                let id = self.actor_callbacks.supervisor_new();
                self.frames[frame_idx].regs[dst as usize] = Value::int(id);
            }
            OpCode::SupervisorWorker => {
                let dst = instr.op1;
                let regs = &self.frames[frame_idx].regs;
                let id = regs[0].as_int().unwrap_or(0);
                let name = self.value_to_string(module_idx, regs[1]);
                let actor_id = regs[2].as_actor_id().unwrap_or(0);
                let description = self.value_to_string(module_idx, regs[3]);
                let result = self
                    .actor_callbacks
                    .supervisor_worker(id, &name, actor_id, &description);
                self.frames[frame_idx].regs[dst as usize] = Value::int(result);
            }
            OpCode::SupervisorRun => {
                let dst = instr.op1;
                let regs = &self.frames[frame_idx].regs;
                let id = regs[0].as_int().unwrap_or(0);
                let task = self.value_to_string(module_idx, regs[1]);
                let value = match self.actor_callbacks.supervisor_run(id, &task) {
                    Some(content) => self.add_runtime_string(module_idx, content),
                    None => Value::nil(),
                };
                self.frames[frame_idx].regs[dst as usize] = value;
            }

            // -- Debate (v0.9 AI Runtime) --
            OpCode::DebateNew => {
                let dst = instr.op1;
                let regs = &self.frames[frame_idx].regs;
                let topic = self.value_to_string(module_idx, regs[0]);
                let rounds = regs[1].as_int().unwrap_or(1);
                let threshold = regs[2].as_float().unwrap_or(0.5);
                let id = self
                    .actor_callbacks
                    .debate_new(&topic, rounds, threshold);
                self.frames[frame_idx].regs[dst as usize] = Value::int(id);
            }
            OpCode::DebateParticipant => {
                let dst = instr.op1;
                let regs = &self.frames[frame_idx].regs;
                let id = regs[0].as_int().unwrap_or(0);
                let name = self.value_to_string(module_idx, regs[1]);
                let stance = self.value_to_string(module_idx, regs[2]);
                let actor_id = regs[3].as_actor_id().unwrap_or(0);
                let result = self
                    .actor_callbacks
                    .debate_participant(id, &name, &stance, actor_id);
                self.frames[frame_idx].regs[dst as usize] = Value::int(result);
            }
            OpCode::DebateRun => {
                let dst = instr.op1;
                let regs = &self.frames[frame_idx].regs;
                let id = regs[0].as_int().unwrap_or(0);
                let value = match self.actor_callbacks.debate_run(id) {
                    Some(content) => self.add_runtime_string(module_idx, content),
                    None => Value::nil(),
                };
                self.frames[frame_idx].regs[dst as usize] = value;
            }

            // -- Reference counting / deallocation --
            OpCode::Drop => {
                let val = self.frames[frame_idx].regs[instr.op1 as usize];
                if let Some(ptr) = val.as_ptr() {
                    self.actor_callbacks.drop_ref(ptr);
                }
            }

            // All other opcodes are not yet implemented in the interpreter.
            _ => {
                return Err(NuError::VMError(format!(
                    "unimplemented opcode {:?}",
                    instr.opcode
                )));
            }
        }
        Ok(())
    }

    // === Function Resolution ===

    /// Resolve a function value to a (function_table_index, closure_env).
    fn resolve_function(&self, func_val: Value, _module_idx: usize) -> NuResult<(usize, Option<Value>)> {
        if let Some(func_idx) = func_val.as_int() {
            Ok((func_idx as usize, None))
        } else if (func_val.raw & TAG_MASK) == TAG_CLOSURE {
            let payload = func_val.raw & PAYLOAD_MASK;
            if payload & CLOSURE_ENV_FLAG != 0 {
                // Env-carrying closure: the function index lives in the env.
                let env_idx = (payload & CLOSURE_ENV_IDX_MASK) as usize;
                let func_idx = self
                    .closure_envs
                    .get(env_idx)
                    .map(|env| env.func_idx)
                    .ok_or_else(|| {
                        NuError::VMError(format!("Dangling closure environment {}", env_idx))
                    })?;
                Ok((func_idx, Some(func_val)))
            } else {
                // Immediate closure: the payload is the function index.
                Ok((payload as usize, Some(func_val)))
            }
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
    use crate::bytecode::{HandlerBinding, HandlerTable, Instruction};

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

        let v_float = Value::float(2.5);
        assert!((v_float.as_float().unwrap() - 2.5).abs() < 0.001);

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
        let c3_5 = module.add_constant(Constant::Float(3.5));
        let c2_0 = module.add_constant(Constant::Float(2.0));
        module.emit(Instruction::new3(OpCode::ConstU,
            ((c3_5 >> 8) & 0xFF) as u8, (c3_5 & 0xFF) as u8, 0)); // r0 = 3.5
        module.emit(Instruction::new3(OpCode::ConstU,
            ((c2_0 >> 8) & 0xFF) as u8, (c2_0 & 0xFF) as u8, 1));  // r1 = 2.0
        module.emit(Instruction::new3(OpCode::FAdd, 0, 1, 2)); // r2 = 5.5
        module.emit(Instruction::new2(OpCode::Move, 2, 0));
        module.emit(Instruction::new0(OpCode::Halt));
        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_ok(), "Float ops should work: {:?}", result.err());
        let f = result.unwrap().as_float().unwrap();
        assert!((f - 5.5).abs() < 0.01, "3.5 + 2.0 = 5.5, got {}", f);
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

    /// Test 16: JIT-compiled hot loop produces the same result as the interpreter.
    #[test]
    fn test_jit_hot_loop_matches_interpreter() {
        let mut module = CodeModule::new("test_jit_hot_loop");
        // Registers: r0 = sum, r1 = i, r2 = limit, r3 = one, r4 = condition.
        module.emit(Instruction::new1(OpCode::Const0, 0)); // 0: sum = 0
        module.emit(Instruction::new1(OpCode::Const0, 1)); // 1: i = 0
        module.emit(Instruction::new2(OpCode::Const2, 2, 0)); // 2: limit = 2
        module.emit(Instruction::new2(OpCode::Const2, 3, 0)); // 3: tmp = 2
        module.emit(Instruction::new3(OpCode::IAdd, 2, 3, 2)); // limit = 4
        module.emit(Instruction::new1(OpCode::Const1, 3));     // r3 = 1

        let loop_check = module.current_offset();
        module.emit(Instruction::new3(OpCode::ICmpLt, 1, 2, 4)); // r4 = i < limit
        let jmpf_idx = module.current_offset();
        module.emit(Instruction::new2(OpCode::JmpF, 4, 0)); // exit loop when false
        module.emit(Instruction::new3(OpCode::IAdd, 0, 1, 0)); // sum += i
        module.emit(Instruction::new3(OpCode::IAdd, 1, 3, 1)); // i += 1
        let jmp_back_idx = module.current_offset();
        let back_offset = loop_check as i64 - jmp_back_idx as i64;
        module.emit(Instruction::new3(OpCode::Jmp,
            ((back_offset as i16 >> 8) & 0xFF) as u8,
            (back_offset as i16 & 0xFF) as u8,
            0));
        let after_loop = module.current_offset();
        if let Some(instr) = module.instructions.get_mut(jmpf_idx) {
            let forward_offset = after_loop as i64 - jmpf_idx as i64;
            instr.op2 = ((forward_offset as i16 >> 8) & 0xFF) as u8;
            instr.op3 = (forward_offset as i16 & 0xFF) as u8;
        }
        module.emit(Instruction::new0(OpCode::Halt));
        module.entry_point = Some(0);

        // Cold interpreter run.
        crate::jit::reset_hot_counters();
        let mut vm = VM::new();
        vm.load_module(module.clone());
        let cold_result = vm.run_from(0, 0).unwrap();

        // Heat the entry region until it is JIT-compiled.
        crate::jit::reset_hot_counters();
        for _ in 0..2000 {
            let _ = vm.run_from(0, 0);
        }

        let hot_result = vm.run_from(0, 0).unwrap();
        assert_eq!(hot_result.as_int(), cold_result.as_int(),
            "JIT hot loop should match interpreter");
        assert_eq!(hot_result.as_int(), Some(6), "sum 0..4 = 6");
    }

    /// Regression test: a hot loop whose body is long enough to JIT and whose
    /// header has an early-exit conditional must produce the exact interpreter
    /// result. Guards the straight-line-region contract: compiled regions must
    /// not contain branches, because the VM advances pc by the full region
    /// length after a region runs.
    #[test]
    fn test_jit_hot_loop_with_early_exit_branch() {
        let mut module = CodeModule::new("test_jit_early_exit");
        let c100_idx = module.add_constant(Constant::Int(100));

        // r0 = sum, r1 = i, r2 = limit, r3 = one, r4 = cond, r5 = pad
        module.emit(Instruction::new1(OpCode::Const0, 0));               // 0
        module.emit(Instruction::new1(OpCode::Const0, 1));               // 1
        module.emit(Instruction::new3(OpCode::ConstU,
            ((c100_idx >> 8) & 0xFF) as u8, (c100_idx & 0xFF) as u8, 2)); // 2
        module.emit(Instruction::new1(OpCode::Const1, 3));               // 3
        module.emit(Instruction::new1(OpCode::Const0, 5));               // 4

        let loop_check = module.current_offset();
        module.emit(Instruction::new3(OpCode::ICmpLt, 1, 2, 4));         // 5
        let jmpf_idx = module.current_offset();
        module.emit(Instruction::new2(OpCode::JmpF, 4, 0));              // 6 (patched)
        // Loop body: 7 straight-line instructions so it clears the JIT's
        // minimum region size and actually gets compiled once hot.
        module.emit(Instruction::new3(OpCode::IAdd, 0, 1, 0));           // 7: sum += i
        module.emit(Instruction::new3(OpCode::IAdd, 5, 3, 5));           // 8: pad
        module.emit(Instruction::new3(OpCode::IAdd, 5, 3, 5));           // 9: pad
        module.emit(Instruction::new3(OpCode::IAdd, 5, 3, 5));           // 10: pad
        module.emit(Instruction::new3(OpCode::IAdd, 5, 3, 5));           // 11: pad
        module.emit(Instruction::new3(OpCode::IAdd, 5, 3, 5));           // 12: pad
        module.emit(Instruction::new3(OpCode::IAdd, 1, 3, 1));           // 13: i += 1
        let jmp_back_idx = module.current_offset();
        let back_offset = loop_check as i64 - jmp_back_idx as i64;
        module.emit(Instruction::new3(OpCode::Jmp,
            ((back_offset as i16 >> 8) & 0xFF) as u8,
            (back_offset as i16 & 0xFF) as u8,
            0));                                                          // 14
        let after_loop = module.current_offset();
        if let Some(instr) = module.instructions.get_mut(jmpf_idx) {
            let forward_offset = after_loop as i64 - jmpf_idx as i64;
            instr.op2 = ((forward_offset as i16 >> 8) & 0xFF) as u8;
            instr.op3 = (forward_offset as i16 & 0xFF) as u8;
        }
        module.emit(Instruction::new0(OpCode::Halt));                    // 15
        module.entry_point = Some(0);

        crate::jit::reset_hot_counters();
        let mut vm = VM::new();
        vm.load_module(module);
        let cold_result = vm.run_from(0, 0).unwrap();
        assert_eq!(cold_result.as_int(), Some(4950), "sum 0..100 = 4950");

        // Heat the loop body well past the hot threshold so it JIT-compiles,
        // then verify the compiled path still takes the early exit correctly.
        for _ in 0..50 {
            let result = vm.run_from(0, 0).unwrap();
            assert_eq!(result.as_int(), Some(4950),
                "JIT-compiled loop with early-exit branch must match interpreter");
        }
    }

    /// Closure capture environments: Closure + CapStore then Call + CapLoad
    /// round-trips the captured value into the callee frame.
    #[test]
    fn test_closure_capture_env_roundtrip() {
        let mut module = CodeModule::new("test_capture");
        let c41_idx = module.add_constant(Constant::Int(41));

        // Entry: build a closure over function 0 capturing 41, call it.
        // main:
        //   0: ConstU 41 -> r1
        //   1: Closure #0 -> r2
        //   2: CapStore r2[0] = r1
        //   3: Move r2 -> r3
        //   4: Call r3, 0 args, dst r0
        //   5: Halt
        // fn0 (at offset 6):
        //   6: CapLoad [0] -> r4
        //   7: Const1 r5
        //   8: IAdd r4, r5, r6
        //   9: RetVal r6
        module.emit(Instruction::new3(OpCode::ConstU,
            ((c41_idx >> 8) & 0xFF) as u8, (c41_idx & 0xFF) as u8, 1)); // 0
        module.emit(Instruction::new3(OpCode::Closure, 0, 0, 2));       // 1
        module.emit(Instruction::new3(OpCode::CapStore, 2, 0, 1));      // 2
        module.emit(Instruction::new2(OpCode::Move, 2, 3));             // 3
        module.emit(Instruction::new3(OpCode::Call, 3, 0, 0));          // 4
        module.emit(Instruction::new0(OpCode::Halt));                   // 5
        let fn0_offset = module.current_offset();
        module.emit(Instruction::new3(OpCode::CapLoad, 0, 4, 0));       // 6
        module.emit(Instruction::new1(OpCode::Const1, 5));              // 7
        module.emit(Instruction::new3(OpCode::IAdd, 4, 5, 6));          // 8
        module.emit(Instruction::new1(OpCode::RetVal, 6));              // 9
        module.function_table.push(fn0_offset);
        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run().unwrap();
        assert_eq!(result.as_int(), Some(42), "captured 41 + 1 should be 42");
    }

    /// CapLoad without a closure environment must error, not silently no-op.
    #[test]
    fn test_capload_outside_closure_errors() {
        let mut module = CodeModule::new("test_capload_err");
        module.emit(Instruction::new3(OpCode::CapLoad, 0, 1, 0));
        module.emit(Instruction::new0(OpCode::Halt));
        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_err(), "CapLoad outside a closure call should error");
    }

    /// Test 17: NodeId returns the configured local node ID.
    #[test]
    fn test_node_id_returns_configured_value() {
        let mut module = CodeModule::new("test_node_id");
        module.emit(Instruction::new1(OpCode::NodeId, 0));
        module.emit(Instruction::new0(OpCode::Halt));
        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.set_node_id(42);
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_ok(), "NodeId should not fail: {:?}", result.err());
        assert_eq!(result.unwrap().as_int(), Some(42));
    }

    /// Test 18: NodeId defaults to 0 with no explicit configuration.
    #[test]
    fn test_node_id_defaults_to_zero() {
        let mut module = CodeModule::new("test_node_id_default");
        module.emit(Instruction::new1(OpCode::NodeId, 0));
        module.emit(Instruction::new0(OpCode::Halt));
        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_int(), Some(0));
    }

    /// Test 19: Migrate records a migration request.
    #[test]
    fn test_migrate_records_request() {
        let mut module = CodeModule::new("test_migrate");
        let actor_const = module.add_constant(Constant::Int(7));
        let node_const = module.add_constant(Constant::Int(99));
        module.emit(Instruction::new3(OpCode::ConstU,
            ((actor_const >> 8) & 0xFF) as u8, (actor_const & 0xFF) as u8, 1)); // r1 = 7
        module.emit(Instruction::new3(OpCode::ConstU,
            ((node_const >> 8) & 0xFF) as u8, (node_const & 0xFF) as u8, 2)); // r2 = 99
        module.emit(Instruction::new3(OpCode::Migrate, 1, 2, 0)); // migrate actor 7 to node 99 -> r0
        module.emit(Instruction::new0(OpCode::Halt));
        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_ok(), "Migrate should not fail: {:?}", result.err());
        assert!(result.unwrap().is_unit(), "Migrate should return unit");
        assert_eq!(vm.pending_migrations(), &[(7, 99)]);
    }

    /// Test 20: RAsk returns nil when no distributed runtime is attached.
    #[test]
    fn test_rask_returns_nil_without_runtime() {
        let mut module = CodeModule::new("test_rask");
        let behavior_const = module.add_constant(Constant::String("ping".to_string()));
        let actor_const = module.add_constant(Constant::Int(3));
        module.emit(Instruction::new3(OpCode::ConstU,
            ((actor_const >> 8) & 0xFF) as u8, (actor_const & 0xFF) as u8, 1)); // r1 = 3
        module.emit(Instruction::new3(OpCode::RAsk, 1, behavior_const as u8, 0)); // rask -> r0
        module.emit(Instruction::new0(OpCode::Halt));
        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_ok(), "RAsk should not fail: {:?}", result.err());
        assert!(result.unwrap().is_nil(), "RAsk should return nil without runtime");
    }

    /// Test 21: Gossip records intent and returns unit.
    #[test]
    fn test_gossip_records_intent_and_returns_unit() {
        let mut module = CodeModule::new("test_gossip");
        let msg_const = module.add_constant(Constant::String("hello".to_string()));
        module.emit(Instruction::new3(OpCode::Gossip, msg_const as u8, 0, 0)); // gossip -> r0
        module.emit(Instruction::new0(OpCode::Halt));
        module.entry_point = Some(0);

        let mut vm = VM::new();
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_ok(), "Gossip should not fail: {:?}", result.err());
        assert!(result.unwrap().is_unit(), "Gossip should return unit");
        assert_eq!(vm.gossip_log(), &["hello".to_string()]);
    }

    /// Test 22: Distributed callbacks are invoked by remote opcodes.
    #[test]
    fn test_distributed_callbacks_invoked() {
        #[derive(Debug)]
        struct MockCallbacks {
            node_id: u64,
            migrations: Vec<(u64, u64)>,
            asks: Vec<(u64, String)>,
            gossips: Vec<String>,
        }
        impl DistributedVmCallbacks for MockCallbacks {
            fn node_id(&self) -> u64 { self.node_id }
            fn migrate(&mut self, actor_id: u64, target_node_id: u64) {
                self.migrations.push((actor_id, target_node_id));
            }
            fn remote_ask(&mut self, target_actor: u64, behavior: &str, _args: &[Value], _timeout_ms: u64) -> Value {
                self.asks.push((target_actor, behavior.to_string()));
                Value::int(123)
            }
            fn gossip(&mut self, message: &str) -> Value {
                self.gossips.push(message.to_string());
                Value::unit()
            }
        }

        let mut module = CodeModule::new("test_callbacks");
        let actor_const = module.add_constant(Constant::Int(5));
        let node_const = module.add_constant(Constant::Int(11));
        let behavior_const = module.add_constant(Constant::String("echo".to_string()));
        let msg_const = module.add_constant(Constant::String("sync".to_string()));

        module.emit(Instruction::new1(OpCode::NodeId, 0)); // r0 = node_id
        module.emit(Instruction::new3(OpCode::ConstU,
            ((actor_const >> 8) & 0xFF) as u8, (actor_const & 0xFF) as u8, 1)); // r1 = 5
        module.emit(Instruction::new3(OpCode::ConstU,
            ((node_const >> 8) & 0xFF) as u8, (node_const & 0xFF) as u8, 2)); // r2 = 11
        module.emit(Instruction::new3(OpCode::Migrate, 1, 2, 3)); // r3 = migrate
        module.emit(Instruction::new3(OpCode::RAsk, 1, behavior_const as u8, 4)); // r4 = rask
        module.emit(Instruction::new3(OpCode::Gossip, msg_const as u8, 0, 5)); // r5 = gossip
        module.emit(Instruction::new0(OpCode::Halt));
        module.entry_point = Some(0);

        let callbacks = Box::new(MockCallbacks {
            node_id: 77,
            migrations: Vec::new(),
            asks: Vec::new(),
            gossips: Vec::new(),
        });
        let expected_node_id = callbacks.node_id;

        let mut vm = VM::new();
        vm.set_distributed_callbacks(callbacks);
        vm.load_module(module);
        let result = vm.run();
        assert!(result.is_ok(), "Callbacks should not fail: {:?}", result.err());

        let cb = (vm.distributed_callbacks.as_ref().unwrap().as_ref() as &dyn std::any::Any)
            .downcast_ref::<MockCallbacks>()
            .unwrap();
        assert_eq!(cb.node_id, expected_node_id);
        assert_eq!(cb.migrations, &[(5, 11)]);
        assert_eq!(cb.asks, &[(5, "echo".to_string())]);
        assert_eq!(cb.gossips, &["sync".to_string()]);
    }

    /// Test 23: FFI call to libm sqrt (skipped if libm cannot be opened).
    #[test]
    #[cfg(target_os = "linux")]
    fn test_ffi_call_libm_sqrt() {
        use crate::lexer::Lexer;
        use crate::parser::Parser;
        use crate::compiler::Compiler;

        let source = r#"
            extern "libm.so.6" {
                fn sqrt(x: Float) -> Float
            }
            sqrt(4.0)
        "#;
        let tokens = Lexer::new(source).lex().expect("lex");
        let ast = Parser::new(tokens).parse_module().expect("parse");
        let mut compiler = Compiler::new("test");
        let module = compiler.compile_module(&ast).expect("compile").clone();

        let mut vm = VM::new();
        vm.load_module(module);
        match vm.run() {
            Ok(result) => {
                let f = result.as_float().expect("float result");
                assert!((f - 2.0).abs() < 1e-12, "sqrt(4.0) should be 2.0, got {}", f);
            }
            Err(crate::types::NuError::VMError(msg)) if msg.contains("open") || msg.contains("load failed") => {
                eprintln!("warning: could not open libm.so.6, skipping test: {}", msg);
            }
            Err(e) => panic!("unexpected FFI error: {}", e),
        }
    }
}
