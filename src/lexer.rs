//! Lexical analyzer for Nulang.
//!
//! Hand-written state machine. Single-pass over input source.

use crate::types::{NuError, NuResult, Span};

// ---------------------------------------------------------------------------
// Token Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Literals
    IntLit(i64),
    FloatLit(f64),
    StringLit(String),
    BoolLit(bool),
    UnitLit,

    // Keywords
    Fn, Let, Rec, In, If, Else, Match, With,
    Actor, Agent, Behavior, State, SelfKw, Spawn, Send, Ask,
    Effect, Perform, Handle, Resume,
    Module, Import, Pub, Priv, Where,
    Migrate, Node, Monitor, Link, Exit,
    For, Loop, Break, Return,
    Type, Alias,
    Iso, Trn, Ref, Val, Box, Tag,
    True, False, Unit,

    // Identifiers
    Ident(String),
    UpperIdent(String), // Type/actor/agent names (convention)

    // Operators
    Plus, Minus, Star, Slash, Percent,       // + - * / %
    Eq, Ne, Lt, Le, Gt, Ge,                 // == != < <= > >=
    And, Or, Not,                            // && || !
    Ampersand, Pipe, PipeOp, Caret, Tilde,  // & | |> ^ ~
    Shl, Shr,                               // << >>
    Assign, PlusAssign, MinusAssign,        // = += -=
    Arrow, FatArrow, ThinArrow,             // -> => <-
    Dot, DotDot, Colon, DoubleColon,        // . .. : ::
    At,                                     // @
    Bang, Question,                         // ! ?

    // Delimiters
    LParen, RParen,                         // ( )
    LBrace, RBrace,                         // { }
    LBracket, RBracket,                     // [ ]
    Comma, Semicolon,                       // , ;

    // Special
    Newline,
    Comment(String),
    DocComment(String),
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

