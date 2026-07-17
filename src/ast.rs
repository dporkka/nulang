//! Abstract Syntax Tree definitions for Nulang.

use crate::types::{Capability, EffectRow, Span, Type};
use serde::{Deserialize, Serialize};

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
    Wild,                                  // _
    Var(String),                           // x
    Lit(Literal),                          // 42, "hello"
    Tuple(Vec<Pattern>),                   // (p1, p2)
    Record(Vec<(String, Pattern)>),        // { a: p1, b: p2 }
    Variant(String, Option<Box<Pattern>>), // Some(x), None
    Alias(String, Box<Pattern>),           // x @ Pattern
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
    /// Let binding: let x [: T] = e1 in e2
    Let {
        name: String,
        ty: Option<Type>,
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
    /// Pattern match. Each arm is `(pattern, optional guard, body)`; the
    /// guard is a boolean expression evaluated with the pattern's bindings
    /// in scope after the pattern matches (`| pat if cond => body`).
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<(Pattern, Option<Expr>, Expr)>,
        span: Span,
    },
    /// Block expression: { e1; e2 }
    Block { exprs: Vec<Expr>, span: Span },
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
    /// Actor spawn: spawn ActorName { init } or spawn ActorName(args)
    Spawn {
        actor_type: Box<Expr>,
        init: Vec<(String, Expr)>,
        /// Positional constructor args: `spawn Foo(a, b)`.
        positional_args: Option<Vec<Expr>>,
        /// Named registration: `spawn Foo() as "name"`.
        register_as: Option<String>,
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
    /// Receive: receive { | Behavior => expr } [after ms => timeout_expr]
    Receive {
        arms: Vec<(String, Vec<String>, Expr)>,
        /// Optional timeout clause: `(timeout_ms, timeout_body)`. `timeout_ms`
        /// is an Int expression; on no matching message the actor waits up to
        /// that many milliseconds, then evaluates `timeout_body`.
        after: Option<(Box<Expr>, Box<Expr>)>,
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
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Assign,
    Pipe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    Deref,
    Ref(Capability),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
// State machine events (state_machine declaration transitions)
// ---------------------------------------------------------------------------

/// A single event transition inside a `state_machine` declaration:
/// `event name(params): Target`.
#[derive(Debug, Clone, PartialEq)]
pub struct StateMachineEvent {
    pub name: String,
    pub params: Vec<(String, Option<Type>)>,
    /// Target state name. Must be one of the states declared via `state`
    /// lines (enforced by the parser); handler-function targets like
    /// gen_statem's are not supported.
    pub target: String,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Actor backend kind
// ---------------------------------------------------------------------------

/// Actor execution backend kind, selected at compile time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ActorBackendKind {
    #[default]
    Native,
    WasmComponent,
}

// ---------------------------------------------------------------------------
// Function annotations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionAnnotation {
    /// `@tool(description: "...")` marks a function as an LLM-callable tool.
    Tool { description: String },
    /// `@backend(native | wasm)` selects the actor execution backend.
    Backend { kind: ActorBackendKind },
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
// Agent fallback & retry configuration
// ---------------------------------------------------------------------------
/// One entry in an agent's fallback pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentFallbackEntry {
    pub model: String,
    pub on: Vec<String>,
    pub max_tokens: Option<usize>,
}

/// Backoff strategy for agent LLM retries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentBackoff {
    Exponential { initial_ms: u64, factor: f64, max_ms: u64 },
    Fixed { delay_ms: u64 },
}

