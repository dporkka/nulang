//! LSP (Language Server Protocol) server for Nulang.
//!
//! Provides inlay hints showing inferred types, capability annotations,
//! and effect rows inline in the source code.
//!
//! Run with: `nulang --lsp` (starts stdin/stdout JSON-RPC server)
//!
//! # Supported LSP Features (MVP)
//!
//! | Feature | Description |
//! |---------|-------------|
//! | `textDocument/inlayHint` | Show inferred types after bindings |
//! | Type inlays | `let x = 42` shows `: Int` after `x` |
//! | Capability inlays | `let y: iso String` shows `: iso` |
//! | Effect inlays | `fun f() ! IO` shows `[IO]` |

use std::collections::HashMap;
use std::sync::Mutex;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

// ---------------------------------------------------------------------------
// LSP Server
// ---------------------------------------------------------------------------

/// Nulang language server implementing the LSP protocol.
pub struct NulangLanguageServer {
    client: Client,
    documents: Mutex<HashMap<Url, DocumentState>>,
}

struct DocumentState {
    version: i32,
    source: String,
}

impl NulangLanguageServer {
    pub fn new(client: Client) -> Self {
        NulangLanguageServer {
            client,
            documents: Mutex::new(HashMap::new()),
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for NulangLanguageServer {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                inlay_hint_provider: Some(OneOf::Right(InlayHintServerCapabilities::Options(
                    InlayHintOptions {
                        resolve_provider: Some(false),
                        work_done_progress_options: WorkDoneProgressOptions::default(),
                    },
                ))),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: None,
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                    all_commit_characters: None,
                    completion_item: None,
                }),
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        will_save: None,
                        will_save_wait_until: None,
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions::default())),
                    },
                )),
                ..ServerCapabilities::default()
            },
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "Nulang LSP server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let mut docs = self.documents.lock().unwrap();
        docs.insert(
            params.text_document.uri,
            DocumentState {
                version: params.text_document.version,
                source: params.text_document.text,
            },
        );
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let mut docs = self.documents.lock().unwrap();
        if let Some(doc) = docs.get_mut(&params.text_document.uri) {
            if let Some(change) = params.content_changes.into_iter().next() {
                doc.version = params.text_document.version;
                doc.source = change.text;
            }
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let mut docs = self.documents.lock().unwrap();
        docs.remove(&params.text_document.uri);
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let docs = self.documents.lock().unwrap();
        let source = match docs.get(&params.text_document.uri) {
            Some(doc) => doc.source.clone(),
            None => return Ok(None),
        };
        drop(docs);

        let engine = InlayHintEngine::new(&source);
        let hints = engine.generate_inlay_hints();
        Ok(Some(hints))
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let docs = self.documents.lock().unwrap();
        let source = match docs.get(&params.text_document_position.text_document.uri) {
            Some(doc) => doc.source.clone(),
            None => return Ok(None),
        };
        drop(docs);

        let engine = CompletionEngine::new(&source);
        let items = engine.complete(params.text_document_position.position);
        Ok(Some(CompletionResponse::Array(items)))
    }
}

// ---------------------------------------------------------------------------
// Inlay Hint Engine
// ---------------------------------------------------------------------------

/// Generates inlay hints for Nulang source code.
///
/// Parses the source, runs type inference, and produces inlay hints
/// showing inferred types, capabilities, and effect annotations.
pub struct InlayHintEngine<'a> {
    source: &'a str,
}

/// A type annotation to display as an inlay hint.
#[derive(Debug, Clone)]
pub struct TypeAnnotation {
    pub line: u32,
    pub character: u32,
    pub label: String,
    pub kind: AnnotationKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationKind {
    Type,       // : Int, : Float, etc.
    Capability, // : iso, : val, etc.
    Effect,     // [IO], [FileSystem], etc.
}

impl<'a> InlayHintEngine<'a> {
    pub fn new(source: &'a str) -> Self {
        InlayHintEngine { source }
    }

