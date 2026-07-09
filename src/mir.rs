//! Mid-level Intermediate Representation (MIR).
//!
//! MIR is a lower-level 3-address code with explicit basic blocks. It is
//! target-independent and serves as the last IR before lowering to bytecode
//! (or, in the future, Cranelift/LLVM/WASM).
//!
//! Every block carries an explicit terminator; `Terminator::Unterminated` is
//! the builder's placeholder and reaching codegen with it is an internal
//! error, never silent misbehavior.

use crate::ast::{BinOp, UnOp};
use crate::bytecode::{ActorMeta, Constant};
use crate::types::Type;

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
    /// Actor behaviors, compiled the same way as `functions` but never
    /// addressable via `Call` — only through `RValue::Spawn/Send/Ask`, which
    /// reference them by index into this vector.
    pub behaviors: Vec<Function>,
    /// One entry per lowered `actor` declaration. `behavior_indices` are
    /// indices into `behaviors` above; codegen copies this vector into
    /// `CodeModule.actor_metadata` unchanged.
    pub actor_metadata: Vec<ActorMeta>,
    pub foreign_functions: Vec<ForeignFunction>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: String,
    pub params: Vec<LocalId>,
    /// Locals populated from the enclosing closure's capture slots, in slot
    /// order. Codegen emits a `CapLoad` prologue for these.
    pub captures: Vec<LocalId>,
    pub ret: Option<Type>,
    pub locals: Vec<Local>,
    pub blocks: Vec<Block>,
    pub entry: BlockId,
    /// Effect-handler tables installed by `Stmt::EnterHandle` in this
    /// function. Offsets are resolved by codegen.
    pub handler_tables: Vec<HandlerTableDef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForeignFunction {
    pub library: String,
    pub symbol: String,
    pub params: Vec<Type>,
    pub ret: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HandlerTableDef {
    pub bindings: Vec<HandlerBindingDef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HandlerBindingDef {
    /// Effect name matched against `Perform` at runtime (e.g. "IO").
    pub effect_name: String,
    /// Handler parameters; the VM delivers arguments in r0..rN, and codegen
    /// moves them into these locals at the handler block's start.
    pub params: Vec<LocalId>,
    /// Block containing the handler body; must end in `Terminator::Resume`.
    pub body: BlockId,
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
    /// Store to a named record field (module-wide field id resolved by codegen).
    StoreFieldNamed { obj: LocalId, field: String, src: LocalId },
    /// Store to a positional tuple slot.
    StoreTupleSlot { obj: LocalId, slot: u8, src: LocalId },
    ArrayStore { arr: LocalId, idx: LocalId, src: LocalId },
    /// Install handler table `table` (index into `Function::handler_tables`).
    EnterHandle { table: usize },
    /// Pop the innermost handler frame (bytecode `Unwind`).
    PopHandler,
    Emit { event: String, args: Vec<LocalId> },
    /// `self.field = src` inside a behavior body (bytecode `StateSet`).
    StateSet { field: String, src: LocalId },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RValue {
    Const(Constant),
    Load(LocalId),
    /// Read a named record field (module-wide field id resolved by codegen).
    LoadFieldNamed { obj: LocalId, field: String },
    ArrayLoad { arr: LocalId, idx: LocalId },
    ArrayLen(LocalId),
    /// Array literal: allocate and fill.
    ArrayLit(Vec<LocalId>),
    Unary(UnOp, LocalId),
    Binary(BinOp, LocalId, LocalId),
    /// String equality (variant tag tests).
    StringEq(LocalId, LocalId),
    Call { func: FuncRef, args: Vec<LocalId> },
    /// Create a closure over module function `func` capturing `captures`.
    Closure { func: usize, captures: Vec<LocalId> },
    Tuple(Vec<LocalId>),
    Record(Vec<(String, LocalId)>),
    Perform { effect: String, op: String, args: Vec<LocalId> },
    /// `perform LLM.ask(prompt)` — wired to the runtime's LLM client.
    LlmAsk { prompt: LocalId },
    /// `perform Signal.wait("name")` — workflow signal wait.
    SignalWait { name: String },
    FFICall { idx: usize, args: Vec<LocalId> },
    Migrate { actor: LocalId, node: LocalId },
    SelfRef,
    CapabilityCheck { val: LocalId },
    /// `self.field` inside a behavior body (bytecode `StateGet`).
    StateGet { field: String },
    /// `spawn ActorName { ... }`. `behavior_idx` is the actor's first
    /// behavior's index into `Module::behaviors` — the VM resolves the rest
    /// of the actor's behaviors and state defaults from there via
    /// `ActorMeta`. Spawn-site init argument values are not passed through
    /// (matching the stable compiler): only literal `state` field defaults
    /// take effect.
    Spawn { behavior_idx: usize },
    /// `send actor behavior(args...)`. Fire-and-forget; evaluates to 0.
    Send { actor: LocalId, behavior_idx: usize, args: Vec<LocalId> },
    /// `ask actor behavior(args...)`. Evaluates to the behavior's result.
    Ask { actor: LocalId, behavior_idx: usize, args: Vec<LocalId> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum FuncRef {
    /// Direct reference to a module function by index.
    Index(usize),
    /// Call through a local holding a function value (closure or index).
    Local(LocalId),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Terminator {
    Return(Option<LocalId>),
    Jump(BlockId),
    Branch { cond: LocalId, then_: BlockId, else_: BlockId },
    /// Resume from an effect handler with a result (bytecode `Resume`).
    Resume(LocalId),
    /// Builder placeholder; reaching codegen with this is an internal error.
    Unterminated,
}

// ---------------------------------------------------------------------------
// Builder helpers
// ---------------------------------------------------------------------------

pub struct FunctionBuilder {
    name: String,
    params: Vec<LocalId>,
    captures: Vec<LocalId>,
    ret: Option<Type>,
    locals: Vec<Local>,
    blocks: Vec<Block>,
    handler_tables: Vec<HandlerTableDef>,
    current: BlockId,
    next_local: u32,
    next_block: u32,
}

impl FunctionBuilder {
    pub fn new(name: impl Into<String>, ret: Option<Type>) -> Self {
        let mut builder = FunctionBuilder {
            name: name.into(),
            params: Vec::new(),
            captures: Vec::new(),
            ret,
            locals: Vec::new(),
            blocks: Vec::new(),
            handler_tables: Vec::new(),
            current: BlockId(0),
            next_local: 0,
            next_block: 0,
        };
        builder.create_block(); // entry block
        builder
    }

    pub fn add_param(&mut self, name: impl Into<String>, ty: Type) -> LocalId {
        let id = self.add_local(name, ty);
        self.params.push(id);
        id
    }

    pub fn add_capture(&mut self, name: impl Into<String>, ty: Type) -> LocalId {
        let id = self.add_local(name, ty);
        self.captures.push(id);
        id
    }

    pub fn add_local(&mut self, name: impl Into<String>, ty: Type) -> LocalId {
        let id = LocalId(self.next_local);
        self.next_local += 1;
        self.locals.push(Local { id, name: Some(name.into()), ty });
        id
    }

    pub fn add_temp(&mut self, ty: Type) -> LocalId {
        let id = LocalId(self.next_local);
        self.next_local += 1;
        self.locals.push(Local { id, name: None, ty });
        id
    }

    pub fn add_handler_table(&mut self, table: HandlerTableDef) -> usize {
        self.handler_tables.push(table);
        self.handler_tables.len() - 1
    }

    pub fn handler_table_mut(&mut self, idx: usize) -> &mut HandlerTableDef {
        &mut self.handler_tables[idx]
    }

    pub fn create_block(&mut self) -> BlockId {
        let id = BlockId(self.next_block);
        self.next_block += 1;
        self.blocks.push(Block { id, stmts: Vec::new(), terminator: Terminator::Unterminated });
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

    pub fn is_terminated(&self) -> bool {
        !matches!(
            self.blocks[self.current.0 as usize].terminator,
            Terminator::Unterminated
        )
    }

    pub fn build(self) -> Function {
        Function {
            name: self.name,
            params: self.params,
            captures: self.captures,
            ret: self.ret,
            locals: self.locals,
            blocks: self.blocks,
            entry: BlockId(0),
            handler_tables: self.handler_tables,
        }
    }
}

impl Module {
    pub fn new(name: impl Into<String>) -> Self {
        Module {
            name: name.into(),
            functions: Vec::new(),
            behaviors: Vec::new(),
            actor_metadata: Vec::new(),
            foreign_functions: Vec::new(),
        }
    }
}