/// Retry configuration for an agent declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRetryConfig {
    pub max_attempts: u32,
    pub backoff: AgentBackoff,
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
        /// Compile-time backend selection. `None` means use the CLI default.
        backend: Option<ActorBackendKind>,
        /// Optional initializer block: `initial name(params) { body }`.
        initializer: Option<(String, Vec<(String, Option<Type>)>, Expr)>,
        span: Span,
    },
    /// State machine declaration (BEAM_PRIMITIVES §4.2 gen_statem adaptation):
    /// `state_machine Name { state S, event e(p): T, on_entry S { .. }, on_exit S { .. } }`.
    /// Kept as a real declaration so the typechecker, effect checker, and LSP
    /// see the source-level structure; desugared to an ordinary `Decl::Actor`
    /// by [`desugar_state_machine`].
    StateMachine {
        name: String,
        /// Declared states in source order; `states[0]` is the initial state.
        states: Vec<String>,
        events: Vec<StateMachineEvent>,
        /// `(state, body)` hooks run on transitions entering `state`.
        entry_hooks: Vec<(String, Expr)>,
        /// `(state, body)` hooks run on transitions leaving `state`.
        exit_hooks: Vec<(String, Expr)>,
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
    /// Agent declaration (v0.9): agent Name = { model: "...", system_prompt: "...", tools: [...], memory: { max_turns: N }, semantic_memory: { dimensions: D }, procedural_memory: { namespace: "..." }, fallback: [{ model: "...", on: [Timeout, RateLimit], max_tokens: 8192 }], retry: { max_attempts: 3, backoff: Exponential { initial_ms: 200, factor: 2.0, max_ms: 3000 } } }
    Agent {
        name: String,
        model: String,
        system_prompt: Option<String>,
        tools: Vec<String>,
        memory: Option<AgentMemoryConfig>,
        semantic_memory: Option<AgentSemanticMemoryConfig>,
        procedural_memory: Option<AgentProceduralMemoryConfig>,
        pricing: Option<AgentPricing>,
        fallback: Vec<AgentFallbackEntry>,
        retry: Option<AgentRetryConfig>,
        span: Span,
    },
    /// Database declaration: database Name { table Name { col: Type, ... } }
    Database {
        name: String,
        tables: Vec<DatabaseTable>,
        span: Span,
    },
}

// ---------------------------------------------------------------------------
// Database declaration (Turso/libSQL first-class integration)
// ---------------------------------------------------------------------------

/// A column definition inside a `database` table.
#[derive(Debug, Clone, PartialEq)]
pub struct DatabaseColumn {
    pub name: String,
    pub col_type: Type,
    pub modifiers: Vec<String>, // "primary_key", "unique", "not_null", etc.
    pub span: Span,
}

/// A table definition inside a `database` declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct DatabaseTable {
    pub name: String,
    pub columns: Vec<DatabaseColumn>,
    pub span: Span,
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
// State machine desugar (state_machine -> actor)
// ---------------------------------------------------------------------------

