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
    /// Saga compensation pairs: `(behavior_idx, compensation_behavior_idx)`,
    /// both indices into `behaviors`. Compensation bodies compile through
    /// the exact same machinery as ordinary behaviors, but are never
    /// dispatched by name — codegen patches the owning behavior's
    /// `BehaviorTableEntry::compensate_offset` from the compiled
    /// compensation function's code offset instead.
    pub compensation_of: Vec<(usize, usize)>,
    /// `(behavior_idx, branch_names)` pairs for steps synthesized from a
    /// `parallel { ... }` block; codegen copies `branch_names` into the
    /// matching `BehaviorTableEntry::parallel_branches` unchanged.
    pub parallel_branches_of: Vec<(usize, Vec<String>)>,
    pub foreign_functions: Vec<ForeignFunction>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: String,
    pub params: Vec<LocalId>,
    pub captures: Vec<LocalId>,
    pub ret: Option<Type>,
    pub locals: Vec<Local>,
    pub blocks: Vec<Block>,
    pub entry: BlockId,
    pub handler_tables: Vec<HandlerTableDef>,
    /// Compile-time type metadata for each local (by register index).
    pub type_metadata: crate::type_metadata::TypeMetadata,
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
    /// Whether the handler resumes the captured continuation
    /// (`| E.op() resume => body`). A resuming handler's body block ends in
    /// `Terminator::Resume`; a non-resuming (abortive) handler's block
    /// instead assigns the body value to the handle expression's dst, pops
    /// the handler frame (discarding the captured continuation), and jumps
    /// to the handle's join block.
    pub resume: bool,
    /// Block containing the handler body; ends in `Terminator::Resume` for
    /// resuming handlers, in a `PopHandler` + `Terminator::Jump` to the
    /// handle's join block for non-resuming ones.
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
    Assign {
        dst: LocalId,
        op: RValue,
    },
    /// Store to a named record field (module-wide field id resolved by codegen).
    StoreFieldNamed {
        obj: LocalId,
        field: String,
        src: LocalId,
    },
    ArrayStore {
        arr: LocalId,
        idx: LocalId,
        src: LocalId,
    },
    /// Install handler table `table` (index into `Function::handler_tables`).
    EnterHandle {
        table: usize,
    },
    /// Pop the innermost handler frame (bytecode `Unwind`).
    PopHandler,
    Emit {
        event: String,
        args: Vec<LocalId>,
    },
    /// `self.field = src` inside a behavior body (bytecode `StateSet`).
    StateSet {
        field: String,
        src: LocalId,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RValue {
    Const(Constant),
    Load(LocalId),
    /// Read a named record field (module-wide field id resolved by codegen).
    LoadFieldNamed {
        obj: LocalId,
        field: String,
    },
    /// Read a positional tuple field (bytecode `FieldL`; yields nil when
    /// the object is not a tuple or the index is out of range).
    LoadFieldPos {
        obj: LocalId,
        index: u8,
    },
    ArrayLoad {
        arr: LocalId,
        idx: LocalId,
    },
    ArrayLen(LocalId),
    /// Array literal: allocate and fill.
    ArrayLit(Vec<LocalId>),
    Unary(UnOp, LocalId),
    Binary(BinOp, LocalId, LocalId),
    /// String equality (variant tag tests).
    StringEq(LocalId, LocalId),
    /// String concatenation: `s1 + s2`.
    StrConcat(LocalId, LocalId),
    Call {
        func: FuncRef,
        args: Vec<LocalId>,
    },
    /// Create a closure over module function `func` capturing `captures`.
    Closure {
        func: usize,
        captures: Vec<LocalId>,
    },
    Tuple(Vec<LocalId>),
    Record(Vec<(String, LocalId)>),
    Perform {
        effect: String,
        op: String,
        args: Vec<LocalId>,
    },
    /// `perform LLM.ask(prompt)` — wired to the runtime's LLM client.
    LlmAsk {
        prompt: LocalId,
    },
    /// `perform Signal.wait("name")` — workflow signal wait.
    SignalWait {
        name: String,
    },
    /// `receive { | Behavior(params) => expr ... }` with no arms (or in the
    /// no-match fallback block): pop the next message from the actor's
    /// mailbox; evaluates to its first payload value (nil when the mailbox
    /// is empty or outside an actor context).
    Receive,
    /// Selective receive: scan the mailbox for the first message whose
    /// behavior id is in `behavior_ids` (bytecode `ReceiveMatch`). Writes
    /// the matched arm index to dst (or `behavior_ids.len()` when nothing
    /// matches) and up to `max_params` payload values into the registers
    /// immediately following dst — the lowering must allocate dst and the
    /// `max_params` payload temps as one contiguous run of locals.
    ReceiveMatch {
        behavior_ids: Vec<u16>,
        max_params: usize,
    },
    /// Timed selective receive: `receive { | Behavior(params) => expr ... }
    /// after ms => timeout_expr` (bytecode `ReceiveWait`). Like
    /// `ReceiveMatch`, but the codegen additionally stages the `timeout`
    /// local (milliseconds) into r0; on no match the VM suspends the actor
    /// via the `"ReceiveWait:suspend"` sentinel until a matching message
    /// arrives or the timer fires. Writes the matched arm index to dst (or
    /// `behavior_ids.len()` on timeout / non-positive timeout) and up to
    /// `max_params` payload values into the registers following dst — dst and
    /// the payload temps must form one contiguous run of locals, exactly as
    /// for `ReceiveMatch`.
    ReceiveWait {
        behavior_ids: Vec<u16>,
        max_params: usize,
        timeout: LocalId,
    },
    /// Commit a selective receive: removes the matched ("tried") message from
    /// the skip-buffer and clears remaining "tried" flags. Emitted after a
    /// pattern+guard check succeeds, before binding pattern variables and
    /// entering the arm body. Bytecode `OpCode::ReceiveCommit`.
    ReceiveCommit,
    FFICall {
        idx: usize,
        args: Vec<LocalId>,
    },
    Migrate {
        actor: LocalId,
        node: LocalId,
    },
    SelfRef,
    CapabilityCheck {
        val: LocalId,
    },
    /// `self.field` inside a behavior body (bytecode `StateGet`).
    StateGet {
        field: String,
    },
    /// `spawn ActorName { ... }`. `behavior_idx` is the actor's first
    /// behavior's index into `Module::behaviors` — the VM resolves the rest
    /// of the actor's behaviors and state defaults from there via
    /// `ActorMeta`. Spawn-site init argument values are not passed through
    /// (matching the stable compiler): only literal `state` field defaults
    /// take effect.
    Spawn {
        behavior_idx: usize,
    },
    /// `send actor behavior(args...)`. Fire-and-forget; evaluates to 0.
    Send {
        actor: LocalId,
        behavior_idx: usize,
        args: Vec<LocalId>,
    },
    /// `ask actor behavior(args...)`. Evaluates to the behavior's result.
    Ask {
        actor: LocalId,
        behavior_idx: usize,
        args: Vec<LocalId>,
    },
    // AI runtime builtins
    PipelineNew,
    PipelineStage {
        id: LocalId,
        name: LocalId,
        actor: LocalId,
        template: LocalId,
    },
    PipelineRun {
        id: LocalId,
        input: LocalId,
    },
    SupervisorNew,
    SupervisorWorker {
        id: LocalId,
        name: LocalId,
        actor: LocalId,
        description: LocalId,
    },
    SupervisorRun {
        id: LocalId,
        task: LocalId,
    },
    DebateNew {
        topic: LocalId,
        rounds: LocalId,
        threshold: LocalId,
    },
    DebateParticipant {
        id: LocalId,
        name: LocalId,
        stance: LocalId,
        actor: LocalId,
    },
    DebateRun {
        id: LocalId,
    },
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
    Branch {
        cond: LocalId,
        then_: BlockId,
        else_: BlockId,
    },
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
        self.locals.push(Local {
            id,
            name: Some(name.into()),
            ty,
        });
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
        self.blocks.push(Block {
            id,
            stmts: Vec::new(),
            terminator: Terminator::Unterminated,
        });
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

    /// Register offset: MIR locals start at register 16 (r0-r15 are scratch).
    pub const LOCAL_BASE: u32 = 16;

    pub fn build(self) -> Function {
        let type_metadata = crate::type_metadata::TypeMetadata::from_mir_locals(
            self.locals.iter().map(|loc| {
                (Self::LOCAL_BASE as usize + loc.id.0 as usize, &loc.ty)
            }),
        );
        Function {
            name: self.name,
            params: self.params,
            captures: self.captures,
            ret: self.ret,
            locals: self.locals,
            blocks: self.blocks,
            entry: BlockId(0),
            handler_tables: self.handler_tables,
            type_metadata,
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
            compensation_of: Vec::new(),
            parallel_branches_of: Vec::new(),
            foreign_functions: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinOp, UnOp};
    use crate::bytecode::Constant;

    #[test]
    fn test_local_id() {
        assert_eq!(LocalId(0), LocalId(0));
        assert_ne!(LocalId(0), LocalId(42));
    }

    #[test]
    fn test_block_id() {
        assert_eq!(BlockId(0), BlockId(0));
        assert_ne!(BlockId(0), BlockId(42));
    }

    #[test]
    fn test_function_builder_new_block() {
        let builder = FunctionBuilder::new("test", None);
        assert_eq!(builder.current_block(), BlockId(0));
    }

    #[test]
    fn test_function_builder_append_term() {
        let mut builder = FunctionBuilder::new("test", None);
        assert!(!builder.is_terminated());
        builder.terminate(Terminator::Return(None));
        assert!(builder.is_terminated());
    }

    #[test]
    fn test_module_new() {
        let m = Module::new("test");
        assert_eq!(m.name, "test");
        assert!(m.functions.is_empty());
        assert!(m.behaviors.is_empty());
        assert!(m.foreign_functions.is_empty());
    }

    #[test]
    fn test_module_lookup_missing() {
        let m = Module::new("test");
        assert!(m.functions.is_empty());
    }

    #[test]
    fn test_rvalue_variants() {
        // Construct every RValue variant to confirm they compile.
        let _ = RValue::Const(Constant::Int(42));
        let _ = RValue::Load(LocalId(0));
        let _ = RValue::LoadFieldNamed {
            obj: LocalId(0),
            field: "x".into(),
        };
        let _ = RValue::ArrayLoad {
            arr: LocalId(0),
            idx: LocalId(1),
        };
        let _ = RValue::ArrayLen(LocalId(0));
        let _ = RValue::ArrayLit(vec![LocalId(0)]);
        let _ = RValue::Unary(UnOp::Neg, LocalId(0));
        let _ = RValue::Binary(BinOp::Add, LocalId(0), LocalId(1));
        let _ = RValue::StringEq(LocalId(0), LocalId(1));
        let _ = RValue::Call {
            func: FuncRef::Index(0),
            args: vec![LocalId(0)],
        };
        let _ = RValue::Closure {
            func: 0,
            captures: vec![LocalId(0)],
        };
        let _ = RValue::Tuple(vec![LocalId(0)]);
        let _ = RValue::Record(vec![("x".into(), LocalId(0))]);
        let _ = RValue::Perform {
            effect: "eff".into(),
            op: "op".into(),
            args: vec![LocalId(0)],
        };
        let _ = RValue::LlmAsk { prompt: LocalId(0) };
        let _ = RValue::SignalWait { name: "sig".into() };
        let _ = RValue::Receive;
        let _ = RValue::ReceiveMatch {
            behavior_ids: vec![1, 2],
            max_params: 1,
        };
        let _ = RValue::ReceiveWait {
            behavior_ids: vec![1, 2],
            max_params: 1,
            timeout: LocalId(0),
        };
        let _ = RValue::ReceiveCommit;
        let _ = RValue::FFICall {
            idx: 0,
            args: vec![LocalId(0)],
        };
        let _ = RValue::Migrate {
            actor: LocalId(0),
            node: LocalId(1),
        };
        let _ = RValue::SelfRef;
        let _ = RValue::CapabilityCheck { val: LocalId(0) };
        let _ = RValue::StateGet { field: "f".into() };
        let _ = RValue::Spawn { behavior_idx: 0 };
        let _ = RValue::Send {
            actor: LocalId(0),
            behavior_idx: 0,
            args: vec![LocalId(0)],
        };
        let _ = RValue::Ask {
            actor: LocalId(0),
            behavior_idx: 0,
            args: vec![LocalId(0)],
        };
        let _ = RValue::PipelineNew;
        let _ = RValue::PipelineStage {
            id: LocalId(0),
            name: LocalId(0),
            actor: LocalId(0),
            template: LocalId(0),
        };
        let _ = RValue::PipelineRun {
            id: LocalId(0),
            input: LocalId(0),
        };
        let _ = RValue::SupervisorNew;
        let _ = RValue::SupervisorWorker {
            id: LocalId(0),
            name: LocalId(0),
            actor: LocalId(0),
            description: LocalId(0),
        };
        let _ = RValue::SupervisorRun {
            id: LocalId(0),
            task: LocalId(0),
        };
        let _ = RValue::DebateNew {
            topic: LocalId(0),
            rounds: LocalId(0),
            threshold: LocalId(0),
        };
        let _ = RValue::DebateParticipant {
            id: LocalId(0),
            name: LocalId(0),
            stance: LocalId(0),
            actor: LocalId(0),
        };
        let _ = RValue::DebateRun { id: LocalId(0) };
    }

    #[test]
    fn test_local_new() {
        let local = Local {
            id: LocalId(0),
            name: Some("x".into()),
            ty: Type::int(),
        };
        assert_eq!(local.id, LocalId(0));
        assert_eq!(local.name, Some("x".into()));
        assert_eq!(local.ty, Type::int());
    }
}
