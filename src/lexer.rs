//! Lexer: hand-written state machine.

use crate::types::Span;

// ---------------------------------------------------------------------------
// Token types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Literals
    IntLit(i64),
    FloatLit(f64),
    StringLit(String),
    BoolLit(bool),

    // Keywords
    Let, Rec, In, If, Then, Else,
    Match, With, Fn, Fun,
    Actor, Behavior, State, Initial,
    Agent, Llm, SystemPrompt, Temperature,
    Spawn, Send, Receive, Ask, SelfTok,
    Async, Await,
    Perform, Handle, Resume,
    Effect, Try, Catch, Finally,
    Type, Alias,
    Iso, Trn, Ref, Val, Box, Tag,
    Import, Export, Module,
    Cap, Of,
    Migrate, Node,
    Pipe, Cons,

    // Identifiers
    Ident(String),
    UpperIdent(String),
    EffectName(String),

    // Operators
    Plus, Minus, Star, Slash, Percent,
    Eq, Ne, Lt, Le, Gt, Ge,
    And, Or, Not,
    Assign, Arrow, FatArrow,

    // Delimiters
    LParen, RParen,
    LBrace, RBrace,
    LBracket, RBracket,
    Comma, Semicolon, Colon, Dot,
    Question,

    Bang,

    // Special
    Newline,
    Indent(u32),
    Dedent(u32),
    Eof,
    Error(String),
}

// ---------------------------------------------------------------------------
// Lexer state
// ---------------------------------------------------------------------------

