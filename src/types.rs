//! Shared type definitions used across all Nulang compiler and runtime modules.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

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
                // Open rows share the same row variable convention
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
                // Remove bound vars
                acc.retain(|v| !vars.contains(v));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Type Context (Gamma)
// ---------------------------------------------------------------------------

/// Typing context: maps variable names to their (type, capability) bindings.
#[derive(Debug, Clone, Default)]
pub struct TypeContext {
    bindings: HashMap<String, (Type, Capability)>,
    type_aliases: HashMap<String, Type>,
}

impl TypeContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bind(&mut self, name: impl Into<String>, ty: Type, cap: Capability) {
        self.bindings.insert(name.into(), (ty, cap));
    }

    pub fn lookup(&self, name: &str) -> Option<&(Type, Capability)> {
        self.bindings.get(name)
    }

    pub fn extend(&self, name: impl Into<String>, ty: Type, cap: Capability) -> Self {
        let mut ctx = self.clone();
        ctx.bind(name, ty, cap);
        ctx
    }
}

// ---------------------------------------------------------------------------
// Source Location
// ---------------------------------------------------------------------------

/// Source span for error reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize, // byte offset
    pub end: usize,
    pub line: usize,
    pub column: usize,
}

impl Span {
    pub fn new(start: usize, end: usize, line: usize, column: usize) -> Self {
        Span {
            start,
            end,
            line,
            column,
        }
    }
}

// ---------------------------------------------------------------------------
// Nulang Result Type
// ---------------------------------------------------------------------------

pub type NuResult<T> = Result<T, NuError>;

#[derive(Debug, Clone)]
pub enum NuError {
    LexError { msg: String, span: Span },
    ParseError { msg: String, span: Span },
    TypeError { msg: String, span: Span },
    EffectError { msg: String, span: Span },
    CapError { msg: String, span: Span },
    RuntimeError(String),
    VMError(String),
}

impl std::fmt::Display for NuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NuError::LexError { msg, span } => {
                write!(f, "Lex error at {}:{}: {}", span.line, span.column, msg)
            }
            NuError::ParseError { msg, span } => {
                write!(f, "Parse error at {}:{}: {}", span.line, span.column, msg)
            }
            NuError::TypeError { msg, span } => {
                write!(f, "Type error at {}:{}: {}", span.line, span.column, msg)
            }
            NuError::EffectError { msg, span } => {
                write!(f, "Effect error at {}:{}: {}", span.line, span.column, msg)
            }
            NuError::CapError { msg, span } => {
                write!(f, "Capability error at {}:{}: {}", span.line, span.column, msg)
            }
            NuError::RuntimeError(msg) => write!(f, "Runtime error: {}", msg),
            NuError::VMError(msg) => write!(f, "VM error: {}", msg),
        }
    }
}

impl std::error::Error for NuError {}
