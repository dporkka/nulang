//! Abstract Syntax Tree definitions for Nulang.

use crate::types::{Capability, EffectRow, Span, Type};

// ---------------------------------------------------------------------------
// Literals
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
    Unit,
}

// ---------------------------------------------------------------------------
// Patterns
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Wild,                       // _
    Var(String),                // x
    Lit(Literal),               // 42, "hello"
    Tuple(Vec<Pattern>),        // (p1, p2)
    Record(Vec<(String, Pattern)>), // { a: p1, b: p2 }
    Variant(String, Option<Box<Pattern>>), // Some(x), None
    Alias(String, Box<Pattern>), // x @ Pattern
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Literal value
    Literal(Literal, Span),
    /// Variable reference
    Var(String, Span),
    /// Lambda: fn(x: T) -> e
    Lambda {
        params: Vec<(String, Option<Type>)>,
        body: Box<Expr>,
        effect: Option<EffectRow>,
        span: Span,
    },
    /// Function application: f(x, y)
    App {
        func: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    /// Let binding: let x = e1 in e2
    Let {
        name: String,
        value: Box<Expr>,
        body: Box<Expr>,
        span: Span,
    },
    /// Let-rec: let rec f = e1 in e2
    LetRec {
        name: String,
        params: Vec<(String, Option<Type>)>,
        value: Box<Expr>,
        body: Box<Expr>,
        span: Span,
    },
    /// If/else (expression, not statement)
    If {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Option<Box<Expr>>,
        span: Span,
    },
    /// Pattern match
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<(Pattern, Expr)>,
        span: Span,
    },
    /// Block expression: { e1; e2 }
    Block {
        exprs: Vec<Expr>,
        span: Span,
    },
    /// Tuple: (e1, e2)
    Tuple(Vec<Expr>, Span),
    /// Record literal: { a: e1, b: e2 }
    Record(Vec<(String, Expr)>, Span),
    /// Record field access: rec.field
    FieldAccess {
        expr: Box<Expr>,
        field: String,
        span: Span,
    },
    /// Array literal: [e1, e2]
    Array(Vec<Expr>, Span),
    /// Array index: arr[i]
    Index {
        arr: Box<Expr>,
        idx: Box<Expr>,
        span: Span,
    },
    /// Binary operator
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    /// Unary operator
    Unary {
        op: UnOp,
        expr: Box<Expr>,
        span: Span,
    },
    /// Assignment: x = e
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
        span: Span,
    },
    /// Actor spawn: spawn ActorName { init }
    Spawn {
        actor_type: Box<Expr>,
        init: Vec<(String, Expr)>,
        span: Span,
    },
    /// Message send: actor ! behavior(args)
    Send {
        actor: Box<Expr>,
        behavior: String,
        args: Vec<Expr>,
        span: Span,
    },
    /// Request/response: ask actor behavior(args)
    Ask {
        actor: Box<Expr>,
        behavior: String,
        args: Vec<Expr>,
        span: Span,
    },
    /// Receive: receive { | Behavior => expr }
    Receive {
        arms: Vec<(String, Vec<String>, Expr)>,
        span: Span,
    },
    /// Self reference within actor
    SelfRef(Span),
    /// Perform effect: perform Effect.op(arg)
    Perform {
        effect: String,
        op: String,
        args: Vec<Expr>,
        span: Span,
    },
    /// Handle effect: handle expr { | op(x) => ... | return(x) => ... }
    Handle {
        body: Box<Expr>,
        handlers: Vec<EffectHandler>,
        span: Span,
    },
    /// Actor migration: migrate actor to node
    Migrate {
        actor: Box<Expr>,
        node: Box<Expr>,
        span: Span,
    },
    /// Capability annotation: actor :cap iso
    CapAnnotate {
        expr: Box<Expr>,
        cap: Capability,
        span: Span,
    },
    /// Type annotation: expr : Type
    TypeAnnotate {
        expr: Box<Expr>,
        ty: Type,
        span: Span,
    },
    /// Pipe: x |> f
    Pipe {
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    /// Try/catch: try expr catch { | Error => handler }
    Try {
        body: Box<Expr>,
        catch_arms: Vec<(Pattern, Expr)>,
        span: Span,
    },
    /// Await: async result binding
    Await {
        expr: Box<Expr>,
        span: Span,
    },
    /// Actor definition (nested within expressions)
    ActorDef(ActorDef, Span),
    /// Agent definition (nested within expressions)
    AgentDef(AgentDef, Span),
}

// ---------------------------------------------------------------------------
// Binary / Unary operators
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod,
    Eq, Ne, Lt, Le, Gt, Ge,
    And, Or,
    Cons,
    Pipe,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UnOp {
    Neg, Not,
}

// ---------------------------------------------------------------------------
// Effect handler
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct EffectHandler {
    pub effect: String,
    pub op: String,
    pub params: Vec<String>,
    pub body: Expr,
    pub resume: bool,
}

// ---------------------------------------------------------------------------
// Actor definition
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct ActorDef {
    pub name: String,
    pub type_params: Vec<String>,
    pub fields: Vec<(String, Type)>,
    pub behaviors: Vec<Behavior>,
    pub initial_behaviour: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Behavior {
    pub name: String,
    pub params: Vec<(String, Type)>,
    pub body: Expr,
    pub effect_annotation: Option<EffectRow>,
}

// ---------------------------------------------------------------------------
// Agent definition
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct AgentDef {
    pub name: String,
    pub fields: Vec<(String, Type)>,
    pub behaviors: Vec<Behavior>,
    pub llm_config: Option<LlmConfig>,
    pub tool_bindings: Vec<ToolBinding>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlmConfig {
    pub model: String,
    pub system_prompt: String,
    pub temperature: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolBinding {
    pub name: String,
    pub effect: String,
    pub description: String,
}

// ---------------------------------------------------------------------------
// Declarations (top-level)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Decl {
    Fun {
        name: String,
        type_params: Vec<String>,
        params: Vec<(String, Type)>,
        ret_type: Option<Type>,
        effect: Option<EffectRow>,
        body: Expr,
        span: Span,
    },
    Actor {
        def: ActorDef,
        span: Span,
    },
    Agent {
        def: AgentDef,
        span: Span,
    },
    TypeAlias {
        name: String,
        params: Vec<String>,
        body: Type,
        span: Span,
    },
    Import {
        path: String,
        names: Vec<String>,
        span: Span,
    },
    Module {
        name: String,
        decls: Vec<Decl>,
        span: Span,
    },
}

// ---------------------------------------------------------------------------
// Module (top-level)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub name: String,
    pub decls: Vec<Decl>,
    pub span: Span,
}