/// Desugar a `state_machine` declaration into an ordinary actor. The
/// typechecker, effect checker, and HIR lowering all run the result through
/// exactly the same paths as a hand-written `Decl::Actor`, so the feature
/// needs no IR, bytecode, or runtime support (BEAM_PRIMITIVES §15 Phase 2).
///
/// The generated actor has:
/// - a `Local` string state field `_sm_state` initialized to the first
///   declared state (`states` must be non-empty; enforced by the parser),
///   holding the current state tag;
/// - one behavior per `event name(params): Target`, whose body
///   1. runs the `on_exit` hook of the *current* state, if one is declared —
///      an if-chain comparing `_sm_state` against each hooked state tag;
///   2. assigns `_sm_state = "Target"`;
///   3. runs the `on_entry Target` hook inline, if one is declared (the
///      target is statically known, so no dispatch is needed);
///   4. evaluates to `nil`.
///
/// Hooks run on every matching transition, including self-transitions
/// (e.g. `disconnect: Closed` taken while already `Closed` runs both the
/// `Closed` exit and entry hooks). Because events do not name source states,
/// every event is allowed in every state — gen_statem's "event ignored in a
/// state where it is not allowed" case cannot arise, so no message is ever
/// dropped for state reasons. `send`/`ask` against the machine behave
/// exactly as against any actor.
pub fn desugar_state_machine(
    name: &str,
    states: &[String],
    events: &[StateMachineEvent],
    entry_hooks: &[(String, Expr)],
    exit_hooks: &[(String, Expr)],
    span: Span,
) -> Decl {
    let initial = states.first().cloned().unwrap_or_default();
    let state_field = (
        "_sm_state".to_string(),
        StateModel::Local,
        Type::string(),
        Expr::Literal(Literal::String(initial), span),
    );

    let sm_state = || Expr::FieldAccess {
        expr: Box::new(Expr::SelfRef(span)),
        field: "_sm_state".to_string(),
        span,
    };

    let behaviors = events
        .iter()
        .map(|event| {
            let mut body_exprs: Vec<Expr> = Vec::new();
            // on_exit: dispatch on the current state tag over the states
            // that declare an exit hook. The hook value is discarded into a
            // trailing `unit` because an `if` without `else` unifies its
            // then-branch with Unit — a non-Unit hook body (e.g. one ending
            // in `nil`) would otherwise fail type checking.
            for (state, hook) in exit_hooks {
                body_exprs.push(Expr::If {
                    cond: Box::new(Expr::Binary {
                        op: BinOp::Eq,
                        left: Box::new(sm_state()),
                        right: Box::new(Expr::Literal(Literal::String(state.clone()), span)),
                        span,
                    }),
                    then_branch: Box::new(Expr::Block {
                        exprs: vec![hook.clone(), Expr::Literal(Literal::Unit, span)],
                        span,
                    }),
                    else_branch: None,
                    span,
                });
            }
            // Transition to the target state.
            body_exprs.push(Expr::Assign {
                target: Box::new(sm_state()),
                value: Box::new(Expr::Literal(Literal::String(event.target.clone()), span)),
                span,
            });
            // on_entry of the (statically known) target state, if declared.
            if let Some((_, hook)) = entry_hooks.iter().find(|(s, _)| s == &event.target) {
                body_exprs.push(hook.clone());
            }
            body_exprs.push(Expr::Literal(Literal::Nil, span));
            Behavior {
                name: event.name.clone(),
                params: event.params.clone(),
                body: Expr::Block {
                    exprs: body_exprs,
                    span,
                },
                effect: None,
                cap: Capability::Ref,
                span: event.span,
            }
        })
        .collect();

    Decl::Actor {
        name: name.to_string(),
        type_params: vec![],
        persistent: false,
        state_fields: vec![state_field],
        behaviors,
        init: vec![],
        backend: None,
        initializer: None,
        span,
    }
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

    #[test]
    fn test_desugar_state_machine() {
        let sp = Span::default();
        let hook = || Expr::Literal(Literal::Unit, sp);
        let events = vec![
            StateMachineEvent {
                name: "connect".to_string(),
                params: vec![("address".to_string(), None)],
                target: "Connecting".to_string(),
                span: sp,
            },
            StateMachineEvent {
                name: "disconnect".to_string(),
                params: vec![],
                target: "Closed".to_string(),
                span: sp,
            },
        ];
        let decl = desugar_state_machine(
            "TcpConnection",
            &["Closed".to_string(), "Connecting".to_string()],
            &events,
            &[("Connecting".to_string(), hook())],
            &[("Closed".to_string(), hook())],
            sp,
        );
        let Decl::Actor {
            name,
            persistent,
            state_fields,
            behaviors,
            ..
        } = decl
        else {
            panic!("state machine should desugar to Decl::Actor");
        };
        assert_eq!(name, "TcpConnection");
        assert!(!persistent);
        // The generated state field holds the initial state tag.
        assert_eq!(state_fields.len(), 1);
        assert_eq!(state_fields[0].0, "_sm_state");
        assert_eq!(state_fields[0].1, StateModel::Local);
        assert_eq!(state_fields[0].2, Type::string());
        assert!(matches!(
            &state_fields[0].3,
            Expr::Literal(Literal::String(s), _) if s == "Closed"
        ));
        // One behavior per event, preserving names and params.
        assert_eq!(behaviors.len(), 2);
        assert_eq!(behaviors[0].name, "connect");
        assert_eq!(behaviors[0].params, vec![("address".to_string(), None)]);
        assert_eq!(behaviors[1].name, "disconnect");
        // Behavior body: [exit-hook ifs.., assign target, entry hook?, nil].
        let Expr::Block { exprs, .. } = &behaviors[0].body else {
            panic!("event behavior body should be a block");
        };
        // connect: one exit-hook if (Closed), the transition assign, the
        // Connecting entry hook, and the trailing nil.
        assert_eq!(exprs.len(), 4);
        assert!(matches!(&exprs[0], Expr::If { else_branch: None, .. }));
        assert!(
            matches!(&exprs[1], Expr::Assign { value, .. }
                if matches!(value.as_ref(), Expr::Literal(Literal::String(s), _) if s == "Connecting"))
        );
        assert!(matches!(&exprs[2], Expr::Literal(Literal::Unit, _)));
        assert!(matches!(&exprs[3], Expr::Literal(Literal::Nil, _)));
        // disconnect targets Closed, which has no entry hook: no inline hook.
        let Expr::Block { exprs, .. } = &behaviors[1].body else {
            panic!("event behavior body should be a block");
        };
        assert_eq!(exprs.len(), 3);
    }

    #[test]
    fn test_desugar_state_machine_empty_states_is_defensive() {
        // The parser never produces an empty state list; the desugar must
        // still not panic if called with one.
        let decl = desugar_state_machine("M", &[], &[], &[], &[], Span::default());
        let Decl::Actor { state_fields, .. } = decl else {
            panic!("state machine should desugar to Decl::Actor");
        };
        assert!(matches!(
            &state_fields[0].3,
            Expr::Literal(Literal::String(s), _) if s.is_empty()
        ));
    }
}
