//! High-level Intermediate Representation (HIR).
//!
//! HIR is the output of type checking and effect/capability analysis.
//! It mirrors the AST but:
//!   - every binding and operand carries a resolved `Type`;
//!   - nested expressions are flattened into statements/terminators;
//!   - actor/module structure is preserved;
//!   - patterns are preserved with type annotations.

use crate::ast::{BinOp, Literal, Pattern, StateModel, UnOp};
use crate::types::{Capability, EffectRow, Span, Type};

// ---------------------------------------------------------------------------
// Module and declarations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub name: String,
    pub decls: Vec<Decl>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decl {
    Function(FunctionDef),
    Actor(ActorDef),
    TypeAlias {
        name: String,
        type_params: Vec<String>,
        body: Type,
        public: bool,
        span: Span,
    },
    RecordType {
        name: String,
        type_params: Vec<String>,
        fields: Vec<(String, Type)>,
        public: bool,
        span: Span,
    },
    VariantType {
        name: String,
        type_params: Vec<String>,
        variants: Vec<(String, Option<Type>)>,
        public: bool,
        span: Span,
    },
    EffectDecl {
        name: String,
        ops: Vec<(String, Vec<Type>, Type)>,
        span: Span,
    },
    Module {
        name: String,
        exports: Vec<String>,
        decls: Vec<Decl>,
        span: Span,
    },
    Import {
        path: String,
        items: Vec<String>,
        span: Span,
    },
    ExternBlock {
        library: String,
        funcs: Vec<ExternFunc>,
        span: Span,
    },
    Workflow {
        name: String,
        span: Span,
    },
    Agent {
        name: String,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDef {
    pub name: String,
    pub type_params: Vec<String>,
    pub params: Vec<(String, Type)>,
    pub ret: Type,
    pub effect: EffectRow,
    pub cap: Capability,
    pub body: Body,
    pub public: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActorDef {
    pub name: String,
    pub type_params: Vec<String>,
    pub persistent: bool,
    pub state_fields: Vec<(String, StateModel, Type, Operand)>,
    pub behaviors: Vec<BehaviorDef>,
    pub init: Vec<(String, Operand)>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BehaviorDef {
    pub name: String,
    pub params: Vec<(String, Type)>,
    pub ret: Type,
    pub effect: EffectRow,
    pub cap: Capability,
    pub body: Body,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExternFunc {
    pub name: String,
    pub params: Vec<(String, Type)>,
    pub ret: Type,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Body: statements + terminator
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Body {
    pub stmts: Vec<Stmt>,
    pub terminator: Terminator,
}

impl Default for Body {
    fn default() -> Self {
        Body {
            stmts: Vec::new(),
            terminator: Terminator::Return(None),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Let {
        name: String,
        ty: Type,
        value: RValue,
        span: Span,
    },
    Assign {
        target: Place,
        value: RValue,
        span: Span,
    },
    StateSet {
        field: String,
        value: Operand,
        span: Span,
    },
    Emit {
        event: String,
        args: Vec<Operand>,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Place {
    Var(String, Type),
    Field { base: Box<Place>, field: String, ty: Type },
    Index { base: Box<Place>, idx: Operand, ty: Type },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RValue {
    Use(Operand),
    Literal(Literal, Type),
    Binary(BinOp, Operand, Operand, Type),
    Unary(UnOp, Operand, Type),
    Call { func: Operand, args: Vec<Operand>, ty: Type },
    Closure { params: Vec<(String, Type)>, body: Box<Body>, captures: Vec<String>, ty: Type },
    Tuple(Vec<Operand>, Type),
    Record(Vec<(String, Operand)>, Type),
    Array(Vec<Operand>, Type),
    FieldAccess { base: Operand, field: String, ty: Type },
    Index { base: Operand, idx: Operand, ty: Type },
    Spawn { actor_type: String, init: Vec<(String, Operand)>, ty: Type },
    Send { actor: Operand, behavior: String, args: Vec<Operand>, ty: Type },
    Ask { actor: Operand, behavior: String, args: Vec<Operand>, ty: Type },
    SelfRef(Type),
    Perform { effect: String, op: String, args: Vec<Operand>, ty: Type },
    Handle { body: Box<Body>, handlers: Vec<EffectHandler>, ty: Type },
    Receive { arms: Vec<(String, Vec<String>, Box<Body>)>, ty: Type },
    Migrate { actor: Operand, node: Operand, ty: Type },
    CapCheck { operand: Operand, required: Capability },
    FFICall { symbol: String, args: Vec<Operand>, ty: Type },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Terminator {
    Return(Option<Operand>),
    If {
        cond: Operand,
        result: String,
        then_body: Box<Body>,
        else_body: Option<Box<Body>>,
    },
    Match {
        scrutinee: Operand,
        result: String,
        arms: Vec<(Pattern, Box<Body>)>,
    },
    Block(Vec<Box<Body>>),
    Break,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectHandler {
    pub effect_name: String,
    pub op_name: String,
    pub params: Vec<(String, Type)>,
    pub resume: bool,
    pub body: Box<Body>,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Operands
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Var(String, Type),
    Literal(Literal, Type),
    Unit,
}

impl Operand {
    pub fn ty(&self) -> Type {
        match self {
            Operand::Var(_, ty) => ty.clone(),
            Operand::Literal(_, ty) => ty.clone(),
            Operand::Unit => Type::unit(),
        }
    }
}

impl Place {
    pub fn ty(&self) -> Type {
        match self {
            Place::Var(_, ty) => ty.clone(),
            Place::Field { ty, .. } => ty.clone(),
            Place::Index { ty, .. } => ty.clone(),
        }
    }
}

impl RValue {
    pub fn ty(&self) -> Type {
        match self {
            RValue::Use(op) => op.ty(),
            RValue::Literal(_, ty) => ty.clone(),
            RValue::Binary(_, _, _, ty) => ty.clone(),
            RValue::Unary(_, _, ty) => ty.clone(),
            RValue::Call { ty, .. } => ty.clone(),
            RValue::Closure { ty, .. } => ty.clone(),
            RValue::Tuple(_, ty) => ty.clone(),
            RValue::Record(_, ty) => ty.clone(),
            RValue::Array(_, ty) => ty.clone(),
            RValue::FieldAccess { ty, .. } => ty.clone(),
            RValue::Index { ty, .. } => ty.clone(),
            RValue::Spawn { ty, .. } => ty.clone(),
            RValue::Send { ty, .. } => ty.clone(),
            RValue::Ask { ty, .. } => ty.clone(),
            RValue::SelfRef(ty) => ty.clone(),
            RValue::Perform { ty, .. } => ty.clone(),
            RValue::Handle { ty, .. } => ty.clone(),
            RValue::Receive { ty, .. } => ty.clone(),
            RValue::Migrate { ty, .. } => ty.clone(),
            RValue::CapCheck { .. } => Type::bool(),
            RValue::FFICall { ty, .. } => ty.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Builder helpers
// ---------------------------------------------------------------------------

impl Body {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_terminator(terminator: Terminator) -> Self {
        Self { stmts: Vec::new(), terminator }
    }

    pub fn push(&mut self, stmt: Stmt) {
        self.stmts.push(stmt);
    }

    pub fn set_terminator(&mut self, terminator: Terminator) {
        self.terminator = terminator;
    }
}

impl Module {
    pub fn new(name: impl Into<String>) -> Self {
        Module { name: name.into(), decls: Vec::new() }
    }
}
