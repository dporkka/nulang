//! Shared type definitions used across all Nulang compiler and runtime modules.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

// ---------------------------------------------------------------------------
// ExitReason (BEAM-style typed exit signals)
// ---------------------------------------------------------------------------

/// Reason for an actor's exit, modeled after Erlang's exit reasons.
///
/// Used with link/monitor signal propagation and supervision decisions.
/// The `Shutdown` variant carries an optional timeout for graceful shutdown.
#[derive(Debug, Clone, PartialEq)]
pub enum ExitReason {
    /// Normal termination — no supervisor notification, linked actors unaffected.
    Normal,
    /// Unconditional kill — cannot be trapped, always triggers cascading exit.
    Kill,
    /// Actor was killed by another actor (the `Kill` reason after propagation).
    Killed,
    /// Graceful shutdown with optional timeout.
    Shutdown(Option<Duration>),
    /// Error with description.
    Error(String),
    /// User-defined exit reason (any serializable value).
    Custom(String),
}

impl ExitReason {
    /// Returns true if this reason represents abnormal termination.
    ///
    /// Normal exits do NOT trigger linked actor exits (per Erlang semantics).
    /// All other reasons trigger cascading failure for linked actors
    /// that don't trap exits.
    pub fn is_abnormal(&self) -> bool {
        !matches!(self, ExitReason::Normal)
    }

    /// Returns a short string tag for logging/monitoring.
    pub fn tag(&self) -> &'static str {
        match self {
            ExitReason::Normal => "normal",
            ExitReason::Kill => "kill",
            ExitReason::Killed => "killed",
            ExitReason::Shutdown(_) => "shutdown",
            ExitReason::Error(_) => "error",
            ExitReason::Custom(_) => "custom",
        }
    }
}

// ---------------------------------------------------------------------------
// Type Variables & Regions
// ---------------------------------------------------------------------------

