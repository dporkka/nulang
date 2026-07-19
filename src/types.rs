//! Shared type definitions used across all Nulang compiler and runtime modules.

use crate::type_ir::NtirNode;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Type Variables & Regions
// ---------------------------------------------------------------------------

static TYPE_VAR_COUNTER: AtomicU64 = AtomicU64::new(1);
static REGION_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeVar(pub u64);

impl std::fmt::Display for TypeVar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "'t{}", self.0)
    }
}

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
    Nil,
    Unit,
    Never,
    Address, // Actor address
}

// ---------------------------------------------------------------------------
// Reference Capabilities (Pony-inspired)
// ---------------------------------------------------------------------------

/// Reference capability lattice:
/// ```text
///       LinearIso
///       /      \
///     Iso     Linear
///     / \      /
///   Trn Val<--/
///    |   |
///   Ref Box
///     \ /
///     Tag
/// ```
/// Subtyping: lineariso <: iso <: trn <: ref <: box, linear <: val <: box, ref <: tag, val <: tag, box <: tag
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    LinearIso, // Unique ownership with linear type tracking (provably consumed exactly once)
    Linear,    // Immutable + linear-tracked + remote-sendable ("linear Val")
    Iso,       // Unique ownership (can be sent to another actor)
    Trn,       // Unique writer (can be recovered to iso)
    Ref,       // Shared read/write reference
    Val,       // Immutable shared reference (sendable)
    Box,       // Read-only reference (any cap except tag can be read as box)
    Tag,       // Opaque identity only (tagged pointer, no dereference)
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Capability::LinearIso => write!(f, "lineariso"),
            Capability::Linear => write!(f, "linear"),
            Capability::Iso => write!(f, "iso"),
            Capability::Trn => write!(f, "trn"),
            Capability::Ref => write!(f, "ref"),
            Capability::Val => write!(f, "val"),
            Capability::Box => write!(f, "box"),
            Capability::Tag => write!(f, "tag"),
        }
    }
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

            // Linear joins: Linear behaves like Val except Linear join Linear = Linear
            (Linear, Linear) => Linear,
            (Linear, Val) | (Val, Linear) => Val,
            (Linear, LinearIso) | (LinearIso, Linear) => Val,
            (Linear, Iso) | (Iso, Linear) => Val,
            (Linear, Trn) | (Trn, Linear) => Val,
            (Linear, Ref) | (Ref, Linear) => Box,
            (Linear, Box) | (Box, Linear) => Box,
            (Linear, Tag) | (Tag, Linear) => Linear,

            // Original capability joins (unchanged)
            (Iso, Iso) => Iso,
            (Iso, Trn) | (Trn, Iso) | (Trn, Trn) => Trn,
            (Iso, Ref) | (Ref, Iso) | (Trn, Ref) | (Ref, Trn) | (Ref, Ref) => Ref,
            (Iso, Val) | (Val, Iso) | (Trn, Val) | (Val, Trn) | (Val, Val) => Val,
            (Ref, Val) | (Val, Ref) => Box,
            (Iso, Box)
            | (Box, Iso)
            | (Trn, Box)
            | (Box, Trn)
            | (Ref, Box)
            | (Box, Ref)
            | (Val, Box)
            | (Box, Val)
            | (Box, Box) => Box,
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
            Capability::LinearIso
                | Capability::Linear
                | Capability::Iso
                | Capability::Val
                | Capability::Tag
        )
    }

    /// Can this capability be sent over the network (serializable)?
    pub fn is_remote_sendable(self) -> bool {
        matches!(self, Capability::Linear | Capability::Val | Capability::Tag)
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
        matches!(self, Capability::LinearIso | Capability::Linear)
    }

    /// Discharge linear tracking: LinearIso→Iso, Linear→Val.
    pub fn discharge_linear(self) -> Capability {
        match self {
            Capability::LinearIso => Capability::Iso,
            Capability::Linear => Capability::Val,
            other => other,
        }
    }

    #[deprecated(note = "use discharge_linear")]
    pub fn promote_to_iso(self) -> Capability {
        self.discharge_linear()
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
    FFI,
    DB,
    UserDefined(String),
}

