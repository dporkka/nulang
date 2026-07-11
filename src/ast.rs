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
    Nil,
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
    /// Emit event: emit EventName(args)
    Emit {
        event: String,
        args: Vec<Expr>,
        span: Span,
    },
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
    /// For comprehension
    For {
        var: String,
        iterable: Box<Expr>,
        body: Box<Expr>,
        span: Span,
    },
    /// Return from function
    Return(Option<Box<Expr>>, Span),
    /// Break from loop
    Break(Span),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod,
    Eq, Ne, Lt, Le, Gt, Ge,
    And, Or,
    BitAnd, BitOr, BitXor, Shl, Shr,
    Assign,
    Pipe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg, Not, Deref, Ref(Capability),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectHandler {
    pub effect_name: String,
    pub op_name: String,
    pub params: Vec<String>,
    pub body: Expr,
    pub resume: bool,
}

// ---------------------------------------------------------------------------
// Behaviors (actor message handlers)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Behavior {
    pub name: String,
    pub params: Vec<(String, Option<Type>)>,
    pub body: Expr,
    pub effect: Option<EffectRow>,
    pub cap: Capability,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// State models for actor fields
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateModel {
    Local,
    Durable,
    EventSourced,
    Crdt,
}

impl Default for StateModel {
    fn default() -> Self {
        StateModel::Local
    }
}

// ---------------------------------------------------------------------------
// Function annotations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionAnnotation {
    /// `@tool(description: "...")` marks a function as an LLM-callable tool.
    Tool { description: String },
}

// ---------------------------------------------------------------------------
// Agent memory configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct AgentMemoryConfig {
    pub max_turns: usize,
}

/// Per-token pricing configuration for an agent declaration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentPricing {
    pub input: f64,
    pub output: f64,
}

/// Semantic-memory configuration for an agent declaration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentSemanticMemoryConfig {
    pub dimensions: usize,
}

/// Procedural-memory configuration for an agent declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentProceduralMemoryConfig {
    pub namespace: String,
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Decl {
    /// Function declaration: pub fn name[T](x: T) -> R ! E { body }
    Function {
        name: String,
        type_params: Vec<String>,
        params: Vec<(String, Option<Type>)>,
        ret_type: Option<Type>,
        effect: Option<EffectRow>,
        cap: Option<Capability>,
        body: Expr,
        annotations: Vec<FunctionAnnotation>,
        public: bool,
        span: Span,
    },
    /// Actor declaration: [persistent] actor Name { state [model] name: Type = expr, behavior ... }
    Actor {
        name: String,
        type_params: Vec<String>,
        persistent: bool,
        state_fields: Vec<(String, StateModel, Type, Expr)>, // name, model, type, default
        behaviors: Vec<Behavior>,
        init: Vec<(String, Expr)>,
        span: Span,
    },
    /// Type alias: type MyInt = Int
    TypeAlias {
        name: String,
        type_params: Vec<String>,
        body: Type,
        public: bool,
        span: Span,
    },
    /// Record type: type Point = { x: Int, y: Int }
    RecordType {
        name: String,
        type_params: Vec<String>,
        fields: Vec<(String, Type)>,
        public: bool,
        span: Span,
    },
    /// Variant type: type Option[T] = Some(T) | None
    VariantType {
        name: String,
        type_params: Vec<String>,
        variants: Vec<(String, Option<Type>)>,
        public: bool,
        span: Span,
    },
    /// Effect declaration: effect MyEffect { op1: A -> B }
    EffectDecl {
        name: String,
        ops: Vec<(String, Vec<Type>, Type)>, // name, arg types, ret type
        span: Span,
    },
    /// Module declaration: module Name { ... }
    Module {
        name: String,
        exports: Vec<String>,
        decls: Vec<Decl>,
        span: Span,
    },
    /// Import: import "path" or import Module.{name1, name2}
    Import {
        path: String,
        items: Vec<String>,
        span: Span,
    },
    /// Foreign function interface block: extern "lib" { fn f(x: T) -> R }
    Extern {
        library: String,
        funcs: Vec<ExternFunc>,
        span: Span,
    },
    /// Workflow declaration (v0.8): workflow Name { step name { body } ... }
    Workflow {
        name: String,
        input: Option<(String, Type)>,
        items: Vec<WorkflowItem>,
        compensate: Option<Expr>,
        span: Span,
    },
    /// Agent declaration (v0.9): agent Name = { model: "...", system_prompt: "...", tools: [...], memory: { max_turns: N }, semantic_memory: { dimensions: D }, procedural_memory: { namespace: "..." } }
    Agent {
        name: String,
        model: String,
        system_prompt: Option<String>,
        tools: Vec<String>,
        memory: Option<AgentMemoryConfig>,
        semantic_memory: Option<AgentSemanticMemoryConfig>,
        procedural_memory: Option<AgentProceduralMemoryConfig>,
        pricing: Option<AgentPricing>,
        span: Span,
    },
}

/// A single item inside a `workflow` declaration, preserving the original
/// source order of sequential steps and parallel blocks.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkflowItem {
    Step(WorkflowStep),
    Parallel(Vec<WorkflowStep>),
}

/// A single step inside a `workflow` declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowStep {
    pub name: String,
    pub body: Expr,
    /// Optional saga compensation expression run when a later step fails.
    pub compensate: Option<Expr>,
    pub span: Span,
}

