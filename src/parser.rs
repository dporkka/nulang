//! Recursive descent parser for Nulang.
//!
//! Uses Pratt parser (precedence climbing) for expressions.
//! Entry point: `Parser::parse_module()`.

use std::collections::HashMap;
use crate::ast::*;
use crate::lexer::{Token, TokenKind};
use crate::types::{Capability, Effect, EffectRow, NuError, NuResult, Span, Type, TypeVar, Region, PrimitiveType};

// ---------------------------------------------------------------------------
// Operator Precedence (13 levels, higher = tighter binding)
// ---------------------------------------------------------------------------

const PREC_LOWEST: u8 = 0;
const PREC_ASSIGN: u8 = 1;  // = += -=
const PREC_PIPE: u8 = 2;    // |>
const PREC_OR: u8 = 3;      // ||
const PREC_AND: u8 = 4;     // &&
const PREC_EQ: u8 = 5;      // == !=
const PREC_CMP: u8 = 6;     // < <= > >=
const PREC_TERM: u8 = 7;    // + -
const PREC_FACTOR: u8 = 8;  // * / %
const PREC_SHIFT: u8 = 9;   // << >>
const PREC_PREFIX: u8 = 10; // ! - & (prefix)
const PREC_CALL: u8 = 11;   // f(x), obj.field, arr[i]
const PREC_HIGHEST: u8 = 12;// Literals, identifiers, grouping

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
        TokenKind::PipeOp => (PREC_PIPE, true),
        TokenKind::Or => (PREC_OR, false),
        TokenKind::And => (PREC_AND, false),
        TokenKind::Eq | TokenKind::Ne => (PREC_EQ, false),
        TokenKind::Lt | TokenKind::Le | TokenKind::Gt | TokenKind::Ge => (PREC_CMP, false),
        TokenKind::Plus | TokenKind::Minus => (PREC_TERM, false),
        TokenKind::Star | TokenKind::Slash | TokenKind::Percent => (PREC_FACTOR, false),
        TokenKind::Shl | TokenKind::Shr => (PREC_SHIFT, false),
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
            if self.is_at_end() { break; }

            // Try declaration first, then expression
            match self.parse_decl() {
                Ok(decl) => decls.push(decl),
                Err(_) => {
                    // Not a declaration — try expression
                    let expr = self.parse_expr()?;
                    // Wrap expression as synthetic function __main
                    decls.push(Decl::Function {
                        name: "__main".to_string(),
                        type_params: vec![],
                        params: vec![],
                        ret_type: None,
                        effect: None,
                        cap: None,
                        body: expr,
                        public: false,
                        span: Span::new(0, 0, 0, 0),
                    });
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
        let public = self.consume_if(&TokenKind::Pub);

        self.skip_newlines();
        match self.peek_kind().clone() {
            TokenKind::Fn => self.parse_function(public),
            TokenKind::Actor => self.parse_actor(),
            TokenKind::Agent => self.parse_agent(),
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
                msg: format!("Unexpected token in declaration: {:?}", other),
                span: self.current_span(),
            }),
        }
    }

    fn parse_function(&mut self, public: bool) -> NuResult<Decl> {
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
        let effect = if self.consume_if(&TokenKind::Bang) {
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
            public,
            span,
        })
    }

    fn parse_actor(&mut self) -> NuResult<Decl> {
        let span = self.current_span();
        self.advance(); // consume 'actor'
        let name = self.expect_ident("actor name")?;
        let type_params = self.parse_type_params()?;
        self.expect(TokenKind::LBrace)?;

        let mut state_fields = Vec::new();
        let mut behaviors = Vec::new();

        self.skip_newlines();
        while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.match_token(&TokenKind::RBrace) {
                break;
            }
            match self.peek_kind().clone() {
                TokenKind::State => {
                    self.advance(); // 'state'
                    let field_name = self.expect_ident("state field name")?;
                    self.expect(TokenKind::Assign)?;
                    let default = self.parse_expr()?;
                    state_fields.push((field_name, Type::unit(), default));
                    self.skip_newlines_semicolons();
                }
                TokenKind::Behavior => {
                    behaviors.push(self.parse_behavior()?);
                }
                _ => {
                    // Try to parse as behavior without explicit 'behavior' keyword
                    // (some syntax variants)
                    return Err(NuError::ParseError {
                        msg: format!(
                            "Expected 'state' or 'behavior' in actor body, got {:?}",
                            self.peek_kind()
                        ),
                        span: self.current_span(),
                    });
                }
            }
        }
        self.expect(TokenKind::RBrace)?;

        Ok(Decl::Actor {
            name,
            type_params,
            state_fields,
            behaviors,
            init: vec![],
            span,
        })
    }

    fn parse_agent(&mut self) -> NuResult<Decl> {
        let span = self.current_span();
        self.advance(); // consume 'agent'
        let name = self.expect_ident("agent name")?;
        self.expect(TokenKind::LBrace)?;

        let mut state_fields = Vec::new();
        let mut memory_fields = Vec::new();
        let mut tools = Vec::new();
        let policy = None;
        let mut behaviors = Vec::new();

        self.skip_newlines();
        while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.match_token(&TokenKind::RBrace) {
                break;
            }
            match self.peek_kind().clone() {
                TokenKind::State => {
                    self.advance(); // 'state'
                    let field_name = self.expect_ident("state field name")?;
                    self.expect(TokenKind::Assign)?;
                    let default = self.parse_expr()?;
                    state_fields.push((field_name, Type::unit(), default));
                    self.skip_newlines_semicolons();
                }
                TokenKind::Behavior => {
                    behaviors.push(self.parse_behavior()?);
                }
                _ => {
                    // For simplicity, try to parse fields generically
                    let field = self.expect_ident("field name")?;
                    self.skip_newlines();
                    if self.consume_if(&TokenKind::Colon) {
                        self.skip_newlines();
                        let ty = self.parse_type()?;
                        memory_fields.push((field, ty));
                    } else {
                        // Could be a tool or policy
                        tools.push(field);
                    }
                    self.skip_newlines_semicolons();
                }
            }
        }
        self.expect(TokenKind::RBrace)?;

        // Observe expression defaults to unit if none was parsed
        let observe_expr = Expr::Literal(Literal::Unit, span);

        Ok(Decl::Agent {
            name,
            state_fields,
            memory_fields,
            tools,
            policy,
            observe: observe_expr,
            behaviors,
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

    fn parse_record_type(&mut self, public: bool) -> NuResult<Decl> {
        self.parse_type_decl_variant_or_record(public)
    }

    fn parse_variant_type(&mut self, public: bool) -> NuResult<Decl> {
        self.parse_type_decl_variant_or_record(public)
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
        let mut items = Vec::new();
        self.skip_newlines_semicolons();
        Ok(Decl::Import { path, items, span })
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
        let effect = if self.consume_if(&TokenKind::Bang) {
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
                    TokenKind::IntLit(_) | TokenKind::FloatLit(_) | TokenKind::StringLit(_)
                    | TokenKind::BoolLit(_) | TokenKind::UnitLit => self.parse_literal(),

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
                        if self.consume_if(&TokenKind::Rec) {
                            self.parse_let_rec()
                        } else {
                            self.parse_let()
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
                    TokenKind::For => self.parse_for(),
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
                        Ok(Expr::Break(self.current_span()))
                    }
                    TokenKind::SelfKw => self.parse_self_ref(),

                    _ => Err(NuError::ParseError {
                        msg: format!("Unexpected token in expression: {:?}", kind),
                        span,
                    }),
                }
            }
        }
    }

    fn parse_infix(&mut self, left: Expr, op: TokenKind, _prec: u8) -> NuResult<Expr> {
        // This is handled inline in parse_expr_with_prec for most cases.
        // This method exists for extensibility.
        let span = self.current_span();
        Err(NuError::ParseError {
            msg: format!("Unhandled infix operator: {:?}", op),
            span,
        })
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
                Ok(Expr::Literal(Literal::String(s), span))
            }
            TokenKind::BoolLit(b) => {
                self.advance();
                Ok(Expr::Literal(Literal::Bool(b), span))
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
        let span = self.current_span();
        let name = self.expect_ident("variable name")?;

        // Optional type annotation
        let _ty = if self.consume_if(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };

        self.expect(TokenKind::Assign)?;
        let value = self.parse_expr()?;
        self.expect(TokenKind::In)?;
        let body = self.parse_expr()?;
        Ok(Expr::Let {
            name,
            value: Box::new(value),
            body: Box::new(body),
            span,
        })
    }

    fn parse_let_rec(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        let name = self.expect_ident("function name")?;
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
        self.expect(TokenKind::With)?;
        self.expect(TokenKind::LBrace)?;

        let mut arms = Vec::new();
        self.skip_newlines();
        while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.match_token(&TokenKind::RBrace) {
                break;
            }

            // Pattern starts with '|'
            if self.consume_if(&TokenKind::Pipe) {
                // OK
            }
            self.skip_newlines();

            let pat = self.parse_pattern()?;
            self.expect(TokenKind::FatArrow)?;
            let expr = self.parse_expr()?;
            arms.push((pat, expr));
            self.skip_newlines_semicolons();
        }
        self.expect(TokenKind::RBrace)?;

        Ok(Expr::Match {
            scrutinee: Box::new(scrutinee),
            arms,
            span,
        })
    }

    fn parse_block(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.advance(); // consume '{'
        let mut exprs = Vec::new();
        self.skip_newlines();
        while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.match_token(&TokenKind::RBrace) {
                break;
            }
            exprs.push(self.parse_expr()?);
            self.skip_newlines_semicolons();
        }
        self.expect(TokenKind::RBrace)?;
        Ok(Expr::Block {
            exprs,
            span: self.current_span(),
        })
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
        let actor_type = self.parse_expr()?;
        self.expect(TokenKind::LBrace)?;
        let mut init = Vec::new();
        self.skip_newlines();
        while !self.match_token(&TokenKind::RBrace) && !self.is_at_end() {
            self.skip_newlines();
            if self.match_token(&TokenKind::RBrace) {
                break;
            }
            let field = self.expect_ident("field name")?;
            self.expect(TokenKind::Assign)?;
            let val = self.parse_expr()?;
            init.push((field, val));
            self.skip_newlines_semicolons();
        }
        self.expect(TokenKind::RBrace)?;
        Ok(Expr::Spawn {
            actor_type: Box::new(actor_type),
            init,
            span,
        })
    }

    fn parse_send_or_ask(&mut self, actor: Expr) -> NuResult<Expr> {
        let span = self.current_span();
        self.advance(); // consume '!' or we already have 'ask'
        let behavior = self.expect_ident("behavior name")?;
        self.expect(TokenKind::LParen)?;
        let args = self.parse_arg_list()?;
        Ok(Expr::Send {
            actor: Box::new(actor),
            behavior,
            args,
            span,
        })
    }

    fn parse_send_keyword(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.advance(); // consume 'send'
        let actor = self.parse_expr()?;
        let behavior = self.expect_ident("behavior name")?;
        self.expect(TokenKind::LParen)?;
        let args = self.parse_arg_list()?;
        Ok(Expr::Send {
            actor: Box::new(actor),
            behavior,
            args,
            span,
        })
    }

    fn parse_ask(&mut self) -> NuResult<Expr> {
        let span = self.current_span();
        self.advance(); // consume 'ask'
        let actor = self.parse_expr()?;
        let behavior = self.expect_ident("behavior name")?;
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
        let op = self.expect_ident("operation name")?;
        self.expect(TokenKind::LParen)?;
        let args = self.parse_arg_list()?;
        Ok(Expr::Perform {
            effect,
            op,
            args,
            span,
        })
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
                msg: format!("Expected {:?}", kind),
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
                msg: format!("Expected {}, found {:?}", msg, current_kind),
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
        while self.peek_kind() == &TokenKind::Newline {
            self.advance();
        }
    }

    fn skip_newlines_semicolons(&mut self) {
        while matches!(self.peek_kind(), &TokenKind::Newline | &TokenKind::Semicolon) {
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
                | TokenKind::Handle
                | TokenKind::For
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
        self.expect(TokenKind::LBrace)?;
        let mut handlers = Vec::new();
        self.skip_newlines();
        while self.peek_kind() == &TokenKind::Pipe {
            self.advance(); // consume '|'
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
                self.advance();
                let ty = match name.as_str() {
                    "Int" => Type::Primitive(PrimitiveType::Int),
                    "Float" => Type::Primitive(PrimitiveType::Float),
                    "Bool" => Type::Primitive(PrimitiveType::Bool),
                    "String" => Type::Primitive(PrimitiveType::String),
                    "Unit" => Type::Primitive(PrimitiveType::Unit),
                    "Never" => Type::Primitive(PrimitiveType::Never),
                    "Address" => Type::Primitive(PrimitiveType::Address),
                    _ => {
                        if let Some(&tv) = self.local_type_params.get(&name) {
                            Type::Var(tv)
                        } else {
                            let tv = *self.global_type_constructors
                                .entry(name)
                                .or_insert_with(TypeVar::fresh);
                            Type::Var(tv)
                        }
                    }
                };

                if self.peek_kind() == &TokenKind::LBracket {
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
                    Ok(Type::App {
                        constructor: Box::new(ty),
                        args,
                    })
                } else {
                    Ok(ty)
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
                msg: format!("Expected type, found {:?}", current_kind),
                span: self.current_span(),
            }),
        }
    }

    fn parse_capability(&mut self) -> NuResult<Capability> {
        let current_kind = self.peek_kind();
        match current_kind {
            TokenKind::Iso => { self.advance(); Ok(Capability::Iso) }
            TokenKind::Trn => { self.advance(); Ok(Capability::Trn) }
            TokenKind::Ref => { self.advance(); Ok(Capability::Ref) }
            TokenKind::Val => { self.advance(); Ok(Capability::Val) }
            TokenKind::Box => { self.advance(); Ok(Capability::Box) }
            TokenKind::Tag => { self.advance(); Ok(Capability::Tag) }
            TokenKind::Ident(s) if s == "lineariso" => { self.advance(); Ok(Capability::LinearIso) }
            _ => Err(NuError::ParseError {
                msg: format!("Expected capability (iso, trn, ref, val, box, tag, lineariso), found {:?}", current_kind),
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
        match name {
            "IO" => Effect::IO,
            "Net" => Effect::Net,
            "FS" => Effect::FS,
            "Rand" => Effect::Rand,
            "Time" => Effect::Time,
            "Spawn" => Effect::Spawn,
            "Send" => Effect::Send,
            "Receive" => Effect::Receive,
            "Migrate" => Effect::Migrate,
            "STM" => Effect::STM,
            "Async" => Effect::Async,
            "LLM" => Effect::LLM,
            "Cost" => Effect::Cost,
            _ => Effect::UserDefined(name.to_string()),
        }
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
            TokenKind::UnitLit => {
                self.advance();
                Ok(Pattern::Lit(Literal::Unit))
            }
            TokenKind::True => { self.advance(); Ok(Pattern::Lit(Literal::Bool(true))) }
            TokenKind::False => { self.advance(); Ok(Pattern::Lit(Literal::Bool(false))) }
            TokenKind::Unit => { self.advance(); Ok(Pattern::Lit(Literal::Unit)) }
            _ => Err(NuError::ParseError {
                msg: format!("Expected pattern, found {:?}", current_kind),
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
        TokenKind::Caret => Some(BinOp::BitXor),
        TokenKind::Shl => Some(BinOp::Shl),
        TokenKind::Shr => Some(BinOp::Shr),
        TokenKind::Assign => Some(BinOp::Assign),
        _ => None,
    }
}