impl std::fmt::Display for Effect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Effect::IO => write!(f, "IO"),
            Effect::Net => write!(f, "Net"),
            Effect::FS => write!(f, "FS"),
            Effect::Rand => write!(f, "Rand"),
            Effect::Time => write!(f, "Time"),
            Effect::Spawn => write!(f, "Spawn"),
            Effect::Send => write!(f, "Send"),
            Effect::Receive => write!(f, "Receive"),
            Effect::Migrate => write!(f, "Migrate"),
            Effect::STM => write!(f, "STM"),
            Effect::Async => write!(f, "Async"),
            Effect::LLM => write!(f, "LLM"),
            Effect::Cost => write!(f, "Cost"),
            Effect::Event => write!(f, "Event"),
            Effect::FFI => write!(f, "FFI"),
            Effect::DB => write!(f, "DB"),
            Effect::UserDefined(s) => write!(f, "{}", s),
        }
    }
}

/// Effect row: either closed (fixed set) or open (set + row variable).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EffectRow {
    Closed(Vec<Effect>),
    Open(Vec<Effect>, Region),
}

impl std::fmt::Display for EffectRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EffectRow::Closed(effects) => {
                write!(f, "{{")?;
                for (i, e) in effects.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", e)?;
                }
                write!(f, "}}")
            }
            EffectRow::Open(effects, _) => {
                write!(f, "{{")?;
                for (i, e) in effects.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", e)?;
                }
                if !effects.is_empty() {
                    write!(f, ", ")?;
                }
                write!(f, "..}}")
            }
        }
    }
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
    ///
    /// A record whose field list ends with the reserved pseudo-field
    /// [`RECORD_ROW_TAIL_FIELD`] is an *open* record: the pseudo-field's type
    /// is a row variable standing for "possibly more fields". Records from
    /// literals and annotations are always closed (no tail).
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
    App {
        constructor: Box<Type>,
        args: Vec<Type>,
    },
    /// Reference type with capability: &cap Type
    Reference { cap: Capability, inner: Box<Type> },
    /// Existential / type scheme: forall vars. Type
    Scheme { vars: Vec<TypeVar>, body: Box<Type> },
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Var(v) => write!(f, "'t{}", v.0),
            Type::Primitive(p) => match p {
                PrimitiveType::Int => write!(f, "Int"),
                PrimitiveType::Float => write!(f, "Float"),
                PrimitiveType::Bool => write!(f, "Bool"),
                PrimitiveType::String => write!(f, "String"),
                PrimitiveType::Unit => write!(f, "Unit"),
                PrimitiveType::Nil => write!(f, "Nil"),
                PrimitiveType::Never => write!(f, "Never"),
                PrimitiveType::Address => write!(f, "Address"),
            },
            Type::Tuple(ts) => {
                write!(f, "(")?;
                for (i, t) in ts.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", t)?;
                }
                write!(f, ")")
            }
            Type::Record(fs) => {
                write!(f, "{{ ")?;
                for (i, (n, t)) in fs.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", n, t)?;
                }
                write!(f, " }}")
            }
            Type::Variant(vs) => {
                for (i, (n, t)) in vs.iter().enumerate() {
                    if i > 0 {
                        write!(f, " | ")?;
                    }
                    match t {
                        Some(t) => write!(f, "{} {}", n, t)?,
                        None => write!(f, "{}", n)?,
                    }
                }
                Ok(())
            }
            Type::Array(t) => write!(f, "[{}]", t),
            Type::Function {
                param,
                ret,
                effect: _,
                cap: _,
            } => {
                write!(f, "{} -> {}", param, ret)
            }
            Type::Actor { state, behavior } => {
                write!(f, "Actor[{}, {}]", state, behavior)
            }
            Type::App { constructor, args } => {
                write!(f, "{}", constructor)?;
                if !args.is_empty() {
                    write!(f, "[")?;
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}", a)?;
                    }
                    write!(f, "]")?;
                }
                Ok(())
            }
            Type::Reference { cap, inner } => write!(f, "&{} {}", cap, inner),
            Type::Scheme { vars, body } => {
                write!(f, "forall ")?;
                for (i, v) in vars.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "'t{}", v.0)?;
                }
                write!(f, ". {}", body)
            }
        }
    }
}

