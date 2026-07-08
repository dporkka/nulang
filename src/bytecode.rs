//! Bytecode ISA, instruction encoding, and module format for the Nulang VM.

use crate::ai::request::ToolSchema;

// ---------------------------------------------------------------------------
// Opcodes (91 total across 10 categories)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OpCode {
    // == Special (0x00-0x0F) ==
    Nop     = 0x00, // No operation
    Halt    = 0x01, // Stop execution
    Panic   = 0x02, // Runtime panic with message from const pool
    Const0  = 0x03, // Load constant 0 (small int optimization)
    Const1  = 0x04, // Load constant 1
    Const2  = 0x05, // Load constant 2
    ConstM1 = 0x06, // Load constant -1
    ConstU  = 0x07, // Load constant from pool (idx: u16)
    ConstL  = 0x08, // Load large constant from pool (idx: u32)

    // == Stack & Locals (0x10-0x1F) ==
    Load    = 0x10, // Load from local register (src_reg, dst_reg, _)
    Store   = 0x11, // Store to local register (src_reg, dst_reg, _)
    Move    = 0x12, // Register to register copy (src, dst, _)
    Pop     = 0x13, // Pop top of call stack into register
    Dup     = 0x14, // Duplicate register value
    Swap    = 0x15, // Swap two registers

    // == Arithmetic - Integer (0x20-0x2F) ==
    IAdd    = 0x20, // Integer add (r1, r2, dst)
    ISub    = 0x21, // Integer sub
    IMul    = 0x22, // Integer mul
    IDiv    = 0x23, // Integer div (checked)
    IMod    = 0x24, // Integer modulo
    INeg    = 0x25, // Integer negate
    IInc    = 0x26, // Increment register by 1
    IDec    = 0x27, // Decrement register by 1
    IPow    = 0x28, // Integer power
    Xor     = 0x29, // Bitwise xor
    Shl     = 0x2A, // Bitwise shift left
    Shr     = 0x2B, // Bitwise shift right
    BitAnd  = 0x2C, // Bitwise and
    BitOr   = 0x2D, // Bitwise or

    // == Arithmetic - Float (0x30-0x3F) ==
    FAdd    = 0x30, // Float add
    FSub    = 0x31, // Float sub
    FMul    = 0x32, // Float mul
    FDiv    = 0x33, // Float div
    FNeg    = 0x34, // Float negate
    FMod    = 0x35, // Float modulo
    IToF    = 0x36, // Int to Float conversion
    FToI    = 0x37, // Float to Int (truncate)
    FToS    = 0x38, // Float to String

    // == Comparison & Logic (0x40-0x4F) ==
    ICmpEq  = 0x40, // Int compare ==
    ICmpLt  = 0x41, // Int compare <
    ICmpGt  = 0x42, // Int compare >
    ICmpLe  = 0x43, // Int <=
    ICmpGe  = 0x44, // Int >=
    FCmpEq  = 0x45, // Float ==
    FCmpLt  = 0x46, // Float <
    FCmpGt  = 0x47, // Float >
    SCmpEq  = 0x48, // String ==
    Not     = 0x49, // Boolean not
    And     = 0x4A, // Boolean and
    Or      = 0x4B, // Boolean or

    // == Control Flow (0x50-0x5F) ==
    Jmp     = 0x50, // Unconditional jump (offset: i16)
    JmpT    = 0x51, // Jump if true (reg, offset: i16)
    JmpF    = 0x52, // Jump if false
    Switch  = 0x53, // Switch table (reg, table_idx)
    Call    = 0x54, // Call function (func_reg, argc, dst_reg)
    TailCall= 0x55, // Tail call optimization
    Ret     = 0x56, // Return from function
    RetVal  = 0x57, // Return value in register

    // == Closures (0x60-0x6F) ==
    Closure = 0x60, // Create closure (func_idx, env_count, dst)
    CapLoad = 0x61, // Load from capture (closure_reg, idx, dst)
    CapStore= 0x62, // Store to capture
    FreeVar = 0x63, // Free variable capture declaration
    ClosureCall = 0x64, // Call closure (closure_reg, argc, dst)

    // == Memory & Objects (0x70-0x7F) ==
    Alloc   = 0x70, // Allocate object (size, type_id, dst)
    FieldL  = 0x71, // Load field (obj_reg, field_idx, dst)
    FieldS  = 0x72, // Store field (obj_reg, field_idx, src)
    ArrAlloc= 0x73, // Allocate array (len_reg, elem_type, dst)
    ArrLoad = 0x74, // Array load (arr_reg, idx_reg, dst)
    ArrStore= 0x75, // Array store
    ArrLen  = 0x76, // Array length (arr_reg, dst)
    TupleMk = 0x77, // Create tuple (count, dst)
    TupleL  = 0x78, // Tuple field load
    RecMk   = 0x79, // Create record (field_count, dst)
    RecL    = 0x7A, // Record field load by name (const_idx)
    RecS    = 0x7B, // Record field store
    IsTag   = 0x7C, // Variant tag check (val_reg, tag_id, dst)
    Unpack  = 0x7D, // Variant unpack (val_reg, dst)
    Copy    = 0x7E, // Deep copy (ref_cap, src, dst)
    Drop    = 0x7F, // Drop / deallocate (rc_dec or free)

    // == Actor & Concurrency (0x80-0x8F) ==
    Spawn   = 0x80, // Spawn actor (behavior_idx, init_reg, dst_addr)
    Send    = 0x81, // Send message (addr_reg, behavior_id, args...)
    Ask     = 0x82, // Ask / request-response
    SelfOp   = 0x83, // Get self actor address (dst)
    Receive  = 0x84, // Receive / await message (timeout_reg)
    Monitor  = 0x85, // Monitor actor (target_addr, dst)
    Demon    = 0x86, // Demonitor
    Link     = 0x87, // Link actors bidirectionally
    Unlink   = 0x88, // Unlink actors
    Exit     = 0x89, // Exit / terminate actor (reason_reg)
    Yield    = 0x8A, // Yield execution (reduction quota exhausted)
    StateGet = 0x8B, // Load current actor state field by name (field_const_idx, dst)
    StateSet = 0x8C, // Store to current actor state field by name (val_reg, field_const_idx)
    Emit     = 0x8D, // Emit event (event_name_const_idx, arg_count)
    SignalWait = 0x8E, // Workflow signal wait (signal_name_const_idx, dst)

    // == Effects (0x90-0x93) ==
    Perform = 0x90, // Perform effect operation (eff_id, op_id, args, dst)
    Handle  = 0x91, // Install effect handler (handler_table_idx)
    Resume  = 0x92, // Resume from effect handler with value (val_reg)
    Unwind  = 0x93, // Unwind effect handler

    // == Python Interop (0x94-0x9B) ==
    PyImport  = 0x94, // Import Python module (module_name_const_idx, dst_reg, _)
    PyGetAttr = 0x95, // Get attribute from Python object (obj_reg, attr_name_const_idx, dst_reg)
    PyCall    = 0x96, // Call Python callable (callable_reg, arg_count, dst_reg)
    PyCallKw  = 0x97, // Call Python callable with kwargs (callable_reg, args_tuple_reg, kwargs_dict_reg, dst_reg uses op3)
    PySetAttr = 0x98, // Set attribute on Python object (obj_reg, attr_name_const_idx, val_reg)
    PyToNu    = 0x99, // Convert Python object to Nulang Value (py_val_reg, dst_reg, _)
    PyFromNu  = 0x9A, // Convert Nulang Value to Python object (nu_val_reg, dst_reg, _)
    PyRelease = 0x9B, // Decrement Python object reference count (py_val_reg, _, _)
    LlmAsk    = 0x9C, // LLM ask (model_const_idx in op1+op2, prompt/dst reg in op3)
    PipelineNew   = 0x9D, // Create a new pipeline (dst)
    PipelineStage = 0x9E, // Add stage to pipeline (reads r0=id, r1=name, r2=actor, r3=template; dst)
    PipelineRun   = 0x9F, // Run pipeline (reads r0=id, r1=input; dst)

    // == Capabilities (0xA0-0xAF) ==
    CapChk  = 0xA0, // Capability check (required_cap, fail_label)
    CapUp   = 0xA1, // Capability upgrade (iso <- trn, etc.)
    CapDown = 0xA2, // Capability downgrade (ref -> box)
    CapSend = 0xA3, // Mark value as sendable (check iso/val/tag)

    // == FFI (0xB0-0xBF) ==
    FFICall = 0xB0, // Call foreign function (func_idx high, func_idx low, dst)

    // == Supervisor (0xC0-0xCF) ==
    SupervisorNew    = 0xC0, // Create a new supervisor team (dst)
    SupervisorWorker = 0xC1, // Add worker to team (reads r0=id, r1=name, r2=actor, r3=description; dst)
    SupervisorRun    = 0xC2, // Run supervisor team (reads r0=id, r1=task; dst)

    // == Debate (0xC3-0xCF) ==
    DebateNew        = 0xC3, // Create a new debate (reads r0=topic, r1=rounds, r2=threshold; dst)
    DebateParticipant = 0xC4, // Add participant (reads r0=id, r1=name, r2=stance, r3=actor; dst)
    DebateRun        = 0xC5, // Run debate (reads r0=id; dst)

    // == Distribution (0xD0-0xDF) ==
    NodeId  = 0xD0, // Get current node id (dst)
    Migrate = 0xD1, // Migrate actor (addr_reg, node_id_reg, dst)
    RSend   = 0xD2, // Remote send (addr_reg, behavior_id, args)
    RAsk    = 0xD3, // Remote ask
    RSpawn  = 0xD4, // Remote spawn (node_id, behavior, init)
    Gossip  = 0xD5, // Gossip cluster state

    // == String & IO (0xE0-0xEF) ==
    SConcat = 0xE0, // String concatenation
    SPrint  = 0xE1, // Print to stdout
    SRead   = 0xE2, // Read line from stdin
    FOpen   = 0xE3, // File open
    FRead   = 0xE4, // File read
    FWrite  = 0xE5, // File write
    FClose  = 0xE6, // File close
    Print   = 0xE7, // Print any value (uses debug fmt)

    // == Debug & Meta (0xF0-0xFF) ==
    DbgBreak= 0xF0, // Debugger breakpoint
    DbgPrint= 0xF1, // Debug print register state
    DbgStack= 0xF2, // Debug print call stack
    MetaType= 0xF3, // Get type of value at runtime
    MetaCap = 0xF4, // Get capability of reference at runtime
}