    /// Generate inlay hints for the source file.
    pub fn generate_inlay_hints(&self) -> Vec<InlayHint> {
        let annotations = self.collect_annotations();
        annotations.into_iter().map(|a| self.annotation_to_inlay(a)).collect()
    }

    /// Collect type annotations from the source.
    ///
    /// This is a simplified implementation that uses regex-based parsing
    /// for the MVP. A full implementation would parse the AST and run
    /// the typechecker.
    fn collect_annotations(&self) -> Vec<TypeAnnotation> {
        let mut annotations = Vec::new();
        for (line_idx, line) in self.source.lines().enumerate() {
            let line_num = line_idx as u32;
            let trimmed = line.trim();

            // Skip comments and blank lines
            if trimmed.is_empty() || trimmed.starts_with("--") {
                continue;
            }

            // let binding without explicit type → infer
            if let Some(pos) = line.find("let ") {
                if !line[pos..].contains(":") {
                    // No explicit type annotation
                    let after_let = pos + 4;
                    let rest = &line[after_let..];
                    if let Some(end) = rest.find(|c: char| c == ' ' || c == '=') {
                        let var_name = &rest[..end];
                        let col = (after_let + end) as u32;
                        if let Some(inferred) = self.infer_type(line) {
                            annotations.push(TypeAnnotation {
                                line: line_num,
                                character: col,
                                label: format!(": {}", inferred),
                                kind: AnnotationKind::Type,
                            });
                        }
                    }
                }
            }

            // fun parameter without explicit type
            if trimmed.starts_with("fun ") {
                if let Some(lparen) = line.find('(') {
                    if let Some(rparen) = line.find(')') {
                        let params = &line[lparen + 1..rparen];
                        let mut col_offset = (lparen + 1) as u32;
                        for param in params.split(',') {
                            let param = param.trim();
                            if !param.is_empty() && !param.contains(":") {
                                let param_len = param.len() as u32;
                                if let Some(inferred) = self.infer_param_type(param, line) {
                                    annotations.push(TypeAnnotation {
                                        line: line_num,
                                        character: col_offset + param_len,
                                        label: format!(": {}", inferred),
                                        kind: AnnotationKind::Type,
                                    });
                                }
                                col_offset += param_len + 2; // +2 for ", "
                            }
                        }
                    }
                }
            }

            // Capability annotations (iso, val, trn, ref, box, tag)
            for cap in &["iso", "val", "trn", "ref", "box", "tag"] {
                if let Some(pos) = line.find(&format!(" :{}", cap)) {
                    annotations.push(TypeAnnotation {
                        line: line_num,
                        character: (pos + 1) as u32,
                        label: format!(":{}", cap),
                        kind: AnnotationKind::Capability,
                    });
                }
            }

            // Effect annotations (! IO, ! FileSystem, etc.)
            if let Some(pos) = line.find(" ! ") {
                let after_bang = pos + 3;
                let rest = &line[after_bang..];
                let effect_name = rest.split_whitespace().next().unwrap_or("");
                if !effect_name.is_empty() {
                    annotations.push(TypeAnnotation {
                        line: line_num,
                        character: (after_bang + effect_name.len()) as u32,
                        label: format!(" [{}]", effect_name),
                        kind: AnnotationKind::Effect,
                    });
                }
            }
        }
        annotations
    }

    /// Infer the type of a value from context (simplified heuristic).
    fn infer_type(&self, line: &str) -> Option<String> {
        let trimmed = line.trim();
        // Check RHS of assignment
        if let Some(eq_pos) = trimmed.find('=') {
            let rhs = trimmed[eq_pos + 1..].trim();
            return self.infer_expr_type(rhs);
        }
        None
    }

    /// Infer parameter type from usage context.
    fn infer_param_type(&self, _param: &str, func_line: &str) -> Option<String> {
        // Simplified: check for arithmetic operations
        if func_line.contains('+') || func_line.contains('-') || func_line.contains('*') {
            Some("Int".to_string())
        } else if func_line.contains(".") && !func_line.contains("..") {
            Some("Float".to_string())
        } else {
            Some("a".to_string()) // Generic type variable
        }
    }

