//! Shared type definitions used across all Nulang compiler and runtime modules.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

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
///       lineariso (subtype of iso, tracked for linear consumption)
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
/// Subtyping: lineariso <: iso <: trn <: ref <: box, val <: box, ref <: tag, val <: tag, box <: tag
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    LinearIso, // Unique ownership with linear type tracking (provably consumed exactly once)
    Iso,       // Unique ownership (can be sent to another actor)
    Trn,       // Unique writer (can be recovered to iso)
    Ref,       // Shared read/write reference
    Val,       // Immutable shared reference (sendable)
    Box,       // Read-only reference (any cap except tag can be read as box)
    Tag,       // Opaque identity only (tagged pointer, no dereference)
}

impl Capability {
    /// Least upper bound (join) of two capabilities.
    ///
    /// LinearIso behaves like Iso in joins, except LinearIso ⊔ LinearIso = LinearIso.
    pub fn join(self, other: Capability) -> Capability {
        use Capability::*;
        match (self, other) {
            // LinearIso joins: LinearIso + LinearIso stays LinearIso
            (LinearIso, LinearIso) => LinearIso,
            // LinearIso + Iso promotes to Iso (linear obligation can be discharged)
            (LinearIso, Iso) | (Iso, LinearIso) => Iso,
            // LinearIso with Trn (same as Iso with Trn)
            (LinearIso, Trn) | (Trn, LinearIso) => Trn,
            // LinearIso with Ref (same as Iso with Ref)
            (LinearIso, Ref) | (Ref, LinearIso) => Ref,
            // LinearIso with Val (same as Iso with Val)
            (LinearIso, Val) | (Val, LinearIso) => Val,
            // LinearIso with Box (same as Iso with Box)
            (LinearIso, Box) | (Box, LinearIso) => Box,
            // LinearIso with Tag (same as Iso with Tag)
            (LinearIso, Tag) | (Tag, LinearIso) => LinearIso,

            // Original capability joins (unchanged)
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
        matches!(
            self,
            Capability::LinearIso | Capability::Iso | Capability::Val | Capability::Tag
        )
    }

    /// Can this capability be read through?
    pub fn is_readable(self) -> bool {
        !matches!(self, Capability::Tag)
    }

    /// Can this capability be written through?
    pub fn is_writable(self) -> bool {
        matches!(
            self,
            Capability::LinearIso | Capability::Iso | Capability::Trn | Capability::Ref
        )
    }

    /// Is this a linear capability (requires exactly-one consumption tracking)?
    pub fn is_linear(self) -> bool {
        matches!(self, Capability::LinearIso)
    }

    /// Promote a linear capability to its non-linear form.
    /// LinearIso -> Iso (linear obligation discharged on send/escape).
    pub fn promote_to_iso(self) -> Capability {
        match self {
            Capability::LinearIso => Capability::Iso,
            other => other,
        }
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
    Event,
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
///
/// Tracks linear variable consumption to enforce exactly-once use of linear
/// (`LinearIso`) values. When a linear variable is consumed, it is recorded in
/// `consumed`; subsequent attempts to use it will fail.
#[derive(Debug, Clone, Default)]
pub struct TypeContext {
    bindings: HashMap<String, (Type, Capability)>,
    /// Set of linear variable names that have been consumed (used exactly once).
    consumed: HashSet<String>,
}

impl TypeContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind a variable name to a type and capability.
    ///
    /// If the capability is linear (`LinearIso`), resets its consumption state
    /// so the variable can be consumed exactly once in the current scope.
    pub fn bind(&mut self, name: impl Into<String>, ty: Type, cap: Capability) {
        let name = name.into();
        // Reset consumption state for linear variables on re-bind
        if cap.is_linear() {
            self.consumed.remove(&name);
        }
        self.bindings.insert(name, (ty, cap));
    }

    /// Look up a variable's type and capability.
    pub fn lookup(&self, name: &str) -> Option<&(Type, Capability)> {
        self.bindings.get(name)
    }

    /// Create an extended context with an additional binding.
    pub fn extend(&self, name: impl Into<String>, ty: Type, cap: Capability) -> Self {
        let mut ctx = self.clone();
        ctx.bind(name, ty, cap);
        ctx
    }

    /// Mark a linear variable as consumed.
    ///
    /// Returns `Ok(())` if the variable was not yet consumed and is linear.
    /// Returns `Err(LinearTypeError)` if the variable is already consumed
    /// or does not exist / is not linear.
    pub fn consume(&mut self, name: &str) -> Result<(), NuError> {
        match self.bindings.get(name) {
            Some((_ty, cap)) if cap.is_linear() => {
                if self.consumed.contains(name) {
                    Err(NuError::LinearTypeError {
                        msg: format!(
                            "Linear variable '{}' consumed more than once",
                            name
                        ),
                        span: Span::default(),
                    })
                } else {
                    self.consumed.insert(name.to_string());
                    Ok(())
                }
            }
            Some((_ty, _cap)) => Err(NuError::LinearTypeError {
                msg: format!("Variable '{}' is not linear and cannot be consumed", name),
                span: Span::default(),
            }),
            None => Err(NuError::LinearTypeError {
                msg: format!("Variable '{}' not found in context", name),
                span: Span::default(),
            }),
        }
    }

    /// Check whether a linear variable has already been consumed.
    pub fn is_consumed(&self, name: &str) -> bool {
        self.consumed.contains(name)
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
    /// Linear type violation: a linear value was used more than once,
    /// not consumed, or otherwise violated linearity constraints.
    LinearTypeError { msg: String, span: Span },
    RuntimeError(String),
    VMError(String),
    PythonError(String), // Python interop error
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
            NuError::LinearTypeError { msg, span } => {
                write!(
                    f,
                    "Linear type error at {}:{}: {}",
                    span.line, span.column, msg
                )
            }
            NuError::RuntimeError(msg) => write!(f, "Runtime error: {}", msg),
            NuError::VMError(msg) => write!(f, "VM error: {}", msg),
            NuError::PythonError(msg) => write!(f, "Python error: {}", msg),
        }
    }
}

impl std::error::Error for NuError {}

// ---------------------------------------------------------------------------
// Exit Reason (Actor Lifecycle)
// ---------------------------------------------------------------------------

/// Reason for an actor's exit, modeled after Erlang's exit reasons.
///
/// Used with link/monitor signal propagation and supervision decisions.
/// The `Shutdown` variant carries an optional timeout for graceful shutdown.
#[derive(Debug, Clone, PartialEq, Eq)]
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
// Linear Type Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod linear_tests {
    use super::*;