impl OpCode {
    pub fn from_u8(v: u8) -> Option<Self> {
        use OpCode::*;
        match v {
            0x00 => Some(Nop), 0x01 => Some(Halt), 0x02 => Some(Panic),
            0x03 => Some(Const0), 0x04 => Some(Const1), 0x05 => Some(Const2),
            0x06 => Some(ConstM1), 0x07 => Some(ConstU), 0x08 => Some(ConstL),
            0x10 => Some(Load), 0x11 => Some(Store), 0x12 => Some(Move),
            0x13 => Some(Pop), 0x14 => Some(Dup), 0x15 => Some(Swap),
            0x20 => Some(IAdd), 0x21 => Some(ISub), 0x22 => Some(IMul),
            0x23 => Some(IDiv), 0x24 => Some(IMod), 0x25 => Some(INeg),
            0x26 => Some(IInc), 0x27 => Some(IDec), 0x28 => Some(IPow),
            0x29 => Some(Xor), 0x2A => Some(Shl), 0x2B => Some(Shr),
            0x2C => Some(BitAnd), 0x2D => Some(BitOr),
            0x30 => Some(FAdd), 0x31 => Some(FSub), 0x32 => Some(FMul),
            0x33 => Some(FDiv), 0x34 => Some(FNeg), 0x35 => Some(FMod),
            0x36 => Some(IToF), 0x37 => Some(FToI), 0x38 => Some(FToS),
            0x40 => Some(ICmpEq), 0x41 => Some(ICmpLt), 0x42 => Some(ICmpGt),
            0x43 => Some(ICmpLe), 0x44 => Some(ICmpGe),
            0x45 => Some(FCmpEq), 0x46 => Some(FCmpLt), 0x47 => Some(FCmpGt),
            0x48 => Some(SCmpEq), 0x49 => Some(Not), 0x4A => Some(And), 0x4B => Some(Or),
            0x50 => Some(Jmp), 0x51 => Some(JmpT), 0x52 => Some(JmpF),
            0x53 => Some(Switch), 0x54 => Some(Call), 0x55 => Some(TailCall),
            0x56 => Some(Ret), 0x57 => Some(RetVal),
            0x60 => Some(Closure), 0x61 => Some(CapLoad), 0x62 => Some(CapStore),
            0x63 => Some(FreeVar), 0x64 => Some(ClosureCall),
            0x70 => Some(Alloc), 0x71 => Some(FieldL), 0x72 => Some(FieldS),
            0x73 => Some(ArrAlloc), 0x74 => Some(ArrLoad), 0x75 => Some(ArrStore),
            0x76 => Some(ArrLen), 0x77 => Some(TupleMk), 0x78 => Some(TupleL),
            0x79 => Some(RecMk), 0x7A => Some(RecL), 0x7B => Some(RecS),
            0x7C => Some(IsTag), 0x7D => Some(Unpack), 0x7E => Some(Copy),
            0x7F => Some(Drop),
            0x80 => Some(Spawn), 0x81 => Some(Send), 0x82 => Some(Ask),
            0x83 => Some(SelfOp), 0x84 => Some(Receive), 0x85 => Some(Monitor),
            0x86 => Some(Demon), 0x87 => Some(Link), 0x88 => Some(Unlink),
            0x89 => Some(Exit), 0x8A => Some(Yield),
            0x8B => Some(StateGet), 0x8C => Some(StateSet), 0x8D => Some(Emit),
            0x8E => Some(SignalWait),
            0x90 => Some(Perform), 0x91 => Some(Handle), 0x92 => Some(Resume),
            0x93 => Some(Unwind),
            0x94 => Some(PyImport), 0x95 => Some(PyGetAttr), 0x96 => Some(PyCall),
            0x97 => Some(PyCallKw), 0x98 => Some(PySetAttr), 0x99 => Some(PyToNu),
            0x9A => Some(PyFromNu), 0x9B => Some(PyRelease), 0x9C => Some(LlmAsk),
            0x9D => Some(PipelineNew), 0x9E => Some(PipelineStage), 0x9F => Some(PipelineRun),
            0xA0 => Some(CapChk), 0xA1 => Some(CapUp), 0xA2 => Some(CapDown),
            0xA3 => Some(CapSend),
            0xB0 => Some(FFICall),
            0xC0 => Some(SupervisorNew), 0xC1 => Some(SupervisorWorker), 0xC2 => Some(SupervisorRun),
            0xC3 => Some(DebateNew), 0xC4 => Some(DebateParticipant), 0xC5 => Some(DebateRun),
            0xD0 => Some(NodeId), 0xD1 => Some(Migrate), 0xD2 => Some(RSend),
            0xD3 => Some(RAsk), 0xD4 => Some(RSpawn), 0xD5 => Some(Gossip),
            0xE0 => Some(SConcat), 0xE1 => Some(SPrint), 0xE2 => Some(SRead),
            0xE3 => Some(FOpen), 0xE4 => Some(FRead), 0xE5 => Some(FWrite),
            0xE6 => Some(FClose), 0xE7 => Some(Print),
            0xF0 => Some(DbgBreak), 0xF1 => Some(DbgPrint), 0xF2 => Some(DbgStack),
            0xF3 => Some(MetaType), 0xF4 => Some(MetaCap),
            _ => None,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

// ---------------------------------------------------------------------------
// Instruction Encoding
// ---------------------------------------------------------------------------

/// 32-bit fixed-width instruction.
/// Layout: [opcode: u8] [op1: u8] [op2: u8] [op3: u8]
/// Extended format for larger immediates uses op1+op2 as u16, or op1+op2+op3 as u24.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instruction {
    pub opcode: OpCode,
    pub op1: u8,
    pub op2: u8,
    pub op3: u8,
}

impl Instruction {
    pub fn new0(opcode: OpCode) -> Self {
        Instruction { opcode, op1: 0, op2: 0, op3: 0 }
    }
    pub fn new1(opcode: OpCode, a: u8) -> Self {
        Instruction { opcode, op1: a, op2: 0, op3: 0 }
    }
    pub fn new2(opcode: OpCode, a: u8, b: u8) -> Self {
        Instruction { opcode, op1: a, op2: b, op3: 0 }
    }
    pub fn new3(opcode: OpCode, a: u8, b: u8, c: u8) -> Self {
        Instruction { opcode, op1: a, op2: b, op3: c }
    }

    /// Encode as u32 (big-endian: opcode | op1 | op2 | op3).
    pub fn encode(&self) -> u32 {
        ((self.opcode.as_u8() as u32) << 24)
            | ((self.op1 as u32) << 16)
            | ((self.op2 as u32) << 8)
            | (self.op3 as u32)
    }

    /// Decode from u32.
    pub fn decode(encoded: u32) -> Option<Self> {
        let opcode = OpCode::from_u8((encoded >> 24) as u8)?;
        Some(Instruction {
            opcode,
            op1: ((encoded >> 16) & 0xFF) as u8,
            op2: ((encoded >> 8) & 0xFF) as u8,
            op3: (encoded & 0xFF) as u8,
        })
    }

    /// Get 16-bit immediate from op1+op2 (used by Jmp, ConstU, Call, etc.)
    pub fn imm16(&self) -> u16 {
        ((self.op1 as u16) << 8) | (self.op2 as u16)
    }

    /// Get signed 16-bit immediate from op1+op2.
    pub fn simm16(&self) -> i16 {
        self.imm16() as i16
    }

    /// Get 16-bit offset from op2+op3 (used by JmpT, JmpF which store reg in op1)
    pub fn offset16(&self) -> i16 {
        (((self.op2 as u16) << 8) | (self.op3 as u16)) as i16
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
    Nil,
    Unit,
    TypeDescriptor(String), // String representation of type
    FunctionRef(usize),     // Index into function table
    BehaviorRef(usize),     // Index into behavior table
}

// ---------------------------------------------------------------------------
// Effect Handler Table
// ---------------------------------------------------------------------------

/// A single binding from effect name to handler code offset.
/// Compiled by the compiler when processing `handle eff_name -> { body }` blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct HandlerBinding {
    pub effect_name: String,
    /// Bytecode offset of the handler body (receives args in r0..rn).
    pub handler_offset: usize,
    /// Number of arguments the effect operation expects.
    pub arg_count: u8,
    /// Register to place the effect operation result into (for resume).
    pub result_reg: u8,
}

/// A handler table: maps effect names to their handler implementations.
/// One table per `handle { ... }` block. Pushed onto the handler stack at
/// runtime by the Handle opcode.
#[derive(Debug, Clone, PartialEq)]
pub struct HandlerTable {
    pub bindings: Vec<HandlerBinding>,
    /// Optional fallback: code offset to jump to if no binding matches.
    /// If None, an unhandled effect triggers a runtime error.
    pub fallback_offset: Option<usize>,
}

// ---------------------------------------------------------------------------
// Behavior Table Entry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct BehaviorTableEntry {
    pub name: String,
    pub param_count: usize,
    pub code_offset: usize,    // Offset into bytecode
    pub local_count: usize,    // Number of local registers needed
    pub effect_mask: u32,      // Which effects this behavior may perform (bitmap)
    /// Optional code offset for the saga compensation expression of this step.
    pub compensate_offset: Option<usize>,
    /// For synthetic parallel steps: the ordered names of the branches.
    /// `None` for normal sequential steps.
    pub parallel_branches: Option<Vec<String>>,
}

/// Actor metadata for durable execution.
#[derive(Debug, Clone, PartialEq)]
pub struct ActorMeta {
    pub name: String,
    pub persistent: bool,
    /// State field name -> model (Local, Durable, EventSourced, Crdt).
    pub state_models: Vec<(String, crate::ast::StateModel)>,
    /// Default values for state fields (literals only in the MVP).
    pub state_defaults: Vec<(String, Constant)>,
    /// Indices into the behavior table that belong to this actor.
    pub behavior_indices: Vec<usize>,
    /// True if this actor was generated from a `workflow` declaration.
    pub is_workflow: bool,
    /// True if this actor was generated from an `agent` declaration.
    pub is_agent: bool,
    /// Tool schemas exposed to this agent actor.
    pub tools: Vec<ToolSchema>,
    /// Semantic-memory vector dimensions, if configured for this agent.
    pub semantic_memory_dimensions: Option<usize>,
    /// Procedural-memory namespace, if configured for this agent.
    pub procedural_memory_namespace: Option<String>,
}

impl ActorMeta {
    pub fn new(name: impl Into<String>) -> Self {
        ActorMeta {
            name: name.into(),
            persistent: false,
            state_models: Vec::new(),
            state_defaults: Vec::new(),
            behavior_indices: Vec::new(),
            is_workflow: false,
            is_agent: false,
            tools: Vec::new(),
            semantic_memory_dimensions: None,
            procedural_memory_namespace: None,
        }
    }
}

// ---------------------------------------------------------------------------
// FFI Function Definition
// ---------------------------------------------------------------------------

/// FFI primitive types supported by the bytecode compiler and VM.
#[derive(Debug, Clone, PartialEq)]
pub enum FfiType {
    Int,
    Float,
    Bool,
    String,
    Unit,
    Pointer,
}

/// A foreign function declared in an `extern "lib" { ... }` block.
#[derive(Debug, Clone, PartialEq)]
pub struct ForeignFunctionDef {
    pub library: String,
    pub symbol: String,
    pub params: Vec<FfiType>,
    pub ret: FfiType,
}

// ---------------------------------------------------------------------------
// Code Module
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct CodeModule {
    pub name: String,
    pub constants: Vec<Constant>,
    pub instructions: Vec<Instruction>,
    pub behaviors: Vec<BehaviorTableEntry>,
    pub function_table: Vec<usize>, // code offsets for named functions
    pub exports: Vec<(String, usize)>, // name -> constant/function index
    /// Entry point for inline __main (None if no __main, defaults to 0 in VM)
    pub entry_point: Option<usize>,
    /// Effect handler tables: one per `handle { ... }` block.
    /// Indexed by the handler_table_idx operand of the Handle opcode.
    pub handler_tables: Vec<HandlerTable>,
    /// Actor metadata for durable execution (v0.7).
    pub actor_metadata: Vec<ActorMeta>,
    /// Foreign function definitions from `extern` blocks.
    pub foreign_functions: Vec<ForeignFunctionDef>,
    /// Tool schemas for functions annotated with `@tool(description: "...")`.
    pub tools: Vec<ToolSchema>,
}

impl CodeModule {
    pub fn new(name: impl Into<String>) -> Self {
        CodeModule {
            name: name.into(),
            constants: Vec::new(),
            instructions: Vec::new(),
            behaviors: Vec::new(),
            function_table: Vec::new(),
            exports: Vec::new(),
            entry_point: None,
            handler_tables: Vec::new(),
            actor_metadata: Vec::new(),
            foreign_functions: Vec::new(),
            tools: Vec::new(),
        }
    }