pub struct Lexer<'a> {
    input: &'a str,
    bytes: &'a [u8],
    pos: usize,
    line: u32,
    col: u32,
    indent_stack: Vec<u32>,
    at_line_start: bool,
    parens_depth: u32,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Lexer {
            input,
            bytes: input.as_bytes(),
            pos: 0,
            line: 1,
            col: 1,
            indent_stack: vec![0],
            at_line_start: true,
            parens_depth: 0,
        }
    }

    // -- Low-level character operations --

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek_char(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn advance(&mut self) -> Option<u8> {
        let ch = self.bytes.get(self.pos).copied()?;
        self.pos += 1;
        if ch == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek() {
            if ch == b' ' || ch == b'\t' || ch == b'\r' {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn current_span(&self) -> Span {
        Span {
            start: self.pos as u32,
            end: self.pos as u32,
            line: self.line,
            col: self.col,
        }
    }

    fn span_at(&self, start: usize) -> Span {
        Span {
            start: start as u32,
            end: self.pos as u32,
            line: self.line,
            col: self.col - (self.pos - start) as u32,
        }
    }

    // -- Indentation handling --

    fn count_indent(&self) -> u32 {
        let mut count = 0u32;
        let mut p = self.pos;
        while p < self.bytes.len() {
            match self.bytes[p] {
                b' ' => { count += 1; p += 1; }
                b'\t' => { count += 4; p += 1; }
                _ => break,
            }
        }
        count
    }

    fn handle_indentation(&mut self) -> Option<Token> {
        if !self.at_line_start || self.parens_depth > 0 {
            return None;
        }

        self.skip_whitespace();

        if self.peek() == Some(b'\n') || self.peek() == Some(b'#') {
            return None; // Blank line or comment
        }

        if self.peek().is_none() {
            // EOF - dedent to 0
            if self.indent_stack.len() > 1 {
                self.indent_stack.pop();
                return Some(Token::Dedent(0));
            }
            return None;
        }

        let indent = self.count_indent();
        let current = *self.indent_stack.last().unwrap();

        if indent > current {
            self.indent_stack.push(indent);
            self.at_line_start = false;
            return Some(Token::Indent(indent));
        } else if indent < current {
            // Pop until we match
            let mut dedented = false;
            while self.indent_stack.len() > 1 && *self.indent_stack.last().unwrap() > indent {
                self.indent_stack.pop();
                dedented = true;
            }
            if dedented {
                self.at_line_start = false;
                return Some(Token::Dedent(indent));
            }
        }

        self.at_line_start = false;
        None
    }

    // -- String handling --

    fn read_string(&mut self) -> Token {
        let start = self.pos;
        self.advance(); // opening "
        let mut result = String::new();
        while let Some(ch) = self.peek() {
            if ch == b'"' {
                self.advance();
                return Token::StringLit(result);
            } else if ch == b'\\' {
                self.advance();
                match self.peek() {
                    Some(b'n') => { result.push('\n'); self.advance(); }
                    Some(b't') => { result.push('\t'); self.advance(); }
                    Some(b'"') => { result.push('"'); self.advance(); }
                    Some(b'\\') => { result.push('\\'); self.advance(); }
                    _ => result.push('\\'),
                }
            } else {
                result.push(self.advance().unwrap() as char);
            }
        }
        Token::Error("Unterminated string".to_string())
    }

    // -- Number handling --

    fn read_number(&mut self) -> Token {
        let start = self.pos;
        let mut has_dot = false;

        // Optional minus (but check it's not an arrow)
        if self.peek() == Some(b'-') {
            // Check next char
            if self.pos + 1 < self.bytes.len() && self.bytes[self.pos + 1].is_ascii_digit() {
                self.advance();
            } else {
                // This is the minus operator, not part of a number
                self.advance();
                return Token::Minus;
            }
        }

        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                self.advance();
            } else if ch == b'.' && !has_dot {
                // Check next is digit
                if self.pos + 1 < self.bytes.len() && self.bytes[self.pos + 1].is_ascii_digit() {
                    has_dot = true;
                    self.advance();
                } else {
                    break;
                }
            } else if ch == b'_' {
                self.advance(); // allow underscores in numbers
            } else {
                break;
            }
        }

        let text: String = self.input[start..self.pos].chars().filter(|&c| c != '_').collect();

        if has_dot {
            match text.parse::<f64>() {
                Ok(f) => Token::FloatLit(f),
                Err(_) => Token::Error(format!("Invalid float: {}", text)),
            }
        } else {
            match text.parse::<i64>() {
                Ok(n) => Token::IntLit(n),
                Err(_) => Token::Error(format!("Invalid integer: {}", text)),
            }
        }
    }

    // -- Identifier/keyword handling --

    fn read_ident(&mut self) -> Token {
        let start = self.pos;
        let is_upper = self.peek().map(|c| c.is_ascii_uppercase()).unwrap_or(false);

        while let Some(ch) = self.peek() {
            if ch.is_ascii_alphanumeric() || ch == b'_' {
                self.advance();
            } else {
                break;
            }
        }

        let text = &self.input[start..self.pos];
        self.lookup_keyword(text, is_upper)
    }

    fn lookup_keyword(&self, text: &str, is_upper: bool) -> Token {
        // Effect names: start with uppercase and have lowercase letters
        if is_upper && text.chars().nth(1).map(|c| c.is_ascii_lowercase()).unwrap_or(false) {
            return Token::EffectName(text.to_string());
        }

        match text {
            "let" => Token::Let,
            "rec" => Token::Rec,
            "in" => Token::In,
            "if" => Token::If,
            "then" => Token::Then,
            "else" => Token::Else,
            "match" => Token::Match,
            "with" => Token::With,
            "fn" => Token::Fn,
            "fun" => Token::Fun,
            "actor" => Token::Actor,
            "behavior" => Token::Behavior,
            "state" => Token::State,
            "initial" => Token::Initial,
            "agent" => Token::Agent,
            "llm" => Token::Llm,
            "system_prompt" => Token::SystemPrompt,
            "temperature" => Token::Temperature,
            "spawn" => Token::Spawn,
            "send" => Token::Send,
            "receive" => Token::Receive,
            "ask" => Token::Ask,
            "self" => Token::SelfTok,
            "async" => Token::Async,
            "await" => Token::Await,
            "perform" => Token::Perform,
            "handle" => Token::Handle,
            "resume" => Token::Resume,
            "effect" => Token::Effect,
            "try" => Token::Try,
            "catch" => Token::Catch,
            "finally" => Token::Finally,
            "type" => Token::Type,
            "alias" => Token::Alias,
            "iso" => Token::Iso,
            "trn" => Token::Trn,
            "ref" => Token::Ref,
            "val" => Token::Val,
            "box" => Token::Box,
            "tag" => Token::Tag,
            "import" => Token::Import,
            "export" => Token::Export,
            "module" => Token::Module,
            "cap" => Token::Cap,
            "of" => Token::Of,
            "migrate" => Token::Migrate,
            "node" => Token::Node,
            "true" => Token::BoolLit(true),
            "false" => Token::BoolLit(false),
            "and" => Token::And,
            "or" => Token::Or,
            "not" => Token::Not,
            _ => {
                if is_upper {
                    Token::UpperIdent(text.to_string())
                } else {
                    Token::Ident(text.to_string())
                }
            }
        }
    }

    // -- Comment handling --

    fn skip_comment(&mut self) {
        while let Some(ch) = self.peek() {
            if ch == b'\n' {
                break;
            }
            self.advance();
        }
    }

    fn skip_block_comment(&mut self) {
        self.advance(); // /
        self.advance(); // *
        let mut depth = 1;
        while depth > 0 && self.peek().is_some() {
            if self.peek() == Some(b'/') && self.pos + 1 < self.bytes.len() && self.bytes[self.pos + 1] == b'*' {
                depth += 1;
                self.advance();
                self.advance();
            } else if self.peek() == Some(b'*') && self.pos + 1 < self.bytes.len() && self.bytes[self.pos + 1] == b'/' {
                depth -= 1;
                self.advance();
                self.advance();
            } else {
                self.advance();
            }
        }
    }

    // -- Main dispatch --

    fn next_token_inner(&mut self) -> Token {
        // Check indentation at line start
        if let Some(tok) = self.handle_indentation() {
            return tok;
        }

        self.skip_whitespace();

        let start = self.pos;
        let ch = match self.peek() {
            Some(c) => c,
            None => return Token::Eof,
        };

        // Newline tracking
        if ch == b'\n' {
            self.advance();
            self.at_line_start = true;
            return Token::Newline;
        }

        // Comment
        if ch == b'#' {
            self.skip_comment();
            return self.next_token_inner();
        }
        if ch == b'/' && self.pos + 1 < self.bytes.len() && self.bytes[self.pos + 1] == b'*' {
            self.skip_block_comment();
            return self.next_token_inner();
        }

        // String literal
        if ch == b'"' {
            return self.read_string();
        }

        // Number
        if ch.is_ascii_digit() || (ch == b'-' && self.pos + 1 < self.bytes.len() && self.bytes[self.pos + 1].is_ascii_digit()) {
            return self.read_number();
        }

        // Identifier or keyword
        if ch.is_ascii_alphabetic() || ch == b'_' {
            return self.read_ident();
        }

        // Multi-char operators and delimiters
        self.advance();

        match ch {
            b'+' => Token::Plus,
            b'*' => Token::Star,
            b'/' => Token::Slash,
            b'%' => Token::Percent,
            b'(' => { self.parens_depth += 1; Token::LParen }
            b')' => { self.parens_depth = self.parens_depth.saturating_sub(1); Token::RParen }
            b'{' => Token::LBrace,
            b'}' => Token::RBrace,
            b'[' => Token::LBracket,
            b']' => Token::RBracket,
            b',' => Token::Comma,
            b';' => Token::Semicolon,
            b':' => {
                if self.peek() == Some(b':') {
                    self.advance();
                    Token::Cons
                } else {
                    Token::Colon
                }
            }
            b'.' => Token::Dot,
            b'?' => Token::Question,
            b'!' => {
                if self.peek() == Some(b'=') {
                    self.advance();
                    Token::Ne
                } else {
                    Token::Bang
                }
            }
            b'=' => {
                if self.peek() == Some(b'=') {
                    self.advance();
                    Token::Eq
                } else if self.peek() == Some(b'>') {
                    self.advance();
                    Token::FatArrow
                } else {
                    Token::Assign
                }
            }
            b'<' => {
                if self.peek() == Some(b'=') {
                    self.advance();
                    Token::Le
                } else if self.peek() == Some(b'-') {
                    self.advance();
                    Token::Pipe // <| becomes pipe
                } else if self.peek() == Some(b'|') {
                    self.advance();
                    Token::Pipe // <| becomes pipe
                } else {
                    Token::Lt
                }
            }
            b'>' => {
                if self.peek() == Some(b'=') {
                    self.advance();
                    Token::Ge
                } else {
                    Token::Gt
                }
            }
            b'-' => {
                if self.peek() == Some(b'>') {
                    self.advance();
                    Token::Arrow
                } else {
                    Token::Minus
                }
            }
            b'|' => {
                if self.peek() == Some(b'>') {
                    self.advance();
                    Token::Pipe
                } else {
                    // Could be a pattern guard
                    Token::Error("Unexpected '|'".to_string())
                }
            }
            _ => Token::Error(format!("Unexpected character: {}", ch as char)),
        }
    }
}

// ---------------------------------------------------------------------------
// Iterator
// ---------------------------------------------------------------------------

impl<'a> Iterator for Lexer<'a> {
    type Item = Token;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_token_inner() {
            Token::Eof => None,
            tok => Some(tok),
        }
    }
}