static TYPE_VAR_COUNTER: AtomicU64 = AtomicU64::new(1);
static REGION_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeVar(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Region(pub u64);

impl TypeVar {
    pub fn fresh() -> Self {
        TypeVar(TYPE_VAR_COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

impl Region {
    pub fn fresh() -> Region {
        Region(REGION_COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

// ---------------------------------------------------------------------------
// Primitive Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PrimitiveType {
    Int,
    Float,
    Bool,
    String,
    Unit,
    Never,
    Address, // Actor address
}

// ---------------------------------------------------------------------------
// Reference Capabilities (Pony-inspired)
// ---------------------------------------------------------------------------

/// Reference capability lattice:
/// ```text
///        iso
///         |
///        trn
///         |
///        ref --- box
///         |       ^
///        val      |
///          \     /
///           \   /
///            \ /
///            tag
/// ```
/// Subtyping: iso <: trn <: ref <: box, val <: box, ref <: tag, val <: tag, box <: tag
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    Iso,   // Unique ownership (can be sent to another actor)
    Trn,   // Unique writer (can be recovered to iso)
    Ref,   // Shared read/write reference
    Val,   // Immutable shared reference (sendable)
    Box,   // Read-only reference (any cap except tag can be read as box)
    Tag,   // Opaque identity only (tagged pointer, no dereference)
}

impl Capability {
    /// Least upper bound (join) of two capabilities.
    pub fn join(self, other: Capability) -> Capability {
        use Capability::*;
        match (self, other) {
            (Iso, Iso) => Iso,
            (Iso, Trn) | (Trn, Iso) | (Trn, Trn) => Trn,
            (Iso, Ref) | (Ref, Iso) | (Trn, Ref) | (Ref, Trn) | (Ref, Ref) => Ref,
            (Iso, Val) | (Val, Iso) | (Trn, Val) | (Val, Trn) | (Val, Val) => Val,
            (Ref, Val) | (Val, Ref) => Box,
            (Iso, Box) | (Box, Iso) | (Trn, Box) | (Box, Trn) | (Ref, Box) | (Box, Ref)
            | (Val, Box) | (Box, Val) | (Box, Box) => Box,
            (Tag, c) | (c, Tag) if c == Tag => Tag,
            (Tag, c) | (c, Tag) => c, // tag is bottom-ish for read-only
        }
    }

    /// Check if self <: other (self is a subtype of other).
    pub fn is_subtype_of(self, other: Capability) -> bool {
        self.join(other) == other
    }

    /// Can this capability be sent to another actor?
    pub fn is_sendable(self) -> bool {
        matches!(self, Capability::Iso | Capability::Val | Capability::Tag)
    }

    /// Can this capability be read through?
    pub fn is_readable(self) -> bool {
        !matches!(self, Capability::Tag)
    }

    /// Can this capability be written through?
    pub fn is_writable(self) -> bool {
        matches!(self, Capability::Iso | Capability::Trn | Capability::Ref)
    }
}

// ---------------------------------------------------------------------------
// Effect Rows (Koka-inspired, row polymorphism)
// ---------------------------------------------------------------------------

/// A built-in or user-defined effect.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Effect {
    IO,
    Net,
    FS,
    Rand,
    Time,
    Spawn,
    Send,
    Receive,
    Migrate,
    STM,
    Async,
    LLM,
    Cost,
    UserDefined(String),
}

/// Effect row: either closed (fixed set) or open (set + row variable).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EffectRow {
    Closed(Vec<Effect>),
    Open(Vec<Effect>, Region),
}

impl EffectRow {
    pub fn empty() -> Self {
        EffectRow::Closed(vec![])
    }

    pub fn singleton(e: Effect) -> Self {
        EffectRow::Closed(vec![e])
    }

    /// Row concatenation.
    pub fn combine(self, other: EffectRow) -> EffectRow {
        match (self, other) {
            (EffectRow::Closed(mut a), EffectRow::Closed(b)) => {
                a.extend(b);
                EffectRow::Closed(a)
            }
            (EffectRow::Closed(mut a), EffectRow::Open(b, r))
            | (EffectRow::Open(mut a, r), EffectRow::Closed(b)) => {
                a.extend(b);
                EffectRow::Open(a, r)
            }
            (EffectRow::Open(mut a, r1), EffectRow::Open(b, _)) => {
                a.extend(b);
                EffectRow::Open(a, r1)
            }
        }
    }

    /// Check if a specific effect is in this row.
    pub fn contains(&self, eff: &Effect) -> bool {
        match self {
            EffectRow::Closed(effects) => effects.contains(eff),
            EffectRow::Open(effects, _) => effects.contains(eff),
        }
    }

    /// Remove an effect from this row (for handled effects).
    pub fn remove(self, eff: &Effect) -> EffectRow {
        match self {
            EffectRow::Closed(effects) => {
                EffectRow::Closed(effects.into_iter().filter(|e| e != eff).collect())
            }
            EffectRow::Open(effects, r) => {
                EffectRow::Open(effects.into_iter().filter(|e| e != eff).collect(), r)
            }
        }
    }

    /// Get the set of effects (ignoring row variable).
    pub fn effects(&self) -> &[Effect] {
        match self {
            EffectRow::Closed(effects) => effects,
            EffectRow::Open(effects, _) => effects,
        }
    }
}

// ---------------------------------------------------------------------------
// Core Type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    /// Type variable (for inference)
    Var(TypeVar),
    /// Primitive type
    Primitive(PrimitiveType),
    /// Tuple (A, B, ...)
    Tuple(Vec<Type>),
    /// Record { field: Type, ... }
    Record(Vec<(String, Type)>),
    /// Variant Type1 | Type2 | ...
    Variant(Vec<(String, Option<Type>)>),
    /// Array [Type]
    Array(Box<Type>),
    /// Function: arg type -> return type with effect row and capability
    Function {
        param: Box<Type>,
        ret: Box<Type>,
        effect: EffectRow,
        cap: Capability,
    },
    /// Actor[State, Behavior]
    Actor {
        state: Box<Type>,
        behavior: Box<Type>,
    },
    /// Agent[State, Policy, Memory, Tools]
    Agent {
        state: Box<Type>,
        policy: Box<Type>,
        memory: Box<Type>,
        tools: Box<Type>,
    },
    /// Generic type application: List[Int], Map[String, Int]
    App { constructor: Box<Type>, args: Vec<Type> },
    /// Reference type with capability: &cap Type
    Reference { cap: Capability, inner: Box<Type> },
    /// Existential / type scheme: forall vars. Type
    Scheme { vars: Vec<TypeVar>, body: Box<Type> },
}

impl Type {
    pub fn int() -> Type {
        Type::Primitive(PrimitiveType::Int)
    }
    pub fn float() -> Type {
        Type::Primitive(PrimitiveType::Float)
    }
    pub fn bool() -> Type {
        Type::Primitive(PrimitiveType::Bool)
    }
    pub fn string() -> Type {
        Type::Primitive(PrimitiveType::String)
    }
    pub fn unit() -> Type {
        Type::Primitive(PrimitiveType::Unit)
    }

    /// Free type variables in this type.
    pub fn free_vars(&self) -> Vec<TypeVar> {
        let mut vars = vec![];
        self.collect_free_vars(&mut vars);
        vars.sort_by_key(|v| v.0);
        vars.dedup_by_key(|v| v.0);
        vars
    }

    fn collect_free_vars(&self, acc: &mut Vec<TypeVar>) {
        match self {
            Type::Var(v) => acc.push(*v),
            Type::Primitive(_) => {}
            Type::Tuple(ts) => ts.iter().for_each(|t| t.collect_free_vars(acc)),
            Type::Record(fs) => fs.iter().for_each(|(_, t)| t.collect_free_vars(acc)),
            Type::Variant(vs) => vs.iter().for_each(|(_, t)| {
                if let Some(t) = t {
                    t.collect_free_vars(acc)
                }
            }),
            Type::Array(t) => t.collect_free_vars(acc),
            Type::Function { param, ret, .. } => {
                param.collect_free_vars(acc);
                ret.collect_free_vars(acc);
            }
            Type::Actor { state, behavior } => {
                state.collect_free_vars(acc);
                behavior.collect_free_vars(acc);
            }
            Type::Agent {
                state,
                policy,
                memory,
                tools,
            } => {
                state.collect_free_vars(acc);
                policy.collect_free_vars(acc);
                memory.collect_free_vars(acc);
                tools.collect_free_vars(acc);
            }
            Type::App { constructor, args } => {
                constructor.collect_free_vars(acc);
                args.iter().for_each(|a| a.collect_free_vars(acc));
            }
            Type::Reference { inner, .. } => inner.collect_free_vars(acc),
            Type::Scheme { vars, body } => {
                body.collect_free_vars(acc);
                acc.retain(|v| !vars.contains(v));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Type Context (Gamma)
// ---------------------------------------------------------------------------

/// Variable binding: name -> (type, capability).
pub type TypeContext = HashMap<String, (Type, Capability)>;

// ---------------------------------------------------------------------------
// Syntax (Expression / Declaration level — parsed from source)
// ---------------------------------------------------------------------------

/// Capability constraint for an actor's state or message.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Constraint {
    /// Exclusive (only one actor can hold this)
    Exclusive,
    /// Readable by many
    Readable,
    /// Writable by owner
    Writable,
    /// Transferable to another actor
    Transferable,
    /// Managed lifecycle (garbage collected)
    Managed,
}

// ---------------------------------------------------------------------------
// Actor ID (for runtime)
// ---------------------------------------------------------------------------

/// Unique actor identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ActorId(pub u64);

// ---------------------------------------------------------------------------
// AST Node (shared between compiler stages)
// ---------------------------------------------------------------------------

/// An AST node carries a value and source location.
#[derive(Debug, Clone, PartialEq)]
pub struct AstNode<T> {
    pub value: T,
    pub loc: SourceLoc,
}

impl<T> AstNode<T> {
    pub fn new(value: T, loc: SourceLoc) -> Self {
        AstNode { value, loc }
    }

    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> AstNode<U> {
        AstNode {
            value: f(self.value),
            loc: self.loc,
        }
    }
}

/// Source location: (line, column, length).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SourceLoc {
    pub line: u32,
    pub col: u32,
    pub len: u32,
}

// ---------------------------------------------------------------------------
// Compilation Unit
// ---------------------------------------------------------------------------

/// A parsed and type-checked compilation unit.
#[derive(Debug, Clone)]
pub struct CompilationUnit {
    pub source: String,
    pub ast: Vec<AstNode<crate::ast::Decl>>,
    pub type_context: TypeContext,
    pub bytecodes: Vec<u8>,
    pub entrypoint: Option<String>,
}

// ---------------------------------------------------------------------------
// Runtime-facing types
// ---------------------------------------------------------------------------

/// Runtime representation of a message sent between actors.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub from: Option<ActorId>,
    pub to: ActorId,
    pub behavior_id: u16,
    pub payload: Vec<Value>,
}

/// Runtime value (NaN-tagged, fits in 64 bits).
/// Same definition as in vm.rs for interoperability.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(u64),   // interned string index
    Unit,
    Actor(u64),    // actor ID
    Nil,
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Error during parsing.
#[derive(Debug, Clone)]
pub struct ParseError {
    pub loc: SourceLoc,
    pub message: String,
}

/// Error during type checking.
#[derive(Debug, Clone)]
pub struct TypeError {
    pub loc: SourceLoc,
    pub message: String,
}

/// Error during codegen.
#[derive(Debug, Clone)]
pub struct CodegenError {
    pub message: String,
}

/// Error during runtime execution.
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeError {
    DivisionByZero,
    StackOverflow,
    OutOfMemory,
    ActorNotFound(ActorId),
    MailboxFull(ActorId),
    InvalidCapability { required: Capability, actual: Capability },
    UnhandledEffect(Effect),
    Timeout { actor: ActorId, duration_ms: u64 },
    LinkBroken { from: ActorId, to: ActorId },
    Custom(String),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeError::DivisionByZero => write!(f, "Division by zero"),
            RuntimeError::StackOverflow => write!(f, "Stack overflow"),
            RuntimeError::OutOfMemory => write!(f, "Out of memory"),
            RuntimeError::ActorNotFound(id) => write!(f, "Actor not found: {:?}", id),
            RuntimeError::MailboxFull(id) => write!(f, "Mailbox full for actor {:?}", id),
            RuntimeError::InvalidCapability { required, actual } => {
                write!(f, "Invalid capability: required {:?}, actual {:?}", required, actual)
            }
            RuntimeError::UnhandledEffect(eff) => write!(f, "Unhandled effect: {:?}", eff),
            RuntimeError::Timeout { actor, duration_ms } => {
                write!(f, "Timeout for actor {:?} after {}ms", actor, duration_ms)
            }
            RuntimeError::LinkBroken { from, to } => {
                write!(f, "Link broken between {:?} and {:?}", from, to)
            }
            RuntimeError::Custom(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for RuntimeError {}

/// Unified error type.
#[derive(Debug, Clone)]
pub enum NuError {
    Parse(ParseError),
    Type(TypeError),
    Codegen(CodegenError),
    Runtime(RuntimeError),
    IO(String),
}

impl std::fmt::Display for NuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NuError::Parse(e) => write!(f, "Parse error at {:?}: {}", e.loc, e.message),
            NuError::Type(e) => write!(f, "Type error at {:?}: {}", e.loc, e.message),
            NuError::Codegen(e) => write!(f, "Codegen error: {}", e.message),
            NuError::Runtime(e) => write!(f, "Runtime error: {}", e),
            NuError::IO(msg) => write!(f, "IO error: {}", msg),
        }
    }
}

impl std::error::Error for NuError {}

/// Convenience result type.
pub type NuResult<T> = Result<T, NuError>;
