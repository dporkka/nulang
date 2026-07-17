//! Recursive descent parser for Nulang.
//!
//! Uses Pratt parser (precedence climbing) for expressions.
//! Entry point: `Parser::parse_module()`.

use crate::ast::*;
use crate::lexer::{Token, TokenKind};
use crate::types::{
    Capability, Effect, EffectRow, NuError, NuResult, PrimitiveType, Region, Span, Type, TypeVar,
};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Operator Precedence (13 levels, higher = tighter binding)
// ---------------------------------------------------------------------------

const PREC_LOWEST: u8 = 0;
const PREC_ASSIGN: u8 = 1; // = += -=
const PREC_PIPE: u8 = 2; // |>
const PREC_OR: u8 = 3; // ||
const PREC_AND: u8 = 4; // &&
const PREC_EQ: u8 = 5; // == !=
const PREC_CMP: u8 = 6; // < <= > >=
const PREC_TERM: u8 = 7; // + -
const PREC_FACTOR: u8 = 8; // * / %
const PREC_SHIFT: u8 = 9; // << >>
const PREC_BITAND: u8 = 10; // &
const PREC_BITXOR: u8 = 11; // ^
const PREC_BITOR: u8 = 12; // |
const PREC_PREFIX: u8 = 10; // ! - & (prefix)

fn prefix_precedence(op: &TokenKind) -> Option<(u8, bool)> {
    match op {
        TokenKind::Minus | TokenKind::Not | TokenKind::Bang => Some((PREC_PREFIX, true)),
        TokenKind::Ampersand => Some((PREC_PREFIX, true)),
        TokenKind::Star => Some((PREC_PREFIX, true)),
        _ => None,
    }
}