    pub fn add_actor_meta(&mut self, meta: ActorMeta) -> usize {
        let idx = self.actor_metadata.len();
        self.actor_metadata.push(meta);
        idx
    }

    pub fn emit(&mut self, instr: Instruction) -> usize {
        let idx = self.instructions.len();
        self.instructions.push(instr);
        idx
    }

    pub fn patch_jump(&mut self, instr_idx: usize, target_offset: i16) {
        if let Some(instr) = self.instructions.get_mut(instr_idx) {
            let abs_offset = (instr_idx as i64 + target_offset as i64) as u16;
            instr.op1 = (abs_offset >> 8) as u8;
            instr.op2 = (abs_offset & 0xFF) as u8;
        }
    }

    pub fn add_constant(&mut self, c: Constant) -> usize {
        let idx = self.constants.len();
        self.constants.push(c);
        idx
    }

    pub fn add_behavior(&mut self, b: BehaviorTableEntry) -> usize {
        let idx = self.behaviors.len();
        self.behaviors.push(b);
        idx
    }

    pub fn add_handler_table(&mut self, ht: HandlerTable) -> usize {
        let idx = self.handler_tables.len();
        self.handler_tables.push(ht);
        idx
    }

    pub fn current_offset(&self) -> usize {
        self.instructions.len()
    }
}
