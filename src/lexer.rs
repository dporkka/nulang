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
    NilLit,
    UnitLit,

    // Keywords
    Fn,
    Let,
    Rec,
    In,
    If,
    Then,
    Else,
    Match,
    With,
    Case,
    Actor,
    Behavior,
    State,
    StateMachine,
    SelfKw,
    Spawn,
    Send,
    Ask,
    Persistent,
    Local,
    Durable,
    EventSourced,
    Crdt,
    Emit,
    Workflow,
    Step,
    Parallel,
    Compensate,
    Await,
    Subworkflow,
    Agent,
    Database,
    Receive,
    Effect,
    Perform,
    Handle,
    Resume,
    Extern,
    Module,
    Import,
    Pub,
    Priv,
    Where,
    Migrate,
    Node,
    Monitor,
    Link,
    Exit,
    For,
    Loop,
    Break,
    Return,
    Type,
    Alias,
    Iso,
    Trn,
    Ref,
    Val,
    Box,
    Tag,
    True,
    False,
    Unit,
    Tool,

    // Identifiers
    Ident(String),
    UpperIdent(String), // Type/actor names (convention)

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent, // + - * / %
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge, // == != < <= > >=
    And,
    Or,
    Not, // && || !
    Ampersand,
    Pipe,
    PipeOp,
    Pipe3,
    Caret,
    Tilde, // & | |> ||| ^ ~
    Shl,
    Shr, // << >>
    Assign,
    PlusAssign,
    MinusAssign, // = += -=
    Arrow,
    FatArrow,
    ThinArrow, // -> => <-
    Dot,
    DotDot,
    Colon,
    DoubleColon, // . .. : ::
    At,          // @
    Bang,
    Question, // ! ?

    // Delimiters
    LParen,
    RParen, // ( )
    LBrace,
    RBrace, // { }
    LBracket,
    RBracket, // [ ]
    Comma,
    Semicolon, // , ;

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
            b'+' | b'-' | b'*' | b'%' | b'=' | b'!' | b'<' | b'>' | b'&' | b'|' | b'^' | b'~'
            | b'.' | b':' | b'#' => self.read_operator()?,
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
            b'@' => {
                self.advance();
                Token {
                    kind: TokenKind::At,
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
                    if ch < 0x80 {
                        result.push(ch as char);
                        self.advance();
                    } else {
                        // Multi-byte UTF-8 sequence: decode the whole char and
                        // advance over all of its bytes. `self.pos` is on a
                        // char boundary here because the source is valid UTF-8
                        // and every prior step consumed whole chars.
                        match self.source[self.pos..].chars().next() {
                            Some(c) => {
                                result.push(c);
                                for _ in 0..c.len_utf8() {
                                    self.advance();
                                }
                            }
                            None => {
                                return Err(NuError::LexError {
                                    msg: "Unterminated string literal".to_string(),
                                    span: Span::new(start, self.pos, start_line, start_col),
                                })
                            }
                        }
                    }
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
                    if self.match_char(b'|') {
                        TokenKind::Pipe3
                    } else {
                        TokenKind::Or
                    }
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
        "then" => Some(TokenKind::Then),
        "else" => Some(TokenKind::Else),
        "match" => Some(TokenKind::Match),
        "with" => Some(TokenKind::With),
        "case" => Some(TokenKind::Case),
        "actor" => Some(TokenKind::Actor),
        "behavior" => Some(TokenKind::Behavior),
        "state" => Some(TokenKind::State),
        "state_machine" => Some(TokenKind::StateMachine),
        "persistent" => Some(TokenKind::Persistent),
        "local" => Some(TokenKind::Local),
        "durable" => Some(TokenKind::Durable),
        "event_sourced" => Some(TokenKind::EventSourced),
        "crdt" => Some(TokenKind::Crdt),
        "emit" => Some(TokenKind::Emit),
        "workflow" => Some(TokenKind::Workflow),
        "step" => Some(TokenKind::Step),
        "parallel" => Some(TokenKind::Parallel),
        "compensate" => Some(TokenKind::Compensate),
        "await" => Some(TokenKind::Await),
        "subworkflow" => Some(TokenKind::Subworkflow),
        "self" => Some(TokenKind::SelfKw),
        "spawn" => Some(TokenKind::Spawn),
        "send" => Some(TokenKind::Send),
        "ask" => Some(TokenKind::Ask),
        "effect" => Some(TokenKind::Effect),
        "perform" => Some(TokenKind::Perform),
        "handle" => Some(TokenKind::Handle),
        "resume" => Some(TokenKind::Resume),
        "extern" => Some(TokenKind::Extern),
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
        "nil" => Some(TokenKind::NilLit),
        "and" => Some(TokenKind::And),
        "or" => Some(TokenKind::Or),
        "not" => Some(TokenKind::Not),
        "unit" => Some(TokenKind::UnitLit),
        "tool" => Some(TokenKind::Tool),
        "database" => Some(TokenKind::Database),
        "agent" => Some(TokenKind::Agent),
        "receive" => Some(TokenKind::Receive),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(source: &str) -> Vec<TokenKind> {
        let mut lexer = Lexer::new(source);
        lexer.lex().unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn test_simple_tokens() {
        let kinds = lex("let x = 42 in x + 1");
        assert_eq!(
            kinds,
            vec![
                TokenKind::Let,
                TokenKind::Ident("x".to_string()),
                TokenKind::Assign,
                TokenKind::IntLit(42),
                TokenKind::In,
                TokenKind::Ident("x".to_string()),
                TokenKind::Plus,
                TokenKind::IntLit(1),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_nested_block_comments() {
        // Nested comments are supported by the lexer.
        let source = "/* outer /* inner */ still outer */ 42";
        let kinds = lex(source);
        assert_eq!(kinds, vec![TokenKind::IntLit(42), TokenKind::Eof]);
    }

    #[test]
    fn test_doc_comment_preserved() {
        let kinds = lex("/// hello\n42");
        assert_eq!(
            kinds,
            vec![
                TokenKind::DocComment(" hello".to_string()),
                TokenKind::Newline,
                TokenKind::IntLit(42),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_line_comment_skipped() {
        let kinds = lex("// ignored\n42");
        assert_eq!(
            kinds,
            vec![TokenKind::Newline, TokenKind::IntLit(42), TokenKind::Eof]
        );
    }

    #[test]
    fn test_string_escapes() {
        let kinds = lex(r#""a\nb\tc\"d""#);
        assert_eq!(
            kinds,
            vec![
                TokenKind::StringLit("a\nb\tc\"d".to_string()),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn test_string_utf8_multibyte() {
        // Multi-byte UTF-8 chars (2-byte é, 3-byte 你, 4-byte 🎉) must be
        // decoded as chars, not pushed as raw bytes (`é` must not become `Ã©`).
        let kinds = lex("\"héllo 你好 🎉\"");
        assert_eq!(
            kinds,
            vec![
                TokenKind::StringLit("héllo 你好 🎉".to_string()),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn test_float_variants() {
        let kinds = lex("3.5 1e3 2.5e-2");
        assert!(matches!(kinds[0], TokenKind::FloatLit(v) if (v - 3.5).abs() < 1e-9));
        assert!(matches!(kinds[1], TokenKind::FloatLit(v) if (v - 1000.0).abs() < 1e-9));
        assert!(matches!(kinds[2], TokenKind::FloatLit(v) if (v - 0.025).abs() < 1e-9));
    }

    #[test]
    fn test_all_operators() {
        // Exercise every operator that the lexer currently recognizes.
        let source = "+ - * / % == != < <= > >= && || & | |> ^ ~ << >> = += -= -> => <- . .. : ::";
        let kinds = lex(source);
        assert_eq!(
            kinds,
            vec![
                TokenKind::Plus,
                TokenKind::Minus,
                TokenKind::Star,
                TokenKind::Slash,
                TokenKind::Percent,
                TokenKind::Eq,
                TokenKind::Ne,
                TokenKind::Lt,
                TokenKind::Le,
                TokenKind::Gt,
                TokenKind::Ge,
                TokenKind::And,
                TokenKind::Or,
                TokenKind::Ampersand,
                TokenKind::Pipe,
                TokenKind::PipeOp,
                TokenKind::Caret,
                TokenKind::Tilde,
                TokenKind::Shl,
                TokenKind::Shr,
                TokenKind::Assign,
                TokenKind::PlusAssign,
                TokenKind::MinusAssign,
                TokenKind::Arrow,
                TokenKind::FatArrow,
                TokenKind::ThinArrow,
                TokenKind::Dot,
                TokenKind::DotDot,
                TokenKind::Colon,
                TokenKind::DoubleColon,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_keywords_and_identifiers() {
        let kinds = lex("fn let rec if true False MyType _under");
        assert_eq!(
            kinds,
            vec![
                TokenKind::Fn,
                TokenKind::Let,
                TokenKind::Rec,
                TokenKind::If,
                TokenKind::BoolLit(true),
                TokenKind::UpperIdent("False".to_string()),
                TokenKind::UpperIdent("MyType".to_string()),
                TokenKind::Ident("_under".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_unterminated_block_comment() {
        // Unterminated block comments are currently accepted and consume to EOF.
        let kinds = lex("/* never closed");
        assert_eq!(kinds, vec![TokenKind::Eof]);
    }

    #[test]
    fn test_unterminated_string_errors() {
        let mut lexer = Lexer::new("\"hello");
        let err = lexer.lex().unwrap_err();
        match err {
            NuError::LexError { msg, .. } => {
                assert!(msg.contains("Unterminated string literal"));
            }
            _ => panic!("Expected LexError"),
        }
    }

    #[test]
    fn test_unknown_escape_errors() {
        let mut lexer = Lexer::new("\"\\q\"");
        let err = lexer.lex().unwrap_err();
        match err {
            NuError::LexError { msg, .. } => {
                assert!(msg.contains("Unknown escape sequence"));
            }
            _ => panic!("Expected LexError"),
        }
    }

    #[test]
    fn test_invalid_hex_errors() {
        let mut lexer = Lexer::new("0x");
        let err = lexer.lex().unwrap_err();
        match err {
            NuError::LexError { msg, .. } => {
                assert!(msg.contains("Expected hex digits"));
            }
            _ => panic!("Expected LexError"),
        }
    }

    #[test]
    fn test_invalid_float_exponent_errors() {
        let mut lexer = Lexer::new("1e");
        let err = lexer.lex().unwrap_err();
        match err {
            NuError::LexError { msg, .. } => {
                assert!(msg.contains("Expected digits in exponent"));
            }
            _ => panic!("Expected LexError"),
        }
    }

    #[test]
    fn test_unexpected_character_errors() {
        let mut lexer = Lexer::new("$");
        let err = lexer.lex().unwrap_err();
        match err {
            NuError::LexError { msg, .. } => {
                assert!(msg.contains("Unexpected character"));
            }
            _ => panic!("Expected LexError"),
        }
    }

    #[test]
    fn test_unicode_identifier_rejected() {
        // The current lexer only accepts ASCII identifiers.
        let mut lexer = Lexer::new("α");
        let err = lexer.lex().unwrap_err();
        assert!(matches!(err, NuError::LexError { .. }));
    }

    #[test]
    fn test_workflow_keywords() {
        let mut lexer = Lexer::new("workflow step parallel compensate await subworkflow");
        let tokens = lexer.lex().unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| t.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Workflow,
                TokenKind::Step,
                TokenKind::Parallel,
                TokenKind::Compensate,
                TokenKind::Await,
                TokenKind::Subworkflow,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_tool_annotation_tokens() {
        let source = r#"@tool(description: "Adds two integers.")"#;
        let kinds = lex(source);
        assert_eq!(
            kinds,
            vec![
                TokenKind::At,
                TokenKind::Tool,
                TokenKind::LParen,
                TokenKind::Ident("description".to_string()),
                TokenKind::Colon,
                TokenKind::StringLit("Adds two integers.".to_string()),
                TokenKind::RParen,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_after_is_a_plain_identifier() {
        // `after` is a CONTEXTUAL keyword: it is only special immediately
        // after a receive block (`receive { ... } after ms => body`, see
        // parse_receive) and lexes as an ordinary identifier everywhere
        // else — e.g. workflow steps may be named `after`. This mirrors the
        // `to` identifier in `migrate actor to node` (parse_migrate).
        let kinds = lex("after 100");
        assert_eq!(
            kinds,
            vec![
                TokenKind::Ident("after".to_string()),
                TokenKind::IntLit(100),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_agent_keyword() {
        let kinds = lex("agent MyAgent = { model: \"gpt-4o\" }");
        assert_eq!(
            kinds,
            vec![
                TokenKind::Agent,
                TokenKind::UpperIdent("MyAgent".to_string()),
                TokenKind::Assign,
                TokenKind::LBrace,
                TokenKind::Ident("model".to_string()),
                TokenKind::Colon,
                TokenKind::StringLit("gpt-4o".to_string()),
                TokenKind::RBrace,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn test_state_machine_keyword() {
        // `state_machine` is a reserved keyword; `event`/`on_entry`/`on_exit`
        // are CONTEXTUAL keywords — only special inside a state_machine body
        // (see parse_state_machine), plain identifiers everywhere else. This
        // mirrors `after` in `receive { } after ms => body`.
        let kinds = lex("state_machine event on_entry on_exit");
        assert_eq!(
            kinds,
            vec![
                TokenKind::StateMachine,
                TokenKind::Ident("event".to_string()),
                TokenKind::Ident("on_entry".to_string()),
                TokenKind::Ident("on_exit".to_string()),
                TokenKind::Eof,
            ]
        );
    }
}