    /// Infer expression type from syntax (heuristic).
    fn infer_expr_type(&self, expr: &str) -> Option<String> {
        let expr = expr.trim();
        if expr.is_empty() { return None; }

        // Integer literal
        if expr.parse::<i64>().is_ok() {
            return Some("Int".to_string());
        }
        // Float literal
        if expr.parse::<f64>().is_ok() && expr.contains('.') {
            return Some("Float".to_string());
        }
        // String literal
        if (expr.starts_with('"') && expr.ends_with('"'))
            || (expr.starts_with('\'') && expr.ends_with('\'')) {
            return Some("String".to_string());
        }
        // Boolean
        if expr == "true" || expr == "false" {
            return Some("Bool".to_string());
        }
        // List/array
        if expr.starts_with('[') && expr.ends_with(']') {
            return Some("[a]".to_string());
        }
        // Unit
        if expr == "unit" || expr == "()" {
            return Some("Unit".to_string());
        }
        // Arithmetic operation
        if expr.contains('+') || expr.contains('-') || expr.contains('*') || expr.contains("/") {
            return Some("Int".to_string());
        }
        // Function call
        if expr.contains('(') && expr.contains(')') {
            return Some("b".to_string());
        }
        Some("a".to_string())
    }

    /// Convert a TypeAnnotation to an LSP InlayHint.
    fn annotation_to_inlay(&self, ann: TypeAnnotation) -> InlayHint {
        InlayHint {
            position: Position {
                line: ann.line,
                character: ann.character,
            },
            label: InlayHintLabel::String(ann.label),
            kind: Some(match ann.kind {
                AnnotationKind::Type => InlayHintKind::TYPE,
                AnnotationKind::Capability => InlayHintKind::PARAMETER,
                AnnotationKind::Effect => InlayHintKind::TYPE,
            }),
            text_edits: None,
            tooltip: Some(InlayHintTooltip::String(match ann.kind {
                AnnotationKind::Type => "Inferred type".to_string(),
                AnnotationKind::Capability => "Reference capability".to_string(),
                AnnotationKind::Effect => "Effect row".to_string(),
            })),
            padding_left: Some(false),
            padding_right: Some(false),
            data: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Completion Engine
// ---------------------------------------------------------------------------

/// Generates completion items for Nulang source code.
///
/// The engine is intentionally lightweight: it offers keywords, built-in
/// effect names, and top-level function names extracted from the current
/// document. It does not require a full parse or typecheck.
pub struct CompletionEngine<'a> {
    source: &'a str,
}

impl<'a> CompletionEngine<'a> {
    /// Nulang language keywords offered by the completion provider.
    const KEYWORDS: &'static [&'static str] = &[
        "fn", "let", "if", "else", "match", "effect", "actor", "type",
        "module", "import", "handle", "perform", "resume", "return",
        "true", "false", "nil", "unit",
    ];

    /// Built-in effect names offered by the completion provider.
    const EFFECTS: &'static [&'static str] = &[
        "IO", "Net", "FS", "Spawn", "Send", "Receive", "Migrate", "STM",
        "Async", "LLM", "Cost", "Rand", "Time",
    ];

    pub fn new(source: &'a str) -> Self {
        CompletionEngine { source }
    }