/// Reserved pseudo-field name carrying the *row tail* of an open record type.
///
/// Record row polymorphism is encoded without changing the shape of
/// `Type::Record(Vec<(String, Type)>)` — exhaustive matches on `Type` exist
/// across the crate (`main.rs`, `repl.rs`, `mir_codegen.rs`, `ai/schema.rs`),
/// so the representation stays additive. An open record `{ x: a | rho }` is
/// represented as `Record([("x", a), ("..", Var(rho))])`. The name `".."`
/// can never collide with a user field: record field names are parsed with
/// `expect_ident`, and `".."` is not a valid identifier.
///
/// The tail's type is a fresh `Type::Var` when produced; record unification
/// may substitute it with an open record (row extension) or a closed record
/// (row closing). Because the row variable is an ordinary type variable in an
/// ordinary field, `free_vars`, `ref_free_vars`, substitution, the occurs
/// check, and generalization all handle it with no special casing.
pub const RECORD_ROW_TAIL_FIELD: &str = "..";

impl Type {
    /// Convert to an NTIR structural representation for content-addressed hashing.
    pub fn to_ntir(&self) -> NtirNode {
        self.to_ntir_with_stack(&mut Vec::new())
    }

    fn to_ntir_with_stack(&self, stack: &mut Vec<TypeVar>) -> NtirNode {
        if let Type::Var(v) = self {
            if let Some(pos) = stack.iter().rev().position(|x| x == v) {
                return NtirNode::Cycle(pos as u64);
            }
        }

        let push_var = if let Type::Var(v) = self {
            stack.push(*v);
            true
        } else {
            false
        };

        let res = match self {
            Type::Var(_) => NtirNode::Primitive(PrimitiveType::Unit),
            Type::Primitive(p) => NtirNode::Primitive(p.clone()),
            Type::Tuple(ts) => {
                NtirNode::Tuple(ts.iter().map(|t| t.to_ntir_with_stack(stack)).collect())
            }
            Type::Record(fs) => {
                let mut mapped: Vec<_> = fs
                    .iter()
                    .map(|(n, t)| (n.clone(), t.to_ntir_with_stack(stack)))
                    .collect();
                mapped.sort_by(|a, b| a.0.cmp(&b.0));
                NtirNode::Record(mapped)
            }
            Type::Variant(vs) => {
                let mut mapped: Vec<_> = vs
                    .iter()
                    .map(|(n, t_opt)| {
                        let t_ntir = match t_opt {
                            Some(t) => t.to_ntir_with_stack(stack),
                            None => NtirNode::Primitive(PrimitiveType::Unit),
                        };
                        (n.clone(), t_ntir)
                    })
                    .collect();
                mapped.sort_by(|a, b| a.0.cmp(&b.0));
                NtirNode::Variant(mapped)
            }
            Type::Array(inner) => NtirNode::Tuple(vec![inner.to_ntir_with_stack(stack)]),
            Type::Function {
                param, ret, cap, ..
            } => NtirNode::Capability(
                *cap,
                Box::new(NtirNode::Tuple(vec![
                    param.to_ntir_with_stack(stack),
                    ret.to_ntir_with_stack(stack),
                ])),
            ),
            Type::Actor { state, behavior } => NtirNode::Tuple(vec![
                state.to_ntir_with_stack(stack),
                behavior.to_ntir_with_stack(stack),
            ]),
            Type::App { constructor, args } => {
                let mut elems = vec![constructor.to_ntir_with_stack(stack)];
                for a in args {
                    elems.push(a.to_ntir_with_stack(stack));
                }
                NtirNode::Tuple(elems)
            }
            Type::Reference { cap, inner } => {
                NtirNode::Capability(*cap, Box::new(inner.to_ntir_with_stack(stack)))
            }
            Type::Scheme { body, .. } => body.to_ntir_with_stack(stack),
        };

        if push_var {
            stack.pop();
        }
        res
    }