pub struct Lexer<'a> {
    source: &'a str,
    bytes: &'a [u8],
    pos: usize,
    line: usize,
    column: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Lexer {
            source,
            bytes: source.as_bytes(),
            pos: 0,
            line: 1,
            column: 1,
        }
    }

    /// Run the lexer, producing a vector of tokens. Newlines are preserved
    /// for semicolon insertion logic but comments are filtered out.
    pub fn lex(&mut self) -> NuResult<Vec<Token>> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token()?;
            match tok.kind {
                TokenKind::Eof => {
                    tokens.push(tok);
                    break;
                }
                // Filter out regular comments, keep doc comments as tokens
                TokenKind::Comment(_) => {
                    // skip regular comments
                }
                _ => tokens.push(tok),
            }
        }
        Ok(tokens)
    }

    // --- Core tokenizer ---

    fn next_token(&mut self) -> NuResult<Token> {
        self.skip_whitespace();

        let start = self.pos;
        let start_line = self.line;
        let start_col = self.column;

        let ch = match self.peek() {
            Some(c) => c,
            None => {
                return Ok(Token {
                    kind: TokenKind::Eof,
                    span: Span::new(start, start, start_line, start_col),
                })
            }
        };

        let tok = match ch {
            b'\n' | b'\r' => {
                self.advance();
                if ch == b'\r' && self.peek() == Some(b'\n') {
                    self.advance();
                }
                Token {
                    kind: TokenKind::Newline,
                    span: Span::new(start, self.pos, start_line, start_col),
                }
            }
            b'/' => {
                // Could be comment or division operator
                let next = self.bytes.get(self.pos + 1);
                if next == Some(&b'/') || next == Some(&b'*') {
                    self.read_comment()
                } else {
                    self.read_operator()?
                }
            }
            b'a'..=b'z' | b'_' => self.read_identifier(),
            b'A'..=b'Z' => self.read_identifier(),
            b'0'..=b'9' => self.read_number()?,
            b'"' => self.read_string()?,
            b'+' | b'-' | b'*' | b'%' | b'=' | b'!' | b'<' | b'>' | b'&' | b'|'
            | b'^' | b'~' | b'.' | b':' | b'#' => self.read_operator()?,
            b'(' => {
                self.advance();
                Token {
                    kind: TokenKind::LParen,
                    span: Span::new(start, self.pos, start_line, start_col),
                }
            }
            b')' => {
                self.advance();
                Token {
                    kind: TokenKind::RParen,
                    span: Span::new(start, self.pos, start_line, start_col),
                }
            }
            b'{' => {
                self.advance();
                Token {
                    kind: TokenKind::LBrace,
                    span: Span::new(start, self.pos, start_line, start_col),
                }
            }
            b'}' => {
                self.advance();
                Token {
                    kind: TokenKind::RBrace,
                    span: Span::new(start, self.pos, start_line, start_col),
                }
            }
            b'[' => {
                self.advance();
                Token {
                    kind: TokenKind::LBracket,
                    span: Span::new(start, self.pos, start_line, start_col),
                }
            }
            b']' => {
                self.advance();
                Token {
                    kind: TokenKind::RBracket,
                    span: Span::new(start, self.pos, start_line, start_col),
                }
            }
            b',' => {
                self.advance();
                Token {
                    kind: TokenKind::Comma,
                    span: Span::new(start, self.pos, start_line, start_col),
                }
            }
            b';' => {
                self.advance();
                Token {
                    kind: TokenKind::Semicolon,
                    span: Span::new(start, self.pos, start_line, start_col),
                }
            }
            _ => {
                return Err(NuError::LexError {
                    msg: format!("Unexpected character: '{}' (byte {})", ch as char, ch),
                    span: Span::new(start, start + 1, start_line, start_col),
                })
            }
        };
        Ok(tok)
    }

    // --- Helper methods ---

    fn skip_whitespace(&mut self) {
        loop {
            match self.peek() {
                Some(b' ') | Some(b'\t') => {
                    self.advance();
                }
                _ => break,
            }
        }
    }

    fn read_identifier(&mut self) -> Token {
        let start = self.pos;
        let start_line = self.line;
        let start_col = self.column;

        // First char: lowercase, uppercase, or underscore
        self.advance(); // consume first char

        // Rest: alphanumeric or underscore
        while let Some(ch) = self.peek() {
            if ch.is_ascii_alphanumeric() || ch == b'_' {
                self.advance();
            } else {
                break;
            }
        }

        let text = &self.source[start..self.pos];

        // Check if it's a keyword
        if let Some(kw) = keyword(text) {
            Token {
                kind: kw,
                span: Span::new(start, self.pos, start_line, start_col),
            }
        } else if text.starts_with(|c: char| c.is_ascii_uppercase()) {
            Token {
                kind: TokenKind::UpperIdent(text.to_string()),
                span: Span::new(start, self.pos, start_line, start_col),
            }
        } else {
            Token {
                kind: TokenKind::Ident(text.to_string()),
                span: Span::new(start, self.pos, start_line, start_col),
            }
        }
    }

    fn read_number(&mut self) -> NuResult<Token> {
        let start = self.pos;
        let start_line = self.line;
        let start_col = self.column;

        // Check for hex prefix
        if self.peek() == Some(b'0') {
            let next = self.bytes.get(self.pos + 1);
            if next == Some(&b'x') || next == Some(&b'X') {
                self.advance(); // '0'
                self.advance(); // 'x' or 'X'
                let hex_start = self.pos;
                while let Some(ch) = self.peek() {
                    if ch.is_ascii_hexdigit() {
                        self.advance();
                    } else {
                        break;
                    }
                }
                if self.pos == hex_start {
                    return Err(NuError::LexError {
                        msg: "Expected hex digits after 0x".to_string(),
                        span: Span::new(start, self.pos, start_line, start_col),
                    });
                }
                let hex_str = &self.source[hex_start..self.pos];
                let val = i64::from_str_radix(hex_str, 16).map_err(|_| NuError::LexError {
                    msg: format!("Invalid hex number: {}", hex_str),
                    span: Span::new(start, self.pos, start_line, start_col),
                })?;
                return Ok(Token {
                    kind: TokenKind::IntLit(val),
                    span: Span::new(start, self.pos, start_line, start_col),
                });
            }
        }

        // Read integer part
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                self.advance();
            } else {
                break;
            }
        }

        // Check for float: either .fraction or exponent
        let mut is_float = false;
        let _int_end = self.pos;

        // Fractional part
        if self.peek() == Some(b'.') {
            // Make sure it's not followed by another dot (range operator)
            let after_dot = self.bytes.get(self.pos + 1);
            if after_dot.map_or(false, |c| c.is_ascii_digit()) {
                is_float = true;
                self.advance(); // '.'
                while let Some(ch) = self.peek() {
                    if ch.is_ascii_digit() {
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
        }

        // Exponent part
        if self.peek() == Some(b'e') || self.peek() == Some(b'E') {
            is_float = true;
            self.advance(); // e or E
            if self.peek() == Some(b'+') || self.peek() == Some(b'-') {
                self.advance();
            }
            let exp_start = self.pos;
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() {
                    self.advance();
                } else {
                    break;
                }
            }
            if self.pos == exp_start {
                return Err(NuError::LexError {
                    msg: "Expected digits in exponent".to_string(),
                    span: Span::new(start, self.pos, start_line, start_col),
                });
            }
        }

        let num_str = &self.source[start..self.pos];

        if is_float {
            let val = num_str.parse::<f64>().map_err(|_| NuError::LexError {
                msg: format!("Invalid float literal: {}", num_str),
                span: Span::new(start, self.pos, start_line, start_col),
            })?;
            Ok(Token {
                kind: TokenKind::FloatLit(val),
                span: Span::new(start, self.pos, start_line, start_col),
            })
        } else {
            let val = num_str.parse::<i64>().map_err(|_| NuError::LexError {
                msg: format!("Invalid integer literal: {}", num_str),
                span: Span::new(start, self.pos, start_line, start_col),
            })?;
            Ok(Token {
                kind: TokenKind::IntLit(val),
                span: Span::new(start, self.pos, start_line, start_col),
            })
        }
    }

    fn read_string(&mut self) -> NuResult<Token> {
        let start = self.pos;
        let start_line = self.line;
        let start_col = self.column;

        self.advance(); // opening "

        let mut result = String::new();
        loop {
            match self.peek() {
                Some(b'"') => {
                    self.advance(); // closing "
                    break;
                }
                Some(b'\\') => {
                    self.advance(); // backslash
                    match self.advance() {
                        Some(b'n') => result.push('\n'),
                        Some(b't') => result.push('\t'),
                        Some(b'\\') => result.push('\\'),
                        Some(b'"') => result.push('"'),
                        Some(b'r') => result.push('\r'),
                        Some(b'0') => result.push('\0'),
                        Some(other) => {
                            return Err(NuError::LexError {
                                msg: format!("Unknown escape sequence: \\\\{}", other as char),
                                span: Span::new(self.pos - 1, self.pos, self.line, self.column),
                            })
                        }
                        None => {
                            return Err(NuError::LexError {
                                msg: "Unterminated string escape".to_string(),
                                span: Span::new(start, self.pos, start_line, start_col),
                            })
                        }
                    }
                }
                Some(ch) => {
                    result.push(ch as char);
                    self.advance();
                }
                None => {
                    return Err(NuError::LexError {
                        msg: "Unterminated string literal".to_string(),
                        span: Span::new(start, self.pos, start_line, start_col),
                    })
                }
            }
        }

        Ok(Token {
            kind: TokenKind::StringLit(result),
            span: Span::new(start, self.pos, start_line, start_col),
        })
    }

    fn read_comment(&mut self) -> Token {
        let start = self.pos;
        let start_line = self.line;
        let start_col = self.column;

        self.advance(); // first '/'
        let kind = self.advance().unwrap_or(b' ');

        match kind {
            b'/' => {
                // Line comment: // or ///
                let is_doc = self.peek() == Some(b'/');
                if is_doc {
                    self.advance(); // third '/'
                }

                let content_start = self.pos;
                while let Some(ch) = self.peek() {
                    if ch == b'\n' || ch == b'\r' {
                        break;
                    }
                    self.advance();
                }
                let content = self.source[content_start..self.pos].to_string();
                if is_doc {
                    Token {
                        kind: TokenKind::DocComment(content),
                        span: Span::new(start, self.pos, start_line, start_col),
                    }
                } else {
                    Token {
                        kind: TokenKind::Comment(content),
                        span: Span::new(start, self.pos, start_line, start_col),
                    }
                }
            }
            b'*' => {
                // Block comment: /* ... */
                let content_start = self.pos;
                let mut depth = 1;
                while depth > 0 {
                    match self.peek() {
                        Some(b'*') => {
                            self.advance();
                            if self.peek() == Some(b'/') {
                                self.advance();
                                depth -= 1;
                            }
                        }
                        Some(b'/') => {
                            self.advance();
                            if self.peek() == Some(b'*') {
                                self.advance();
                                depth += 1;
                            }
                        }
                        Some(_) => {
                            self.advance();
                        }
                        None => break,
                    }
                }
                let content = self.source[content_start..self.pos.saturating_sub(2)].to_string();
                Token {
                    kind: TokenKind::Comment(content),
                    span: Span::new(start, self.pos, start_line, start_col),
                }
            }
            _ => unreachable!(),
        }
    }

    fn read_operator(&mut self) -> NuResult<Token> {
        let start = self.pos;
        let start_line = self.line;
        let start_col = self.column;

        let ch = self.advance().unwrap_or(b'\0');

        let kind = match ch {
            b'+' => {
                if self.match_char(b'=') {
                    TokenKind::PlusAssign
                } else {
                    TokenKind::Plus
                }
            }
            b'-' => {
                if self.match_char(b'>') {
                    TokenKind::Arrow
                } else if self.match_char(b'=') {
                    TokenKind::MinusAssign
                } else {
                    TokenKind::Minus
                }
            }
            b'*' => TokenKind::Star,
            b'/' => TokenKind::Slash,
            b'%' => TokenKind::Percent,
            b'=' => {
                if self.match_char(b'=') {
                    TokenKind::Eq
                } else if self.match_char(b'>') {
                    TokenKind::FatArrow
                } else {
                    TokenKind::Assign
                }
            }
            b'!' => {
                if self.match_char(b'=') {
                    TokenKind::Ne
                } else {
                    TokenKind::Bang
                }
            }
            b'<' => {
                if self.match_char(b'=') {
                    TokenKind::Le
                } else if self.match_char(b'<') {
                    TokenKind::Shl
                } else if self.match_char(b'-') {
                    TokenKind::ThinArrow
                } else {
                    TokenKind::Lt
                }
            }
            b'>' => {
                if self.match_char(b'=') {
                    TokenKind::Ge
                } else if self.match_char(b'>') {
                    TokenKind::Shr
                } else {
                    TokenKind::Gt
                }
            }
            b'&' => {
                if self.match_char(b'&') {
                    TokenKind::And
                } else {
                    TokenKind::Ampersand
                }
            }
            b'|' => {
                if self.match_char(b'|') {
                    TokenKind::Or
                } else if self.match_char(b'>') {
                    TokenKind::PipeOp
                } else {
                    TokenKind::Pipe
                }
            }
            b'^' => TokenKind::Caret,
            b'~' => TokenKind::Tilde,
            b'.' => {
                if self.match_char(b'.') {
                    TokenKind::DotDot
                } else {
                    TokenKind::Dot
                }
            }
            b':' => {
                if self.match_char(b':') {
                    TokenKind::DoubleColon
                } else {
                    TokenKind::Colon
                }
            }
            _ => {
                return Err(NuError::LexError {
                    msg: format!("Unexpected operator character: {}", ch as char),
                    span: Span::new(start, self.pos, start_line, start_col),
                })
            }
        };

        Ok(Token {
            kind,
            span: Span::new(start, self.pos, start_line, start_col),
        })
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<u8> {
        let ch = self.bytes.get(self.pos).copied();
        if let Some(c) = ch {
            self.pos += 1;
            if c == b'\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
        }
        ch
    }

    fn match_char(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn span_at(&self, start: usize) -> Span {
        Span::new(start, self.pos, self.line, self.column)
    }
}

// ---------------------------------------------------------------------------
// Keywords lookup
// ---------------------------------------------------------------------------

fn keyword(s: &str) -> Option<TokenKind> {
    match s {
        "fn" => Some(TokenKind::Fn),
        "let" => Some(TokenKind::Let),
        "rec" => Some(TokenKind::Rec),
        "in" => Some(TokenKind::In),
        "if" => Some(TokenKind::If),
        "else" => Some(TokenKind::Else),
        "match" => Some(TokenKind::Match),
        "with" => Some(TokenKind::With),
        "actor" => Some(TokenKind::Actor),
        "agent" => Some(TokenKind::Agent),
        "behavior" => Some(TokenKind::Behavior),
        "state" => Some(TokenKind::State),
        "self" => Some(TokenKind::SelfKw),
        "spawn" => Some(TokenKind::Spawn),
        "send" => Some(TokenKind::Send),
        "ask" => Some(TokenKind::Ask),
        "effect" => Some(TokenKind::Effect),
        "perform" => Some(TokenKind::Perform),
        "handle" => Some(TokenKind::Handle),
        "resume" => Some(TokenKind::Resume),
        "module" => Some(TokenKind::Module),
        "import" => Some(TokenKind::Import),
        "pub" => Some(TokenKind::Pub),
        "priv" => Some(TokenKind::Priv),
        "where" => Some(TokenKind::Where),
        "migrate" => Some(TokenKind::Migrate),
        "node" => Some(TokenKind::Node),
        "monitor" => Some(TokenKind::Monitor),
        "link" => Some(TokenKind::Link),
        "exit" => Some(TokenKind::Exit),
        "for" => Some(TokenKind::For),
        "loop" => Some(TokenKind::Loop),
        "break" => Some(TokenKind::Break),
        "return" => Some(TokenKind::Return),
        "type" => Some(TokenKind::Type),
        "alias" => Some(TokenKind::Alias),
        "iso" => Some(TokenKind::Iso),
        "trn" => Some(TokenKind::Trn),
        "ref" => Some(TokenKind::Ref),
        "val" => Some(TokenKind::Val),
        "box" => Some(TokenKind::Box),
        "tag" => Some(TokenKind::Tag),
        "true" => Some(TokenKind::BoolLit(true)),
        "false" => Some(TokenKind::BoolLit(false)),
        "unit" => Some(TokenKind::UnitLit),
        _ => None,
    }
}
