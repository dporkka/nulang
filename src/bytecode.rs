//! Bytecode format: register-based, fixed-width 32-bit instructions.

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Opcodes
// ---------------------------------------------------------------------------

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpCode {
    // -- Constants & Moves --
    LoadConst,      // rA = const[B]
    LoadNull,       // rA = null
    Move,           // rA = rB

    // -- Arithmetic --
    Add, Sub, Mul, Div, Mod,
    Neg,            // unary

    // -- Comparisons --
    Eq, Ne, Lt, Le, Gt, Ge,

    // -- Logic --
    And, Or, Not,

    // -- Control flow --
    Jump,           // pc += offset
    JumpIf,         // if rA { pc += offset }
    JumpIfNot,      // if !rA { pc += offset }
    Call,           // rA = rB(rC..rD)
    TailCall,       // tail call optimization
    Ret,            // return rA

    // -- Heap --
    NewTuple,       // rA = tuple(rB..rC)
    NewRecord,      // rA = record { fieldB: rC, ... }
    FieldGet,       // rA = rB.fieldC
    FieldSet,       // rB.fieldC = rA
    NewArray,       // rA = array(len=rB)
    ArrayGet,       // rA = rB[rC]
    ArraySet,       // rB[rC] = rA
    Cons,           // rA = rB :: rC

    // -- Pattern matching --
    TestTag,        // rA = rB matches tag C
    TestTupleLen,   // rA = tuple_len(rB) == C
    Destructure,    // rA..rB = destructure rC

    // -- Actor operations --
    Spawn,          // rA = spawn behaviorB(initC)
    Send,           // send rA ! behaviorB(argsC..D)
    Ask,            // rA = ask rB ! behaviorC(argsD..E)
    Receive,        // rA = receive { table_offset B }
    SelfAddr,       // rA = self()

    // -- Effects --
    Perform,        // rA = perform effectB.op(argsC..)
    Handle,         // setup handler table at offset B

    // -- Administrative --
    PopHandler,     // remove top effect handler
    Migrate,        // migrate rA -> node rB
    Halt,           // stop execution
}

impl OpCode {
    pub fn from_u8(v: u8) -> Option<Self> {
        use OpCode::*;
        Some(match v {
            0 => LoadConst, 1 => LoadNull, 2 => Move,
            3 => Add, 4 => Sub, 5 => Mul, 6 => Div, 7 => Mod, 8 => Neg,
            9 => Eq, 10 => Ne, 11 => Lt, 12 => Le, 13 => Gt, 14 => Ge,
            15 => And, 16 => Or, 17 => Not,
            18 => Jump, 19 => JumpIf, 20 => JumpIfNot,
            21 => Call, 22 => TailCall, 23 => Ret,
            24 => NewTuple, 25 => NewRecord, 26 => FieldGet, 27 => FieldSet,
            28 => NewArray, 29 => ArrayGet, 30 => ArraySet, 31 => Cons,
            32 => TestTag, 33 => TestTupleLen, 34 => Destructure,
            35 => Spawn, 36 => Send, 37 => Ask, 38 => Receive, 39 => SelfAddr,
            40 => Perform, 41 => Handle, 42 => PopHandler, 43 => Migrate, 44 => Halt,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// Instruction (32-bit fixed width)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instruction {
    pub opcode: u8,
    pub a: u8,
    pub b: u8,
    pub c: u8,
}

impl Instruction {
    pub fn encode(op: OpCode, a: u8, b: u8, c: u8) -> Self {
        Instruction { opcode: op as u8, a, b, c }
    }

    /// Extended operand: combine b and c into 16-bit value
    pub fn extended_bc(&self) -> u16 {
        ((self.b as u16) << 8) | (self.c as u16)
    }

    /// Decode from u32
    pub fn from_u32(word: u32) -> Self {
        Instruction {
            opcode: ((word >> 24) & 0xFF) as u8,
            a: ((word >> 16) & 0xFF) as u8,
            b: ((word >> 8) & 0xFF) as u8,
            c: (word & 0xFF) as u8,
        }
    }

    /// Encode to u32
    pub fn to_u32(&self) -> u32 {
        ((self.opcode as u32) << 24)
            | ((self.a as u32) << 16)
            | ((self.b as u32) << 8)
            | (self.c as u32)
    }
}

// ---------------------------------------------------------------------------
// Constant pool
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
    Unit,
    TypeDescriptor(Vec<u8>),
    FunRef { module: u32, index: u32 },
    BehaviorRef { module: u32, actor: u32, behavior: u32 },
}

// ---------------------------------------------------------------------------
// Bytecode module
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BehaviorTableEntry {
    pub name: String,
    pub param_count: u8,
    pub entry_point: u32,
    pub effect_annotation: Option<Vec<u8>>, // serialized
}

#[derive(Debug, Clone)]
pub struct Module {
    pub name: String,
    pub constants: Vec<Constant>,
    pub instructions: Vec<Instruction>,
    pub behavior_table: Vec<BehaviorTableEntry>,
    pub string_table: Vec<String>,
    pub debug_info: HashMap<u32, (u32, u32)>, // pc -> (line, col)
}

impl Module {
    pub fn new(name: String) -> Self {
        Module {
            name,
            constants: Vec::new(),
            instructions: Vec::new(),
            behavior_table: Vec::new(),
            string_table: Vec::new(),
            debug_info: HashMap::new(),
        }
    }

    pub fn add_constant(&mut self, c: Constant) -> u16 {
        let idx = self.constants.len();
        self.constants.push(c);
        idx as u16
    }

    pub fn emit(&mut self, op: OpCode, a: u8, b: u8, c: u8) -> u32 {
        let pc = self.instructions.len() as u32;
        self.instructions.push(Instruction::encode(op, a, b, c));
        pc
    }

    pub fn add_string(&mut self, s: String) -> u16 {
        let idx = self.string_table.len();
        self.string_table.push(s);
        idx as u16
    }

    pub fn patch_jump(&mut self, pc: u32, offset: i16) {
        let inst = &mut self.instructions[pc as usize];
        let bc = offset as u16;
        inst.b = ((bc >> 8) & 0xFF) as u8;
        inst.c = (bc & 0xFF) as u8;
    }
}