/// Foreign function declaration inside an `extern` block.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternFunc {
    pub name: String,
    pub params: Vec<(String, Type)>,
    pub ret: Type,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Top-level AST
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct AstModule {
    pub name: String,
    pub decls: Vec<Decl>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_literal_variants() {
        // Construct each Literal variant and verify Debug output
        let i = Literal::Int(42);
        assert_eq!(format!("{:?}", i), "Int(42)");

        let f = Literal::Float(3.14);
        assert_eq!(format!("{:?}", f), "Float(3.14)");

        let s = Literal::String("hello".to_string());
        assert_eq!(format!("{:?}", s), "String(\"hello\")");

        let b = Literal::Bool(true);
        assert_eq!(format!("{:?}", b), "Bool(true)");

        let n = Literal::Nil;
        assert_eq!(format!("{:?}", n), "Nil");

        let u = Literal::Unit;
        assert_eq!(format!("{:?}", u), "Unit");
    }

    #[test]
    fn test_binop_variants() {
        // Construct each BinOp variant
        let ops = vec![
            (BinOp::Add, "Add"),
            (BinOp::Sub, "Sub"),
            (BinOp::Mul, "Mul"),
            (BinOp::Div, "Div"),
            (BinOp::Mod, "Mod"),
            (BinOp::Eq, "Eq"),
            (BinOp::Ne, "Ne"),
            (BinOp::Lt, "Lt"),
            (BinOp::Le, "Le"),
            (BinOp::Gt, "Gt"),
            (BinOp::Ge, "Ge"),
            (BinOp::And, "And"),
            (BinOp::Or, "Or"),
            (BinOp::BitAnd, "BitAnd"),
            (BinOp::BitOr, "BitOr"),
            (BinOp::BitXor, "BitXor"),
            (BinOp::Shl, "Shl"),
            (BinOp::Shr, "Shr"),
            (BinOp::Assign, "Assign"),
            (BinOp::Pipe, "Pipe"),
        ];
        for (op, name) in ops {
            assert_eq!(format!("{:?}", op), name);
        }
    }

    #[test]
    fn test_unop_variants() {
        assert_eq!(format!("{:?}", UnOp::Neg), "Neg");
        assert_eq!(format!("{:?}", UnOp::Not), "Not");
        assert_eq!(format!("{:?}", UnOp::Deref), "Deref");
        assert_eq!(format!("{:?}", UnOp::Ref(Capability::Val)), "Ref(Val)");
    }

    #[test]
    fn test_state_model_default() {
        assert_eq!(StateModel::default(), StateModel::Local);
    }

    #[test]
    fn test_span_default() {
        let s = Span::default();
        assert_eq!(s.start, 0);
        assert_eq!(s.end, 0);
        assert_eq!(s.line, 0);
        assert_eq!(s.column, 0);
    }

    #[test]
    fn test_ast_module_new() {
        let m = AstModule {
            name: "test".to_string(),
            decls: vec![],
        };
        assert_eq!(m.name, "test");
        assert!(m.decls.is_empty());
    }

    #[test]
    fn test_effect_handler_new() {
        // Without resume offset
        let h = EffectHandler {
            effect_name: "IO".to_string(),
            op_name: "print".to_string(),
            params: vec!["msg".to_string()],
            body: Expr::Literal(Literal::Unit, Span::default()),
            resume: false,
        };
        assert_eq!(h.effect_name, "IO");
        assert_eq!(h.op_name, "print");
        assert_eq!(h.params, vec!["msg"]);
        assert!(matches!(h.body, Expr::Literal(Literal::Unit, _)));
        assert!(!h.resume);

        // With resume offset
        let h2 = EffectHandler {
            effect_name: "Net".to_string(),
            op_name: "fetch".to_string(),
            params: vec!["url".to_string()],
            body: Expr::Literal(Literal::Int(0), Span::default()),
            resume: true,
        };
        assert_eq!(h2.effect_name, "Net");
        assert!(h2.resume);
    }

    #[test]
    fn test_behavior_new() {
        let b = Behavior {
            name: "handle_msg".to_string(),
            params: vec![("x".to_string(), Some(Type::int()))],
            body: Expr::Literal(Literal::Unit, Span::default()),
            effect: Some(EffectRow::empty()),
            cap: Capability::Val,
            span: Span::default(),
        };
        assert_eq!(b.name, "handle_msg");
        assert_eq!(b.params.len(), 1);
        assert_eq!(b.params[0].0, "x");
        assert_eq!(b.params[0].1, Some(Type::int()));
        assert_eq!(b.effect, Some(EffectRow::empty()));
        assert_eq!(b.cap, Capability::Val);
    }

    #[test]
    fn test_agent_pricing_default() {
        let p = AgentPricing {
            input: 0.001,
            output: 0.002,
        };
        assert_eq!(p.input, 0.001);
        assert_eq!(p.output, 0.002);
    }

    #[test]
    fn test_extern_func_new() {
        let f = ExternFunc {
            name: "sqrt".to_string(),
            params: vec![("x".to_string(), Type::float())],
            ret: Type::float(),
            span: Span::default(),
        };
        assert_eq!(f.name, "sqrt");
        assert_eq!(f.params.len(), 1);
        assert_eq!(f.params[0].0, "x");
        assert_eq!(f.params[0].1, Type::float());
        assert_eq!(f.ret, Type::float());
    }
}
