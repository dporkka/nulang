//! Recursive descent parser with Pratt precedence for expressions.

use crate::ast::*;
use crate::lexer::*;
use crate::types::{Capability, EffectRow, Span, Type};

// ---------------------------------------------------------------------------
// Parser state
// ---------------------------------------------------------------------------

pub struct Parser<'a> {
    lexer: std::iter::Peekable<Lexer<'a>>,
    current: Token,
    span: Span,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str) -> Self {
        let mut lexer = Lexer::new(input).peekable();
        let current = lexer.next().unwrap_or(Token::Eof);
        Parser {
            lexer,
            current,
            span: Span { start: 0, end: 0, line: 1, col: 1 },
        }
    }

    // -- Low-level operations --

    fn advance(&mut self) -> Token {
        let old = std::mem::replace(&mut self.current, self.lexer.next().unwrap_or(Token::Eof));
        old
    }

    fn peek(&mut self) -> Option<&Token> {
        self.lexer.peek()
    }

    fn at(&self, token: &Token) -> bool {
        std::mem::discriminant(&self.current) == std::mem::discriminant(token)
    }

    fn at_keyword(&self, kw: &Token) -> bool {
        self.current == *kw
    }

    fn expect(&mut self, token: Token) -> Result<(), ParseError> {
        if self.at(&token) {
            self.advance();
            Ok(())
        } else {
            Err(ParseError::Unexpected {
                expected: format!("{:?}", token),
                found: format!("{:?}", self.current),
                span: self.span.clone(),
            })
        }
    }

    fn consume_newlines(&mut self) {
        while self.at(&Token::Newline) || self.at(&Token::Indent(0)) {
            self.advance();
        }
    }

    fn consume_indent(&mut self) {
        if let Token::Indent(_) = self.current {
            self.advance();
        }
    }

    fn consume_dedent(&mut self) {
        if let Token::Dedent(_) = self.current {
            self.advance();
        }
    }

    fn expect_dedent(&mut self) -> Result<(), ParseError> {
        if let Token::Dedent(_) = self.current {
            self.advance();
            Ok(())
        } else {
            // Allow end-of-input as implicit dedent
            if self.at(&Token::Eof) {
                return Ok(());
            }
            Err(ParseError::Unexpected {
                expected: "dedent".to_string(),
                found: format!("{:?}", self.current),
                span: self.span.clone(),
            })
        }
    }

    // -- Type parsing --

    fn parse_type(&mut self) -> Result<Type, ParseError> {
        self.parse_type_arrow()
    }

    fn parse_type_arrow(&mut self) -> Result<Type, ParseError> {
        let left = self.parse_type_app()?;
        if self.at(&Token::Arrow) {
            self.advance();
            let right = self.parse_type_arrow()?;
            Ok(Type::Arrow(Box::new(left), Box::new(right)))
        } else {
            Ok(left)
        }
    }

    fn parse_type_app(&mut self) -> Result<Type, ParseError> {
        let mut base = self.parse_type_atomic()?;
        while let Some(Token::UpperIdent(_)) | Some(Token::Ident(_)) = self.peek() {
            if self.at(&Token::RParen) || self.at(&Token::Comma) || self.at(&Token::RBracket) || self.at(&Token::RBrace) {
                break;
            }
            let arg = self.parse_type_atomic()?;
            base = Type::App(Box::new(base), Box::new(arg));
        }
        Ok(base)
    }

    fn parse_type_atomic(&mut self) -> Result<Type, ParseError> {
        match &self.current {
            Token::UpperIdent(name) => {
                let name = name.clone();
                self.advance();
                match name.as_str() {
                    "Int" => Ok(Type::Prim(crate::types::Primitive::Int)),
                    "Float" => Ok(Type::Prim(crate::types::Primitive::Float)),
                    "String" => Ok(Type::Prim(crate::types::Primitive::String)),
                    "Bool" => Ok(Type::Prim(crate::types::Primitive::Bool)),
                    "Unit" => Ok(Type::Unit),
                    _ => Ok(Type::Named(name)),
                }
            }
            Token::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(Type::Named(name))
            }
            Token::LParen => {
                self.advance();
                if self.at(&Token::RParen) {
                    self.advance();
                    return Ok(Type::Unit);
                }
                let first = self.parse_type()?;
                if self.at(&Token::Comma) {
                    let mut elems = vec![first];
                    while self.at(&Token::Comma) {
                        self.advance();
                        elems.push(self.parse_type()?);
                    }
                    self.expect(Token::RParen)?;
                    Ok(Type::Tuple(elems))
                } else {
                    self.expect(Token::RParen)?;
                    Ok(first)
                }
            }
            Token::LBrace => {
                self.advance();
                let mut fields = Vec::new();
                while !self.at(&Token::RBrace) {
                    let name = match &self.current {
                        Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                        _ => return Err(ParseError::Unexpected {
                            expected: "field name".to_string(),
                            found: format!("{:?}", self.current),
                            span: self.span.clone(),
                        }),
                    };
                    self.expect(Token::Colon)?;
                    let ty = self.parse_type()?;
                    fields.push((name, ty));
                    if self.at(&Token::Comma) {
                        self.advance();
                    }
                }
                self.expect(Token::RBrace)?;
                Ok(Type::Record(fields))
            }
            Token::LBracket => {
                self.advance();
                let elem = self.parse_type()?;
                self.expect(Token::RBracket)?;
                Ok(Type::Array(Box::new(elem)))
            }
            Token::Fn => {
                self.advance();
                self.expect(Token::LParen)?;
                let mut params = Vec::new();
                while !self.at(&Token::RParen) {
                    params.push(self.parse_type()?);
                    if self.at(&Token::Comma) {
                        self.advance();
                    }
                }
                self.expect(Token::RParen)?;
                let effect = if self.at(&Token::Ident(s)) if s == "with" => {
                    self.advance();
                    Some(self.parse_effect_row()?)
                } else {
                    None
                };
                self.expect(Token::Arrow)?;
                let ret = self.parse_type()?;
                Ok(Type::Arrow(
                    Box::new(if params.len() == 1 {
                        params.into_iter().next().unwrap()
                    } else {
                        Type::Tuple(params)
                    }),
                    Box::new(ret),
                ))
            }
            _ => Err(ParseError::Unexpected {
                expected: "type".to_string(),
                found: format!("{:?}", self.current),
                span: self.span.clone(),
            }),
        }
    }

    fn parse_effect_row(&mut self) -> Result<EffectRow, ParseError> {
        let mut effects = Vec::new();
        while let Token::EffectName(name) = &self.current {
            effects.push(name.clone());
            self.advance();
            if self.at(&Token::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        Ok(EffectRow::Closed(effects))
    }

    fn parse_capability(&mut self) -> Result<Capability, ParseError> {
        let cap = match &self.current {
            Token::Iso => { self.advance(); Capability::Iso }
            Token::Trn => { self.advance(); Capability::Trn }
            Token::Ref => { self.advance(); Capability::Ref }
            Token::Val => { self.advance(); Capability::Val }
            Token::Box => { self.advance(); Capability::Box }
            Token::Tag => { self.advance(); Capability::Tag }
            _ => return Err(ParseError::Unexpected {
                expected: "capability".to_string(),
                found: format!("{:?}", self.current),
                span: self.span.clone(),
            }),
        };
        Ok(cap)
    }

    // -- Pattern parsing --

    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        let pat = match &self.current {
            Token::Underscore => { self.advance(); Pattern::Wild }
            Token::Ident(name) => {
                let name = name.clone();
                self.advance();
                if self.at(&Token::At) {
                    self.advance();
                    let inner = self.parse_pattern()?;
                    Pattern::Alias(name, Box::new(inner))
                } else {
                    Pattern::Var(name)
                }
            }
            Token::UpperIdent(name) => {
                let name = name.clone();
                self.advance();
                if self.at(&Token::LParen) {
                    self.advance();
                    let inner = self.parse_pattern()?;
                    self.expect(Token::RParen)?;
                    Pattern::Variant(name, Some(Box::new(inner)))
                } else {
                    Pattern::Variant(name, None)
                }
            }
            Token::IntLit(n) => { let n = *n; self.advance(); Pattern::Lit(Literal::Int(n)) }
            Token::FloatLit(f) => { let f = *f; self.advance(); Pattern::Lit(Literal::Float(f)) }
            Token::StringLit(s) => { let s = s.clone(); self.advance(); Pattern::Lit(Literal::String(s)) }
            Token::BoolLit(b) => { let b = *b; self.advance(); Pattern::Lit(Literal::Bool(b)) }
            Token::LParen => {
                self.advance();
                if self.at(&Token::RParen) {
                    self.advance();
                    Pattern::Lit(Literal::Unit)
                } else {
                    let mut elems = Vec::new();
                    while !self.at(&Token::RParen) {
                        elems.push(self.parse_pattern()?);
                        if self.at(&Token::Comma) {
                            self.advance();
                        }
                    }
                    self.expect(Token::RParen)?;
                    if elems.len() == 1 {
                        elems.into_iter().next().unwrap()
                    } else {
                        Pattern::Tuple(elems)
                    }
                }
            }
            Token::LBrace => {
                self.advance();
                let mut fields = Vec::new();
                while !self.at(&Token::RBrace) {
                    let name = match &self.current {
                        Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                        _ => return Err(ParseError::Unexpected {
                            expected: "field name".to_string(),
                            found: format!("{:?}", self.current),
                            span: self.span.clone(),
                        }),
                    };
                    self.expect(Token::Colon)?;
                    let pat = self.parse_pattern()?;
                    fields.push((name, pat));
                    if self.at(&Token::Comma) {
                        self.advance();
                    }
                }
                self.expect(Token::RBrace)?;
                Pattern::Record(fields)
            }
            Token::LBracket => {
                self.advance();
                let mut elems = Vec::new();
                while !self.at(&Token::RBracket) {
                    elems.push(self.parse_pattern()?);
                    if self.at(&Token::Comma) {
                        self.advance();
                    }
                }
                self.expect(Token::RBracket)?;
                Pattern::Array(elems)
            }
            Token::Cons => {
                self.advance();
                let head = self.parse_pattern()?;
                self.expect(Token::Cons)?;
                let tail = self.parse_pattern()?;
                Pattern::Cons(Box::new(head), Box::new(tail))
            }
            _ => return Err(ParseError::Unexpected {
                expected: "pattern".to_string(),
                found: format!("{:?}", self.current),
                span: self.span.clone(),
            }),
        };
        Ok(pat)
    }

    // -- Expression parsing (Pratt / precedence climbing) --

    pub fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_expr_prec(0)
    }

    fn parse_expr_prec(&mut self, min_prec: u8) -> Result<Expr, ParseError> {
        let mut left = self.parse_prefix()?;

        loop {
            let (op_prec, right_assoc) = self.infix_precedence();
            if op_prec < min_prec {
                break;
            }

            let op = match self.current.clone() {
                Token::Plus => { self.advance(); BinOp::Add }
                Token::Minus => { self.advance(); BinOp::Sub }
                Token::Star => { self.advance(); BinOp::Mul }
                Token::Slash => { self.advance(); BinOp::Div }
                Token::Percent => { self.advance(); BinOp::Mod }
                Token::Eq => { self.advance(); BinOp::Eq }
                Token::Ne => { self.advance(); BinOp::Ne }
                Token::Lt => { self.advance(); BinOp::Lt }
                Token::Le => { self.advance(); BinOp::Le }
                Token::Gt => { self.advance(); BinOp::Gt }
                Token::Ge => { self.advance(); BinOp::Ge }
                Token::And => { self.advance(); BinOp::And }
                Token::Or => { self.advance(); BinOp::Or }
                Token::Pipe => { self.advance(); BinOp::Pipe }
                Token::Cons => { self.advance(); BinOp::Cons }
                _ => break,
            };

            let next_min_prec = if right_assoc { op_prec } else { op_prec + 1 };
            let right = self.parse_expr_prec(next_min_prec)?;
            let span = self.span.clone();
            left = Expr::Binary { op, left: Box::new(left), right: Box::new(right), span };
        }

        Ok(left)
    }

    fn infix_precedence(&self) -> (u8, bool) {
        match self.current {
            Token::Pipe => (1, false),
            Token::Or => (2, false),
            Token::And => (3, false),
            Token::Eq | Token::Ne | Token::Lt | Token::Le | Token::Gt | Token::Ge => (4, false),
            Token::Cons => (5, true),
            Token::Plus | Token::Minus => (6, false),
            Token::Star | Token::Slash | Token::Percent => (7, false),
            Token::Arrow => (8, true),
            _ => (0, false),
        }
    }

    fn parse_prefix(&mut self) -> Result<Expr, ParseError> {
        match self.current.clone() {
            Token::IntLit(n) => { self.advance(); Ok(Expr::Literal(Literal::Int(n), self.span.clone())) }
            Token::FloatLit(f) => { self.advance(); Ok(Expr::Literal(Literal::Float(f), self.span.clone())) }
            Token::StringLit(ref s) => { let s = s.clone(); self.advance(); Ok(Expr::Literal(Literal::String(s), self.span.clone())) }
            Token::BoolLit(b) => { self.advance(); Ok(Expr::Literal(Literal::Bool(b), self.span.clone())) }
            Token::Ident(ref name) if name == "unit" => {
                self.advance();
                Ok(Expr::Literal(Literal::Unit, self.span.clone()))
            }
            Token::Ident(ref name) => {
                let name = name.clone();
                self.advance();
                // Check for function call
                if self.at(&Token::LParen) && !matches!(self.peek(), Some(Token::RParen)) {
                    self.advance(); // (
                    let mut args = Vec::new();
                    while !self.at(&Token::RParen) {
                        args.push(self.parse_expr()?);
                        if self.at(&Token::Comma) {
                            self.advance();
                        }
                    }
                    self.expect(Token::RParen)?;
                    let span = self.span.clone();
                    Ok(Expr::App { func: Box::new(Expr::Var(name, span.clone())), args, span })
                } else {
                    let span = self.span.clone();
                    Ok(Expr::Var(name, span))
                }
            }
            Token::UpperIdent(ref name) => {
                let name = name.clone();
                self.advance();
                let span = self.span.clone();
                if self.at(&Token::LParen) && !matches!(self.peek(), Some(Token::RParen)) {
                    self.advance();
                    let mut args = Vec::new();
                    while !self.at(&Token::RParen) {
                        args.push(self.parse_expr()?);
                        if self.at(&Token::Comma) {
                            self.advance();
                        }
                    }
                    self.expect(Token::RParen)?;
                    Ok(Expr::App { func: Box::new(Expr::Var(name, span.clone())), args, span: self.span.clone() })
                } else {
                    Ok(Expr::Var(name, span))
                }
            }
            Token::Minus => {
                self.advance();
                // Could be negative number or unary negation
                if let Token::IntLit(n) = self.current {
                    self.advance();
                    Ok(Expr::Literal(Literal::Int(-n), self.span.clone()))
                } else if let Token::FloatLit(f) = self.current {
                    self.advance();
                    Ok(Expr::Literal(Literal::Float(-f), self.span.clone()))
                } else {
                    let expr = self.parse_prefix()?;
                    let span = self.span.clone();
                    Ok(Expr::Unary { op: UnOp::Neg, expr: Box::new(expr), span })
                }
            }
            Token::Not => {
                self.advance();
                let expr = self.parse_prefix()?;
                let span = self.span.clone();
                Ok(Expr::Unary { op: UnOp::Not, expr: Box::new(expr), span })
            }
            Token::LParen => {
                self.advance();
                if self.at(&Token::RParen) {
                    self.advance();
                    return Ok(Expr::Literal(Literal::Unit, self.span.clone()));
                }
                let expr = self.parse_expr()?;
                if self.at(&Token::Comma) {
                    let mut elems = vec![expr];
                    while self.at(&Token::Comma) {
                        self.advance();
                        elems.push(self.parse_expr()?);
                    }
                    self.expect(Token::RParen)?;
                    let span = self.span.clone();
                    Ok(Expr::Tuple(elems, span))
                } else {
                    self.expect(Token::RParen)?;
                    Ok(expr)
                }
            }
            Token::LBrace => {
                self.advance();
                let mut exprs = Vec::new();
                while !self.at(&Token::RBrace) {
                    exprs.push(self.parse_expr()?);
                    if self.at(&Token::Semicolon) || self.at(&Token::Newline) {
                        self.advance();
                    }
                }
                self.expect(Token::RBrace)?;
                let span = self.span.clone();
                Ok(Expr::Block { exprs, span })
            }
            Token::LBracket => {
                self.advance();
                let mut elems = Vec::new();
                while !self.at(&Token::RBracket) {
                    elems.push(self.parse_expr()?);
                    if self.at(&Token::Comma) {
                        self.advance();
                    }
                }
                self.expect(Token::RBracket)?;
                let span = self.span.clone();
                Ok(Expr::Array(elems, span))
            }
            Token::Let => self.parse_let(),
            Token::Rec => self.parse_let_rec(),
            Token::If => self.parse_if(),
            Token::Match => self.parse_match(),
            Token::Fn => self.parse_lambda(),
            Token::Fun => self.parse_lambda(),
            Token::Spawn => self.parse_spawn(),
            Token::Ask => self.parse_ask(),
            Token::Perform => self.parse_perform(),
            Token::Handle => self.parse_handle(),
            Token::Try => self.parse_try(),
            Token::Await => self.parse_await(),
            Token::SelfTok => {
                self.advance();
                let span = self.span.clone();
                Ok(Expr::SelfRef(span))
            }
            Token::EffectName(ref name) => {
                let name = name.clone();
                self.advance();
                if self.at(&Token::LParen) {
                    self.advance();
                    let mut args = Vec::new();
                    while !self.at(&Token::RParen) {
                        args.push(self.parse_expr()?);
                        if self.at(&Token::Comma) {
                            self.advance();
                        }
                    }
                    self.expect(Token::RParen)?;
                    let span = self.span.clone();
                    Ok(Expr::App { func: Box::new(Expr::Var(name, span.clone())), args, span })
                } else {
                    let span = self.span.clone();
                    Ok(Expr::Var(name, span))
                }
            }
            _ => Err(ParseError::Unexpected {
                expected: "expression".to_string(),
                found: format!("{:?}", self.current),
                span: self.span.clone(),
            }),
        }
    }

    // -- Specific expression parsers --

    fn parse_let(&mut self) -> Result<Expr, ParseError> {
        self.expect(Token::Let)?;
        let name = match &self.current {
            Token::Ident(n) => { let n = n.clone(); self.advance(); n }
            _ => return Err(ParseError::Unexpected {
                expected: "identifier".to_string(),
                found: format!("{:?}", self.current),
                span: self.span.clone(),
            }),
        };
        self.expect(Token::Assign)?;
        let value = Box::new(self.parse_expr()?);
        self.consume_newlines();
        self.expect(Token::In)?;
        let body = Box::new(self.parse_expr()?);
        let span = self.span.clone();
        Ok(Expr::Let { name, value, body, span })
    }

    fn parse_let_rec(&mut self) -> Result<Expr, ParseError> {
        self.expect(Token::Rec)?;
        let name = match &self.current {
            Token::Ident(n) => { let n = n.clone(); self.advance(); n }
            _ => return Err(ParseError::Unexpected {
                expected: "identifier".to_string(),
                found: format!("{:?}", self.current),
                span: self.span.clone(),
            }),
        };
        // Parse parameters
        let mut params = Vec::new();
        while let Token::Ident(n) = &self.current {
            let n = n.clone();
            self.advance();
            params.push((n, None));
        }
        self.expect(Token::Assign)?;
        let value = Box::new(self.parse_expr()?);
        self.consume_newlines();
        self.expect(Token::In)?;
        let body = Box::new(self.parse_expr()?);
        let span = self.span.clone();
        Ok(Expr::LetRec { name, params, value, body, span })
    }

    fn parse_if(&mut self) -> Result<Expr, ParseError> {
        self.expect(Token::If)?;
        let cond = Box::new(self.parse_expr()?);
        self.consume_newlines();
        self.expect(Token::Then)?;
        let then_branch = Box::new(self.parse_expr()?);
        self.consume_newlines();
        let else_branch = if self.at(&Token::Else) {
            self.advance();
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        let span = self.span.clone();
        Ok(Expr::If { cond, then_branch, else_branch, span })
    }

    fn parse_match(&mut self) -> Result<Expr, ParseError> {
        self.expect(Token::Match)?;
        let scrutinee = Box::new(self.parse_expr()?);
        self.consume_newlines();
        self.expect(Token::With)?;
        self.consume_newlines();
        self.consume_indent();
        let mut arms = Vec::new();
        while !self.at(&Token::Dedent(0)) && !self.at(&Token::Eof) {
            if self.at(&Token::FatArrow) || self.at(&Token::Pipe) {
                // Consume optional pipe
                if self.at(&Token::Pipe) {
                    self.advance();
                }
                // Parse pattern - could be a constructor pattern
                let pat = if let Token::UpperIdent(name) = &self.current {
                    let name = name.clone();
                    self.advance();
                    if self.at(&Token::LParen) {
                        self.advance();
                        let inner = self.parse_pattern()?;
                        self.expect(Token::RParen)?;
                        Pattern::Variant(name, Some(Box::new(inner)))
                    } else {
                        Pattern::Variant(name, None)
                    }
                } else {
                    self.parse_pattern()?
                };
                self.expect(Token::FatArrow)?;
                let body = self.parse_expr()?;
                arms.push((pat, body));
                self.consume_newlines();
            } else if self.at(&Token::Indent(_)) {
                self.consume_indent();
            } else if self.at(&Token::Newline) {
                self.advance();
            } else {
                break;
            }
        }
        self.consume_dedent();
        let span = self.span.clone();
        Ok(Expr::Match { scrutinee, arms, span })
    }

    fn parse_lambda(&mut self) -> Result<Expr, ParseError> {
        if self.at(&Token::Fn) || self.at(&Token::Fun) {
            self.advance();
        }
        self.expect(Token::LParen)?;
        let mut params = Vec::new();
        while !self.at(&Token::RParen) {
            let name = match &self.current {
                Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                _ => return Err(ParseError::Unexpected {
                    expected: "parameter name".to_string(),
                    found: format!("{:?}", self.current),
                    span: self.span.clone(),
                }),
            };
            let ty = if self.at(&Token::Colon) {
                self.advance();
                Some(self.parse_type()?)
            } else {
                None
            };
            params.push((name, ty));
            if self.at(&Token::Comma) {
                self.advance();
            }
        }
        self.expect(Token::RParen)?;
        let effect = if self.at(&Token::Ident(ref s)) if s == "with" => {
            self.advance();
            Some(self.parse_effect_row()?)
        } else {
            None
        };
        self.expect(Token::Arrow)?;
        let body = Box::new(self.parse_expr()?);
        let span = self.span.clone();
        Ok(Expr::Lambda { params, body, effect, span })
    }

    fn parse_spawn(&mut self) -> Result<Expr, ParseError> {
        self.expect(Token::Spawn)?;
        let actor_type = Box::new(self.parse_expr()?);
        self.expect(Token::LBrace)?;
        let mut init = Vec::new();
        while !self.at(&Token::RBrace) {
            let name = match &self.current {
                Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                _ => break,
            };
            self.expect(Token::Colon)?;
            let expr = self.parse_expr()?;
            init.push((name, expr));
            if self.at(&Token::Comma) || self.at(&Token::Newline) || self.at(&Token::Semicolon) {
                self.advance();
            }
        }
        self.expect(Token::RBrace)?;
        let span = self.span.clone();
        Ok(Expr::Spawn { actor_type, init, span })
    }

    fn parse_ask(&mut self) -> Result<Expr, ParseError> {
        self.expect(Token::Ask)?;
        let actor = Box::new(self.parse_expr()?);
        self.expect(Token::Bang)?;
        let behavior = match &self.current {
            Token::Ident(n) => { let n = n.clone(); self.advance(); n }
            _ => return Err(ParseError::Unexpected {
                expected: "behavior name".to_string(),
                found: format!("{:?}", self.current),
                span: self.span.clone(),
            }),
        };
        self.expect(Token::LParen)?;
        let mut args = Vec::new();
        while !self.at(&Token::RParen) {
            args.push(self.parse_expr()?);
            if self.at(&Token::Comma) {
                self.advance();
            }
        }
        self.expect(Token::RParen)?;
        let span = self.span.clone();
        Ok(Expr::Ask { actor, behavior, args, span })
    }

    fn parse_perform(&mut self) -> Result<Expr, ParseError> {
        self.expect(Token::Perform)?;
        let effect = match &self.current {
            Token::EffectName(n) | Token::Ident(n) => { let n = n.clone(); self.advance(); n }
            _ => return Err(ParseError::Unexpected {
                expected: "effect name".to_string(),
                found: format!("{:?}", self.current),
                span: self.span.clone(),
            }),
        };
        self.expect(Token::Dot)?;
        let op = match &self.current {
            Token::Ident(n) => { let n = n.clone(); self.advance(); n }
            _ => return Err(ParseError::Unexpected {
                expected: "operation name".to_string(),
                found: format!("{:?}", self.current),
                span: self.span.clone(),
            }),
        };
        self.expect(Token::LParen)?;
        let mut args = Vec::new();
        while !self.at(&Token::RParen) {
            args.push(self.parse_expr()?);
            if self.at(&Token::Comma) {
                self.advance();
            }
        }
        self.expect(Token::RParen)?;
        let span = self.span.clone();
        Ok(Expr::Perform { effect, op, args, span })
    }

    fn parse_handle(&mut self) -> Result<Expr, ParseError> {
        self.expect(Token::Handle)?;
        let body = Box::new(self.parse_expr()?);
        self.consume_newlines();
        self.expect(Token::With)?;
        self.consume_newlines();
        self.consume_indent();
        let mut handlers = Vec::new();
        while !self.at(&Token::Dedent(0)) && !self.at(&Token::Eof) {
            if self.at(&Token::FatArrow) || self.at(&Token::Pipe) {
                if self.at(&Token::Pipe) {
                    self.advance();
                }
                let effect_name = match &self.current {
                    Token::EffectName(n) | Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                    _ => break,
                };
                self.expect(Token::Dot)?;
                let op_name = match &self.current {
                    Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                    _ => break,
                };
                self.expect(Token::LParen)?;
                let mut handler_params = Vec::new();
                while !self.at(&Token::RParen) {
                    let name = match &self.current {
                        Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                        _ => break,
                    };
                    handler_params.push(name);
                    if self.at(&Token::Comma) {
                        self.advance();
                    }
                }
                self.expect(Token::RParen)?;
                let has_resume = self.at(&Token::Ident(ref s)) if s == "resume" => {
                    self.advance();
                    true
                } else {
                    false
                };
                self.expect(Token::FatArrow)?;
                let handler_body = self.parse_expr()?;
                handlers.push(EffectHandler {
                    effect: effect_name,
                    op: op_name,
                    params: handler_params,
                    body: handler_body,
                    resume: has_resume,
                });
                self.consume_newlines();
            } else if self.at(&Token::Indent(_)) {
                self.consume_indent();
            } else if self.at(&Token::Newline) {
                self.advance();
            } else {
                break;
            }
        }
        self.consume_dedent();
        let span = self.span.clone();
        Ok(Expr::Handle { body, handlers, span })
    }

    fn parse_try(&mut self) -> Result<Expr, ParseError> {
        self.expect(Token::Try)?;
        let body = Box::new(self.parse_expr()?);
        self.consume_newlines();
        self.expect(Token::Catch)?;
        self.consume_newlines();
        self.consume_indent();
        let mut catch_arms = Vec::new();
        while !self.at(&Token::Dedent(0)) && !self.at(&Token::Eof) {
            if self.at(&Token::FatArrow) || self.at(&Token::Pipe) {
                if self.at(&Token::Pipe) {
                    self.advance();
                }
                let pat = self.parse_pattern()?;
                self.expect(Token::FatArrow)?;
                let arm_body = self.parse_expr()?;
                catch_arms.push((pat, arm_body));
                self.consume_newlines();
            } else if self.at(&Token::Newline) {
                self.advance();
            } else {
                break;
            }
        }
        self.consume_dedent();
        let span = self.span.clone();
        Ok(Expr::Try { body, catch_arms, span })
    }

    fn parse_await(&mut self) -> Result<Expr, ParseError> {
        self.expect(Token::Await)?;
        let expr = Box::new(self.parse_expr()?);
        let span = self.span.clone();
        Ok(Expr::Await { expr, span })
    }

    // -- Declaration parsing --

    pub fn parse_decl(&mut self) -> Result<Decl, ParseError> {
        match self.current.clone() {
            Token::Fun => self.parse_fun_decl(),
            Token::Actor => self.parse_actor_decl(),
            Token::Agent => self.parse_agent_decl(),
            Token::Type => self.parse_type_alias(),
            Token::Import => self.parse_import(),
            Token::Module => self.parse_module_decl(),
            _ => Err(ParseError::Unexpected {
                expected: "declaration".to_string(),
                found: format!("{:?}", self.current),
                span: self.span.clone(),
            }),
        }
    }

    fn parse_fun_decl(&mut self) -> Result<Decl, ParseError> {
        self.expect(Token::Fun)?;
        let name = match &self.current {
            Token::Ident(n) | Token::EffectName(n) => { let n = n.clone(); self.advance(); n }
            _ => return Err(ParseError::Unexpected {
                expected: "function name".to_string(),
                found: format!("{:?}", self.current),
                span: self.span.clone(),
            }),
        };
        // Type parameters
        let mut type_params = Vec::new();
        if self.at(&Token::LBracket) {
            self.advance();
            while !self.at(&Token::RBracket) {
                let tp = match &self.current {
                    Token::UpperIdent(n) | Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                    _ => break,
                };
                type_params.push(tp);
                if self.at(&Token::Comma) {
                    self.advance();
                }
            }
            self.expect(Token::RBracket)?;
        }
        self.expect(Token::LParen)?;
        let mut params = Vec::new();
        while !self.at(&Token::RParen) {
            let pname = match &self.current {
                Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                _ => break,
            };
            let pty = if self.at(&Token::Colon) {
                self.advance();
                Some(self.parse_type()?)
            } else {
                None
            };
            params.push((pname, pty));
            if self.at(&Token::Comma) {
                self.advance();
            }
        }
        self.expect(Token::RParen)?;
        let ret_type = if self.at(&Token::Arrow) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };
        let effect = if self.at(&Token::Colon) {
            self.advance();
            if self.at(&Token::LBrace) {
                self.advance();
                let row = self.parse_effect_row()?;
                self.expect(Token::RBrace)?;
                Some(row)
            } else {
                None
            }
        } else {
            None
        };
        self.expect(Token::Assign)?;
        let body = self.parse_expr()?;
        let span = self.span.clone();
        Ok(Decl::Fun { name, type_params, params, ret_type, effect, body, span })
    }

    fn parse_actor_decl(&mut self) -> Result<Decl, ParseError> {
        self.expect(Token::Actor)?;
        let name = match &self.current {
            Token::Ident(n) | Token::UpperIdent(n) => { let n = n.clone(); self.advance(); n }
            _ => return Err(ParseError::Unexpected {
                expected: "actor name".to_string(),
                found: format!("{:?}", self.current),
                span: self.span.clone(),
            }),
        };
        // Type parameters
        let mut type_params = Vec::new();
        if self.at(&Token::LBracket) {
            self.advance();
            while !self.at(&Token::RBracket) {
                let tp = match &self.current {
                    Token::UpperIdent(n) | Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                    _ => break,
                };
                type_params.push(tp);
                if self.at(&Token::Comma) {
                    self.advance();
                }
            }
            self.expect(Token::RBracket)?;
        }
        self.expect(Token::LBrace)?;
        self.consume_newlines();
        let mut fields = Vec::new();
        let mut behaviors = Vec::new();
        let mut initial_behaviour = String::new();
        while !self.at(&Token::RBrace) && !self.at(&Token::Eof) {
            if self.at(&Token::State) {
                self.advance();
                while !self.at(&Token::Behavior) && !self.at(&Token::Initial) && !self.at(&Token::RBrace) {
                    let fname = match &self.current {
                        Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                        _ => break,
                    };
                    self.expect(Token::Colon)?;
                    let fty = self.parse_type()?;
                    fields.push((fname, fty));
                    if self.at(&Token::Comma) || self.at(&Token::Newline) || self.at(&Token::Semicolon) {
                        self.advance();
                    }
                }
            } else if self.at(&Token::Initial) {
                self.advance();
                initial_behaviour = match &self.current {
                    Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                    _ => String::new(),
                };
                self.consume_newlines();
            } else if self.at(&Token::Behavior) {
                self.advance();
                let bname = match &self.current {
                    Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                    _ => break,
                };
                self.expect(Token::LParen)?;
                let mut bparams = Vec::new();
                while !self.at(&Token::RParen) {
                    let pname = match &self.current {
                        Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                        _ => break,
                    };
                    let pty = if self.at(&Token::Colon) {
                        self.advance();
                        Some(self.parse_type()?)
                    } else {
                        None
                    };
                    bparams.push((pname, pty.unwrap_or(Type::Var(0))));
                    if self.at(&Token::Comma) {
                        self.advance();
                    }
                }
                self.expect(Token::RParen)?;
                self.expect(Token::Assign)?;
                self.consume_newlines();
                self.consume_indent();
                let bbody = self.parse_expr()?;
                self.consume_dedent();
                let effect_annotation = if self.at(&Token::Colon) {
                    self.advance();
                    if self.at(&Token::LBrace) {
                        self.advance();
                        let row = self.parse_effect_row()?;
                        self.expect(Token::RBrace)?;
                        Some(row)
                    } else {
                        None
                    }
                } else {
                    None
                };
                behaviors.push(Behavior {
                    name: bname,
                    params: bparams,
                    body: bbody,
                    effect_annotation,
                });
                self.consume_newlines();
            } else if self.at(&Token::Newline) || self.at(&Token::Semicolon) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(Token::RBrace)?;
        let span = self.span.clone();
        if initial_behaviour.is_empty() && !behaviors.is_empty() {
            initial_behaviour = behaviors[0].name.clone();
        }
        Ok(Decl::Actor {
            def: ActorDef {
                name,
                type_params,
                fields,
                behaviors,
                initial_behaviour,
            },
            span,
        })
    }

    fn parse_agent_decl(&mut self) -> Result<Decl, ParseError> {
        self.expect(Token::Agent)?;
        let name = match &self.current {
            Token::Ident(n) | Token::UpperIdent(n) => { let n = n.clone(); self.advance(); n }
            _ => return Err(ParseError::Unexpected {
                expected: "agent name".to_string(),
                found: format!("{:?}", self.current),
                span: self.span.clone(),
            }),
        };
        self.expect(Token::LBrace)?;
        self.consume_newlines();
        let mut fields = Vec::new();
        let mut behaviors = Vec::new();
        let mut llm_config = None::<LlmConfig>;
        let mut tool_bindings = Vec::new();
        while !self.at(&Token::RBrace) && !self.at(&Token::Eof) {
            if self.at(&Token::Llm) {
                self.advance();
                let model = match &self.current {
                    Token::StringLit(s) => { let s = s.clone(); self.advance(); s }
                    Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                    _ => "gpt-4".to_string(),
                };
                self.expect(Token::Comma)?;
                let system_prompt = match &self.current {
                    Token::StringLit(s) => { let s = s.clone(); self.advance(); s }
                    _ => String::new(),
                };
                self.expect(Token::Comma)?;
                let temperature = match &self.current {
                    Token::FloatLit(f) => { let f = *f; self.advance(); f }
                    Token::IntLit(n) => { let n = *n; self.advance(); n as f64 }
                    _ => 0.7,
                };
                llm_config = Some(LlmConfig { model, system_prompt, temperature });
                self.consume_newlines();
            } else if self.at(&Token::Ident(ref s)) if s == "tool" => {
                self.advance();
                let tname = match &self.current {
                    Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                    _ => break,
                };
                self.expect(Token::Colon)?;
                let effect_name = match &self.current {
                    Token::EffectName(n) | Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                    _ => break,
                };
                let description = if self.at(&Token::Comma) {
                    self.advance();
                    match &self.current {
                        Token::StringLit(s) => { let s = s.clone(); self.advance(); s }
                        _ => String::new(),
                    }
                } else {
                    String::new()
                };
                tool_bindings.push(ToolBinding {
                    name: tname,
                    effect: effect_name,
                    description,
                });
                self.consume_newlines();
            } else if self.at(&Token::State) {
                self.advance();
                while !self.at(&Token::Behavior) && !self.at(&Token::RBrace) {
                    let fname = match &self.current {
                        Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                        _ => break,
                    };
                    self.expect(Token::Colon)?;
                    let fty = self.parse_type()?;
                    fields.push((fname, fty));
                    if self.at(&Token::Comma) || self.at(&Token::Newline) || self.at(&Token::Semicolon) {
                        self.advance();
                    }
                }
            } else if self.at(&Token::Behavior) {
                self.advance();
                let bname = match &self.current {
                    Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                    _ => break,
                };
                self.expect(Token::LParen)?;
                let mut bparams = Vec::new();
                while !self.at(&Token::RParen) {
                    let pname = match &self.current {
                        Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                        _ => break,
                    };
                    let pty = if self.at(&Token::Colon) {
                        self.advance();
                        Some(self.parse_type()?)
                    } else {
                        None
                    };
                    bparams.push((pname, pty.unwrap_or(Type::Var(0))));
                    if self.at(&Token::Comma) {
                        self.advance();
                    }
                }
                self.expect(Token::RParen)?;
                self.expect(Token::Assign)?;
                self.consume_newlines();
                self.consume_indent();
                let bbody = self.parse_expr()?;
                self.consume_dedent();
                behaviors.push(Behavior {
                    name: bname,
                    params: bparams,
                    body: bbody,
                    effect_annotation: None,
                });
                self.consume_newlines();
            } else if self.at(&Token::Newline) || self.at(&Token::Semicolon) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(Token::RBrace)?;
        let span = self.span.clone();
        Ok(Decl::Agent {
            def: AgentDef {
                name,
                fields,
                behaviors,
                llm_config,
                tool_bindings,
            },
            span,
        })
    }

    fn parse_type_alias(&mut self) -> Result<Decl, ParseError> {
        self.expect(Token::Type)?;
        let name = match &self.current {
            Token::UpperIdent(n) | Token::Ident(n) => { let n = n.clone(); self.advance(); n }
            _ => return Err(ParseError::Unexpected {
                expected: "type name".to_string(),
                found: format!("{:?}", self.current),
                span: self.span.clone(),
            }),
        };
        let mut params = Vec::new();
        if self.at(&Token::LBracket) {
            self.advance();
            while !self.at(&Token::RBracket) {
                let p = match &self.current {
                    Token::UpperIdent(n) | Token::Ident(n) => { let n = n.clone(); self.advance(); n }
                    _ => break,
                };
                params.push(p);
                if self.at(&Token::Comma) {
                    self.advance();
                }
            }
            self.expect(Token::RBracket)?;
        }
        self.expect(Token::Assign)?;
        let body = self.parse_type()?;
        let span = self.span.clone();
        Ok(Decl::TypeAlias { name, params, body, span })
    }

    fn parse_import(&mut self) -> Result<Decl, ParseError> {
        self.expect(Token::Import)?;
        let path = match &self.current {
            Token::StringLit(s) => { let s = s.clone(); self.advance(); s }
            Token::Ident(n) | Token::UpperIdent(n) => { let n = n.clone(); self.advance(); n }
            _ => return Err(ParseError::Unexpected {
                expected: "import path".to_string(),
                found: format!("{:?}", self.current),
                span: self.span.clone(),
            }),
        };
        let mut names = Vec::new();
        if self.at(&Token::Colon) {
            self.advance();
            self.expect(Token::LParen)?;
            while !self.at(&Token::RParen) {
                let name = match &self.current {
                    Token::Ident(n) | Token::UpperIdent(n) => { let n = n.clone(); self.advance(); n }
                    _ => break,
                };
                names.push(name);
                if self.at(&Token::Comma) {
                    self.advance();
                }
            }
            self.expect(Token::RParen)?;
        }
        let span = self.span.clone();
        Ok(Decl::Import { path, names, span })
    }

    fn parse_module_decl(&mut self) -> Result<Decl, ParseError> {
        self.expect(Token::Module)?;
        let name = match &self.current {
            Token::UpperIdent(n) | Token::Ident(n) => { let n = n.clone(); self.advance(); n }
            _ => return Err(ParseError::Unexpected {
                expected: "module name".to_string(),
                found: format!("{:?}", self.current),
                span: self.span.clone(),
            }),
        };
        self.consume_newlines();
        self.consume_indent();
        let mut decls = Vec::new();
        while !self.at(&Token::Dedent(0)) && !self.at(&Token::Eof) {
            decls.push(self.parse_decl()?);
            self.consume_newlines();
        }
        self.consume_dedent();
        let span = self.span.clone();
        Ok(Decl::Module { name, decls, span })
    }

    // -- Top-level module --

    pub fn parse_module(&mut self) -> Result<ast::Module, ParseError> {
        let mut decls = Vec::new();
        while !self.at(&Token::Eof) {
            self.consume_newlines();
            if self.at(&Token::Eof) {
                break;
            }
            decls.push(self.parse_decl()?);
            self.consume_newlines();
        }
        Ok(ast::Module {
            name: "main".to_string(),
            decls,
            span: self.span.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Parse error
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ParseError {
    Unexpected { expected: String, found: String, span: Span },
    Unterminated { what: String, span: Span },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Unexpected { expected, found, span } => {
                write!(f, "Parse error at line {} col {}: expected {}, found {}",
                    span.line, span.col, expected, found)
            }
            ParseError::Unterminated { what, span } => {
                write!(f, "Parse error at line {} col {}: unterminated {}",
                    span.line, span.col, what)
            }
        }
    }
}

impl std::error::Error for ParseError {}

// ---------------------------------------------------------------------------
// Convenience functions
// ---------------------------------------------------------------------------

pub fn parse(input: &str) -> Result<ast::Module, ParseError> {
    let mut parser = Parser::new(input);
    parser.parse_module()
}

pub fn parse_expr(input: &str) -> Result<Expr, ParseError> {
    let mut parser = Parser::new(input);
    parser.parse_expr()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_literal() {
        let result = parse_expr("42");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Expr::Literal(Literal::Int(42), Span::default()));
    }

    #[test]
    fn test_parse_let() {
        let result = parse_expr("let x = 42 in x + 1");
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_function() {
        let result = parse("fun add(x, y) = x + y\n");
        assert!(result.is_ok());
        let m = result.unwrap();
        assert_eq!(m.decls.len(), 1);
    }

    #[test]
    fn test_parse_actor() {
        let input = r#"
actor Counter {
  state count: Int
  initial init

  behavior init() =
    receive
    | Tick =>
        self ! count(count + 1)
    | Get =>
        count
}
"#;
        let result = parse(input);
        assert!(result.is_ok(), "Parse failed: {:?}", result.err());
    }

    #[test]
    fn test_parse_lambda() {
        let result = parse_expr("fn(x) -> x + 1");
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_match() {
        let result = parse_expr("match x with\n  | Some(a) => a\n  | None => 0\n");
        assert!(result.is_ok());
    }
}