    /// Return completion items at the given LSP position.
    pub fn complete(&self, position: Position) -> Vec<CompletionItem> {
        let offset = self.position_to_offset(position);
        let prefix = self.prefix_at(offset);
        let prefix_lower = prefix.to_lowercase();

        let mut items = Vec::new();

        // Keywords.
        for &kw in Self::KEYWORDS {
            if kw.to_lowercase().starts_with(&prefix_lower) {
                items.push(CompletionItem {
                    label: kw.to_string(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    ..CompletionItem::default()
                });
            }
        }

        // Built-in effects.
        for &eff in Self::EFFECTS {
            let eff_lower = eff.to_lowercase();
            if eff_lower.starts_with(prefix_lower.as_str()) {
                items.push(CompletionItem {
                    label: eff.to_string(),
                    kind: Some(CompletionItemKind::ENUM_MEMBER),
                    detail: Some("built-in effect".to_string()),
                    ..CompletionItem::default()
                });
            }
        }

        // Top-level function names (`fun name(...)`).
        for name in self.top_level_functions() {
            if name.to_lowercase().starts_with(&prefix_lower) {
                items.push(CompletionItem {
                    label: name,
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: Some("function".to_string()),
                    ..CompletionItem::default()
                });
            }
        }

        items
    }

    /// Convert an LSP position to a byte offset in the source.
    fn position_to_offset(&self, position: Position) -> usize {
        let mut offset = 0usize;
        for (line_idx, line) in self.source.lines().enumerate() {
            if line_idx as u32 == position.line {
                return offset + (position.character as usize).min(line.len());
            }
            offset += line.len() + 1; // +1 for newline
        }
        self.source.len()
    }

    /// Extract the identifier fragment the user has typed so far.
    fn prefix_at(&self, offset: usize) -> String {
        let bytes = self.source.as_bytes();
        let mut start = offset;
        while start > 0 {
            let prev = bytes[start - 1] as char;
            if prev.is_alphanumeric() || prev == '_' {
                start -= 1;
            } else {
                break;
            }
        }
        self.source[start..offset].to_string()
    }

    /// Extract top-level function names from the document.
    fn top_level_functions(&self) -> Vec<String> {
        let mut names = Vec::new();
        for line in self.source.lines() {
            let trimmed = line.trim_start();
            if let Some(after_fun) = trimmed.strip_prefix("fun ") {
                let name = after_fun
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim();
                let name = name.split('(').next().unwrap_or("").trim();
                if !name.is_empty() && !name.contains(':') {
                    names.push(name.to_string());
                }
            }
        }
        names
    }
}

// ---------------------------------------------------------------------------
// Server Entry Point
// ---------------------------------------------------------------------------

/// Run the LSP server over stdin/stdout.
pub async fn run_lsp_server() {
    let (stdin, stdout) = (tokio::io::stdin(), tokio::io::stdout());
    let (service, socket) = LspService::new(|client| NulangLanguageServer::new(client));
    Server::new(stdin, stdout, socket).serve(service).await;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod lsp_tests {
    use super::*;

    fn label_to_string(label: &InlayHintLabel) -> String {
        match label {
            InlayHintLabel::String(s) => s.clone(),
            InlayHintLabel::LabelParts(parts) => parts.iter().map(|p| p.value.clone()).collect(),
        }
    }

    #[test]
    fn test_type_inlay_for_let_binding() {
        let source = "let x = 42";
        let engine = InlayHintEngine::new(source);
        let hints = engine.generate_inlay_hints();
        assert!(!hints.is_empty());
        let type_hint = &hints[0];
        assert!(label_to_string(&type_hint.label).contains("Int"));
    }

    #[test]
    fn test_type_inlay_for_float_binding() {
        let source = "let pi = 3.14";
        let engine = InlayHintEngine::new(source);
        let hints = engine.generate_inlay_hints();
        assert!(!hints.is_empty());
        assert!(label_to_string(&hints[0].label).contains("Float"));
    }

    #[test]
    fn test_type_inlay_for_string_binding() {
        let source = "let name = \"hello\"";
        let engine = InlayHintEngine::new(source);
        let hints = engine.generate_inlay_hints();
        assert!(!hints.is_empty());
        assert!(label_to_string(&hints[0].label).contains("String"));
    }

    #[test]
    fn test_capability_inlay_for_iso() {
        let source = "let x :iso String = \"hello\"";
        let engine = InlayHintEngine::new(source);
        let hints = engine.generate_inlay_hints();
        let cap_hints: Vec<_> = hints.iter().filter(|h| label_to_string(&h.label).contains(":iso")).collect();
        assert!(!cap_hints.is_empty());
    }

    #[test]
    fn test_effect_inlay_for_handler() {
        let source = "fun read() ! IO";
        let engine = InlayHintEngine::new(source);
        let hints = engine.generate_inlay_hints();
        let effect_hints: Vec<_> = hints.iter().filter(|h| label_to_string(&h.label).contains("[IO]")).collect();
        assert!(!effect_hints.is_empty());
    }

    #[test]
    fn test_no_inlay_when_explicit_type() {
        let source = "let x : Int = 42";
        let engine = InlayHintEngine::new(source);
        let hints = engine.generate_inlay_hints();
        // Should NOT generate a type inlay since type is already explicit
        let type_inlays: Vec<_> = hints.iter().filter(|h| {
            let label = label_to_string(&h.label);
            label.starts_with(": ") && !label.contains(":iso")
        }).collect();
        assert!(type_inlays.is_empty(), "should not add inlay when type is explicit");
    }

    #[test]
    fn test_inlay_position_calculation() {
        let source = "let abc = 123";
        let engine = InlayHintEngine::new(source);
        let hints = engine.generate_inlay_hints();
        assert_eq!(hints[0].position.line, 0);
        assert_eq!(hints[0].position.character, 7); // after "abc"
    }

    #[test]
    fn test_type_to_inlay_string() {
        let ann = TypeAnnotation {
            line: 0, character: 5, label: ": Int".to_string(), kind: AnnotationKind::Type,
        };
        let engine = InlayHintEngine::new("");
        let inlay = engine.annotation_to_inlay(ann);
        assert_eq!(label_to_string(&inlay.label), ": Int");
        assert_eq!(inlay.kind, Some(InlayHintKind::TYPE));
    }

    #[test]
    fn test_multiple_let_bindings() {
        let source = "let x = 42\nlet y = 3.14\nlet z = \"hi\"";
        let engine = InlayHintEngine::new(source);
        let hints = engine.generate_inlay_hints();
        assert_eq!(hints.len(), 3);
        assert!(label_to_string(&hints[0].label).contains("Int"));
        assert!(label_to_string(&hints[1].label).contains("Float"));
        assert!(label_to_string(&hints[2].label).contains("String"));
    }

    // -- Completion engine tests --

    fn labels(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_str()).collect()
    }

    #[test]
    fn test_completion_keywords() {
        let source = "let x = 42";
        let engine = CompletionEngine::new(source);
        let items = engine.complete(Position { line: 0, character: 0 });
        let labels = labels(&items);
        assert!(labels.contains(&"let"));
        assert!(labels.contains(&"fn"));
        assert!(labels.contains(&"match"));
    }

    #[test]
    fn test_completion_prefix_filtering() {
        let source = "ret";
        let engine = CompletionEngine::new(source);
        // Cursor at end of "ret".
        let items = engine.complete(Position { line: 0, character: 3 });
        let labels = labels(&items);
        assert!(labels.contains(&"return"), "should offer 'return' for prefix 'ret'");
        assert!(!labels.contains(&"let"), "'let' should not match prefix 'ret'");
    }

    #[test]
    fn test_completion_top_level_functions() {
        let source = "fun foo()\nfun bar(x: Int)\nlet x = 1";
        let engine = CompletionEngine::new(source);
        let items = engine.complete(Position { line: 2, character: 0 });
        let labels = labels(&items);
        assert!(labels.contains(&"foo"));
        assert!(labels.contains(&"bar"));
    }

    #[test]
    fn test_completion_effects() {
        let source = "";
        let engine = CompletionEngine::new(source);
        let items = engine.complete(Position { line: 0, character: 0 });
        let labels = labels(&items);
        assert!(labels.contains(&"IO"));
        assert!(labels.contains(&"Migrate"));
        assert!(labels.contains(&"LLM"));
    }

    #[test]
    fn test_completion_case_insensitive() {
        // Effect names are matched case-insensitively by prefix.
        let source = "mi";
        let engine = CompletionEngine::new(source);
        let items = engine.complete(Position { line: 0, character: 2 });
        let labels = labels(&items);
        assert!(labels.contains(&"Migrate"), "should match 'Migrate' for prefix 'mi'");
    }
}