    /// True if the type contains no free type variables.
    pub fn is_ground(&self) -> bool {
        let mut fv = Vec::new();
        self.collect_free_vars(&mut fv);
        fv.is_empty()
    }
    pub fn int() -> Type {
        Type::Primitive(PrimitiveType::Int)
    }

    /// A closed record type: exactly the given fields. Record literals and
    /// annotations are always closed.
    pub fn record(fields: Vec<(String, Type)>) -> Type {
        Type::Record(fields)
    }

    /// An open record type: the given fields plus a fresh row variable
    /// standing for "possibly more fields". Produced by field access on a
    /// record of not-yet-known shape; see [`RECORD_ROW_TAIL_FIELD`].
    pub fn record_open(fields: Vec<(String, Type)>, row: TypeVar) -> Type {
        let mut fields = fields;
        fields.push((RECORD_ROW_TAIL_FIELD.to_string(), Type::Var(row)));
        Type::Record(fields)
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
    pub fn nil() -> Type {
        Type::Primitive(PrimitiveType::Nil)
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

    /// Free type variables that occur underneath a `Reference` constructor.
    ///
    /// Used for the value restriction at generalization: a reference cell is
    /// created once at binding time and shared by every use of the binding, so
    /// quantifying a variable under a `Reference` would let one cell be used at
    /// incompatible types. Function types are not descended into — a reference
    /// in a function's parameter or return type is created per call, so
    /// quantifying it is sound.
    pub fn ref_free_vars(&self) -> Vec<TypeVar> {
        let mut vars = vec![];
        self.collect_ref_free_vars(&mut vars);
        vars.sort_by_key(|v| v.0);
        vars.dedup_by_key(|v| v.0);
        vars
    }

    fn collect_ref_free_vars(&self, acc: &mut Vec<TypeVar>) {
        match self {
            // The shared cell: every free variable inside must stay monomorphic.
            Type::Reference { inner, .. } => inner.collect_free_vars(acc),
            // Function values are created per call — refs in their types are safe.
            Type::Function { .. } => {}
            Type::Tuple(ts) => ts.iter().for_each(|t| t.collect_ref_free_vars(acc)),
            Type::Record(fs) => fs.iter().for_each(|(_, t)| t.collect_ref_free_vars(acc)),
            Type::Variant(vs) => vs.iter().for_each(|(_, t)| {
                if let Some(t) = t {
                    t.collect_ref_free_vars(acc)
                }
            }),
            Type::Array(t) => t.collect_ref_free_vars(acc),
            Type::Actor { state, behavior } => {
                state.collect_ref_free_vars(acc);
                behavior.collect_ref_free_vars(acc);
            }
            Type::App { constructor, args } => {
                constructor.collect_ref_free_vars(acc);
                args.iter().for_each(|a| a.collect_ref_free_vars(acc));
            }
            Type::Scheme { body, .. } => body.collect_ref_free_vars(acc),
            Type::Var(_) | Type::Primitive(_) => {}
        }
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
/// Linear (`LinearIso`) consumption is tracked separately by the capability
/// analyzer (`CapabilityAnalyzer` in `src/effect_checker.rs`), not here.
#[derive(Debug, Clone, Default)]
pub struct TypeContext {
    bindings: HashMap<String, (Type, Capability)>,
}

impl TypeContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind a variable name to a type and capability.
    pub fn bind(&mut self, name: impl Into<String>, ty: Type, cap: Capability) {
        let name = name.into();
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

    /// Iterate over all bindings as `(name, (type, capability))` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &(Type, Capability))> {
        self.bindings.iter()
    }

    /// Free type variables occurring in any binding in the context.
    pub fn free_vars(&self) -> Vec<TypeVar> {
        let mut vars = vec![];
        for (ty, _) in self.bindings.values() {
            vars.extend(ty.free_vars());
        }
        vars.sort_by_key(|v| v.0);
        vars.dedup_by_key(|v| v.0);
        vars
    }
}

// ---------------------------------------------------------------------------
// Source Location
// ---------------------------------------------------------------------------
// Source Map (offset -> line/column resolution)
// ---------------------------------------------------------------------------

use std::cell::RefCell;

thread_local! {
    /// Thread-local source map used by Span::line()/column() to resolve byte
    /// offsets into human-readable positions.  Set once per compilation unit
    /// (by the lexer or test harness) before any Span display.
    static SOURCE_MAP: RefCell<Option<SourceMap>> = RefCell::new(None);
}

/// Maps byte offsets to line:column positions for error reporting.
#[derive(Debug, Clone)]
pub struct SourceMap {
    /// Byte offset of the start of each line.  line_starts[0] is always 0.
    line_starts: Vec<u32>,
}

impl SourceMap {
    /// Build a source map from source text.  Line endings are `\n` only.
    pub fn new(source: &str) -> Self {
        let mut line_starts = vec![0u32];
        for (i, &b) in source.as_bytes().iter().enumerate() {
            if b == b'\n' {
                line_starts.push(i as u32 + 1);
            }
        }
        SourceMap { line_starts }
    }

    /// Resolve a byte offset to (1-indexed line, 1-indexed column).
    pub fn line_col(&self, offset: u32) -> (usize, usize) {
        let idx = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let line = idx + 1;
        let col = offset.saturating_sub(self.line_starts[idx]) + 1;
        (line, col as usize)
    }
}

/// Install a SourceMap for the current thread, consuming the source string
/// to build line-start offsets.  Call before any Span display.
pub fn set_source_map(source: &str) {
    let sm = SourceMap::new(source);
    SOURCE_MAP.with(|slot| {
        *slot.borrow_mut() = Some(sm);
    });
}

/// Clear the thread-local source map (e.g. between tests).
pub fn clear_source_map() {
    SOURCE_MAP.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

/// Compact source span — just byte offsets.  Line/column are resolved on
/// demand via the thread-local SourceMap (set by the lexer or test harness).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: u32, end: u32) -> Self {
        Span { start, end }
    }

    /// 1-indexed line number (reads from thread-local SourceMap; returns 0
    /// if none is set).
    pub fn line(&self) -> usize {
        SOURCE_MAP.with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|sm| sm.line_col(self.start).0)
                .unwrap_or(0)
        })
    }

    /// 1-indexed column (reads from thread-local SourceMap; returns 0 if
    /// none is set).
    pub fn column(&self) -> usize {
        SOURCE_MAP.with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|sm| sm.line_col(self.start).1)
                .unwrap_or(0)
        })
    }
}