    // Test 1: LinearIso capability exists and is_linear returns true
    #[test]
    fn test_linear_iso_is_linear() {
        assert!(Capability::LinearIso.is_linear());
    }

    // Test 2: Regular Iso is NOT linear (no tracking)
    #[test]
    fn test_regular_iso_not_linear() {
        assert!(!Capability::Iso.is_linear());
        assert!(!Capability::Trn.is_linear());
        assert!(!Capability::Ref.is_linear());
        assert!(!Capability::Val.is_linear());
        assert!(!Capability::Box.is_linear());
        assert!(!Capability::Tag.is_linear());
    }

    // Test 3: TypeContext can bind and consume a linear variable
    #[test]
    fn test_typecontext_linear_bind_consume() {
        let mut ctx = TypeContext::new();
        ctx.bind("x", Type::int(), Capability::LinearIso);

        // Initially not consumed
        assert!(!ctx.is_consumed("x"));

        // Consume succeeds
        assert!(ctx.consume("x").is_ok());

        // Now it is consumed
        assert!(ctx.is_consumed("x"));
    }

    // Test 4: Double consumption of a linear variable fails
    #[test]
    fn test_linear_double_consume_fails() {
        let mut ctx = TypeContext::new();
        ctx.bind("x", Type::int(), Capability::LinearIso);

        // First consume succeeds
        assert!(ctx.consume("x").is_ok());

        // Second consume fails with LinearTypeError
        let result = ctx.consume("x");
        assert!(result.is_err());
        match result.unwrap_err() {
            NuError::LinearTypeError { msg, .. } => {
                assert!(msg.contains("consumed more than once"));
            }
            other => panic!("Expected LinearTypeError, got {:?}", other),
        }
    }

    // Test 5: LinearIso is sendable (like Iso)
    #[test]
    fn test_linear_iso_is_sendable() {
        assert!(Capability::LinearIso.is_sendable());
        // Same sendability as Iso, Val, Tag
        assert!(Capability::Iso.is_sendable());
        assert!(Capability::Val.is_sendable());
        assert!(Capability::Tag.is_sendable());
        // Ref, Box, Trn are not sendable
        assert!(!Capability::Ref.is_sendable());
        assert!(!Capability::Box.is_sendable());
        assert!(!Capability::Trn.is_sendable());
    }

    // Test 6: Promote LinearIso to Iso on send
    #[test]
    fn test_linear_iso_promote_to_iso() {
        assert_eq!(Capability::LinearIso.promote_to_iso(), Capability::Iso);
        // Non-linear capabilities are unchanged
        assert_eq!(Capability::Iso.promote_to_iso(), Capability::Iso);
        assert_eq!(Capability::Trn.promote_to_iso(), Capability::Trn);
        assert_eq!(Capability::Ref.promote_to_iso(), Capability::Ref);
        assert_eq!(Capability::Val.promote_to_iso(), Capability::Val);
        assert_eq!(Capability::Box.promote_to_iso(), Capability::Box);
        assert_eq!(Capability::Tag.promote_to_iso(), Capability::Tag);
    }

    // Test 7: LinearIso joins correctly with other capabilities
    #[test]
    fn test_linear_iso_join() {
        // LinearIso ⊔ LinearIso = LinearIso
        assert_eq!(
            Capability::LinearIso.join(Capability::LinearIso),
            Capability::LinearIso
        );
        // LinearIso ⊔ Iso = Iso
        assert_eq!(
            Capability::LinearIso.join(Capability::Iso),
            Capability::Iso
        );
        assert_eq!(
            Capability::Iso.join(Capability::LinearIso),
            Capability::Iso
        );
        // LinearIso ⊔ Trn = Trn
        assert_eq!(
            Capability::LinearIso.join(Capability::Trn),
            Capability::Trn
        );
        // LinearIso ⊔ Ref = Ref
        assert_eq!(
            Capability::LinearIso.join(Capability::Ref),
            Capability::Ref
        );
        // LinearIso ⊔ Tag = LinearIso
        assert_eq!(
            Capability::LinearIso.join(Capability::Tag),
            Capability::LinearIso
        );
        // Subtyping: LinearIso <: Iso
        assert!(Capability::LinearIso.is_subtype_of(Capability::Iso));
        assert!(!Capability::Iso.is_subtype_of(Capability::LinearIso));
    }

    // Test 8: Linear type error creation
    #[test]
    fn test_linear_type_error() {
        let err = NuError::LinearTypeError {
            msg: "test linear error".to_string(),
            span: Span::new(0, 10, 1, 5),
        };
        let display = format!("{}", err);
        assert!(display.contains("Linear type error"));
        assert!(display.contains("test linear error"));
        assert!(display.contains("1:5"));
    }
}