fn infix_precedence(op: &TokenKind) -> Option<(u8, bool)> {
    let (prec, right_assoc) = match op {
        TokenKind::Assign | TokenKind::PlusAssign | TokenKind::MinusAssign => (PREC_ASSIGN, true),
        TokenKind::PipeOp => (PREC_PIPE, false),
        TokenKind::Or => (PREC_OR, false),
        TokenKind::And => (PREC_AND, false),
        TokenKind::Eq | TokenKind::Ne => (PREC_EQ, false),
        TokenKind::Lt | TokenKind::Le | TokenKind::Gt | TokenKind::Ge => (PREC_CMP, false),
        TokenKind::Plus | TokenKind::Minus => (PREC_TERM, false),
        TokenKind::Star | TokenKind::Slash | TokenKind::Percent => (PREC_FACTOR, false),
        TokenKind::Shl | TokenKind::Shr => (PREC_SHIFT, false),
        TokenKind::Ampersand => (PREC_BITAND, false),
        TokenKind::Caret => (PREC_BITXOR, false),
        TokenKind::Pipe3 => (PREC_BITOR, false),
        // NOTE: single `|` is intentionally omitted. It is used as a match-arm
        // separator and function-type delimiter, so bitwise OR uses `|||`.
        _ => return None,
    };
    Some((prec, right_assoc))
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    local_type_params: HashMap<String, TypeVar>,
    global_type_constructors: HashMap<String, TypeVar>,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser {
            tokens,
            pos: 0,
            local_type_params: HashMap::new(),
            global_type_constructors: HashMap::new(),
        }
    }

    // === Entry Points ===

    pub fn parse_module(&mut self) -> NuResult<AstModule> {
        let mut decls = Vec::new();
        self.skip_newlines();
        while !self.is_at_end() {
            self.skip_newlines();
            if self.is_at_end() {
                break;
            }

            // Try declaration first, then expression
            let decl_start = self.pos;
            match self.parse_decl() {
                Ok(decl) => decls.push(decl),
                Err(e) => {
                    let consumed = self.pos - decl_start;
                    // Rewind any tokens a failed declaration parse consumed.
                    self.pos = decl_start;
                    if consumed > 0 {
                        // A declaration started but failed mid-way: surface the
                        // real declaration error instead of retrying the
                        // remaining tokens as an expression.
                        return Err(e);
                    }
                    // Not a declaration — this must be the top-level script body.
                    // Parse all remaining tokens as a block of expressions,
                    // using the recursive helper to splice statement-lets.
                    let exprs = self.collect_block_exprs(None)?;
                    let final_expr = if exprs.len() == 1 {
                        exprs.into_iter().next().unwrap()
                    } else {
                        Expr::Block { exprs, span: Span::default() }
                    };
                    decls.push(Decl::Function {
                        name: "__main".to_string(),
                        type_params: vec![],
                        params: vec![],
                        ret_type: None,
                        effect: None,
                        cap: None,
                        body: final_expr,
                        annotations: vec![],
                        public: false,
                        span: Span::new(0, 0),
                    });
                    break;
                }
            }
            self.skip_newlines_semicolons();
        }
        Ok(AstModule {
            name: "main".to_string(),
            decls,
        })
    }

    // === Declarations ===

    fn parse_decl(&mut self) -> NuResult<Decl> {
        self.local_type_params.clear();
        let _span = self.current_span();
        let annotations = self.parse_function_annotations()?;
        self.skip_newlines();
        let public = self.consume_if(&TokenKind::Pub);
        self.skip_newlines();
        match self.peek_kind().clone() {
            TokenKind::Fn => self.parse_function(public, annotations),
            TokenKind::Actor | TokenKind::Persistent => {
                let backend = annotations.iter().find_map(|a| match a {
                    crate::ast::FunctionAnnotation::Backend { kind } => Some(*kind),
                    _ => None,
                });
                self.parse_actor(backend)
            }
            TokenKind::StateMachine => self.parse_state_machine(),
            TokenKind::Agent => self.parse_agent(),
            TokenKind::Workflow => self.parse_workflow(),
            TokenKind::Database => self.parse_database(),
            TokenKind::Type => {
                self.advance(); // consume 'type'
                self.skip_newlines();
                match self.peek_kind().clone() {
                    TokenKind::Alias => self.parse_type_alias(public),
                    _ => {
                        // Peek ahead: if we see a '{' it's a record, if '|' or variant-like it's variant
                        // Actually: type Name = ... determines it
                        // We already consumed 'type', so look at what follows the name
                        self.parse_type_decl_variant_or_record(public)
                    }
                }
            }
            TokenKind::Effect => self.parse_effect_decl(),
            TokenKind::Extern => self.parse_extern(public),
            TokenKind::Import => self.parse_import(),
            TokenKind::Module => {
                self.advance(); // consume 'module'
                let name = self.expect_ident("module name")?;
                self.expect(TokenKind::LBrace)?;
                let mut decls = Vec::new();
                self.skip_newlines();
                while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
                    self.skip_newlines();
                    if self.match_token(&TokenKind::RBrace) {
                        break;
                    }
                    decls.push(self.parse_decl()?);
                    self.skip_newlines();
                }
                self.expect(TokenKind::RBrace)?;
                Ok(Decl::Module {
                    name,
                    exports: vec![],
                    decls,
                    span: self.current_span(),
                })
            }
            TokenKind::Eof => Err(NuError::ParseError {
                msg: "Unexpected end of file in declaration".to_string(),
                span: self.current_span(),
            }),
            other => Err(NuError::ParseError {
                msg: format!("Unexpected token in declaration: {}", other),
                span: self.current_span(),
            }),
        }
    }

    fn parse_function_annotations(&mut self) -> NuResult<Vec<FunctionAnnotation>> {
        let mut annotations = Vec::new();
        while self.consume_if(&TokenKind::At) {
            let name = match self.peek_kind() {
                TokenKind::Tool => {
                    self.advance();
                    "tool".to_string()
                }
                TokenKind::Ident(s) => {
                    let s = s.clone();
                    self.advance();
                    s
                }
                other => {
                    return Err(NuError::ParseError {
                        msg: format!("Expected annotation name, found {}", other),
                        span: self.current_span(),
                    });
                }
            };
            self.expect(TokenKind::LParen)?;
            let mut fields: HashMap<String, String> = HashMap::new();
            self.skip_newlines();
            while !self.match_token(&TokenKind::RParen) && !self.is_at_end() {
                let field_name = self.expect_ident("annotation field name")?;
                if self.consume_if(&TokenKind::Colon) {
                    let field_value = self.expect_string("annotation field value")?;
                    fields.insert(field_name, field_value);
                } else {
                    fields.insert(String::new(), field_name);
                }
                self.skip_newlines();
                if !self.consume_if(&TokenKind::Comma) {
                    break;
                }
                self.skip_newlines();
            }
            self.expect(TokenKind::RParen)?;
            match name.as_str() {
                "tool" => {
                    let description = fields.remove("description").unwrap_or_default();
                    annotations.push(FunctionAnnotation::Tool { description });
                }
                "backend" => {
                    let kind_str = fields
                        .remove("kind")
                        .or_else(|| fields.remove(""))
                        .unwrap_or_default();
                    let kind = match kind_str.as_str() {
                        "native" => crate::ast::ActorBackendKind::Native,
                        "wasm" => crate::ast::ActorBackendKind::WasmComponent,
                        other => return Err(NuError::ParseError {
                            msg: format!("Unknown backend '{}'; expected 'native' or 'wasm'", other),
                            span: self.current_span(),
                        }),
                    };
                    annotations.push(FunctionAnnotation::Backend { kind });
                }
                _ => {
                    return Err(NuError::ParseError {
                        msg: format!("Unknown function annotation: @{}", name),
                        span: self.current_span(),
                    });
                }
            }
        }
        Ok(annotations)
    }

    fn parse_function(
        &mut self,
        public: bool,
        annotations: Vec<FunctionAnnotation>,
    ) -> NuResult<Decl> {
        let span = self.current_span();
        self.advance(); // consume 'fn'
        let name = self.expect_ident("function name")?;

        // Type parameters [T, U]
        let type_params = self.parse_type_params()?;

        self.expect(TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(TokenKind::RParen)?;

        // Return type
        let ret_type = if self.consume_if(&TokenKind::Arrow) {
            Some(self.parse_type()?)
        } else {
            None
        };

        // Effect annotation
        let effect = if self.consume_if(&TokenKind::Bang) || self.consume_if(&TokenKind::Throws) {
            Some(self.parse_effect_row()?)
        } else {
            None
        };

        // Capability annotation
        let cap = if self.consume_if(&TokenKind::Colon) {
            Some(self.parse_capability()?)
        } else {
            None
        };

        let body = self.parse_expr()?;
        Ok(Decl::Function {
            name,
            type_params,
            params,
            ret_type,
            effect,
            cap,
            body,
            annotations,
            public,
            span,
        })
    }

    fn parse_actor(&mut self, backend: Option<crate::ast::ActorBackendKind>) -> NuResult<Decl> {
        let span = self.current_span();
        let persistent = self.consume_if(&TokenKind::Persistent);
        self.expect(TokenKind::Actor)?;
        let name = self.expect_ident("actor name")?;
        let type_params = self.parse_type_params()?;
        self.expect(TokenKind::LBrace)?;

        let mut state_fields = Vec::new();
        let mut behaviors = Vec::new();
        let mut initializer: Option<(String, Vec<(String, Option<Type>)>, Expr)> = None;
        self.skip_newlines();
        while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.match_token(&TokenKind::RBrace) {
                break;
            }
            match self.peek_kind().clone() {
                TokenKind::State => {
                    self.advance(); // 'state'
                    let model = self.parse_state_model();
                    let field_name = self.expect_ident("state field name")?;
                    let ty = if self.consume_if(&TokenKind::Colon) {
                        self.parse_type()?
                    } else {
                        Type::unit()
                    };
                    if !self.consume_if(&TokenKind::Assign) {
                        self.expect(TokenKind::Colon)?;
                    }
                    let default = self.parse_expr()?;
                    state_fields.push((field_name, model, ty, default));
                    self.skip_newlines_semicolons();
                }
                TokenKind::Behavior => {
                    behaviors.push(self.parse_behavior()?);
                }
                TokenKind::Initial => {
                    if initializer.is_some() {
                        return Err(NuError::ParseError {
                            msg: "Duplicate 'initial' block in actor".to_string(),
                            span: self.current_span(),
                        });
                    }
                    self.advance(); // consume 'initial'
                    let init_name = self.expect_ident("initializer name")?;
                    self.expect(TokenKind::LParen)?;
                    let params = self.parse_params()?;
                    self.expect(TokenKind::RParen)?;
                    let body = self.parse_expr()?;
                    initializer = Some((init_name, params, body));
                }
                _ => {
                    return Err(NuError::ParseError {
                        msg: format!(
                            "Expected 'state', 'behavior', or 'initial' in actor body, got {}",
                            self.peek_kind()
                        ),
                        span: self.current_span(),
                    });
                }
            }
        }
        // Post-loop: parse the closing brace.
        self.expect(TokenKind::RBrace)?;

        Ok(Decl::Actor {
            name,
            type_params,
            persistent,
            state_fields,
            behaviors,
            init: vec![],
            backend,
            initializer,
            span,
        })
    }

    /// Parse a `state_machine` declaration (BEAM_PRIMITIVES §4.2 gen_statem
    /// adaptation, desugared to an actor by [`crate::ast::desugar_state_machine`]):
    ///
    /// ```text
    /// state_machine Name {
    ///   state StateName                       // one or more; first = initial
    ///   event event_name(params): StateName   // target must be a declared state
    ///   on_entry StateName { body }           // hooks; state must be declared
    ///   on_exit StateName { body }
    /// }
    /// ```
    ///
    /// `event`/`on_entry`/`on_exit` are contextual identifiers (like `after`
    /// in `receive ... after`), not reserved keywords. Unlike gen_statem, an
    /// event target MUST be a declared state name — handler-function targets
    /// (e.g. `event data_received(bytes): handle_data` in the §4.2 sketch)
    /// are rejected with a clear error. States must be declared explicitly
    /// with `state` lines, so the aspirational §4.2 sketch parses only once
    /// `Connecting`/`Connected` are declared.
    fn parse_state_machine(&mut self) -> NuResult<Decl> {
        let span = self.current_span();
        self.advance(); // consume 'state_machine'
        let name = self.expect_ident("state_machine name")?;
        self.expect(TokenKind::LBrace)?;

        let mut states: Vec<String> = Vec::new();
        let mut events: Vec<StateMachineEvent> = Vec::new();
        let mut entry_hooks: Vec<(String, Expr)> = Vec::new();
        let mut exit_hooks: Vec<(String, Expr)> = Vec::new();

        self.skip_newlines();
        while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.match_token(&TokenKind::RBrace) {
                break;
            }
            match self.peek_kind().clone() {
                TokenKind::State => {
                    self.advance(); // 'state'
                    states.push(self.expect_ident("state name")?);
                    self.skip_newlines_semicolons();
                }
                TokenKind::Ident(item) => {
                    let tok = self.advance();
                    match item.as_str() {
                        "event" => {
                            let event_name = self.expect_ident("event name")?;
                            let params = if self.consume_if(&TokenKind::LParen) {
                                let params = self.parse_params()?;
                                self.expect(TokenKind::RParen)?;
                                params
                            } else {
                                Vec::new()
                            };
                            self.expect(TokenKind::Colon)?;
                            let target = self.expect_ident("event target state")?;
                            events.push(StateMachineEvent {
                                name: event_name,
                                params,
                                target,
                                span: tok.span,
                            });
                            self.skip_newlines_semicolons();
                        }
                        "on_entry" | "on_exit" => {
                            let state_name = self.expect_ident("hook state name")?;
                            let body = self.parse_expr()?;
                            if item == "on_entry" {
                                entry_hooks.push((state_name, body));
                            } else {
                                exit_hooks.push((state_name, body));
                            }
                            self.skip_newlines_semicolons();
                        }
                        other => {
                            return Err(NuError::ParseError {
                                msg: format!(
                                    "Expected 'state', 'event', 'on_entry', or 'on_exit' in state_machine body, got '{}'",
                                    other
                                ),
                                span: tok.span,
                            });
                        }
                    }
                }
                _ => {
                    return Err(NuError::ParseError {
                        msg: format!(
                            "Expected 'state', 'event', 'on_entry', or 'on_exit' in state_machine body, got {}",
                            self.peek_kind()
                        ),
                        span: self.current_span(),
                    });
                }
            }
        }
        self.expect(TokenKind::RBrace)?;

        // Two-pass validation, run only now so `state` lines are known
        // regardless of where they appear relative to events and hooks.
        if states.is_empty() {
            return Err(NuError::ParseError {
                msg: format!(
                    "state_machine '{}' requires at least one 'state <Name>' declaration (the first declared state is the initial state)",
                    name
                ),
                span,
            });
        }
        for (i, state) in states.iter().enumerate() {
            if states[..i].contains(state) {
                return Err(NuError::ParseError {
                    msg: format!("duplicate state '{}' in state_machine '{}'", state, name),
                    span,
                });
            }
        }
        let state_list = states.join(", ");
        let declared = |state: &str| states.iter().any(|s| s == state);
        for (i, event) in events.iter().enumerate() {
            if events[..i].iter().any(|e| e.name == event.name) {
                return Err(NuError::ParseError {
                    msg: format!("duplicate event '{}' in state_machine '{}'", event.name, name),
                    span: event.span,
                });
            }
            if !declared(&event.target) {
                return Err(NuError::ParseError {
                    msg: format!(
                        "event '{}' targets unknown state '{}' in state_machine '{}' (declared states: {})",
                        event.name, event.target, name, state_list
                    ),
                    span: event.span,
                });
            }
        }
        for (kind, hooks) in [("on_entry", &entry_hooks), ("on_exit", &exit_hooks)] {
            for (i, (state, _)) in hooks.iter().enumerate() {
                if !declared(state) {
                    return Err(NuError::ParseError {
                        msg: format!(
                            "{} hook references unknown state '{}' in state_machine '{}' (declared states: {})",
                            kind, state, name, state_list
                        ),
                        span,
                    });
                }
                if hooks[..i].iter().any(|(s, _)| s == state) {
                    return Err(NuError::ParseError {
                        msg: format!(
                            "duplicate {} hook for state '{}' in state_machine '{}'",
                            kind, state, name
                        ),
                        span,
                    });
                }
            }
        }

        Ok(Decl::StateMachine {
            name,
            states,
            events,
            entry_hooks,
            exit_hooks,
            span,
        })
    }

    fn parse_agent(&mut self) -> NuResult<Decl> {
        let span = self.current_span();
        self.advance(); // consume 'agent'
        let name = self.expect_ident("agent name")?;
        self.expect(TokenKind::Assign)?;
        self.expect(TokenKind::LBrace)?;

        let mut fallback: Vec<AgentFallbackEntry> = Vec::new();
        let mut retry: Option<AgentRetryConfig> = None;
        let mut model: Option<String> = None;
        let mut system_prompt: Option<String> = None;
        let mut tools: Vec<String> = Vec::new();
        let mut memory: Option<AgentMemoryConfig> = None;
        let mut semantic_memory: Option<AgentSemanticMemoryConfig> = None;
        let mut procedural_memory: Option<AgentProceduralMemoryConfig> = None;
        let mut pricing: Option<AgentPricing> = None;

        self.skip_newlines();
        while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.match_token(&TokenKind::RBrace) {
                break;
            }
            let field_name = self.expect_ident("agent field name")?;
            self.expect(TokenKind::Colon)?;
            match field_name.as_str() {
                "model" => {
                    model = Some(self.expect_string("agent model")?);
                }
                "system_prompt" => {
                    system_prompt = Some(self.expect_string("agent system prompt")?);
                }
                "tools" => {
                    self.expect(TokenKind::LBracket)?;
                    self.skip_newlines();
                    while !self.match_token(&TokenKind::RBracket) && !self.is_at_end() {
                        self.skip_newlines();
                        if self.match_token(&TokenKind::RBracket) {
                            break;
                        }
                        tools.push(self.expect_ident("tool name")?);
                        self.skip_newlines();
                        if !self.consume_if(&TokenKind::Comma) {
                            break;
                        }
                        self.skip_newlines();
                    }
                    self.expect(TokenKind::RBracket)?;
                }
                "memory" => {
                    self.expect(TokenKind::LBrace)?;
                    self.skip_newlines();
                    let mut max_turns: Option<usize> = None;
                    while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
                        self.skip_newlines();
                        if self.match_token(&TokenKind::RBrace) {
                            break;
                        }
                        let mem_field = self.expect_ident("memory field name")?;
                        self.expect(TokenKind::Colon)?;
                        match mem_field.as_str() {
                            "max_turns" => {
                                let n = self.expect_int("max_turns")?;
                                max_turns = Some(n as usize);
                            }
                            other => {
                                return Err(NuError::ParseError {
                                    msg: format!("Unknown memory field: {}", other),
                                    span: self.current_span(),
                                });
                            }
                        }
                        self.skip_newlines();
                        if !self.consume_if(&TokenKind::Comma) {
                            break;
                        }
                        self.skip_newlines();
                    }
                    self.expect(TokenKind::RBrace)?;
                    memory = Some(AgentMemoryConfig {
                        max_turns: max_turns.unwrap_or(50),
                    });
                }
                "semantic_memory" => {
                    self.expect(TokenKind::LBrace)?;
                    self.skip_newlines();
                    let mut dimensions: Option<usize> = None;
                    while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
                        self.skip_newlines();
                        if self.match_token(&TokenKind::RBrace) {
                            break;
                        }
                        let sm_field = self.expect_ident("semantic memory field name")?;
                        self.expect(TokenKind::Colon)?;
                        match sm_field.as_str() {
                            "dimensions" => {
                                let n = self.expect_int("dimensions")?;
                                dimensions = Some(n as usize);
                            }
                            other => {
                                return Err(NuError::ParseError {
                                    msg: format!("Unknown semantic_memory field: {}", other),
                                    span: self.current_span(),
                                });
                            }
                        }
                        self.skip_newlines();
                        if !self.consume_if(&TokenKind::Comma) {
                            break;
                        }
                        self.skip_newlines();
                    }
                    self.expect(TokenKind::RBrace)?;
                    semantic_memory = Some(AgentSemanticMemoryConfig {
                        dimensions: dimensions.unwrap_or(64),
                    });
                }
                "procedural_memory" => {
                    self.expect(TokenKind::LBrace)?;
                    self.skip_newlines();
                    let mut namespace: Option<String> = None;
                    while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
                        self.skip_newlines();
                        if self.match_token(&TokenKind::RBrace) {
                            break;
                        }
                        let pm_field = self.expect_ident("procedural memory field name")?;
                        self.expect(TokenKind::Colon)?;
                        match pm_field.as_str() {
                            "namespace" => {
                                namespace = Some(self.expect_string("namespace")?);
                            }
                            other => {
                                return Err(NuError::ParseError {
                                    msg: format!("Unknown procedural_memory field: {}", other),
                                    span: self.current_span(),
                                });
                            }
                        }
                        self.skip_newlines();
                        if !self.consume_if(&TokenKind::Comma) {
                            break;
                        }
                        self.skip_newlines();
                    }
                    self.expect(TokenKind::RBrace)?;
                    procedural_memory = Some(AgentProceduralMemoryConfig {
                        namespace: namespace.unwrap_or_else(|| "default".to_string()),
                    });
                }
                "pricing" => {
                    self.expect(TokenKind::LBrace)?;
                    self.skip_newlines();
                    let mut input_cost: Option<f64> = None;
                    let mut output_cost: Option<f64> = None;
                    while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
                        self.skip_newlines();
                        if self.match_token(&TokenKind::RBrace) {
                            break;
                        }
                        let price_field = self.expect_ident("pricing field name")?;
                        self.expect(TokenKind::Colon)?;
                        match price_field.as_str() {
                            "input" => {
                                input_cost = Some(self.expect_float("pricing input")?);
                            }
                            "output" => {
                                output_cost = Some(self.expect_float("pricing output")?);
                            }
                            other => {
                                return Err(NuError::ParseError {
                                    msg: format!("Unknown pricing field: {}", other),
                                    span: self.current_span(),
                                });
                            }
                        }
                        self.skip_newlines();
                        if !self.consume_if(&TokenKind::Comma) {
                            break;
                        }
                        self.skip_newlines();
                    }
                    self.expect(TokenKind::RBrace)?;
                    pricing = Some(AgentPricing {
                        input: input_cost.unwrap_or(0.0),
                        output: output_cost.unwrap_or(0.0),
                    });
                }
                "fallback" => {
                    self.expect(TokenKind::LBracket)?;
                    self.skip_newlines();
                    while !self.match_token(&TokenKind::RBracket) && !self.is_at_end() {
                        self.skip_newlines();
                        if self.match_token(&TokenKind::RBracket) {
                            break;
                        }
                        self.expect(TokenKind::LBrace)?;
                        self.skip_newlines();
                        let mut fb_model: Option<String> = None;
                        let mut fb_on: Vec<String> = Vec::new();
                        let mut fb_max_tokens: Option<usize> = None;
                        while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
                            self.skip_newlines();
                            if self.match_token(&TokenKind::RBrace) {
                                break;
                            }
                            let fb_field = self.expect_ident("fallback field name")?;
                            self.expect(TokenKind::Colon)?;
                            match fb_field.as_str() {
                                "model" => {
                                    fb_model = Some(self.expect_string("fallback model")?);
                                }
                                "on" => {
                                    self.expect(TokenKind::LBracket)?;
                                    self.skip_newlines();
                                    while !self.match_token(&TokenKind::RBracket) && !self.is_at_end() {
                                        self.skip_newlines();
                                        if self.match_token(&TokenKind::RBracket) {
                                            break;
                                        }
                                        fb_on.push(self.expect_ident("error kind")?);
                                        self.skip_newlines();
                                        if !self.consume_if(&TokenKind::Comma) {
                                            break;
                                        }
                                        self.skip_newlines();
                                    }
                                    self.expect(TokenKind::RBracket)?;
                                }
                                "max_tokens" => {
                                    let n = self.expect_int("max_tokens")?;
                                    fb_max_tokens = Some(n as usize);
                                }
                                other => {
                                    return Err(NuError::ParseError {
                                        msg: format!("Unknown fallback field: {}", other),
                                        span: self.current_span(),
                                    });
                                }
                            }
                            self.skip_newlines();
                            if !self.consume_if(&TokenKind::Comma) {
                                break;
                            }
                            self.skip_newlines();
                        }
                        self.expect(TokenKind::RBrace)?;
                        let model = fb_model.unwrap_or_default();
                        fallback.push(AgentFallbackEntry {
                            model,
                            on: fb_on,
                            max_tokens: fb_max_tokens,
                        });
                        self.skip_newlines();
                        if !self.consume_if(&TokenKind::Comma) {
                            break;
                        }
                        self.skip_newlines();
                    }
                    self.expect(TokenKind::RBracket)?;
                }
                "retry" => {
                    self.expect(TokenKind::LBrace)?;
                    self.skip_newlines();
                    let mut max_attempts: Option<u32> = None;
                    let mut backoff: Option<AgentBackoff> = None;
                    while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
                        self.skip_newlines();
                        if self.match_token(&TokenKind::RBrace) {
                            break;
                        }
                        let retry_field = self.expect_ident("retry field name")?;
                        self.expect(TokenKind::Colon)?;
                        match retry_field.as_str() {
                            "max_attempts" => {
                                let n = self.expect_int("max_attempts")?;
                                max_attempts = Some(n as u32);
                            }
                            "backoff" => {
                                let name = self.expect_ident("backoff strategy")?;
                                match name.as_str() {
                                    "Exponential" => {
                                        self.expect(TokenKind::LBrace)?;
                                        self.skip_newlines();
                                        let mut initial_ms: Option<u64> = None;
                                        let mut factor: Option<f64> = None;
                                        let mut max_ms: Option<u64> = None;
                                        while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
                                            self.skip_newlines();
                                            if self.match_token(&TokenKind::RBrace) {
                                                break;
                                            }
                                            let bo_field = self.expect_ident("backoff field")?;
                                            self.expect(TokenKind::Colon)?;
                                            match bo_field.as_str() {
                                                "initial_ms" => {
                                                    initial_ms = Some(self.expect_int("initial_ms")? as u64);
                                                }
                                                "factor" => {
                                                    factor = Some(self.expect_float("factor")?);
                                                }
                                                "max_ms" => {
                                                    max_ms = Some(self.expect_int("max_ms")? as u64);
                                                }
                                                other => {
                                                    return Err(NuError::ParseError {
                                                        msg: format!("Unknown Exponential backoff field: {}", other),
                                                        span: self.current_span(),
                                                    });
                                                }
                                            }
                                            self.skip_newlines();
                                            if !self.consume_if(&TokenKind::Comma) {
                                                break;
                                            }
                                            self.skip_newlines();
                                        }
                                        self.expect(TokenKind::RBrace)?;
                                        backoff = Some(AgentBackoff::Exponential {
                                            initial_ms: initial_ms.unwrap_or(200),
                                            factor: factor.unwrap_or(2.0),
                                            max_ms: max_ms.unwrap_or(3000),
                                        });
                                    }
                                    "Fixed" => {
                                        self.expect(TokenKind::LBrace)?;
                                        self.skip_newlines();
                                        let mut delay_ms: Option<u64> = None;
                                        while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
                                            self.skip_newlines();
                                            if self.match_token(&TokenKind::RBrace) {
                                                break;
                                            }
                                            let field = self.expect_ident("Fixed backoff field")?;
                                            self.expect(TokenKind::Colon)?;
                                            if field == "delay_ms" {
                                                delay_ms = Some(self.expect_int("delay_ms")? as u64);
                                            } else {
                                                return Err(NuError::ParseError {
                                                    msg: format!("Unknown Fixed backoff field: {}", field),
                                                    span: self.current_span(),
                                                });
                                            }
                                            self.skip_newlines();
                                            if !self.consume_if(&TokenKind::Comma) {
                                                break;
                                            }
                                            self.skip_newlines();
                                        }
                                        self.expect(TokenKind::RBrace)?;
                                        backoff = Some(AgentBackoff::Fixed {
                                            delay_ms: delay_ms.unwrap_or(1000),
                                        });
                                    }
                                    other => {
                                        return Err(NuError::ParseError {
                                            msg: format!("Unknown backoff strategy: {}", other),
                                            span: self.current_span(),
                                        });
                                    }
                                }
                            }
                            other => {
                                return Err(NuError::ParseError {
                                    msg: format!("Unknown retry field: {}", other),
                                    span: self.current_span(),
                                });
                            }
                        }
                        self.skip_newlines();
                        if !self.consume_if(&TokenKind::Comma) {
                            break;
                        }
                        self.skip_newlines();
                    }
                    self.expect(TokenKind::RBrace)?;
                    retry = Some(AgentRetryConfig {
                        max_attempts: max_attempts.unwrap_or(3),
                        backoff: backoff.unwrap_or(AgentBackoff::Exponential {
                            initial_ms: 200,
                            factor: 2.0,
                            max_ms: 3000,
                        }),
                    });
                }
                other => {
                    return Err(NuError::ParseError {
                        msg: format!("Unknown agent field: {}", other),
                        span: self.current_span(),
                    });
                }
            }
            self.skip_newlines();
            if !self.consume_if(&TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }
        self.expect(TokenKind::RBrace)?;

        let model = model.ok_or_else(|| NuError::ParseError {
            msg: "Agent declaration requires a 'model' field".to_string(),
            span,
        })?;

        Ok(Decl::Agent {
            name,
            model,
            system_prompt,
            tools,
            memory: memory.or(Some(AgentMemoryConfig { max_turns: 50 })),
            semantic_memory,
            procedural_memory,
            pricing,
            fallback,
            retry,
            span,
        })
    }

    /// Parse a database declaration:
    /// `database Name { TableName { col: Type modifier*, ... } ... }`
    fn parse_database(&mut self) -> NuResult<Decl> {
        let span = self.current_span();
        self.advance(); // consume 'database'
        let name = self.expect_ident("database name")?;
        self.expect(TokenKind::LBrace)?;
        self.skip_newlines();
        let mut tables: Vec<DatabaseTable> = Vec::new();
        while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.match_token(&TokenKind::RBrace) { break; }
            // Each table: Name { col: Type modifier*, ... }
            let table_name = self.expect_ident("table name")?;
            self.expect(TokenKind::LBrace)?;
            self.skip_newlines();
            let mut columns: Vec<DatabaseColumn> = Vec::new();
            while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
                self.skip_newlines();
                if self.match_token(&TokenKind::RBrace) { break; }
                let col_name = self.expect_ident("column name")?;
                self.expect(TokenKind::Colon)?;
                let col_type = self.parse_type()?;
                let mut modifiers: Vec<String> = Vec::new();
                while matches!(self.peek_kind(),
                    TokenKind::Ident(_) | TokenKind::UpperIdent(_)
                ) {
                    let m = self.expect_ident("column modifier")?;
                    modifiers.push(m);
                    self.skip_newlines();
                    if self.match_token(&TokenKind::Comma) || self.match_token(&TokenKind::RBrace) {
                        break;
                    }
                }
                columns.push(DatabaseColumn {
                    name: col_name,
                    col_type,
                    modifiers,
                    span: self.current_span(),
                });
                self.skip_newlines();
                let _ = self.consume_if(&TokenKind::Comma);
                self.skip_newlines();
            }
            self.expect(TokenKind::RBrace)?;
            tables.push(DatabaseTable {
                name: table_name,
                columns,
                span: self.current_span(),
            });
            self.skip_newlines();
        }
        Ok(Decl::Database { name, tables, span })
    }

    fn parse_state_model(&mut self) -> StateModel {
        match self.peek_kind() {
            TokenKind::Local => {
                self.advance();
                StateModel::Local
            }
            TokenKind::Durable => {
                self.advance();
                StateModel::Durable
            }
            TokenKind::EventSourced => {
                self.advance();
                StateModel::EventSourced
            }
            TokenKind::Crdt => {
                self.advance();
                StateModel::Crdt
            }
            _ => StateModel::Local,
        }
    }

    fn parse_workflow(&mut self) -> NuResult<Decl> {
        let span = self.current_span();
        self.advance(); // consume 'workflow'
        let name = self.expect_ident("workflow name")?;
        self.expect(TokenKind::LBrace)?;

        let mut items = Vec::new();
        let mut compensate = None;

        self.skip_newlines();
        while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.match_token(&TokenKind::RBrace) {
                break;
            }
            match self.peek_kind().clone() {
                TokenKind::Step => {
                    items.push(WorkflowItem::Step(self.parse_workflow_step()?));
                }
                TokenKind::Parallel => {
                    self.advance(); // 'parallel'
                    self.expect(TokenKind::LBrace)?;
                    let mut branch = Vec::new();
                    self.skip_newlines();
                    while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
                        self.skip_newlines();
                        if self.match_token(&TokenKind::RBrace) {
                            break;
                        }
                        branch.push(self.parse_workflow_step()?);
                        self.skip_newlines_semicolons();
                    }
                    self.expect(TokenKind::RBrace)?;
                    items.push(WorkflowItem::Parallel(branch));
                    self.skip_newlines_semicolons();
                }
                TokenKind::Compensate => {
                    self.advance(); // 'compensate'
                    self.expect(TokenKind::LBrace)?;
                    self.skip_newlines();
                    compensate = Some(self.parse_expr()?);
                    self.skip_newlines();
                    self.expect(TokenKind::RBrace)?;
                    self.skip_newlines_semicolons();
                }
                _ => {
                    return Err(NuError::ParseError {
                        msg: format!(
                            "Expected 'step', 'parallel', or 'compensate' in workflow body, got {}",
                            self.peek_kind()
                        ),
                        span: self.current_span(),
                    });
                }
            }
        }
        self.expect(TokenKind::RBrace)?;

        Ok(Decl::Workflow {
            name,
            input: None,
            items,
            compensate,
            span,
        })
    }

    fn parse_workflow_step(&mut self) -> NuResult<WorkflowStep> {
        let span = self.current_span();
        self.expect(TokenKind::Step)?;
        let name = self.expect_ident("step name")?;
        self.expect(TokenKind::LBrace)?;
        self.skip_newlines();
        let body = self.parse_expr()?;
        self.skip_newlines();
        self.expect(TokenKind::RBrace)?;
        let compensate = if self.consume_if(&TokenKind::Compensate) {
            self.expect(TokenKind::LBrace)?;
            self.skip_newlines();
            let expr = self.parse_expr()?;
            self.skip_newlines();
            self.expect(TokenKind::RBrace)?;
            Some(expr)
        } else {
            None
        };
        Ok(WorkflowStep {
            name,
            body,
            compensate,
            span,
        })
    }

    fn parse_type_alias(&mut self, public: bool) -> NuResult<Decl> {
        let span = self.current_span();
        self.advance(); // consume 'alias'
        let name = self.expect_ident("type alias name")?;
        let type_params = self.parse_type_params()?;
        self.expect(TokenKind::Assign)?;
        let body = self.parse_type()?;
        Ok(Decl::TypeAlias {
            name,
            type_params,
            body,
            public,
            span,
        })
    }

    fn parse_type_decl_variant_or_record(&mut self, public: bool) -> NuResult<Decl> {
        let span = self.current_span();
        let name = self.expect_ident("type name")?;
        let type_params = self.parse_type_params()?;
        self.expect(TokenKind::Assign)?;

        // Look ahead to determine if it's a record or variant
        self.skip_newlines();
        match self.peek_kind().clone() {
            TokenKind::LBrace => {
                // Record type
                self.advance(); // '{'
                let fields = self.parse_record_type_fields()?;
                Ok(Decl::RecordType {
                    name,
                    type_params,
                    fields,
                    public,
                    span,
                })
            }
            _ => {
                // Variant type: A | B | C
                let variants = self.parse_variants()?;
                Ok(Decl::VariantType {
                    name,
                    type_params,
                    variants,
                    public,
                    span,
                })
            }
        }
    }

    fn parse_effect_decl(&mut self) -> NuResult<Decl> {
        let span = self.current_span();
        self.advance(); // consume 'effect'
        let name = self.expect_ident("effect name")?;
        self.expect(TokenKind::LBrace)?;

        let mut ops = Vec::new();
        self.skip_newlines();
        while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.match_token(&TokenKind::RBrace) {
                break;
            }
            let op_name = self.expect_ident("operation name")?;
            self.expect(TokenKind::Colon)?;

            // Parse argument types
            // Forms: op: A -> B (single arg), op: (A, B) -> C (multiple args), op: -> B (no args)
            let mut arg_types = Vec::new();
            if self.consume_if(&TokenKind::LParen) {
                // Multi-arg form: op: (A, B) -> C
                while !self.match_token(&TokenKind::RParen) && !self.is_at_end() {
                    arg_types.push(self.parse_type()?);
                    if !self.consume_if(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(TokenKind::RParen)?;
            } else if !self.match_token(&TokenKind::Arrow) {
                // Single arg form: op: A -> B
                arg_types.push(self.parse_type()?);
            }
            // else: no-arg form op: -> B

            self.expect(TokenKind::Arrow)?;
            let ret_type = self.parse_type()?;
            ops.push((op_name, arg_types, ret_type));
            self.skip_newlines_semicolons();
        }
        self.expect(TokenKind::RBrace)?;

        Ok(Decl::EffectDecl { name, ops, span })
    }

    fn parse_import(&mut self) -> NuResult<Decl> {
        let span = self.current_span();
        self.advance(); // consume 'import'
        let path = self.expect_ident("import path")?;
        let items = Vec::new();
        self.skip_newlines_semicolons();
        Ok(Decl::Import { path, items, span })
    }

    fn parse_extern(&mut self, _public: bool) -> NuResult<Decl> {
        let span = self.current_span();
        self.advance(); // consume 'extern'

        let library = match self.peek_kind().clone() {
            TokenKind::StringLit(s) => {
                let s = s.clone();
                self.advance();
                s
            }
            other => {
                return Err(NuError::ParseError {
                    msg: format!(
                        "Expected string literal for library path, found {:?}",
                        other
                    ),
                    span: self.current_span(),
                })
            }
        };

        self.expect(TokenKind::LBrace)?;

        let mut funcs = Vec::new();
        self.skip_newlines();
        while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.match_token(&TokenKind::RBrace) {
                break;
            }

            let func_span = self.current_span();
            self.expect(TokenKind::Fn)?;
            let name = self.expect_ident("function name")?;
            self.expect(TokenKind::LParen)?;
            let raw_params = self.parse_params()?;
            self.expect(TokenKind::RParen)?;

            // Extern parameters must have explicit types.
            let mut params = Vec::new();
            for (param_name, param_ty) in raw_params {
                match param_ty {
                    Some(ty) => params.push((param_name, ty)),
                    None => {
                        return Err(NuError::ParseError {
                            msg: format!(
                                "Extern function '{}' parameter '{}' requires an explicit type",
                                name, param_name
                            ),
                            span: func_span,
                        })
                    }
                }
            }

            self.expect(TokenKind::Arrow)?;
            let ret = self.parse_type()?;

            funcs.push(ExternFunc {
                name,
                params,
                ret,
                span: func_span,
            });
            self.skip_newlines_semicolons();
        }

        self.expect(TokenKind::RBrace)?;
        Ok(Decl::Extern {
            library,
            funcs,
            span,
        })
    }

    // === Behaviors ===

    fn parse_behavior(&mut self) -> NuResult<Behavior> {
        let span = self.current_span();
        self.advance(); // consume 'behavior'
        let name = self.expect_ident("behavior name")?;
        self.expect(TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(TokenKind::RParen)?;

        // Optional effect annotation
        let effect = if self.consume_if(&TokenKind::Bang) || self.consume_if(&TokenKind::Throws) {
            Some(self.parse_effect_row()?)
        } else {
            None
        };

        // Optional capability annotation
        let cap = if self.consume_if(&TokenKind::Colon) {
            self.parse_capability()?
        } else {
            Capability::Ref // default
        };

        let body = self.parse_expr()?;
        Ok(Behavior {
            name,
            params,
            body,
            effect,
            cap,
            span,
        })
    }

    // === Expressions (Pratt parser) ===

    fn parse_expr(&mut self) -> NuResult<Expr> {
        self.parse_expr_with_prec(PREC_LOWEST)
    }

    fn parse_expr_with_prec(&mut self, min_prec: u8) -> NuResult<Expr> {
        // Parse prefix / primary expression
        let mut left = self.parse_prefix()?;

        // Handle infix operators
        loop {
            self.skip_newlines();
            let op = self.peek_kind().clone();
            if op == TokenKind::Eof {
                break;
            }

            // Special cases: function call, field access, array index, send
            if self.match_token(&TokenKind::LParen) {
                // Function call: left(args)
                self.advance(); // consume '('
                let args = self.parse_arg_list()?;
                let span = self.current_span();
                left = Expr::App {
                    func: Box::new(left),
                    args,
                    span,
                };
                continue;
            }

            // Send: actor ! behavior(args)
            if self.consume_if(&TokenKind::Bang) {
                let behavior = self.expect_ident("behavior name")?;
                self.expect(TokenKind::LParen)?;
                let args = self.parse_arg_list()?;
                let span = self.current_span();
                left = Expr::Send {
                    actor: Box::new(left),
                    behavior,
                    args,
                    remote: false,
                    span,
                };
                continue;
            }

            if self.consume_if(&TokenKind::Dot) {
                // Field access: expr.field or expr.0
                let field = match self.peek_kind().clone() {
                    TokenKind::IntLit(n) => {
                        self.advance();
                        format!("{}", n)
                    }
                    _ => self.expect_ident("field name")?,
                };
                let span = self.current_span();
                left = Expr::FieldAccess {
                    expr: Box::new(left),
                    field,
                    span,
                };
                continue;
            }

            // Try operator: expr? desugars to match on Ok/Error
            if self.consume_if(&TokenKind::Question) {
                let span = self.current_span();
                let x = "__try_x".to_string();
                let e = "__try_e".to_string();
                left = Expr::Match {
                    scrutinee: Box::new(left),
                    arms: vec![
                        (Pattern::Variant("Ok".to_string(), Some(Box::new(Pattern::Var(x.clone())))), None,
                         Expr::Var(x, span)),
                        (Pattern::Variant("Error".to_string(), Some(Box::new(Pattern::Var(e.clone())))), None,
                         Expr::Return(
                            Some(Box::new(Expr::App {
                                func: Box::new(Expr::Var("Error".to_string(), span)),
                                args: vec![Expr::Var(e, span)],
                                span,
                            })),
                            span,
                        )),
                    ],
                    span,
                };
                continue;
            }
            if self.consume_if(&TokenKind::LBracket) {
                // Array index: arr[idx]
                let idx = self.parse_expr()?;
                self.expect(TokenKind::RBracket)?;
                let span = self.current_span();
                left = Expr::Index {
                    arr: Box::new(left),
                    idx: Box::new(idx),
                    span,
                };
                continue;
            }

            if self.consume_if(&TokenKind::Colon) {
                let span = self.current_span();
                let is_cap = if let TokenKind::Ident(ref s) = self.peek_kind() {
                    s == "cap"
                } else {
                    false
                };
                if is_cap {
                    self.advance(); // consume 'cap'
                    let cap = self.parse_capability()?;
                    left = Expr::CapAnnotate {
                        expr: Box::new(left),
                        cap,
                        span,
                    };
                } else {
                    let ty = self.parse_type()?;
                    left = Expr::TypeAnnotate {
                        expr: Box::new(left),
                        ty,
                        span,
                    };
                }
                continue;
            }

            // Check for infix operators
            let (prec, right_assoc) = match infix_precedence(&op) {
                Some(p) => p,
                None => break,
            };

            if prec < min_prec {
                break;
            }

            self.advance(); // consume operator
            let next_min_prec = if right_assoc { prec } else { prec + 1 };
            let right = self.parse_expr_with_prec(next_min_prec)?;

            let span = self.current_span();
            if op == TokenKind::PipeOp {
                left = Expr::Pipe {
                    left: Box::new(left),
                    right: Box::new(right),
                    span,
                };
                continue;
            }

            let bin_op = token_to_binop(&op).ok_or_else(|| NuError::ParseError {
                msg: format!("Not a binary operator: {:?}", op),
                span: span.clone(),
            })?;

            left = Expr::Binary {
                op: bin_op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    fn parse_prefix(&mut self) -> NuResult<Expr> {
        self.skip_newlines();
        let span = self.current_span();

        match self.peek_kind().clone() {
            TokenKind::Eof => Err(NuError::ParseError {
                msg: "Unexpected end of file in expression".to_string(),
                span,
            }),
            kind => {
                // Check for prefix operators
                if let Some((prec, _)) = prefix_precedence(&kind) {
                    self.advance(); // consume operator
                    let operand = self.parse_expr_with_prec(prec)?;
                    let span = self.current_span();
                    let un_op = match kind {
                        TokenKind::Minus => UnOp::Neg,
                        TokenKind::Not | TokenKind::Bang => UnOp::Not,
                        TokenKind::Ampersand => UnOp::Ref(Capability::Ref),
                        TokenKind::Star => UnOp::Deref,
                        _ => unreachable!(),
                    };
                    return Ok(Expr::Unary {
                        op: un_op,
                        expr: Box::new(operand),
                        span,
                    });
                }

                match kind {
                    // Literals
                    TokenKind::IntLit(_)
                    | TokenKind::FloatLit(_)
                    | TokenKind::StringLit(_)
                    | TokenKind::BoolLit(_)
                    | TokenKind::NilLit
                    | TokenKind::UnitLit => self.parse_literal(),

                    // Identifiers
                    TokenKind::Ident(name) => {
                        let name = name.clone();
                        self.advance();
                        // Check for assignment
                        if self.consume_if(&TokenKind::Assign) {
                            let val = self.parse_expr()?;
                            let span = self.current_span();
                            Ok(Expr::Assign {
                                target: Box::new(Expr::Var(name, span.clone())),
                                value: Box::new(val),
                                span,
                            })
                        } else {
                            Ok(Expr::Var(name, span))
                        }
                    }
                    TokenKind::UpperIdent(name) => {
                        let name = name.clone();
                        self.advance();
                        Ok(Expr::Var(name, span))
                    }

                    // Keywords that start expressions
                    TokenKind::Fn => self.parse_lambda(),
                    TokenKind::Let => {
                        self.advance();
                        self.skip_newlines();
                        self.consume_if(&TokenKind::Rec);
                        let name = self.expect_ident("variable name")?;
                        if self.peek_kind() == &TokenKind::LParen {
                            self.parse_let_rec_named(name)
                        } else {
                            self.parse_let_named(name)
                        }
                    }
                    TokenKind::If => self.parse_if(),
                    TokenKind::Match => self.parse_match(),
                    TokenKind::LBrace => {
                        // Look ahead to distinguish record literal from block
                        if self.is_record_literal_ahead() {
                            self.parse_record_literal()
                        } else {
                            self.parse_block()
                        }
                    }
                    TokenKind::LParen => self.parse_tuple_or_paren(),
                    TokenKind::LBracket => self.parse_array(),
                    TokenKind::Spawn => self.parse_spawn(),
                    TokenKind::Send => self.parse_send_keyword(),
                    TokenKind::Ask => self.parse_ask(),
                    TokenKind::Perform => self.parse_perform(),
                    TokenKind::Handle => self.parse_handle(),
                    TokenKind::Emit => self.parse_emit(),
                    TokenKind::Receive => self.parse_receive(),
                    TokenKind::For => self.parse_for(),
                    TokenKind::While => self.parse_while(),
                    TokenKind::Migrate => self.parse_migrate(),
                    TokenKind::Return => {
                        self.advance();
                        if self.is_expr_start() {
                            let val = self.parse_expr()?;
                            Ok(Expr::Return(Some(Box::new(val)), self.current_span()))
                        } else {
                            Ok(Expr::Return(None, self.current_span()))
                        }
                    }
                    TokenKind::Break => {
                        self.advance();
                        if self.is_expr_start() {
                            let val = self.parse_expr()?;
                            Ok(Expr::Break(Some(Box::new(val)), self.current_span()))
                        } else {
                            Ok(Expr::Break(None, self.current_span()))
                        }
                    }
                    TokenKind::SelfKw => self.parse_self_ref(),

                    _ => Err(NuError::ParseError {
                        msg: format!("Unexpected token in expression: {}", kind),
                        span,
                    }),
                }
            }
        }
    }

    // === Expression Primitives ===

    fn parse_literal(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        match self.peek_kind().clone() {
            TokenKind::IntLit(v) => {
                self.advance();
                Ok(Expr::Literal(Literal::Int(v), span))
            }
            TokenKind::FloatLit(v) => {
                self.advance();
                Ok(Expr::Literal(Literal::Float(v), span))
            }
            TokenKind::StringLit(s) => {
                self.advance();
                if s.contains("#{") {
                    self.parse_interpolated_string(&s, span)
                } else {
                    Ok(Expr::Literal(Literal::String(s), span))
                }
            }
            TokenKind::BoolLit(b) => {
                self.advance();
                Ok(Expr::Literal(Literal::Bool(b), span))
            }
            TokenKind::NilLit => {
                self.advance();
                Ok(Expr::Literal(Literal::Nil, span))
            }
            TokenKind::UnitLit => {
                self.advance();
                Ok(Expr::Literal(Literal::Unit, span))
            }
            _ => Err(NuError::ParseError {
                msg: "Expected literal".to_string(),
                span,
            }),
        }
    }

    fn parse_lambda(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.advance(); // consume 'fn'
        self.expect(TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(TokenKind::RParen)?;

        // Lambda syntax: fn(x) -> body  or  fn(x) body
        // The -> introduces the body expression (not a return type)
        self.consume_if(&TokenKind::Arrow);

        let body = self.parse_expr()?;
        Ok(Expr::Lambda {
            params,
            body: Box::new(body),
            effect: None,
            span,
        })
    }

    fn parse_let(&mut self) -> NuResult<Expr> {
        let name = self.expect_ident("variable name")?;
        self.parse_let_named(name)
    }

    fn parse_let_named(&mut self, name: String) -> NuResult<Expr> {
        let span = self.current_span();

        // Optional type annotation
        let ty = if self.consume_if(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };

        self.expect(TokenKind::Assign)?;
        let value = self.parse_expr()?;
        let body = if self.consume_if(&TokenKind::In) {
            self.parse_expr()?
        } else {
            Expr::Block { exprs: vec![], span: Span::default() }
        };
        Ok(Expr::Let {
            name,
            ty,
            value: Box::new(value),
            body: Box::new(body),
            span,
        })
    }

    fn parse_let_rec(&mut self) -> NuResult<Expr> {
        let name = self.expect_ident("function name")?;
        self.parse_let_rec_named(name)
    }

    /// Parse a string containing `#{...}` interpolation markers.
    fn parse_interpolated_string(&self, raw: &str, span: Span) -> NuResult<Expr> {
        let mut parts: Vec<Expr> = Vec::new();
        let mut remaining = raw;
        while let Some(hash_brace) = remaining.find("#{") {
            if hash_brace > 0 {
                parts.push(Expr::Literal(
                    Literal::String(remaining[..hash_brace].to_string()),
                    span,
                ));
            }
            let expr_start = hash_brace + 2;
            let expr_str = &remaining[expr_start..];
            let mut depth = 1u32;
            let mut expr_end = 0usize;
            for (i, ch) in expr_str.char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 { expr_end = i; break; }
                    }
                    _ => {}
                }
            }
            if depth != 0 {
                return Err(NuError::ParseError { msg: "Unterminated interpolation: missing '}'".to_string(), span });
            }
            let expr_content = &expr_str[..expr_end];
            let expr = self.parse_inline_expr(expr_content, span)?;
            parts.push(expr);
            remaining = &expr_str[expr_end + 1..];
        }
        if !remaining.is_empty() {
            parts.push(Expr::Literal(Literal::String(remaining.to_string()), span));
        }
        if parts.len() == 1 {
            return Ok(parts.into_iter().next().unwrap());
        }
        let mut result = parts.remove(0);
        for part in parts {
            result = Expr::Binary { op: BinOp::Add, left: Box::new(result), right: Box::new(part), span };
        }
        Ok(result)
    }

    fn parse_inline_expr(&self, source: &str, span: Span) -> NuResult<Expr> {
        let mut lexer = crate::lexer::Lexer::new(source);
        let tokens = lexer.lex().map_err(|e| NuError::ParseError { msg: format!("Invalid interpolation expression: {}", e), span })?;
        let mut sub_parser = Parser::new(tokens);
        sub_parser.parse_expr()
    }

    fn parse_let_rec_named(&mut self, name: String) -> NuResult<Expr> {
        let span = self.current_span();
        self.expect(TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(TokenKind::RParen)?;
        self.expect(TokenKind::Assign)?;
        let value = self.parse_expr()?;
        self.expect(TokenKind::In)?;
        let body = self.parse_expr()?;
        Ok(Expr::LetRec {
            name,
            params,
            value: Box::new(value),
            body: Box::new(body),
            span,
        })
    }

    fn parse_if(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.advance(); // consume 'if'
        let cond = self.parse_expr()?;

        // Optional `then` keyword for ML-style syntax: `if c then a else b`
        let _ = self.consume_if(&TokenKind::Then);

        // Parse then branch: either { block } or single expression
        let then_branch = if self.match_token(&TokenKind::LBrace) {
            Box::new(self.parse_block()?)
        } else {
            Box::new(self.parse_expr()?)
        };

        let else_branch = if self.consume_if(&TokenKind::Else) {
            Some(if self.match_token(&TokenKind::LBrace) {
                Box::new(self.parse_block()?)
            } else {
                Box::new(self.parse_expr()?)
            })
        } else {
            None
        };

        Ok(Expr::If {
            cond: Box::new(cond),
            then_branch,
            else_branch,
            span,
        })
    }

    fn parse_match(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.advance(); // consume 'match'
        let scrutinee = self.parse_expr()?;
        let _ = self.consume_if(&TokenKind::With); // `with` is optional
        self.expect(TokenKind::LBrace)?;

        let mut arms = Vec::new();
        self.skip_newlines();
        while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.match_token(&TokenKind::RBrace) {
                break;
            }

            // Optional leading `case` or `|` before each arm.
            let _ = self.consume_if(&TokenKind::Case);
            if self.consume_if(&TokenKind::Pipe) {
                // OK
            }
            self.skip_newlines();

            let pat = self.parse_pattern()?;
            // Optional guard: `| pat if cond => body`. The guard is a full
            // expression; it may reference variables bound by the pattern.
            let guard = if self.consume_if(&TokenKind::If) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            self.expect(TokenKind::FatArrow)?;
            let expr = self.parse_expr()?;
            arms.push((pat, guard, expr));
            self.skip_newlines_semicolons();
            self.consume_if(&TokenKind::Comma);
        }
        self.expect(TokenKind::RBrace)?;

        Ok(Expr::Match {
            scrutinee: Box::new(scrutinee),
            arms,
            span,
        })
    }

    fn parse_block(&mut self) -> NuResult<Expr> {
        let _span = self.current_span();
        self.advance(); // consume '{'
        let exprs = self.collect_block_exprs(Some(TokenKind::RBrace))?;
        self.expect(TokenKind::RBrace)?;
        Ok(Expr::Block {
            exprs,
            span: self.current_span(),
        })
    }

    /// Collect expressions until `end_token` (or EOF), splicing incomplete
    /// let-bindings so that `let x = 1` captures following expressions as
    /// its body. Called recursively so nested statement-lets work correctly.
    fn collect_block_exprs(&mut self, end_token: Option<TokenKind>) -> NuResult<Vec<Expr>> {
        let mut exprs = Vec::new();
        self.skip_newlines();
        while !self.is_at_end() {
            if let Some(ref end) = end_token {
                if self.match_token(end) {
                    break;
                }
            }
            self.skip_newlines();
            if self.is_at_end() { break; }
            if let Some(ref end) = end_token {
                if self.match_token(end) { break; }
            }
            let mut expr = self.parse_expr()?;
            self.skip_newlines_semicolons();
            
            let is_incomplete = matches!(&expr, Expr::Let { body, .. } | Expr::LetRec { body, .. } if matches!(body.as_ref(), Expr::Block { exprs, span } if exprs.is_empty() && span.start == 0 && span.end == 0));
            
            if is_incomplete {
                let rest = self.collect_block_exprs(end_token.clone())?;
                let new_body = if rest.is_empty() {
                    Expr::Literal(Literal::Unit, Span::default())
                } else if rest.len() == 1 {
                    rest.into_iter().next().unwrap()
                } else {
                    Expr::Block { exprs: rest, span: Span::default() }
                };
                expr = match expr {
                    Expr::Let { name, ty, value, span, .. } => Expr::Let { name, ty, value, body: Box::new(new_body), span },
                    Expr::LetRec { name, params, value, span, .. } => Expr::LetRec { name, params, value, body: Box::new(new_body), span },
                    _ => unreachable!(),
                };
                exprs.push(expr);
                break;
            }
            exprs.push(expr);
        }
        Ok(exprs)
    }

    fn parse_tuple_or_paren(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.advance(); // consume '('
        self.skip_newlines();

        // Empty paren = unit
        if self.consume_if(&TokenKind::RParen) {
            return Ok(Expr::Literal(Literal::Unit, span));
        }

        let first = self.parse_expr()?;
        self.skip_newlines();

        // Single paren = grouped expr
        if self.consume_if(&TokenKind::RParen) {
            return Ok(first);
        }

        // Tuple: (e1, e2, ...)
        let mut elems = vec![first];
        while self.consume_if(&TokenKind::Comma) {
            self.skip_newlines();
            if self.match_token(&TokenKind::RParen) {
                break;
            }
            elems.push(self.parse_expr()?);
            self.skip_newlines();
        }
        self.expect(TokenKind::RParen)?;
        Ok(Expr::Tuple(elems, span))
    }

    fn parse_spawn(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.advance(); // consume 'spawn'
        // Optional `link`/`monitor` modifier (BEAM spawn_link/spawn_monitor).
        // `spawn link A { ... }` desugars right here to
        // `let __spawn_ref = spawn A { ... } in { perform Actor.link(__spawn_ref); __spawn_ref }`
        // (likewise `Actor.monitor`), so the form typechecks exactly like a
        // plain spawn (actor ref) and needs no new IR nodes or opcodes.
        let link_op = match self.peek_kind() {
            TokenKind::Link => {
                self.advance();
                Some("link")
            }
            TokenKind::Monitor => {
                self.advance();
                Some("monitor")
            }
            _ => None,
        };
        // Parse the actor name.  In a spawn expression the target is always a
        // simple name (like `Counter` or `DurableCounter`), never an arbitrary
        // expression.  We parse it as an identifier so `spawn Foo(args)` does
        // not get misinterpreted as a function call.
        let actor_name = match self.peek_kind().clone() {
            TokenKind::Ident(s) | TokenKind::UpperIdent(s) => {
                self.advance();
                s
            }
            _ => return Err(NuError::ParseError {
                msg: format!("Expected actor name in spawn, got {}", self.peek_kind()),
                span: self.current_span(),
            }),
        };
        let actor_type = Expr::Var(actor_name, span);

        // Optional positional constructor args: `spawn Foo(a, b)`
        let positional_args = if self.peek_kind() == &TokenKind::LParen {
            self.advance(); // consume '('
            let mut args = Vec::new();
            self.skip_newlines();
            while !self.match_token(&TokenKind::RParen) && !self.is_at_end() {
                args.push(self.parse_expr()?);
                self.skip_newlines();
                if !self.consume_if(&TokenKind::Comma) {
                    break;
                }
                self.skip_newlines();
            }
            self.expect(TokenKind::RParen)?;
            Some(args)
        } else {
            None
        };

        // Field init block `{ field = val, ... }` — required if no positional args.
        let init = if positional_args.is_none() {
            self.expect(TokenKind::LBrace)?;
            let mut fields = Vec::new();
            self.skip_newlines();
            while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
                self.skip_newlines();
                if self.match_token(&TokenKind::RBrace) {
                    break;
                }
                let field = self.expect_ident("field name")?;
                if !self.consume_if(&TokenKind::Assign) {
                    self.expect(TokenKind::Colon)?;
                }
                let val = self.parse_expr()?;
                fields.push((field, val));
                self.skip_newlines_semicolons();
            }
            self.expect(TokenKind::RBrace)?;
            fields
        } else {
            Vec::new()
        };

        // Optional named registration: `spawn Foo() as "name"`
        let register_as = if self.consume_if(&TokenKind::As) {
            Some(self.expect_string("actor name")?)
        } else {
            None
        };
        let spawned = Expr::Spawn {
            actor_type: Box::new(actor_type),
            init,
            positional_args,
            register_as,
            span,
        };
        Ok(match link_op {
            None => spawned,
            Some(op) => {
                let t = "__spawn_ref".to_string();
                Expr::Let {
                    name: t.clone(),
                    ty: None,
                    value: Box::new(spawned),
                    body: Box::new(Expr::Block {
                        exprs: vec![
                            Expr::Perform {
                                effect: "Actor".to_string(),
                                op: op.to_string(),
                                args: vec![Expr::Var(t.clone(), span)],
                                span,
                            },
                            Expr::Var(t, span),
                        ],
                        span,
                    }),
                    span,
                }
            }
        })
    }

    fn parse_send_keyword(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.advance(); // consume 'send'
        let remote = self.consume_if(&TokenKind::Remote);
        let actor = self.parse_expr()?;
        let behavior = self.expect_ident("behavior name")?;
        self.expect(TokenKind::LParen)?;
        let args = self.parse_arg_list()?;
        Ok(Expr::Send {
            actor: Box::new(actor),
            behavior,
            args,
            remote,
            span,
        })
    }

    fn parse_ask(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.advance(); // consume 'ask'
        let actor = self.parse_expr()?;
        // Allow the behavior name to be `ask` itself so agent actors can expose
        // an `ask(prompt)` behavior callable as `ask a ask("...")`.
        let behavior = match self.peek_kind() {
            TokenKind::Ask => {
                self.advance();
                "ask".to_string()
            }
            _ => self.expect_ident("behavior name")?,
        };
        self.expect(TokenKind::LParen)?;
        let args = self.parse_arg_list()?;
        Ok(Expr::Ask {
            actor: Box::new(actor),
            behavior,
            args,
            span,
        })
    }

    fn parse_perform(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.advance(); // consume 'perform'
        let effect = self.expect_ident("effect name")?;
        self.expect(TokenKind::Dot)?;
        // `ask`, `link`, `monitor` and `exit` are reserved keywords, so they
        // lex as keyword tokens rather than identifiers; accept them as
        // operation names (`perform Actor.link(t)`, `perform LLM.ask(p)`).
        let op = match self.peek_kind() {
            TokenKind::Ask => {
                self.advance();
                "ask".to_string()
            }
            TokenKind::Link => {
                self.advance();
                "link".to_string()
            }
            TokenKind::Monitor => {
                self.advance();
                "monitor".to_string()
            }
            TokenKind::Exit => {
                self.advance();
                "exit".to_string()
            }
            _ => self.expect_ident("operation name")?,
        };
        self.expect(TokenKind::LParen)?;
        let args = self.parse_arg_list()?;
        Ok(Expr::Perform {
            effect,
            op,
            args,
            span,
        })
    }

    fn parse_emit(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.advance(); // consume 'emit'
        let event = self.expect_ident("event name")?;
        self.expect(TokenKind::LParen)?;
        let args = self.parse_arg_list()?;
        Ok(Expr::Emit { event, args, span })
    }

    // === Helper Methods ===

    fn is_at_end(&self) -> bool {
        self.peek_kind() == &TokenKind::Eof
    }

    fn peek_kind(&self) -> &TokenKind {
        if self.pos < self.tokens.len() {
            &self.tokens[self.pos].kind
        } else {
            &TokenKind::Eof
        }
    }

    fn advance(&mut self) -> Token {
        if self.pos < self.tokens.len() {
            let tok = self.tokens[self.pos].clone();
            self.pos += 1;
            tok
        } else {
            Token {
                kind: TokenKind::Eof,
                span: self.current_span(),
            }
        }
    }

    fn consume_if(&mut self, kind: &TokenKind) -> bool {
        if self.peek_kind() == kind {
            self.advance();
            true
        } else {
            false
        }
    }

    fn match_token(&self, kind: &TokenKind) -> bool {
        self.peek_kind() == kind
    }

    fn expect(&mut self, kind: TokenKind) -> NuResult<Token> {
        let current_kind = self.peek_kind();
        if current_kind == &kind {
            Ok(self.advance())
        } else {
            Err(NuError::ParseError {
                msg: format!("Expected {}", kind),
                span: self.current_span(),
            })
        }
    }

    fn expect_ident(&mut self, msg: &str) -> NuResult<String> {
        let current_kind = self.peek_kind();
        match current_kind {
            TokenKind::Ident(s) | TokenKind::UpperIdent(s) => {
                let s = s.clone();
                self.advance();
                Ok(s)
            }
            _ => Err(NuError::ParseError {
                msg: format!("Expected {}, found {}", msg, current_kind),
                span: self.current_span(),
            }),
        }
    }

    fn expect_string(&mut self, msg: &str) -> NuResult<String> {
        let current_kind = self.peek_kind();
        match current_kind {
            TokenKind::StringLit(s) => {
                let s = s.clone();
                self.advance();
                Ok(s)
            }
            _ => Err(NuError::ParseError {
                msg: format!("Expected {}, found {}", msg, current_kind),
                span: self.current_span(),
            }),
        }
    }

    fn expect_int(&mut self, msg: &str) -> NuResult<i64> {
        let current_kind = self.peek_kind();
        match current_kind {
            TokenKind::IntLit(n) => {
                let n = *n;
                self.advance();
                Ok(n)
            }
            _ => Err(NuError::ParseError {
                msg: format!("Expected integer {}, found {}", msg, current_kind),
                span: self.current_span(),
            }),
        }
    }

    fn expect_float(&mut self, msg: &str) -> NuResult<f64> {
        let current_kind = self.peek_kind();
        match current_kind {
            TokenKind::FloatLit(f) => {
                let f = *f;
                self.advance();
                Ok(f)
            }
            _ => Err(NuError::ParseError {
                msg: format!("Expected float {}, found {}", msg, current_kind),
                span: self.current_span(),
            }),
        }
    }

    fn current_span(&self) -> Span {
        if self.pos < self.tokens.len() {
            self.tokens[self.pos].span
        } else if !self.tokens.is_empty() {
            self.tokens[self.tokens.len() - 1].span
        } else {
            Span::default()
        }
    }

    fn skip_newlines(&mut self) {
        while matches!(
            self.peek_kind(),
            &TokenKind::Newline | &TokenKind::DocComment(_)
        ) {
            self.advance();
        }
    }

    fn skip_newlines_semicolons(&mut self) {
        while matches!(
            self.peek_kind(),
            &TokenKind::Newline | &TokenKind::Semicolon | &TokenKind::DocComment(_)
        ) {
            self.advance();
        }
    }

    fn is_expr_start(&self) -> bool {
        matches!(
            self.peek_kind(),
            TokenKind::IntLit(_)
                | TokenKind::FloatLit(_)
                | TokenKind::StringLit(_)
                | TokenKind::BoolLit(_)
                | TokenKind::NilLit
                | TokenKind::UnitLit
                | TokenKind::Ident(_)
                | TokenKind::UpperIdent(_)
                | TokenKind::LParen
                | TokenKind::LBracket
                | TokenKind::LBrace
                | TokenKind::Fn
                | TokenKind::Let
                | TokenKind::If
                | TokenKind::Match
                | TokenKind::Spawn
                | TokenKind::Send
                | TokenKind::Ask
                | TokenKind::Perform
                | TokenKind::Emit
                | TokenKind::Handle
                | TokenKind::For
                | TokenKind::While
                | TokenKind::Migrate
                | TokenKind::Return
                | TokenKind::Break
                | TokenKind::SelfKw
                | TokenKind::Minus
                | TokenKind::Not
                | TokenKind::Bang
                | TokenKind::Ampersand
                | TokenKind::Star
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Unit
        )
    }

    fn is_record_literal_ahead(&self) -> bool {
        if self.peek_kind() == &TokenKind::LBrace {
            if self.pos + 2 < self.tokens.len() {
                let next1 = &self.tokens[self.pos + 1].kind;
                let next2 = &self.tokens[self.pos + 2].kind;
                return matches!(next1, TokenKind::Ident(_) | TokenKind::UpperIdent(_))
                    && matches!(next2, TokenKind::Colon);
            }
        }
        false
    }

    fn parse_array(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.expect(TokenKind::LBracket)?;
        let mut exprs = Vec::new();
        self.skip_newlines();
        while self.peek_kind() != &TokenKind::RBracket && !self.is_at_end() {
            exprs.push(self.parse_expr()?);
            self.skip_newlines();
            if !self.consume_if(&TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }
        self.expect(TokenKind::RBracket)?;
        Ok(Expr::Array(exprs, span))
    }

    fn parse_record_literal(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.expect(TokenKind::LBrace)?;
        let mut fields = Vec::new();
        self.skip_newlines();
        while self.peek_kind() != &TokenKind::RBrace && !self.is_at_end() {
            let field = self.expect_ident("field name")?;
            self.expect(TokenKind::Colon)?;
            let val = self.parse_expr()?;
            fields.push((field, val));
            self.skip_newlines();
            if !self.consume_if(&TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }
        self.expect(TokenKind::RBrace)?;
        Ok(Expr::Record(fields, span))
    }

    fn parse_self_ref(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.expect(TokenKind::SelfKw)?;
        Ok(Expr::SelfRef(span))
    }

    fn parse_handle(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.expect(TokenKind::Handle)?;
        let body = self.parse_expr()?;
        self.consume_if(&TokenKind::With);
        self.expect(TokenKind::LBrace)?;
        let mut handlers = Vec::new();
        self.skip_newlines();
        while self.peek_kind() != &TokenKind::RBrace && !self.is_at_end() {
            self.consume_if(&TokenKind::Pipe);
            let effect_name = self.expect_ident("effect name")?;
            self.expect(TokenKind::Dot)?;
            let op_name = self.expect_ident("operation name")?;
            self.expect(TokenKind::LParen)?;
            let mut params = Vec::new();
            self.skip_newlines();
            while self.peek_kind() != &TokenKind::RParen && !self.is_at_end() {
                params.push(self.expect_ident("param name")?);
                self.skip_newlines();
                if !self.consume_if(&TokenKind::Comma) {
                    break;
                }
                self.skip_newlines();
            }
            self.expect(TokenKind::RParen)?;
            let has_resume = self.consume_if(&TokenKind::Resume);
            self.expect(TokenKind::FatArrow)?;
            let handler_body = self.parse_expr()?;
            handlers.push(EffectHandler {
                effect_name,
                op_name,
                params,
                body: handler_body,
                resume: has_resume,
            });
            self.skip_newlines_semicolons();
        }
        self.expect(TokenKind::RBrace)?;
        Ok(Expr::Handle {
            body: Box::new(body),
            handlers,
            span,
        })
    }

    fn parse_receive(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.expect(TokenKind::Receive)?;
        self.expect(TokenKind::LBrace)?;
        let mut arms = Vec::new();
        self.skip_newlines();
        while self.peek_kind() != &TokenKind::RBrace && !self.is_at_end() {
            self.consume_if(&TokenKind::Pipe);
                        let behavior_name = self.expect_ident("behavior name")?;
            self.expect(TokenKind::LParen)?;
            let mut params = Vec::new();
            self.skip_newlines();
            while self.peek_kind() != &TokenKind::RParen && !self.is_at_end() {
                params.push(self.expect_ident("param name")?);
                self.skip_newlines();
                if !self.consume_if(&TokenKind::Comma) {
                    break;
                }
                self.skip_newlines();
            }
            self.expect(TokenKind::RParen)?;
            self.expect(TokenKind::FatArrow)?;
            let body = self.parse_expr()?;
            arms.push((behavior_name, params, body));
            self.skip_newlines_semicolons();
        }
        self.expect(TokenKind::RBrace)?;
        // Optional timeout clause: `receive { ... } after ms_expr => body`.
        // `after` is a contextual keyword — an ordinary identifier expected
        // only in this position (same pattern as `to` in parse_migrate), so
        // user code may still name bindings and workflow steps `after`.
        // Without the clause the receive keeps its non-blocking fallthrough;
        // with it, a no-match suspends up to `ms_expr` milliseconds before
        // running `body` (see mir::RValue::ReceiveWait / OpCode::ReceiveWait).
        let after = if matches!(self.peek_kind(), TokenKind::Ident(s) if s == "after") {
            self.advance(); // consume 'after'
            let timeout_ms = self.parse_expr()?;
            self.expect(TokenKind::FatArrow)?;
            let timeout_body = self.parse_expr()?;
            Some((Box::new(timeout_ms), Box::new(timeout_body)))
        } else {
            None
        };
        Ok(Expr::Receive { arms, after, span })
    }

    fn parse_for(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.expect(TokenKind::For)?;
        let var = self.expect_ident("loop variable")?;
        self.expect(TokenKind::In)?;
        let iterable = self.parse_expr()?;
        let body = self.parse_expr()?;
        Ok(Expr::For {
            var,
            iterable: Box::new(iterable),
            body: Box::new(body),
            span,
        })
    }

    fn parse_while(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.expect(TokenKind::While)?;
        let cond = self.parse_expr()?;
        let body = self.parse_expr()?;
        Ok(Expr::While { cond: Box::new(cond), body: Box::new(body), span })
    }

    fn parse_migrate(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.expect(TokenKind::Migrate)?;
        let actor = self.parse_expr()?;
        let to_ident = self.expect_ident("to")?;
        if to_ident != "to" {
            return Err(NuError::ParseError {
                msg: format!("Expected 'to', found '{}'", to_ident),
                span: self.current_span(),
            });
        }
        let node = self.parse_expr()?;
        Ok(Expr::Migrate {
            actor: Box::new(actor),
            node: Box::new(node),
            span,
        })
    }

    fn parse_type(&mut self) -> NuResult<Type> {
        self.parse_type_arrow()
    }

    fn parse_type_arrow(&mut self) -> NuResult<Type> {
        let left = self.parse_type_atomic()?;
        if self.consume_if(&TokenKind::Arrow) {
            let right = self.parse_type_arrow()?;
            let effect = if self.consume_if(&TokenKind::Bang) {
                self.parse_effect_row()?
            } else {
                EffectRow::empty()
            };
            let cap = if self.consume_if(&TokenKind::Colon) {
                self.parse_capability()?
            } else {
                Capability::Ref
            };
            Ok(Type::Function {
                param: Box::new(left),
                ret: Box::new(right),
                effect,
                cap,
            })
        } else {
            Ok(left)
        }
    }

    fn parse_type_atomic(&mut self) -> NuResult<Type> {
        let current_kind = self.peek_kind();
        match current_kind {
            TokenKind::Ident(s) | TokenKind::UpperIdent(s) => {
                let name = s.clone();
                let name_span = self.current_span();
                self.advance();

                // Optional type arguments (`Option[Int]`). Parsed up front so
                // a declared generic type can have them substituted into its
                // expansion below.
                let args = if self.peek_kind() == &TokenKind::LBracket {
                    self.advance(); // consume '['
                    let mut args = Vec::new();
                    self.skip_newlines();
                    while self.peek_kind() != &TokenKind::RBracket && !self.is_at_end() {
                        args.push(self.parse_type()?);
                        self.skip_newlines();
                        if !self.consume_if(&TokenKind::Comma) {
                            break;
                        }
                        self.skip_newlines();
                    }
                    self.expect(TokenKind::RBracket)?;
                    args
                } else {
                    Vec::new()
                };

                let ty = match name.as_str() {
                    "Int" => Type::Primitive(PrimitiveType::Int),
                    "Float" => Type::Primitive(PrimitiveType::Float),
                    "Bool" => Type::Primitive(PrimitiveType::Bool),
                    "String" => Type::Primitive(PrimitiveType::String),
                    "Nil" => Type::Primitive(PrimitiveType::Nil),
                    "Unit" => Type::Primitive(PrimitiveType::Unit),
                    "Never" => Type::Primitive(PrimitiveType::Never),
                    "Address" => Type::Primitive(PrimitiveType::Address),
                    _ => {
                        if let Some(&tv) = self.local_type_params.get(&name) {
                            Type::Var(tv)
                        } else {
                            // Declared type names (`type` / `type alias`)
                            // expand to their declaration; truly unknown names
                            // are a hard error instead of a silently
                            // unconstrained fresh variable (SPEC2 §3.4.1).
                            return self.resolve_named_type(&name, args, name_span);
                        }
                    }
                };

                if args.is_empty() {
                    Ok(ty)
                } else {
                    Ok(Type::App {
                        constructor: Box::new(ty),
                        args,
                    })
                }
            }
            TokenKind::LParen => {
                self.advance(); // consume '('
                let mut types = Vec::new();
                self.skip_newlines();
                while self.peek_kind() != &TokenKind::RParen && !self.is_at_end() {
                    types.push(self.parse_type()?);
                    self.skip_newlines();
                    if !self.consume_if(&TokenKind::Comma) {
                        break;
                    }
                    self.skip_newlines();
                }
                self.expect(TokenKind::RParen)?;
                if types.len() == 1 {
                    Ok(types[0].clone())
                } else {
                    Ok(Type::Tuple(types))
                }
            }
            TokenKind::LBrace => {
                self.advance(); // consume '{'
                let mut fields = Vec::new();
                self.skip_newlines();
                while self.peek_kind() != &TokenKind::RBrace && !self.is_at_end() {
                    let fname = self.expect_ident("field name")?;
                    self.expect(TokenKind::Colon)?;
                    let fty = self.parse_type()?;
                    fields.push((fname, fty));
                    self.skip_newlines();
                    if !self.consume_if(&TokenKind::Comma) {
                        break;
                    }
                    self.skip_newlines();
                }
                self.expect(TokenKind::RBrace)?;
                Ok(Type::Record(fields))
            }
            TokenKind::Ampersand => {
                self.advance(); // consume '&'
                let cap = self.parse_capability()?;
                let inner = self.parse_type_atomic()?;
                Ok(Type::Reference {
                    cap,
                    inner: Box::new(inner),
                })
            }
            _ => Err(NuError::ParseError {
                msg: format!("Expected type, found {}", current_kind),
                span: self.current_span(),
            }),
        }
    }

    /// Resolve a non-primitive type name against the module's `type` and
    /// `type alias` declarations. The declaration is re-parsed from the token
    /// stream — the parser is a cursor over a fully lexed token vec, so
    /// forward references work — with the use-site arguments substituted for
    /// the declared type parameters. Unknown names are a hard parse error
    /// instead of a silently unconstrained fresh type variable.
    fn resolve_named_type(&mut self, name: &str, args: Vec<Type>, span: Span) -> NuResult<Type> {
        let decl_pos = match self.find_type_decl(name) {
            Some(pos) => pos,
            None => {
                return Err(NuError::ParseError {
                    msg: format!("Unknown type name: '{}'", name),
                    span,
                });
            }
        };

        let saved_pos = self.pos;
        let saved_locals = self.local_type_params.clone();
        // Guard against (mutually) recursive references: while the body is
        // being expanded, the type's own name resolves to a stable abstract
        // variable instead of expanding again (e.g. `Tree[T]` inside the
        // body of `type Tree[T] = ...`).
        let self_tv = *self
            .global_type_constructors
            .entry(name.to_string())
            .or_insert_with(TypeVar::fresh);
        self.local_type_params.insert(name.to_string(), self_tv);

        // Position the cursor at the declaration and re-parse it.
        self.pos = decl_pos + 1; // skip the 'type' keyword
        self.skip_newlines();
        let decl_result = if self.peek_kind() == &TokenKind::Alias {
            self.parse_type_alias(false)
        } else {
            self.parse_type_decl_variant_or_record(false)
        };
        let result = decl_result.and_then(|decl| {
            let (type_params, body) = match decl {
                Decl::TypeAlias {
                    type_params, body, ..
                } => (type_params, body),
                Decl::RecordType {
                    type_params,
                    fields,
                    ..
                } => (type_params, Type::Record(fields)),
                Decl::VariantType {
                    type_params,
                    variants,
                    ..
                } => (type_params, Type::Variant(variants)),
                _ => unreachable!("find_type_decl only matches type declarations"),
            };
            if !args.is_empty() && args.len() != type_params.len() {
                return Err(NuError::ParseError {
                    msg: format!(
                        "Type '{}' expects {} type argument(s), got {}",
                        name,
                        type_params.len(),
                        args.len()
                    ),
                    span,
                });
            }
            // Snapshot the declared parameters' variables before the local
            // map is restored, then splice the use-site arguments in.
            let param_vars: Vec<Option<TypeVar>> = type_params
                .iter()
                .map(|p| self.local_type_params.get(p).copied())
                .collect();
            let mut body = body;
            for (tv, arg) in param_vars.iter().zip(args.iter()) {
                if let Some(tv) = tv {
                    body = Self::subst_type_var(&body, *tv, arg);
                }
            }
            Ok(body)
        });
        self.pos = saved_pos;
        self.local_type_params = saved_locals;
        result
    }

    /// Find the token index of the `type` keyword of the declaration named
    /// `name` (`type Name = ...` or `type alias Name = ...`), if any.
    fn find_type_decl(&self, name: &str) -> Option<usize> {
        for i in 0..self.tokens.len() {
            if self.tokens[i].kind != TokenKind::Type {
                continue;
            }
            let mut j = i + 1;
            while matches!(
                self.tokens.get(j).map(|t| &t.kind),
                Some(TokenKind::Newline) | Some(TokenKind::DocComment(_))
            ) {
                j += 1;
            }
            // Optional 'alias' keyword between 'type' and the name.
            if matches!(
                self.tokens.get(j).map(|t| &t.kind),
                Some(TokenKind::Alias)
            ) {
                j += 1;
            }
            match self.tokens.get(j).map(|t| &t.kind) {
                Some(TokenKind::Ident(n)) | Some(TokenKind::UpperIdent(n)) if n == name => {
                    return Some(i);
                }
                _ => {}
            }
        }
        None
    }

    /// Substitute a single type variable with a concrete type throughout
    /// `ty`. Used to splice use-site arguments into an expanded declared
    /// type; mirrors the type checker's `apply_subst` for one mapping.
    fn subst_type_var(ty: &Type, var: TypeVar, arg: &Type) -> Type {
        match ty {
            Type::Var(v) => {
                if *v == var {
                    arg.clone()
                } else {
                    ty.clone()
                }
            }
            Type::Primitive(_) => ty.clone(),
            Type::Tuple(ts) => Type::Tuple(
                ts.iter()
                    .map(|t| Self::subst_type_var(t, var, arg))
                    .collect(),
            ),
            Type::Record(fs) => Type::Record(
                fs.iter()
                    .map(|(n, t)| (n.clone(), Self::subst_type_var(t, var, arg)))
                    .collect(),
            ),
            Type::Variant(vs) => Type::Variant(
                vs.iter()
                    .map(|(n, t)| {
                        (
                            n.clone(),
                            t.as_ref().map(|t| Self::subst_type_var(t, var, arg)),
                        )
                    })
                    .collect(),
            ),
            Type::Array(t) => Type::Array(Box::new(Self::subst_type_var(t, var, arg))),
            Type::Function {
                param,
                ret,
                effect,
                cap,
            } => Type::Function {
                param: Box::new(Self::subst_type_var(param, var, arg)),
                ret: Box::new(Self::subst_type_var(ret, var, arg)),
                effect: effect.clone(),
                cap: *cap,
            },
            Type::Actor { state, behavior } => Type::Actor {
                state: Box::new(Self::subst_type_var(state, var, arg)),
                behavior: Box::new(Self::subst_type_var(behavior, var, arg)),
            },
            Type::App { constructor, args } => Type::App {
                constructor: Box::new(Self::subst_type_var(constructor, var, arg)),
                args: args
                    .iter()
                    .map(|a| Self::subst_type_var(a, var, arg))
                    .collect(),
            },
            Type::Reference { cap, inner } => Type::Reference {
                cap: *cap,
                inner: Box::new(Self::subst_type_var(inner, var, arg)),
            },
            Type::Scheme { vars, body } => {
                if vars.contains(&var) {
                    ty.clone()
                } else {
                    Type::Scheme {
                        vars: vars.clone(),
                        body: Box::new(Self::subst_type_var(body, var, arg)),
                    }
                }
            }
        }
    }

    fn parse_capability(&mut self) -> NuResult<Capability> {
        let current_kind = self.peek_kind();
        match current_kind {
            TokenKind::Iso => {
                self.advance();
                Ok(Capability::Iso)
            }
            TokenKind::Trn => {
                self.advance();
                Ok(Capability::Trn)
            }
            TokenKind::Ref => {
                self.advance();
                Ok(Capability::Ref)
            }
            TokenKind::Val => {
                self.advance();
                Ok(Capability::Val)
            }
            TokenKind::Box => {
                self.advance();
                Ok(Capability::Box)
            }
            TokenKind::Tag => {
                self.advance();
                Ok(Capability::Tag)
            }
            TokenKind::Ident(s) if s == "lineariso" => {
                self.advance();
                Ok(Capability::LinearIso)
            }
            TokenKind::Ident(s) if s == "linear" => {
                self.advance();
                Ok(Capability::Linear)
            }
            _ => Err(NuError::ParseError {
                msg: format!(
                    "Expected capability (iso, trn, ref, val, box, tag, lineariso, linear), found {}",
                    current_kind
                ),
                span: self.current_span(),
            }),
        }
    }

    fn parse_effect_row(&mut self) -> NuResult<EffectRow> {
        let mut effects = Vec::new();
        if self.consume_if(&TokenKind::LBrace) {
            self.skip_newlines();
            let mut is_open = false;
            let mut region = Region(0);
            while self.peek_kind() != &TokenKind::RBrace && !self.is_at_end() {
                if self.consume_if(&TokenKind::Pipe) {
                    let _rname = self.expect_ident("row variable")?;
                    region = Region::fresh();
                    is_open = true;
                    break;
                }
                let name = self.expect_ident("effect name")?;
                effects.push(self.string_to_effect(&name));
                self.skip_newlines();
                if !self.consume_if(&TokenKind::Comma) {
                    break;
                }
                self.skip_newlines();
            }
            self.expect(TokenKind::RBrace)?;
            if is_open {
                Ok(EffectRow::Open(effects, region))
            } else {
                Ok(EffectRow::Closed(effects))
            }
        } else {
            let name = self.expect_ident("effect name")?;
            Ok(EffectRow::Closed(vec![self.string_to_effect(&name)]))
        }
    }

    fn string_to_effect(&self, name: &str) -> Effect {
        // Single name table shared with the effect checker so annotation
        // parsing and `perform` resolution can never disagree on the
        // built-in effect names (SPEC2 §4.6).
        crate::effect_checker::parse_effect_name(name)
    }

    fn parse_pattern(&mut self) -> NuResult<Pattern> {
        let pat = self.parse_pattern_atomic()?;
        if self.consume_if(&TokenKind::At) {
            if let Pattern::Var(name) = pat {
                let sub = self.parse_pattern()?;
                Ok(Pattern::Alias(name, Box::new(sub)))
            } else {
                Err(NuError::ParseError {
                    msg: "Left side of '@' alias must be a variable".to_string(),
                    span: self.current_span(),
                })
            }
        } else {
            Ok(pat)
        }
    }

    fn parse_pattern_atomic(&mut self) -> NuResult<Pattern> {
        let current_kind = self.peek_kind();
        match current_kind {
            TokenKind::Ident(s) if s == "_" => {
                self.advance();
                Ok(Pattern::Wild)
            }
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(Pattern::Var(name))
            }
            TokenKind::UpperIdent(name) => {
                let name = name.clone();
                self.advance();
                if self.consume_if(&TokenKind::LParen) {
                    let sub = self.parse_pattern()?;
                    self.expect(TokenKind::RParen)?;
                    Ok(Pattern::Variant(name, Some(Box::new(sub))))
                } else {
                    Ok(Pattern::Variant(name, None))
                }
            }
            TokenKind::LParen => {
                self.advance();
                let mut pats = Vec::new();
                self.skip_newlines();
                while self.peek_kind() != &TokenKind::RParen && !self.is_at_end() {
                    pats.push(self.parse_pattern()?);
                    self.skip_newlines();
                    if !self.consume_if(&TokenKind::Comma) {
                        break;
                    }
                    self.skip_newlines();
                }
                self.expect(TokenKind::RParen)?;
                if pats.len() == 1 {
                    Ok(pats[0].clone())
                } else {
                    Ok(Pattern::Tuple(pats))
                }
            }
            TokenKind::LBrace => {
                self.advance();
                let mut fields = Vec::new();
                self.skip_newlines();
                while self.peek_kind() != &TokenKind::RBrace && !self.is_at_end() {
                    let fname = self.expect_ident("field name")?;
                    self.expect(TokenKind::Colon)?;
                    let fpat = self.parse_pattern()?;
                    fields.push((fname, fpat));
                    self.skip_newlines();
                    if !self.consume_if(&TokenKind::Comma) {
                        break;
                    }
                    self.skip_newlines();
                }
                self.expect(TokenKind::RBrace)?;
                Ok(Pattern::Record(fields))
            }
            TokenKind::IntLit(v) => {
                let v = *v;
                self.advance();
                Ok(Pattern::Lit(Literal::Int(v)))
            }
            TokenKind::FloatLit(v) => {
                let v = *v;
                self.advance();
                Ok(Pattern::Lit(Literal::Float(v)))
            }
            TokenKind::StringLit(s) => {
                let s = s.clone();
                self.advance();
                Ok(Pattern::Lit(Literal::String(s)))
            }
            TokenKind::BoolLit(b) => {
                let b = *b;
                self.advance();
                Ok(Pattern::Lit(Literal::Bool(b)))
            }
            TokenKind::NilLit => {
                self.advance();
                Ok(Pattern::Lit(Literal::Nil))
            }
            TokenKind::UnitLit => {
                self.advance();
                Ok(Pattern::Lit(Literal::Unit))
            }
            TokenKind::True => {
                self.advance();
                Ok(Pattern::Lit(Literal::Bool(true)))
            }
            TokenKind::False => {
                self.advance();
                Ok(Pattern::Lit(Literal::Bool(false)))
            }
            TokenKind::Unit => {
                self.advance();
                Ok(Pattern::Lit(Literal::Unit))
            }
            _ => Err(NuError::ParseError {
                msg: format!("Expected pattern, found {}", current_kind),
                span: self.current_span(),
            }),
        }
    }

    fn parse_variants(&mut self) -> NuResult<Vec<(String, Option<Type>)>> {
        let mut variants = Vec::new();
        self.skip_newlines();
        self.consume_if(&TokenKind::Pipe);
        self.skip_newlines();
        while !self.is_at_end() {
            let name = self.expect_ident("variant name")?;
            let ty = if self.consume_if(&TokenKind::LParen) {
                let t = self.parse_type()?;
                self.expect(TokenKind::RParen)?;
                Some(t)
            } else {
                None
            };
            variants.push((name, ty));
            self.skip_newlines();
            if !self.consume_if(&TokenKind::Pipe) {
                break;
            }
            self.skip_newlines();
        }
        Ok(variants)
    }

    fn parse_record_type_fields(&mut self) -> NuResult<Vec<(String, Type)>> {
        let mut fields = Vec::new();
        self.skip_newlines();
        while self.peek_kind() != &TokenKind::RBrace && !self.is_at_end() {
            let fname = self.expect_ident("field name")?;
            self.expect(TokenKind::Colon)?;
            let fty = self.parse_type()?;
            fields.push((fname, fty));
            self.skip_newlines();
            if !self.consume_if(&TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }
        self.expect(TokenKind::RBrace)?;
        Ok(fields)
    }

    fn parse_arg_list(&mut self) -> NuResult<Vec<Expr>> {
        let mut args = Vec::new();
        self.skip_newlines();
        while self.peek_kind() != &TokenKind::RParen && !self.is_at_end() {
            args.push(self.parse_expr()?);
            self.skip_newlines();
            if !self.consume_if(&TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }
        self.expect(TokenKind::RParen)?;
        Ok(args)
    }

    fn parse_type_params(&mut self) -> NuResult<Vec<String>> {
        let mut params = Vec::new();
        if self.consume_if(&TokenKind::LBracket) {
            self.skip_newlines();
            while self.peek_kind() != &TokenKind::RBracket && !self.is_at_end() {
                let name = self.expect_ident("type parameter name")?;
                params.push(name.clone());
                let tv = TypeVar::fresh();
                self.local_type_params.insert(name, tv);
                self.skip_newlines();
                if !self.consume_if(&TokenKind::Comma) {
                    break;
                }
                self.skip_newlines();
            }
            self.expect(TokenKind::RBracket)?;
        }
        Ok(params)
    }

    fn parse_params(&mut self) -> NuResult<Vec<(String, Option<Type>)>> {
        let mut params = Vec::new();
        self.skip_newlines();
        while self.peek_kind() != &TokenKind::RParen && !self.is_at_end() {
            let name = self.expect_ident("parameter name")?;
            let ty = if self.consume_if(&TokenKind::Colon) {
                Some(self.parse_type()?)
            } else {
                None
            };
            params.push((name, ty));
            self.skip_newlines();
            if !self.consume_if(&TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }
        Ok(params)
    }
}

fn token_to_binop(kind: &TokenKind) -> Option<BinOp> {
    match kind {
        TokenKind::Plus => Some(BinOp::Add),
        TokenKind::Minus => Some(BinOp::Sub),
        TokenKind::Star => Some(BinOp::Mul),
        TokenKind::Slash => Some(BinOp::Div),
        TokenKind::Percent => Some(BinOp::Mod),
        TokenKind::Eq => Some(BinOp::Eq),
        TokenKind::Ne => Some(BinOp::Ne),
        TokenKind::Lt => Some(BinOp::Lt),
        TokenKind::Le => Some(BinOp::Le),
        TokenKind::Gt => Some(BinOp::Gt),
        TokenKind::Ge => Some(BinOp::Ge),
        TokenKind::And => Some(BinOp::And),
        TokenKind::Or => Some(BinOp::Or),
        TokenKind::Ampersand => Some(BinOp::BitAnd),
        TokenKind::Pipe => Some(BinOp::BitOr),
        TokenKind::Pipe3 => Some(BinOp::BitOr),
        TokenKind::Caret => Some(BinOp::BitXor),
        TokenKind::Shl => Some(BinOp::Shl),
        TokenKind::Shr => Some(BinOp::Shr),
        TokenKind::Assign => Some(BinOp::Assign),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;

    fn parse(source: &str) -> NuResult<AstModule> {
        let mut lexer = Lexer::new(source);
        let tokens = lexer.lex()?;
        let mut parser = Parser::new(tokens);
        parser.parse_module()
    }

    fn parse_expr(source: &str) -> NuResult<Expr> {
        let mut lexer = Lexer::new(source);
        let tokens = lexer.lex()?;
        let mut parser = Parser::new(tokens);
        parser.parse_expr()
    }

    #[test]
    fn test_parse_record_type() {
        let ast = parse("type Point = { x: Int, y: Int }").unwrap();
        assert_eq!(ast.decls.len(), 1);
        match &ast.decls[0] {
            Decl::RecordType { name, fields, .. } => {
                assert_eq!(name, "Point");
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].0, "x");
                assert_eq!(fields[0].1, Type::Primitive(PrimitiveType::Int));
                assert_eq!(fields[1].0, "y");
                assert_eq!(fields[1].1, Type::Primitive(PrimitiveType::Int));
            }
            _ => panic!("Expected record type declaration"),
        }
    }

    #[test]
    fn test_parse_variant_type() {
        let ast = parse("type Option[T] = Some(T) | None").unwrap();
        match &ast.decls[0] {
            Decl::VariantType {
                name,
                type_params,
                variants,
                ..
            } => {
                assert_eq!(name, "Option");
                assert_eq!(type_params, &["T"]);
                assert_eq!(variants.len(), 2);
                assert_eq!(variants[0].0, "Some");
                assert!(variants[0].1.is_some());
                assert_eq!(variants[1].0, "None");
                assert!(variants[1].1.is_none());
            }
            _ => panic!("Expected variant type declaration"),
        }
    }

    #[test]
    fn test_parse_effect_decl() {
        // Parenthesize the argument so the effect-decl parser does not consume
        // the `->` as part of a function-typed argument.
        let ast = parse("effect IO { print: (String) -> Unit }").unwrap();
        match &ast.decls[0] {
            Decl::EffectDecl { name, ops, .. } => {
                assert_eq!(name, "IO");
                assert_eq!(ops.len(), 1);
                assert_eq!(ops[0].0, "print");
                assert_eq!(ops[0].1, vec![Type::Primitive(PrimitiveType::String)]);
                assert_eq!(ops[0].2, Type::Primitive(PrimitiveType::Unit));
            }
            _ => panic!("Expected effect declaration"),
        }
    }

    #[test]
    fn test_parse_effect_row_builtin_event_and_ffi() {
        // SPEC2 §4.6 lists Event and FFI as built-in effects; annotation
        // parsing must map them to the built-in variants (not UserDefined),
        // exactly like `perform` resolution in the effect checker does.
        let ast = parse("fn f() -> Unit ! {Event, FFI} 1").unwrap();
        match &ast.decls[0] {
            Decl::Function {
                effect: Some(row), ..
            } => {
                assert_eq!(
                    row,
                    &EffectRow::Closed(vec![Effect::Event, Effect::FFI]),
                    "Event and FFI must parse as built-in effects"
                );
            }
            _ => panic!("Expected annotated function declaration"),
        }
    }

    #[test]
    fn test_parse_type_alias() {
        let ast = parse("type alias MyInt = Int").unwrap();
        match &ast.decls[0] {
            Decl::TypeAlias { name, body, .. } => {
                assert_eq!(name, "MyInt");
                assert_eq!(body, &Type::Primitive(PrimitiveType::Int));
            }
            _ => panic!("Expected type alias declaration"),
        }
    }

    #[test]
    fn test_parse_nil_primitive_type() {
        // `Nil` (uppercase) must parse as the primitive Nil type, not a
        // silently unconstrained fresh type variable.
        let ast = parse("fn f(x: Nil) x").unwrap();
        match &ast.decls[0] {
            Decl::Function { params, .. } => {
                assert_eq!(params[0].1, Some(Type::Primitive(PrimitiveType::Nil)));
            }
            _ => panic!("Expected function declaration"),
        }
    }

    #[test]
    fn test_parse_unknown_type_name_errors() {
        let result = parse("fn f(x: Bogus) x");
        match result {
            Err(NuError::ParseError { msg, .. }) => {
                assert!(
                    msg.contains("Unknown type name") && msg.contains("Bogus"),
                    "unexpected message: {}",
                    msg
                );
            }
            other => panic!("expected unknown type name error, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_declared_alias_expands_in_annotation() {
        let ast = parse("type alias MyInt = Int\nfn f(x: MyInt) x").unwrap();
        match &ast.decls[1] {
            Decl::Function { params, .. } => {
                assert_eq!(params[0].1, Some(Type::Primitive(PrimitiveType::Int)));
            }
            _ => panic!("Expected function declaration"),
        }
    }

    #[test]
    fn test_parse_declared_variant_expands_with_args() {
        // `Option[Int]` expands to the variant structure with `T := Int`.
        let ast = parse("type Option[T] = Some(T) | None\nfn f(x: Option[Int]) x").unwrap();
        match &ast.decls[1] {
            Decl::Function { params, .. } => match &params[0].1 {
                Some(Type::Variant(variants)) => {
                    assert_eq!(variants.len(), 2);
                    assert_eq!(variants[0].0, "Some");
                    assert_eq!(variants[0].1, Some(Type::Primitive(PrimitiveType::Int)));
                    assert_eq!(variants[1].0, "None");
                    assert_eq!(variants[1].1, None);
                }
                other => panic!("expected expanded variant annotation, got {:?}", other),
            },
            _ => panic!("Expected function declaration"),
        }
    }

    #[test]
    fn test_parse_type_argument_arity_error() {
        let result = parse("type Option[T] = Some(T) | None\nfn f(x: Option[Int, String]) x");
        match result {
            Err(NuError::ParseError { msg, .. }) => {
                assert!(msg.contains("type argument"), "unexpected message: {}", msg);
            }
            other => panic!("expected arity error, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_module_decl() {
        let ast = parse("module M { fn f() 1 }").unwrap();
        match &ast.decls[0] {
            Decl::Module { name, decls, .. } => {
                assert_eq!(name, "M");
                assert_eq!(decls.len(), 1);
                assert!(matches!(&decls[0], Decl::Function { name, .. } if name == "f"));
            }
            _ => panic!("Expected module declaration"),
        }
    }

    #[test]
    fn test_parse_import() {
        let ast = parse("import Foo").unwrap();
        match &ast.decls[0] {
            Decl::Import { path, .. } => {
                assert_eq!(path, "Foo");
            }
            _ => panic!("Expected import declaration"),
        }
    }

    #[test]
    fn test_parse_actor_decl() {
        let source = r#"actor Counter {
            state count = 0
            behavior get() { self.count }
        }"#;
        let ast = parse(source).unwrap();
        match &ast.decls[0] {
            Decl::Actor {
                name,
                persistent,
                state_fields,
                behaviors,
                ..
            } => {
                assert_eq!(name, "Counter");
                assert!(!persistent);
                assert_eq!(state_fields.len(), 1);
                assert_eq!(state_fields[0].0, "count");
                assert_eq!(state_fields[0].1, StateModel::Local);
                assert_eq!(behaviors.len(), 1);
                assert_eq!(behaviors[0].name, "get");
            }
            _ => panic!("Expected actor declaration"),
        }
    }

    #[test]
    fn test_parse_persistent_actor_with_state_models() {
        let source = r#"
            persistent actor BankAccount {
                state durable balance: Int = 0
                state local temp: Int = 0
                state event_sourced events: Int = 0
                state crdt viewers: Int = 0
                behavior get() { self.balance }
            }
        "#;
        let ast = parse(source).unwrap();
        match &ast.decls[0] {
            Decl::Actor {
                name,
                persistent,
                state_fields,
                behaviors,
                ..
            } => {
                assert_eq!(name, "BankAccount");
                assert!(persistent);
                assert_eq!(state_fields.len(), 4);
                assert_eq!(state_fields[0].0, "balance");
                assert_eq!(state_fields[0].1, StateModel::Durable);
                assert_eq!(state_fields[0].2, Type::int());
                assert_eq!(state_fields[1].1, StateModel::Local);
                assert_eq!(state_fields[2].1, StateModel::EventSourced);
                assert_eq!(state_fields[3].1, StateModel::Crdt);
                assert_eq!(behaviors.len(), 1);
                assert_eq!(behaviors[0].name, "get");
            }
            _ => panic!("Expected actor declaration"),
        }
    }

    #[test]
    fn test_parse_actor_with_initializer() {
        let source = r#"
            persistent actor Counter {
                state durable count: Int = 0
                initial init(start_val: Int) { self.count = start_val }
                behavior inc() { self.count = self.count + 1 }
            }
        "#;
        let ast = parse(source).unwrap();
        match &ast.decls[0] {
            Decl::Actor {
                name,
                initializer,
                ..
            } => {
                assert_eq!(name, "Counter");
                let (init_name, params, _body) = initializer.as_ref().expect("should have initializer");
                assert_eq!(init_name, "init");
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].0, "start_val");
            }
            _ => panic!("Expected actor declaration"),
        }
    }

    #[test]
    fn test_parse_spawn_positional_args_and_as() {
        // spawn Foo(a, b) as "my_foo"
        let expr = parse_expr(r#"spawn Foo(1, 2) as "my_foo""#).unwrap();
        match expr {
            Expr::Spawn {
                positional_args,
                register_as,
                ..
            } => {
                let args = positional_args.expect("should have positional args");
                assert_eq!(args.len(), 2);
                assert_eq!(register_as.as_deref(), Some("my_foo"));
            }
            _ => panic!("Expected Spawn"),
        }

        // spawn Foo { x = 1 } as "bar"
        let expr = parse_expr(r#"spawn Foo { x = 1 } as "bar""#).unwrap();
        match expr {
            Expr::Spawn {
                init,
                positional_args,
                register_as,
                ..
            } => {
                assert!(positional_args.is_none());
                assert_eq!(init.len(), 1);
                assert_eq!(init[0].0, "x");
                assert_eq!(register_as.as_deref(), Some("bar"));
            }
            _ => panic!("Expected Spawn"),
        }

        // spawn Foo(1) without as
        let expr = parse_expr("spawn Foo(1)").unwrap();
        match expr {
            Expr::Spawn {
                positional_args,
                register_as,
                ..
            } => {
                assert!(positional_args.is_some());
                assert!(register_as.is_none());
            }
            _ => panic!("Expected Spawn"),
        }
    }
    #[test]
    fn test_parse_record_literal() {
        let ast = parse("{ x: 1, y: 2 }").unwrap();
        match &ast.decls[0] {
            Decl::Function { name, body, .. } if name == "__main" => match body {
                Expr::Record(fields, _) => {
                    assert_eq!(fields.len(), 2);
                    assert_eq!(fields[0].0, "x");
                    assert_eq!(fields[1].0, "y");
                }
                _ => panic!("Expected record literal"),
            },
            _ => panic!("Expected synthetic __main wrapping record literal"),
        }
    }

    #[test]
    fn test_parse_record_pattern() {
        let source = r#"match r { { x: a, y: b } => a + b }"#;
        let expr = parse_expr(source).unwrap();
        match expr {
            Expr::Match { arms, .. } => {
                assert_eq!(arms.len(), 1);
                match &arms[0].0 {
                    Pattern::Record(fields) => {
                        assert_eq!(fields.len(), 2);
                        assert_eq!(fields[0].0, "x");
                        assert_eq!(fields[1].0, "y");
                    }
                    _ => panic!("Expected record pattern"),
                }
            }
            _ => panic!("Expected match expression"),
        }
    }

    #[test]
    fn test_parse_variant_pattern() {
        let source = r#"match o { Some(x) => x | None => 0 }"#;
        let expr = parse_expr(source).unwrap();
        match expr {
            Expr::Match { arms, .. } => {
                assert_eq!(arms.len(), 2);
                match &arms[0].0 {
                    Pattern::Variant(name, Some(_)) => assert_eq!(name, "Some"),
                    _ => panic!("Expected Some variant pattern"),
                }
                match &arms[1].0 {
                    Pattern::Variant(name, None) => assert_eq!(name, "None"),
                    _ => panic!("Expected None variant pattern"),
                }
            }
            _ => panic!("Expected match expression"),
        }
    }

    #[test]
    fn test_parse_handle_with_resume() {
        let source = r#"handle perform E.op() { | E.op() resume => 42 }"#;
        let expr = parse_expr(source).unwrap();
        match expr {
            Expr::Handle { handlers, .. } => {
                assert_eq!(handlers.len(), 1);
                assert_eq!(handlers[0].effect_name, "E");
                assert_eq!(handlers[0].op_name, "op");
                assert!(handlers[0].resume);
            }
            _ => panic!("Expected handle expression"),
        }
    }

    #[test]
    fn test_parse_pipe_operator() {
        let expr = parse_expr("5 |> f").unwrap();
        match expr {
            Expr::Pipe { left, right, .. } => {
                assert!(matches!(left.as_ref(), Expr::Literal(Literal::Int(5), _)));
                assert!(matches!(right.as_ref(), Expr::Var(name, _) if name == "f"));
            }
            _ => panic!("Expected pipe expression"),
        }
    }

    #[test]
    fn test_parse_spawn_link_desugars() {
        // `spawn link A { ... }` desugars in the parser to
        // `let __spawn_ref = spawn A { ... } in { perform Actor.link(__spawn_ref); __spawn_ref }`.
        let expr = parse_expr("spawn link Counter { count = 0 }").unwrap();
        let Expr::Let {
            name, value, body, ..
        } = expr
        else {
            panic!("Expected let from spawn link desugar, got {:?}", expr);
        };
        assert_eq!(name, "__spawn_ref");
        match value.as_ref() {
            Expr::Spawn { actor_type, init, .. } => {
                assert!(matches!(actor_type.as_ref(), Expr::Var(n, _) if n == "Counter"));
                assert_eq!(init.len(), 1);
                assert_eq!(init[0].0, "count");
            }
            other => panic!("Expected spawn in let value, got {:?}", other),
        }
        match body.as_ref() {
            Expr::Block { exprs, .. } => {
                assert_eq!(exprs.len(), 2);
                match &exprs[0] {
                    Expr::Perform { effect, op, args, .. } => {
                        assert_eq!(effect, "Actor");
                        assert_eq!(op, "link");
                        assert_eq!(args.len(), 1);
                        assert!(matches!(&args[0], Expr::Var(n, _) if n == "__spawn_ref"));
                    }
                    other => panic!("Expected perform Actor.link, got {:?}", other),
                }
                assert!(matches!(&exprs[1], Expr::Var(n, _) if n == "__spawn_ref"));
            }
            other => panic!("Expected block body, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_spawn_monitor_desugars() {
        let expr = parse_expr("spawn monitor Counter { count = 0 }").unwrap();
        let Expr::Let { body, .. } = expr else {
            panic!("Expected let from spawn monitor desugar, got {:?}", expr);
        };
        match body.as_ref() {
            Expr::Block { exprs, .. } => match &exprs[0] {
                Expr::Perform { effect, op, .. } => {
                    assert_eq!(effect, "Actor");
                    assert_eq!(op, "monitor");
                }
                other => panic!("Expected perform Actor.monitor, got {:?}", other),
            },
            other => panic!("Expected block body, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_spawn_plain_not_desugared() {
        let expr = parse_expr("spawn Counter { count = 0 }").unwrap();
        assert!(
            matches!(expr, Expr::Spawn { .. }),
            "plain spawn must stay a Spawn node, got {:?}",
            expr
        );
    }

    #[test]
    fn test_parse_spawn_link_missing_body_errors() {
        assert!(parse_expr("spawn link Counter").is_err());
        assert!(parse_expr("spawn link").is_err());
    }

    #[test]
    fn test_parse_receive_after() {
        let expr = parse_expr("receive { | Msg(x) => x } after 100 => 0").unwrap();
        match expr {
            Expr::Receive { arms, after, .. } => {
                assert_eq!(arms.len(), 1);
                assert_eq!(arms[0].0, "Msg");
                assert_eq!(arms[0].1, vec!["x".to_string()]);
                let (ms, body) = after.expect("after clause");
                assert!(matches!(ms.as_ref(), Expr::Literal(Literal::Int(100), _)));
                assert!(matches!(body.as_ref(), Expr::Literal(Literal::Int(0), _)));
            }
            _ => panic!("Expected receive expression"),
        }
    }

    #[test]
    fn test_parse_receive_without_after() {
        let expr = parse_expr("receive { | Msg(x) => x }").unwrap();
        match expr {
            Expr::Receive { arms, after, .. } => {
                assert_eq!(arms.len(), 1);
                assert!(after.is_none());
            }
            _ => panic!("Expected receive expression"),
        }
    }

    #[test]
    fn test_parse_receive_after_malformed_errors() {
        // Missing `=>` between the timeout expression and the body.
        assert!(parse_expr("receive { | Msg() => 0 } after 100").is_err());
        // Missing timeout expression.
        assert!(parse_expr("receive { | Msg() => 0 } after => 0").is_err());
    }

    #[test]
    fn test_after_stays_a_plain_identifier() {
        // `after` is contextual (only special right after a receive block);
        // elsewhere it remains a usable identifier, e.g. a let binding or a
        // workflow step name (integration_tests has `step after { ... }`).
        let expr = parse_expr("let after = 1 in after + 1").unwrap();
        assert!(
            matches!(&expr, Expr::Let { name, .. } if name == "after"),
            "`after` must still bind as an identifier, got {:?}",
            expr
        );
        let module = parse("workflow W { step after { 1 } }").unwrap();
        assert_eq!(module.decls.len(), 1);
    }

    #[test]
    fn test_parse_perform_keyword_ops() {
        // `link`, `monitor` and `exit` are reserved keywords; they must still
        // parse as effect operation names (BEAM Actor.* builtin effects).
        for (source, expected_op) in [
            ("perform Actor.link(a)", "link"),
            ("perform Actor.monitor(a)", "monitor"),
            ("perform Actor.demonitor(a)", "demonitor"),
            ("perform Actor.unlink(a)", "unlink"),
            ("perform Actor.exit(1)", "exit"),
            ("perform Actor.trap_exit(true)", "trap_exit"),
        ] {
            let expr = parse_expr(source).unwrap();
            match expr {
                Expr::Perform { effect, op, .. } => {
                    assert_eq!(effect, "Actor", "{}", source);
                    assert_eq!(op, expected_op, "{}", source);
                }
                other => panic!("Expected perform for {}, got {:?}", source, other),
            }
        }
    }

    #[test]
    fn test_parse_type_annotation() {
        let expr = parse_expr("(x : Int)").unwrap();
        match expr {
            Expr::TypeAnnotate { expr, ty, .. } => {
                assert!(matches!(expr.as_ref(), Expr::Var(name, _) if name == "x"));
                assert_eq!(ty, Type::Primitive(PrimitiveType::Int));
            }
            _ => panic!("Expected type annotation"),
        }
    }

    #[test]
    fn test_parse_capability_annotation() {
        let expr = parse_expr("x :cap iso").unwrap();
        match expr {
            Expr::CapAnnotate { expr, cap, .. } => {
                assert!(matches!(expr.as_ref(), Expr::Var(name, _) if name == "x"));
                assert_eq!(cap, Capability::Iso);
            }
            _ => panic!("Expected capability annotation"),
        }
    }

    #[test]
    fn test_parse_alias_pattern() {
        let expr = parse_expr("match v { n @ Some(x) => n }").unwrap();
        match expr {
            Expr::Match { arms, .. } => match &arms[0].0 {
                Pattern::Alias(name, inner) => {
                    assert_eq!(name, "n");
                    assert!(matches!(inner.as_ref(), Pattern::Variant(v, _) if v == "Some"));
                }
                _ => panic!("Expected alias pattern"),
            },
            _ => panic!("Expected match expression"),
        }
    }

    #[test]
    fn test_parse_error_unexpected_token() {
        let result = parse("fn");
        assert!(result.is_err(), "Expected parse error for bare 'fn'");
    }

    #[test]
    fn test_parse_error_broken_fn_propagates() {
        // A declaration that fails mid-parse must surface its real error
        // instead of retrying the remaining tokens as an expression — `fn 5`
        // must not parse as the expression `5`.
        let result = parse("fn 5");
        assert!(result.is_err(), "Expected parse error for 'fn 5'");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("function name"),
            "Error should be the real declaration error, got: {}",
            msg
        );
    }

    #[test]
    fn test_parse_error_broken_pub_propagates() {
        // `pub` consumed a token before the decl parse failed, so the
        // original error must propagate rather than falling back to `42`.
        let result = parse("pub 42");
        assert!(result.is_err(), "Expected parse error for 'pub 42'");
    }

    #[test]
    fn test_parse_error_broken_module_propagates() {
        // `module Foo` consumed `module`/`Foo` before failing on the missing
        // brace; the error must come from the declaration parse.
        let result = parse("module Foo");
        assert!(result.is_err(), "Expected parse error for 'module Foo'");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("{") || msg.contains("brace"),
            "Error should mention the missing brace, got: {}",
            msg
        );
    }

    #[test]
    fn test_parse_top_level_expression_still_works() {
        // Zero tokens consumed by the decl parse → expression fallback.
        let ast = parse("42").unwrap();
        assert_eq!(ast.decls.len(), 1);
    }

    #[test]
    fn test_parse_doc_comment_before_decl() {
        let ast = parse("/// doc for foo\nfn foo() { 1 }").unwrap();
        assert_eq!(ast.decls.len(), 1);
    }

    #[test]
    fn test_parse_doc_comment_before_expression() {
        let ast = parse("/// doc\n42").unwrap();
        assert_eq!(ast.decls.len(), 1);
    }

    #[test]
    fn test_parse_doc_comment_only_file() {
        let ast = parse("/// nothing but docs").unwrap();
        assert!(ast.decls.is_empty());
    }

    #[test]
    fn test_parse_let_type_annotation() {
        let expr = parse_expr("let x : Int = 1 in x").unwrap();
        match expr {
            Expr::Let { name, ty, .. } => {
                assert_eq!(name, "x");
                assert_eq!(ty, Some(Type::Primitive(PrimitiveType::Int)));
            }
            _ => panic!("Expected let expression"),
        }
    }

    #[test]
    fn test_parse_let_without_annotation() {
        let expr = parse_expr("let x = 1 in x").unwrap();
        match expr {
            Expr::Let { ty, .. } => assert!(ty.is_none()),
            _ => panic!("Expected let expression"),
        }
    }

    #[test]
    fn test_parse_error_missing_arrow_in_effect() {
        let result = parse("effect E { op: Int }");
        assert!(
            result.is_err(),
            "Expected parse error for effect op missing arrow"
        );
    }

    #[test]
    fn test_parse_extern_block() {
        let ast = parse(r#"extern "libm.so.6" { fn sqrt(x: Float) -> Float fn pow(x: Float, y: Float) -> Float }"#).unwrap();
        assert_eq!(ast.decls.len(), 1);
        match &ast.decls[0] {
            Decl::Extern { library, funcs, .. } => {
                assert_eq!(library, "libm.so.6");
                assert_eq!(funcs.len(), 2);
                assert_eq!(funcs[0].name, "sqrt");
                assert_eq!(funcs[0].params, vec![("x".to_string(), Type::float())]);
                assert_eq!(funcs[0].ret, Type::float());
                assert_eq!(funcs[1].name, "pow");
                assert_eq!(
                    funcs[1].params,
                    vec![
                        ("x".to_string(), Type::float()),
                        ("y".to_string(), Type::float())
                    ]
                );
                assert_eq!(funcs[1].ret, Type::float());
            }
            _ => panic!("Expected extern declaration"),
        }
    }

    #[test]
    fn test_parse_extern_empty_block() {
        let ast = parse(r#"extern "empty" {}"#).unwrap();
        match &ast.decls[0] {
            Decl::Extern { library, funcs, .. } => {
                assert_eq!(library, "empty");
                assert!(funcs.is_empty());
            }
            _ => panic!("Expected extern declaration"),
        }
    }

    #[test]
    fn test_parse_extern_missing_param_type_errors() {
        let result = parse(r#"extern "lib" { fn f(x) -> Int }"#);
        assert!(
            result.is_err(),
            "Expected parse error for missing parameter type in extern"
        );
    }

    #[test]
    fn test_parse_workflow_with_steps() {
        let ast =
            parse("workflow PurchaseOrder { step validate { 1 } step charge { 2 } }").unwrap();
        assert_eq!(ast.decls.len(), 1);
        match &ast.decls[0] {
            Decl::Workflow {
                name,
                items,
                compensate,
                ..
            } => {
                assert_eq!(name, "PurchaseOrder");
                assert_eq!(items.len(), 2);
                match (&items[0], &items[1]) {
                    (WorkflowItem::Step(a), WorkflowItem::Step(b)) => {
                        assert_eq!(a.name, "validate");
                        assert_eq!(b.name, "charge");
                    }
                    _ => panic!("Expected two sequential steps"),
                }
                assert!(compensate.is_none());
            }
            _ => panic!("Expected workflow declaration"),
        }
    }

    #[test]
    fn test_parse_workflow_with_parallel_and_compensate() {
        let ast =
            parse("workflow Booking { parallel { step a { 1 } step b { 2 } } compensate { 0 } }")
                .unwrap();
        match &ast.decls[0] {
            Decl::Workflow {
                items, compensate, ..
            } => {
                assert_eq!(items.len(), 1);
                match &items[0] {
                    WorkflowItem::Parallel(branches) => {
                        assert_eq!(branches.len(), 2);
                    }
                    _ => panic!("Expected parallel block"),
                }
                assert!(compensate.is_some());
            }
            _ => panic!("Expected workflow declaration"),
        }
    }

    #[test]
    fn test_parse_workflow_invalid_body_errors() {
        let result = parse("workflow W { fn f() -> Int { 1 } }");
        assert!(
            result.is_err(),
            "Expected parse error for invalid workflow body"
        );
    }

    #[test]
    fn test_parse_tool_annotation() {
        let source = r#"@tool(description: "Adds two integers.")
        pub fn add(x: Int, y: Int) -> Int { x + y }"#;
        let ast = parse(source).unwrap();
        assert_eq!(ast.decls.len(), 1);
        match &ast.decls[0] {
            Decl::Function {
                name,
                annotations,
                public,
                ..
            } => {
                assert_eq!(name, "add");
                assert!(*public);
                assert_eq!(annotations.len(), 1);
                assert_eq!(
                    annotations[0],
                    FunctionAnnotation::Tool {
                        description: "Adds two integers.".to_string(),
                    }
                );
            }
            _ => panic!("Expected function declaration with tool annotation"),
        }
    }

    #[test]
    fn test_parse_agent_full() {
        let source = r#"
            agent MyAgent = {
                model: "gpt-4o",
                system_prompt: "You are helpful.",
                tools: [add, subtract],
                memory: { max_turns: 100 }
            }
        "#;
        let ast = parse(source).unwrap();
        assert_eq!(ast.decls.len(), 1);
        match &ast.decls[0] {
            Decl::Agent {
                name,
                model,
                system_prompt,
                tools,
                memory,
                ..
            } => {
                assert_eq!(name, "MyAgent");
                assert_eq!(model, "gpt-4o");
                assert_eq!(system_prompt.as_deref(), Some("You are helpful."));
                assert_eq!(tools, &["add".to_string(), "subtract".to_string()]);
                assert_eq!(memory.as_ref().unwrap().max_turns, 100);
            }
            _ => panic!("Expected agent declaration"),
        }
    }

    #[test]
    fn test_parse_agent_minimal() {
        let source = r#"agent MyAgent = { model: "gpt-4o" }"#;
        let ast = parse(source).unwrap();
        match &ast.decls[0] {
            Decl::Agent {
                name,
                model,
                system_prompt,
                tools,
                memory,
                ..
            } => {
                assert_eq!(name, "MyAgent");
                assert_eq!(model, "gpt-4o");
                assert!(system_prompt.is_none());
                assert!(tools.is_empty());
                assert_eq!(memory.as_ref().unwrap().max_turns, 50);
            }
            _ => panic!("Expected agent declaration"),
        }
    }

    #[test]
    fn test_parse_agent_missing_model_errors() {
        let result = parse("agent MyAgent = { system_prompt: \"hi\" }");
        assert!(
            result.is_err(),
            "Expected parse error for agent missing model"
        );
    }

    #[test]
    fn test_parse_agent_unknown_field_errors() {
        let result = parse("agent MyAgent = { model: \"x\", unknown: 1 }");
        assert!(
            result.is_err(),
            "Expected parse error for unknown agent field"
        );
    }

    #[test]
    fn test_parse_agent_procedural_memory() {
        let source = r#"
            agent MyAgent = {
                model: "gpt-4o",
                procedural_memory: { namespace: "my_app" }
            }
        "#;
        let ast = parse(source).unwrap();
        match &ast.decls[0] {
            Decl::Agent {
                procedural_memory, ..
            } => {
                assert_eq!(
                    procedural_memory.as_ref().map(|m| m.namespace.as_str()),
                    Some("my_app")
                );
            }
            _ => panic!("Expected agent declaration"),
        }
    }

    #[test]
    fn test_parse_agent_procedural_memory_default_namespace() {
        let source = r#"agent MyAgent = { model: "gpt-4o", procedural_memory: {} }"#;
        let ast = parse(source).unwrap();
        match &ast.decls[0] {
            Decl::Agent {
                procedural_memory, ..
            } => {
                assert_eq!(
                    procedural_memory.as_ref().map(|m| m.namespace.as_str()),
                    Some("default")
                );
            }
            _ => panic!("Expected agent declaration"),
        }
    }

    #[test]
    fn test_parse_state_machine_full_sketch() {
        // The BEAM_PRIMITIVES §4.2 sketch, completed for the implemented
        // grammar: every state is declared with a `state` line (the first is
        // the initial state) and every event target is a declared state.
        let source = r#"
            state_machine TcpConnection {
                state Closed
                state Connecting
                state Connected

                event connect(address): Connecting
                event connection_established: Connected
                event disconnect: Closed

                on_entry Connected {
                    perform IO.print("up")
                }

                on_exit Connected {
                    perform IO.print("down")
                }
            }
        "#;
        let ast = parse(source).unwrap();
        assert_eq!(ast.decls.len(), 1);
        match &ast.decls[0] {
            Decl::StateMachine {
                name,
                states,
                events,
                entry_hooks,
                exit_hooks,
                ..
            } => {
                assert_eq!(name, "TcpConnection");
                assert_eq!(states, &["Closed", "Connecting", "Connected"]);
                assert_eq!(events.len(), 3);
                assert_eq!(events[0].name, "connect");
                assert_eq!(events[0].params, vec![("address".to_string(), None)]);
                assert_eq!(events[0].target, "Connecting");
                assert_eq!(events[1].name, "connection_established");
                assert!(events[1].params.is_empty());
                assert_eq!(events[1].target, "Connected");
                assert_eq!(events[2].name, "disconnect");
                assert_eq!(events[2].target, "Closed");
                assert_eq!(entry_hooks.len(), 1);
                assert_eq!(entry_hooks[0].0, "Connected");
                assert_eq!(exit_hooks.len(), 1);
                assert_eq!(exit_hooks[0].0, "Connected");
            }
            _ => panic!("Expected state_machine declaration"),
        }
    }

    #[test]
    fn test_parse_state_machine_typed_event_params() {
        let source = r#"
            state_machine M {
                state A
                state B
                event go(x: Int, y: String): B
            }
        "#;
        let ast = parse(source).unwrap();
        match &ast.decls[0] {
            Decl::StateMachine { events, .. } => {
                assert_eq!(events.len(), 1);
                assert_eq!(
                    events[0].params,
                    vec![
                        ("x".to_string(), Some(Type::int())),
                        ("y".to_string(), Some(Type::string())),
                    ]
                );
            }
            _ => panic!("Expected state_machine declaration"),
        }
    }

    #[test]
    fn test_parse_state_machine_unknown_target_errors() {
        // DECISION (see parse_state_machine docs): unlike gen_statem, an
        // event target must be a declared state — the §4.2 sketch's
        // `event data_received(bytes): handle_data` handler-target form is
        // rejected with a clear error.
        let source = r#"
            state_machine TcpConnection {
                state Closed
                event data_received(bytes): handle_data
            }
        "#;
        let err = parse(source).unwrap_err();
        match err {
            NuError::ParseError { msg, .. } => {
                assert!(msg.contains("unknown state 'handle_data'"), "{}", msg);
                assert!(msg.contains("data_received"), "{}", msg);
                assert!(msg.contains("declared states: Closed"), "{}", msg);
            }
            _ => panic!("Expected ParseError"),
        }
    }

    #[test]
    fn test_parse_state_machine_duplicate_state_errors() {
        let source = "state_machine M { state A state A }";
        let err = parse(source).unwrap_err();
        match err {
            NuError::ParseError { msg, .. } => {
                assert!(msg.contains("duplicate state 'A'"), "{}", msg)
            }
            _ => panic!("Expected ParseError"),
        }
    }

    #[test]
    fn test_parse_state_machine_missing_initial_state_errors() {
        let source = "state_machine M { event go: A }";
        let err = parse(source).unwrap_err();
        match err {
            NuError::ParseError { msg, .. } => {
                assert!(msg.contains("requires at least one 'state <Name>'"), "{}", msg)
            }
            _ => panic!("Expected ParseError"),
        }
    }

    #[test]
    fn test_parse_state_machine_hook_unknown_state_errors() {
        let source = r#"
            state_machine M {
                state A
                on_entry B { nil }
            }
        "#;
        let err = parse(source).unwrap_err();
        match err {
            NuError::ParseError { msg, .. } => {
                assert!(msg.contains("on_entry hook references unknown state 'B'"), "{}", msg)
            }
            _ => panic!("Expected ParseError"),
        }
    }

    #[test]
    fn test_parse_state_machine_duplicate_event_errors() {
        let source = r#"
            state_machine M {
                state A
                event go: A
                event go: A
            }
        "#;
        let err = parse(source).unwrap_err();
        match err {
            NuError::ParseError { msg, .. } => {
                assert!(msg.contains("duplicate event 'go'"), "{}", msg)
            }
            _ => panic!("Expected ParseError"),
        }
    }

    #[test]
    fn test_parse_state_machine_duplicate_hook_errors() {
        let source = r#"
            state_machine M {
                state A
                on_exit A { nil }
                on_exit A { nil }
            }
        "#;
        let err = parse(source).unwrap_err();
        match err {
            NuError::ParseError { msg, .. } => {
                assert!(msg.contains("duplicate on_exit hook for state 'A'"), "{}", msg)
            }
            _ => panic!("Expected ParseError"),
        }
    }

    #[test]
    fn test_parse_state_machine_unexpected_item_errors() {
        let source = "state_machine M { state A behavior b() { nil } }";
        let err = parse(source).unwrap_err();
        match err {
            NuError::ParseError { msg, .. } => assert!(
                msg.contains("Expected 'state', 'event', 'on_entry', or 'on_exit'"),
                "{}",
                msg
            ),
            _ => panic!("Expected ParseError"),
        }
    }
}