// ---------------------------------------------------------------------------
// Nulang Result Type
// ---------------------------------------------------------------------------

pub type NuResult<T> = Result<T, NuError>;

#[derive(Debug, Clone)]
pub enum NuError {
    LexError {
        msg: String,
        span: Span,
    },
    ParseError {
        msg: String,
        span: Span,
    },
    TypeError {
        msg: String,
        span: Span,
    },
    EffectError {
        msg: String,
        span: Span,
    },
    CapError {
        msg: String,
        span: Span,
    },
    FFIError {
        msg: String,
        span: Span,
    },
    /// Feature is parsed/typed correctly but has no runtime implementation yet.
    NotYetImplemented {
        feature: String,
        span: Span,
    },
    RuntimeError(String),
    VMError(String),
    PythonError(String),  // Python interop error
    PackageError(String), // `nula` package manager error (manifest/lockfile/resolution)
}

impl std::fmt::Display for NuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NuError::LexError { msg, span } => {
                write!(f, "Lex error at {}:{}: {}", span.line(), span.column(), msg)
            }
            NuError::ParseError { msg, span } => {
                write!(
                    f,
                    "Parse error at {}:{}: {}",
                    span.line(),
                    span.column(),
                    msg
                )
            }
            NuError::TypeError { msg, span } => {
                write!(
                    f,
                    "Type error at {}:{}: {}",
                    span.line(),
                    span.column(),
                    msg
                )
            }
            NuError::EffectError { msg, span } => {
                write!(
                    f,
                    "Effect error at {}:{}: {}",
                    span.line(),
                    span.column(),
                    msg
                )
            }
            NuError::CapError { msg, span } => {
                write!(
                    f,
                    "Capability error at {}:{}: {}",
                    span.line(),
                    span.column(),
                    msg
                )
            }
            NuError::FFIError { msg, span } => {
                write!(f, "FFI error at {}:{}: {}", span.line(), span.column(), msg)
            }
            NuError::NotYetImplemented { feature, span } => {
                write!(
                    f,
                    "Not yet implemented at {}:{}: {}",
                    span.line(),
                    span.column(),
                    feature
                )
            }
            NuError::RuntimeError(msg) => write!(f, "Runtime error: {}", msg),
            NuError::VMError(msg) => write!(f, "VM error: {}", msg),
            NuError::PythonError(msg) => write!(f, "Python error: {}", msg),
            NuError::PackageError(msg) => write!(f, "Package error: {}", msg),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discharge_linear() {
        assert_eq!(Capability::LinearIso.discharge_linear(), Capability::Iso);
        assert_eq!(Capability::Linear.discharge_linear(), Capability::Val);
        assert_eq!(Capability::Iso.discharge_linear(), Capability::Iso);
        assert_eq!(Capability::Val.discharge_linear(), Capability::Val);
    }

    #[test]
    fn test_linear_join_self() {
        assert_eq!(
            Capability::Linear.join(Capability::Linear),
            Capability::Linear
        );
    }

    #[test]
    fn test_linear_join_val() {
        assert_eq!(Capability::Linear.join(Capability::Val), Capability::Val);
        assert_eq!(Capability::Val.join(Capability::Linear), Capability::Val);
    }

    #[test]
    fn test_linear_join_linearioso() {
        assert_eq!(
            Capability::Linear.join(Capability::LinearIso),
            Capability::Val
        );
        assert_eq!(
            Capability::LinearIso.join(Capability::Linear),
            Capability::Val
        );
    }

    #[test]
    fn test_linear_join_iso() {
        assert_eq!(Capability::Linear.join(Capability::Iso), Capability::Val);
        assert_eq!(Capability::Iso.join(Capability::Linear), Capability::Val);
    }

    #[test]
    fn test_linear_join_trn() {
        assert_eq!(Capability::Linear.join(Capability::Trn), Capability::Val);
        assert_eq!(Capability::Trn.join(Capability::Linear), Capability::Val);
    }

    #[test]
    fn test_linear_join_ref() {
        assert_eq!(Capability::Linear.join(Capability::Ref), Capability::Box);
        assert_eq!(Capability::Ref.join(Capability::Linear), Capability::Box);
    }

    #[test]
    fn test_linear_join_box() {
        assert_eq!(Capability::Linear.join(Capability::Box), Capability::Box);
        assert_eq!(Capability::Box.join(Capability::Linear), Capability::Box);
    }

    #[test]
    fn test_linear_join_tag() {
        assert_eq!(Capability::Linear.join(Capability::Tag), Capability::Linear);
        assert_eq!(Capability::Tag.join(Capability::Linear), Capability::Linear);
    }

    #[test]
    fn test_linear_is_sendable() {
        assert!(Capability::Linear.is_sendable());
    }

    #[test]
    fn test_linear_is_remote_sendable() {
        assert!(Capability::Linear.is_remote_sendable());
        assert!(!Capability::Iso.is_remote_sendable());
        assert!(!Capability::LinearIso.is_remote_sendable());
    }

    #[test]
    fn test_linear_is_not_writable() {
        assert!(!Capability::Linear.is_writable());
    }

    #[test]
    fn test_linear_is_readable() {
        assert!(Capability::Linear.is_readable());
    }

    #[test]
    fn test_linear_is_linear() {
        assert!(Capability::Linear.is_linear());
        assert!(Capability::LinearIso.is_linear());
        assert!(!Capability::Iso.is_linear());
    }

    #[test]
    fn test_linear_subtype_of_val() {
        assert!(Capability::Linear.is_subtype_of(Capability::Val));
    }
}
