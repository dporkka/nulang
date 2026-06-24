use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Source location
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub line: u32,
    pub col: u32,
}

// ---------------------------------------------------------------------------
// Capability lattice (inspired by Pony)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Iso,  // Isolated — unique reference, no aliases
    Trn,  // Transition — unique, can become iso or ref
    Ref,  // Reference — shared read/write
    Val,  // Value — shared read-only, deeply immutable
    Box,  // Box — owned, unique but not sendable
    Tag,  // Tag — no read/write, only identity comparison
}

impl Capability {
    /// Check if a capability is sendable across actors.
    pub fn is_sendable(self) -> bool {
        matches!(self, Capability::Iso | Capability::Val | Capability::Tag)
    }

    /// Check if a capability allows reading.
    pub fn is_readable(self) -> bool {
        matches!(self, Capability::Iso | Capability::Trn | Capability::Ref | Capability::Val | Capability::Box)
    }

    /// Check if a capability allows writing.
    pub fn is_writable(self) -> bool {
        matches!(self, Capability::Iso | Capability::Trn | Capability::Ref)
    }

    /// Subtyping: can we use `self` where `other` is expected?
    pub fn is_subtype_of(self, other: Self) -> bool {
        match (self, other) {
            // Iso is top — can be used anywhere a unique ref is needed
            (Capability::Iso, _) => true,
            // Tag is bottom — can only be used where Tag is expected
            (_, Capability::Tag) => true,
            // Same capability
            (a, b) if a == b => true,
            // Val can be used as Ref (immutable shared is safe for shared read)
            (Capability::Val, Capability::Ref) => true,
            // Box can be used as Ref (unique ownership can be temporarily shared)
            (Capability::Box, Capability::Ref) => true,
            // Trn can become Iso or Ref
            (Capability::Trn, Capability::Iso) => false, // Trn can't become Iso without consume
            (Capability::Trn, Capability::Ref) => true,
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Effect {
    pub name: String,
}

impl Effect {
    pub fn new(name: impl Into<String>) -> Self {
        Effect { name: name.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectRow {
    Closed(Vec<Effect>),          // Concrete set of effects
    Open(Vec<Effect>, u64),       // Known effects + free tail variable
}

impl EffectRow {
    /// Create an empty (pure) effect row.
    pub fn pure() -> Self {
        EffectRow::Closed(vec![])
    }

    /// Check if this row contains a given effect.
    pub fn contains(&self, eff: &Effect) -> bool {
        match self {
            EffectRow::Closed(es) | EffectRow::Open(es, _) => es.contains(eff),
        }
    }

    /// Remove an effect from the row (used by handlers).
    pub fn remove(&self, eff: &Effect) -> Self {
        match self {
            EffectRow::Closed(es) => {
                EffectRow::Closed(es.iter().filter(|e| *e != eff).cloned().collect())
            }
            EffectRow::Open(es, v) => {
                EffectRow::Open(es.iter().filter(|e| *e != eff).cloned().collect(), *v)
            }
        }
    }

    /// Combine with another effect row (union).
    pub fn combine(&self, other: &Self) -> Self {
        use crate::effects::effect_row_union;
        effect_row_union(self, other)
    }

    /// Pretty-print the effect row.
    pub fn display(&self) -> String {
        match self {
            EffectRow::Closed(es) if es.is_empty() => "{}".into(),
            EffectRow::Closed(es) => {
                let names: Vec<_> = es.iter().map(|e| e.name.clone()).collect();
                format!("{{{}}}", names.join(", "))
            }
            EffectRow::Open(es, v) if es.is_empty() => {
                format!("{{E{}}}", v)
            }
            EffectRow::Open(es, v) => {
                let names: Vec<_> = es.iter().map(|e| e.name.clone()).collect();
                format!("{{{}, E{}}}", names.join(", "), v)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Type variables
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeVar(pub u64);

impl TypeVar {
    pub fn fresh() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        TypeVar(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region(pub u64);

impl Region {
    pub fn fresh() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Region(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

// ---------------------------------------------------------------------------
// Type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    // Primitives
    Int,
    Float,
    Bool,
    String,
    Unit,
    Never,      // Bottom type
    Address,    // Actor/process address (opaque)

    // Type variable (for inference)
    Var(TypeVar),

    // Reference with capability
    Ref(Box<Type>, Capability),

    // Tuple
    Tuple(Vec<Type>),

    // Record (struct-like, nominal)
    Record(Vec<(String, Type)>),

    // Variant (enum-like)
    Variant(Vec<(String, Vec<Type>)>),

    // Array
    Array(Box<Type>),

    // Function: params -> return with effect
    Arrow(Vec<Type>, Box<Type>, Box<EffectRow>),

    // Generic type application: List[Int]
    App(Box<Type>, Vec<Type>),

    // Named type reference (resolved later)
    Named(String),

    // Polymorphic type scheme
    Scheme(Vec<TypeVar>, Box<Type>),
}

impl Type {
    /// Get free type variables in this type.
    pub fn free_vars(&self) -> Vec<TypeVar> {
        let mut vars = Vec::new();
        self.collect_free_vars(&mut vars);
        vars.sort();
        vars.dedup();
        vars
    }

    fn collect_free_vars(&self, vars: &mut Vec<TypeVar>) {
        match self {
            Type::Var(v) => vars.push(*v),
            Type::Tuple(ts) | Type::App(_, ts) => {
                for t in ts {
                    t.collect_free_vars(vars);
                }
            }
            Type::Record(fs) => {
                for (_, t) in fs {
                    t.collect_free_vars(vars);
                }
            }
            Type::Variant(vs) => {
                for (_, ts) in vs {
                    for t in ts {
                        t.collect_free_vars(vars);
                    }
                }
            }
            Type::Array(t) | Type::Ref(t, _) => t.collect_free_vars(vars),
            Type::Arrow(params, ret, eff) => {
                for p in params {
                    p.collect_free_vars(vars);
                }
                ret.collect_free_vars(vars);
                // Effect rows don't contain type variables in our system
                let _ = eff;
            }
            Type::Scheme(vs, t) => {
                t.collect_free_vars(vars);
                vars.retain(|v| !vs.contains(v));
            }
            _ => {}
        }
    }

    /// Apply a substitution to this type.
    pub fn apply_subst(&self, subst: &[(TypeVar, Type)]) -> Type {
        match self {
            Type::Var(v) => {
                subst.iter().find(|(tv, _)| tv == v)
                    .map(|(_, t)| t.clone())
                    .unwrap_or(Type::Var(*v))
            }
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| t.apply_subst(subst)).collect()),
            Type::Record(fs) => Type::Record(
                fs.iter().map(|(n, t)| (n.clone(), t.apply_subst(subst))).collect()
            ),
            Type::Variant(vs) => Type::Variant(
                vs.iter().map(|(n, ts)| (n.clone(), ts.iter().map(|t| t.apply_subst(subst)).collect())).collect()
            ),
            Type::Array(t) => Type::Array(Box::new(t.apply_subst(subst))),
            Type::Ref(t, cap) => Type::Ref(Box::new(t.apply_subst(subst)), *cap),
            Type::Arrow(params, ret, eff) => Type::Arrow(
                params.iter().map(|p| p.apply_subst(subst)).collect(),
                Box::new(ret.apply_subst(subst)),
                eff.clone(),
            ),
            Type::App(name, ts) => Type::App(
                Box::new(name.apply_subst(subst)),
                ts.iter().map(|t| t.apply_subst(subst)).collect(),
            ),
            Type::Scheme(vs, t) => Type::Scheme(
                vs.clone(),
                Box::new(t.apply_subst(subst)),
            ),
            other => other.clone(),
        }
    }

    /// Display the type in a readable format.
    pub fn display(&self) -> String {
        match self {
            Type::Int => "Int".into(),
            Type::Float => "Float".into(),
            Type::Bool => "Bool".into(),
            Type::String => "String".into(),
            Type::Unit => "()".into(),
            Type::Never => "!".into(),
            Type::Address => "Address".into(),
            Type::Var(v) => format!("t{}", v.0),
            Type::Ref(t, cap) => format!("&{} {}", cap_display(*cap), t.display()),
            Type::Tuple(ts) => {
                let elems: Vec<_> = ts.iter().map(|t| t.display()).collect();
                format!("({})", elems.join(", "))
            }
            Type::Record(fs) => {
                let fields: Vec<_> = fs.iter()
                    .map(|(n, t)| format!("{}: {}", n, t.display()))
                    .collect();
                format!("{{ {} }}", fields.join(", "))
            }
            Type::Variant(vs) => {
                let arms: Vec<_> = vs.iter()
                    .map(|(n, ts)| {
                        if ts.is_empty() {
                            n.clone()
                        } else {
                            let args: Vec<_> = ts.iter().map(|t| t.display()).collect();
                            format!("{}({})", n, args.join(", "))
                        }
                    })
                    .collect();
                format!("[ {} ]", arms.join(" | "))
            }
            Type::Array(t) => format!("[{}]", t.display()),
            Type::Arrow(params, ret, eff) => {
                let ps: Vec<_> = params.iter().map(|p| p.display()).collect();
                let ret_str = ret.display();
                let eff_str = eff.display();
                if ps.len() == 1 {
                    format!("{} -> {} {}", ps[0], ret_str, eff_str)
                } else {
                    format!("({}) -> {} {}", ps.join(", "), ret_str, eff_str)
                }
            }
            Type::App(name, args) => {
                let a: Vec<_> = args.iter().map(|a| a.display()).collect();
                format!("{}[{}]", name.display(), a.join(", "))
            }
            Type::Named(n) => n.clone(),
            Type::Scheme(vs, t) => {
                let vars: Vec<_> = vs.iter().map(|v| format!("t{}", v.0)).collect();
                format!("forall {}. {}", vars.join(" "), t.display())
            }
        }
    }
}

fn cap_display(cap: Capability) -> &'static str {
    match cap {
        Capability::Iso => "iso",
        Capability::Trn => "trn",
        Capability::Ref => "ref",
        Capability::Val => "val",
        Capability::Box => "box",
        Capability::Tag => "tag",
    }
}

// ---------------------------------------------------------------------------
// TypeContext (typing environment)
// ---------------------------------------------------------------------------

/// Gamma — the typing environment mapping names to type schemes.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeContext {
    pub bindings: Vec<(String, Type)>,
}

impl TypeContext {
    pub fn empty() -> Self {
        TypeContext { bindings: Vec::new() }
    }

    pub fn new(bindings: Vec<(String, Type)>) -> Self {
        TypeContext { bindings }
    }

    pub fn lookup(&self, name: &str) -> Option<&Type> {
        self.bindings.iter().rev().find(|(n, _)| n == name).map(|(_, t)| t)
    }

    pub fn extend(&self, name: String, ty: Type) -> Self {
        let mut ctx = self.clone();
        ctx.bindings.push((name, ty));
        ctx
    }

    pub fn extend_many(&self, bindings: Vec<(String, Type)>) -> Self {
        let mut ctx = self.clone();
        for (name, ty) in bindings {
            ctx.bindings.push((name, ty));
        }
        ctx
    }

    /// Get free type variables from the context (used for generalization).
    pub fn free_vars(&self) -> Vec<TypeVar> {
        let mut vars = Vec::new();
        for (_, ty) in &self.bindings {
            vars.extend(ty.free_vars());
        }
        vars.sort();
        vars.dedup();
        vars
    }

    pub fn apply_subst(&self, subst: &[(TypeVar, Type)]) -> Self {
        TypeContext {
            bindings: self.bindings.iter()
                .map(|(n, t)| (n.clone(), t.apply_subst(subst)))
                .collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Runtime value representation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Float(u64),     // Store as bits to derive Eq
    Bool(bool),
    String(String),
    Unit,
    Tuple(Vec<Value>),
    Record(Vec<(String, Value)>),
    Array(Vec<Value>),
    // Actor reference (process ID)
    IntAddr(u64),
    // Variant: tag index + payload
    Variant(usize, Box<Value>),
    // Runtime reference (heap pointer) — only used inside the VM
    Ref(usize),
}

impl Value {
    pub fn float_val(&self) -> f64 {
        match self {
            Value::Float(bits) => f64::from_bits(*bits),
            _ => panic!("not a float: {:?}", self),
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn type_of(&self) -> Type {
        match self {
            Value::Int(_) => Type::Int,
            Value::Float(_) => Type::Float,
            Value::Bool(_) => Type::Bool,
            Value::String(_) => Type::String,
            Value::Unit => Type::Unit,
            Value::Tuple(vs) => Type::Tuple(vs.iter().map(|v| v.type_of()).collect()),
            Value::Record(fs) => Type::Record(
                fs.iter().map(|(n, v)| (n.clone(), v.type_of())).collect()
            ),
            Value::Array(vs) => {
                if let Some(first) = vs.first() {
                    Type::Array(Box::new(first.type_of()))
                } else {
                    Type::Array(Box::new(Type::Var(TypeVar::fresh())))
                }
            }
            Value::IntAddr(_) => Type::Address,
            Value::Variant(tag, payload) => {
                Type::Variant(vec![(format!("V{}", tag), vec![payload.type_of()])])
            }
            Value::Ref(_) => Type::Ref(Box::new(Type::Var(TypeVar::fresh())), Capability::Ref),
        }
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum NuError {
    ParseError { message: String, span: Span },
    TypeError { message: String, span: Span },
    EffectError { message: String, span: Span },
    CapError { message: String, span: Span },
    CompileError { message: String, span: Span },
    RuntimeError { message: String },
    VMError { message: String, pc: usize },
    LinkError { message: String },
    NotImplemented { feature: String },
}

impl fmt::Display for NuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NuError::ParseError { message, span } => {
                write!(f, "Parse error at line {}:{}: {}", span.line, span.col, message)
            }
            NuError::TypeError { message, span } => {
                write!(f, "Type error at line {}:{}: {}", span.line, span.col, message)
            }
            NuError::EffectError { message, span } => {
                write!(f, "Effect error at line {}:{}: {}", span.line, span.col, message)
            }
            NuError::CapError { message, span } => {
                write!(f, "Capability error at line {}:{}: {}", span.line, span.col, message)
            }
            NuError::CompileError { message, span } => {
                write!(f, "Compile error at line {}:{}: {}", span.line, span.col, message)
            }
            NuError::RuntimeError { message } => {
                write!(f, "Runtime error: {}", message)
            }
            NuError::VMError { message, pc } => {
                write!(f, "VM error at pc {}: {}", pc, message)
            }
            NuError::LinkError { message } => {
                write!(f, "Link error: {}", message)
            }
            NuError::NotImplemented { feature } => {
                write!(f, "Not yet implemented: {}", feature)
            }
        }
    }
}

impl std::error::Error for NuError {}

pub type NuResult<T> = Result<T, NuError>;