// ---------------------------------------------------------------------------
// Convenience
// ---------------------------------------------------------------------------

pub fn lex(input: &str) -> Vec<Token> {
    Lexer::new(input).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_literals() {
        let tokens = lex("42 3.14 \"hello\" true false");
        assert_eq!(tokens[0], Token::IntLit(42));
        assert_eq!(tokens[1], Token::FloatLit(3.14));
        assert_eq!(tokens[2], Token::StringLit("hello".to_string()));
        assert_eq!(tokens[3], Token::BoolLit(true));
        assert_eq!(tokens[4], Token::BoolLit(false));
    }

    #[test]
    fn test_keywords() {
        let tokens = lex("let fn actor spawn perform effect");
        assert_eq!(tokens[0], Token::Let);
        assert_eq!(tokens[1], Token::Fn);
        assert_eq!(tokens[2], Token::Actor);
        assert_eq!(tokens[3], Token::Spawn);
        assert_eq!(tokens[4], Token::Perform);
        assert_eq!(tokens[5], Token::Effect);
    }

    #[test]
    fn test_operators() {
        let tokens = lex("+ - * / == != < > <= >= => ->");
        assert_eq!(tokens[0], Token::Plus);
        assert_eq!(tokens[1], Token::Minus);
        assert_eq!(tokens[2], Token::Star);
        assert_eq!(tokens[3], Token::Slash);
        assert_eq!(tokens[4], Token::Eq);
        assert_eq!(tokens[5], Token::Ne);
        assert_eq!(tokens[6], Token::Lt);
        assert_eq!(tokens[7], Token::Gt);
        assert_eq!(tokens[8], Token::Le);
        assert_eq!(tokens[9], Token::Ge);
        assert_eq!(tokens[10], Token::FatArrow);
        assert_eq!(tokens[11], Token::Arrow);
    }

    #[test]
    fn test_indentation() {
        let input = "if true\n  42\n  43\nelse\n  0\n";
        let tokens = lex(input);
        assert_eq!(tokens[0], Token::If);
        assert_eq!(tokens[1], Token::BoolLit(true));
        assert_eq!(tokens[2], Token::Newline);
        assert_eq!(tokens[3], Token::Indent(2));
        assert_eq!(tokens[4], Token::IntLit(42));
    }
}
