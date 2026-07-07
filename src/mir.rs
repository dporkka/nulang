//! Mid-level Intermediate Representation (MIR).
//!
//! MIR is a lower-level 3-address code with explicit basic blocks. It is
//! target-independent and serves as the last IR before lowering to bytecode
//! (or, in the future, Cranelift/LLVM/WASM).

use crate::ast::{BinOp, Literal, UnOp};
use crate::bytecode::Constant;
use crate::types::{Capability, Type};

// ---------------------------------------------------------------------------
// IDs and basic structures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub u32);

#[derive(Debug, Clone, PartialEq)]
pub struct Local {
    pub id: LocalId,
    pub name: Option<String>,
    pub ty: Type,
}

// ---------------------------------------------------------------------------
// Module and functions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub name: String,
    pub functions: Vec<Function>,
    pub actor_inits: Vec<ActorInit>,
    pub foreign_functions: Vec<ForeignFunction>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: String,
    pub params: Vec<LocalId>,
    pub ret: Option<Type>,
    pub locals: Vec<Local>,
    pub blocks: Vec<Block>,
    pub entry: BlockId,
    pub is_behavior: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActorInit {
    pub actor_name: String,
    pub behavior_indices: Vec<usize>, // indices into module.functions
    pub init_function: usize, // index into module.functions
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForeignFunction {
    pub library: String,
    pub symbol: String,
    pub params: Vec<Type>,
    pub ret: Type,
}

// ---------------------------------------------------------------------------
// Blocks and statements
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub id: BlockId,
    pub stmts: Vec<Stmt>,
    pub terminator: Terminator,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Assign { dst: LocalId, op: RValue },
    StoreField { obj: LocalId, field: u8, src: LocalId },
    ArrayStore { arr: LocalId, idx: LocalId, src: LocalId },
    StateSet { field_idx: usize, src: LocalId },
    Emit { event: String, args: Vec<LocalId> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RValue {
    Const(Constant),
    Load(LocalId),
    LoadField { obj: LocalId, field: u8 },
    ArrayLoad { arr: LocalId, idx: LocalId },
    Unary(UnOp, LocalId),
    Binary(BinOp, LocalId, LocalId),
    Call { func: FuncRef, args: Vec<LocalId> },
    Closure { func: FuncRef, captures: Vec<LocalId> },
    Tuple(Vec<LocalId>),
    Record(Vec<(String, LocalId)>),
    Array { len: LocalId },
    Spawn { behavior_idx: usize, init: LocalId },
    Send { actor: LocalId, behavior_id: u16, args: Vec<LocalId> },
    Ask { actor: LocalId, behavior_id: u16, args: Vec<LocalId> },
    Perform { effect_id: u16, op_id: u16, args: Vec<LocalId> },
    FFICall { idx: usize, args: Vec<LocalId> },
    SelfRef,
    NodeId,
    CapabilityCheck { val: LocalId, required: Capability },
}

#[derive(Debug, Clone, PartialEq)]
pub enum FuncRef {
    Named(String),
    Index(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Terminator {
    Return(Option<LocalId>),
    Jump(BlockId),
    Branch { cond: LocalId, then_: BlockId, else_: BlockId },
    Switch { val: LocalId, cases: Vec<(Literal, BlockId)>, default: BlockId },
    Handle { body: BlockId, handlers: Vec<Handler>, resume: BlockId },
    Unwind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Handler {
    pub effect_id: u16,
    pub op_id: u16,
    pub params: Vec<LocalId>,
    pub resume: bool,
    pub body: BlockId,
}

// ---------------------------------------------------------------------------
// Builder helpers
// ---------------------------------------------------------------------------

pub struct FunctionBuilder {
    name: String,
    params: Vec<LocalId>,
    ret: Option<Type>,
    locals: Vec<Local>,
    blocks: Vec<Block>,
    current: BlockId,
    next_local: u32,
    next_block: u32,
    is_behavior: bool,
}

impl FunctionBuilder {
    pub fn new(name: impl Into<String>, ret: Option<Type>) -> Self {
        let mut builder = FunctionBuilder {
            name: name.into(),
            params: Vec::new(),
            ret,
            locals: Vec::new(),
            blocks: Vec::new(),
            current: BlockId(0),
            next_local: 0,
            next_block: 0,
            is_behavior: false,
        };
        builder.create_block(); // entry block
        builder
    }

    pub fn behavior(name: impl Into<String>, ret: Option<Type>) -> Self {
        let mut b = Self::new(name, ret);
        b.is_behavior = true;
        b
    }

    pub fn add_param(&mut self, name: impl Into<String>, ty: Type) -> LocalId {
        let id = self.add_local(name, ty);
        self.params.push(id);
        id
    }

    pub fn add_local(&mut self, name: impl Into<String>, ty: Type) -> LocalId {
        let id = LocalId(self.next_local);
        self.next_local += 1;
        self.locals.push(Local { id, name: Some(name.into()), ty });
        id
    }

    pub fn find_local(&self, name: &str) -> Option<LocalId> {
        self.locals.iter().find(|l| l.name.as_deref() == Some(name)).map(|l| l.id)
    }

    pub fn add_temp(&mut self, ty: Type) -> LocalId {
        let id = LocalId(self.next_local);
        self.next_local += 1;
        self.locals.push(Local { id, name: None, ty });
        id
    }

    pub fn create_block(&mut self) -> BlockId {
        let id = BlockId(self.next_block);
        self.next_block += 1;
        self.blocks.push(Block { id, stmts: Vec::new(), terminator: Terminator::Unwind });
        id
    }

    pub fn switch_to(&mut self, block: BlockId) {
        self.current = block;
    }

    pub fn current_block(&self) -> BlockId {
        self.current
    }

    pub fn emit(&mut self, stmt: Stmt) {
        self.blocks[self.current.0 as usize].stmts.push(stmt);
    }

    pub fn assign(&mut self, dst: LocalId, op: RValue) {
        self.emit(Stmt::Assign { dst, op });
    }

    pub fn terminate(&mut self, term: Terminator) {
        let idx = self.current.0 as usize;
        self.blocks[idx].terminator = term;
    }

    pub fn build(self) -> Function {
        Function {
            name: self.name,
            params: self.params,
            ret: self.ret,
            locals: self.locals,
            blocks: self.blocks,
            entry: BlockId(0),
            is_behavior: self.is_behavior,
        }
    }
}

impl Module {
    pub fn new(name: impl Into<String>) -> Self {
        Module {
            name: name.into(),
            functions: Vec::new(),
            actor_inits: Vec::new(),
            foreign_functions: Vec::new(),
        }
    }
}
