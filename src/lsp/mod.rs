//! LSP (Language Server Protocol) server for Nulang.
//!
//! Run with: `nulang --lsp` (starts stdin/stdout JSON-RPC server)
//!
//! # Supported LSP Features
//!
//! | Feature | Description |
//! |---------|-------------|
//! | `textDocument/diagnostic` | Parse/type/effect/capability diagnostics |
//! | `textDocument/hover` | Function signatures, types, keywords, effects |
//! | `textDocument/definition` | Go to definition for all declaration types |
//! | `textDocument/references` | Find all usages of a symbol |
//! | `textDocument/documentSymbol` | Structured outline (functions, actors, etc.) |
//! | `textDocument/rename` | Rename symbol across document (with prepareRename) |
//! | `textDocument/signatureHelp` | Function parameter hints while typing |
//! | `textDocument/formatting` | Indentation-based code formatting |
//! | `textDocument/semanticTokens` | Syntax highlighting for editors |
//! | `textDocument/codeAction` | Quick fixes (add type annotations) |
//! | `textDocument/inlayHint` | Show inferred types after bindings |
//! | `textDocument/completion` | Keyword/effect/function completion |
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use crate::effect_checker::{CapContext, CapabilityAnalyzer, EffectChecker};
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::typechecker::TypeChecker;
use crate::types::NuError;

/// Convert an LSP position column (UTF-16 code units) into a byte offset
/// within `line`, clamped to a char boundary.
///
/// LSP clients report columns as UTF-16 code units, but Rust strings are
/// sliced by byte offset. Using the raw column as a byte index on non-ASCII
/// text lands inside multibyte characters and panics; this helper walks the
/// line and snaps to the boundary covering the requested column.
fn utf16_col_to_byte(line: &str, col: usize) -> usize {
    let mut utf16 = 0usize;
    for (byte_idx, ch) in line.char_indices() {
        if utf16 >= col {
            return byte_idx;
        }
        utf16 += ch.len_utf16();
    }
    line.len()
}

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
    type_map: Option<HashMap<usize, String>>,
    ast: Option<crate::ast::AstModule>,
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
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: None,
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
                document_formatting_provider: Some(OneOf::Left(true)),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            work_done_progress_options: WorkDoneProgressOptions::default(),
                            legend: SemanticTokensLegend {
                                token_types: vec![
                                    SemanticTokenType::KEYWORD,
                                    SemanticTokenType::FUNCTION,
                                    SemanticTokenType::VARIABLE,
                                    SemanticTokenType::TYPE,
                                    SemanticTokenType::CLASS,
                                    SemanticTokenType::STRING,
                                    SemanticTokenType::NUMBER,
                                    SemanticTokenType::OPERATOR,
                                    SemanticTokenType::COMMENT,
                                    SemanticTokenType::NAMESPACE,
                                ],
                                token_modifiers: vec![
                                    SemanticTokenModifier::DECLARATION,
                                    SemanticTokenModifier::READONLY,
                                ],
                            },
                            range: None,
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                        },
                    ),
                ),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                })),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(false),
                }),
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        will_save: None,
                        will_save_wait_until: None,
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(
                            SaveOptions::default(),
                        )),
                    },
                )),
                diagnostic_provider: Some(DiagnosticServerCapabilities::Options(
                    DiagnosticOptions {
                        identifier: Some("nulang".to_string()),
                        inter_file_dependencies: false,
                        workspace_diagnostics: false,
                        work_done_progress_options: WorkDoneProgressOptions::default(),
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
        let uri = params.text_document.uri.clone();
        let version = params.text_document.version;
        let source = params.text_document.text.clone();

        let (diagnostics, type_map) = Self::compute_diagnostics(&source);

        {
            let mut docs = self.documents.lock().unwrap();
            let ast = Lexer::new(&source)
                .lex()
                .ok()
                .and_then(|tokens| Parser::new(tokens).parse_module().ok());
            docs.insert(
                uri.clone(),
                DocumentState {
                    version,
                    source: source.clone(),
                    type_map: Some(type_map),
                    ast,
                },
            );
        }

        self.client
            .publish_diagnostics(uri, diagnostics, Some(version))
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let version = params.text_document.version;
        let source = params
            .content_changes
            .into_iter()
            .next()
            .map(|c| c.text)
            .unwrap_or_default();

        let (diagnostics, type_map) = Self::compute_diagnostics(&source);

        {
            let mut docs = self.documents.lock().unwrap();
            if let Some(doc) = docs.get_mut(&uri) {
                doc.version = version;
                doc.source = source.clone();
                doc.type_map = Some(type_map);
                doc.ast = Lexer::new(&source)
                    .lex()
                    .ok()
                    .and_then(|tokens| Parser::new(tokens).parse_module().ok());
            }
        }

        self.client
            .publish_diagnostics(uri, diagnostics, Some(version))
            .await;
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
        let doc = match docs.get(&params.text_document_position.text_document.uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let source = doc.source.clone();
        let ast = doc.ast.clone();
        drop(docs);

        let mut engine = CompletionEngine::new(&source);
        if let Some(ref a) = ast {
            engine.set_ast_info(a);
        }
        let doc_dir = params
            .text_document_position
            .text_document
            .uri
            .to_file_path()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()));
        let items = engine.complete(params.text_document_position.position, doc_dir.as_deref());
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let docs = self.documents.lock().unwrap();
        let doc = match docs.get(&params.text_document_position_params.text_document.uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let source = doc.source.clone();
        let type_map = doc.type_map.clone();
        drop(docs);

        // Check type_map first for inferred/explicit types
        if let Some(ref map) = type_map {
            if let Some(offset) = Self::position_to_byte_offset(
                &source,
                &params.text_document_position_params.position,
            ) {
                if let Some(type_str) = Self::find_type_at_offset(map, offset) {
                    return Ok(Some(Hover {
                        contents: HoverContents::Scalar(MarkedString::LanguageString(
                            LanguageString {
                                language: "nulang".to_string(),
                                value: type_str.clone(),
                            },
                        )),
                        range: None,
                    }));
                }
            }
        }

        Ok(self.hover_at(&source, params.text_document_position_params.position))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        let docs = self.documents.lock().unwrap();
        let doc = match docs.get(&uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let source = doc.source.clone();
        let ast = doc.ast.clone();
        drop(docs);

        // Search local definitions first
        if let Some(loc) =
            self.goto_def_local(&source, params.text_document_position_params.position, &uri)
        {
            return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
        }

        // Try cross-file: extract word and search imported modules
        let line = params.text_document_position_params.position.line as usize;
        let col = params.text_document_position_params.position.character as usize;
        if let Some(target_line) = source.lines().nth(line) {
            let byte_col = utf16_col_to_byte(target_line, col);
            if let Some(word) = self.word_at(target_line, byte_col) {
                if let Some(ref a) = ast {
                    if let Some(loc) = self.goto_def_cross_file(&uri, word, a) {
                        return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
                    }
                }
            }
        }
        Ok(None)
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let docs = self.documents.lock().unwrap();
        let source = match docs.get(&params.text_document_position.text_document.uri) {
            Some(doc) => doc.source.clone(),
            None => return Ok(None),
        };
        drop(docs);
        let uri = params.text_document_position.text_document.uri.clone();
        Ok(Some(self.find_refs(
            &source,
            params.text_document_position.position,
            &uri,
        )))
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let docs = self.documents.lock().unwrap();
        let source = match docs.get(&params.text_document.uri) {
            Some(doc) => doc.source.clone(),
            None => return Ok(None),
        };
        drop(docs);
        let locs = self.find_refs(&source, params.position, &params.text_document.uri);
        Ok(locs.first().map(|l| PrepareRenameResponse::Range(l.range)))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let docs = self.documents.lock().unwrap();
        let source = match docs.get(&params.text_document_position.text_document.uri) {
            Some(doc) => doc.source.clone(),
            None => return Ok(None),
        };
        drop(docs);
        let locs = self.find_refs(
            &source,
            params.text_document_position.position,
            &params.text_document_position.text_document.uri,
        );
        if locs.is_empty() {
            return Ok(None);
        }
        let uri = locs[0].uri.clone();
        let edits: Vec<TextEdit> = locs
            .iter()
            .map(|l| TextEdit {
                range: l.range,
                new_text: params.new_name.clone(),
            })
            .collect();
        let mut changes = std::collections::HashMap::new();
        changes.insert(uri, edits);
        Ok(Some(WorkspaceEdit {
            changes: Some(changes),
            ..WorkspaceEdit::default()
        }))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let docs = self.documents.lock().unwrap();
        let source = match docs.get(&params.text_document.uri) {
            Some(doc) => doc.source.clone(),
            None => return Ok(None),
        };
        drop(docs);
        Ok(self.doc_syms(&source).map(DocumentSymbolResponse::Flat))
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let docs = self.documents.lock().unwrap();
        let source = match docs.get(&params.text_document_position_params.text_document.uri) {
            Some(doc) => doc.source.clone(),
            None => return Ok(None),
        };
        drop(docs);
        Ok(Self::sig_help(
            &source,
            params.text_document_position_params.position,
        ))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let docs = self.documents.lock().unwrap();
        let source = match docs.get(&params.text_document.uri) {
            Some(doc) => doc.source.clone(),
            None => return Ok(None),
        };
        drop(docs);
        let formatted = self.fmt_source(&source);
        if formatted == source {
            return Ok(None);
        }
        let lines = source.lines().count() as u32;
        Ok(Some(vec![TextEdit {
            range: Range {
                start: Position::new(0, 0),
                end: Position::new(lines, 0),
            },
            new_text: formatted,
        }]))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let docs = self.documents.lock().unwrap();
        let source = match docs.get(&params.text_document.uri) {
            Some(doc) => doc.source.clone(),
            None => return Ok(None),
        };
        drop(docs);
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: self.sem_tokens(&source),
        })))
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri.clone();
        let docs = self.documents.lock().unwrap();
        let source = match docs.get(&uri) {
            Some(doc) => doc.source.clone(),
            None => return Ok(None),
        };
        drop(docs);
        let range = Some(params.range);
        Ok(Self::code_actions(&source, range, &uri))
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        let docs = self.documents.lock().unwrap();
        let source = match docs.get(&params.text_document.uri) {
            Some(doc) => doc.source.clone(),
            None => return Ok(None),
        };
        drop(docs);
        Ok(Some(Self::compute_folding_ranges(&source)))
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let docs = self.documents.lock().unwrap();
        let doc = match docs.get(&params.text_document.uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let source = doc.source.clone();
        let ast = doc.ast.clone();
        drop(docs);

        let uri = params.text_document.uri;
        let mut lenses = Vec::new();

        if let Some(ref ast) = ast {
            for decl in &ast.decls {
                let (_name, line) = match decl {
                    crate::ast::Decl::Function { name, span, .. } => (name, span.line() as u32),
                    crate::ast::Decl::Actor { name, span, .. } => (name, span.line() as u32),
                    crate::ast::Decl::VariantType { name, span, .. } => (name, span.line() as u32),
                    crate::ast::Decl::TypeAlias { name, span, .. } => (name, span.line() as u32),
                    _ => continue,
                };
                let refs = self.find_refs(&source, Position::new(line.saturating_sub(1), 0), &uri);
                if !refs.is_empty() {
                    lenses.push(CodeLens {
                        range: Range {
                            start: Position::new(line - 1, 0),
                            end: Position::new(line - 1, 0),
                        },
                        command: Some(Command {
                            title: format!("{} references", refs.len()),
                            command: "nulang.showReferences".to_string(),
                            arguments: None,
                        }),
                        data: None,
                    });
                }
            }
        }

        Ok(Some(lenses))
    }
}

impl NulangLanguageServer {
    /// Run the compiler frontend on `source` and return LSP diagnostics.
    ///
    /// This is intentionally tolerant: each stage is tried in order, and the
    /// first fatal error in a stage is reported. Effect and capability checks
    /// also report accumulated warnings from their internal diagnostic lists.
    fn hover_at(&self, source: &str, position: Position) -> Option<Hover> {
        let line = position.line as usize;
        let col = position.character as usize;
        let target_line = source.lines().nth(line)?;
        // LSP columns are UTF-16 code units; map to a byte offset on a char
        // boundary before touching the line's bytes.
        let word = self.word_at(target_line, utf16_col_to_byte(target_line, col))?;
        let tokens = Lexer::new(source).lex().ok()?;
        let ast = Parser::new(tokens).parse_module().ok()?;
        let mut tc = TypeChecker::new();
        let mt = tc.check_module(&ast).ok()?;
        for decl in &ast.decls {
            if let crate::ast::Decl::Function {
                name,
                params,
                ret_type,
                ..
            } = decl
            {
                if name == word {
                    let p = params
                        .iter()
                        .map(|(n, t)| {
                            format!(
                                "{}: {}",
                                n,
                                t.as_ref()
                                    .map(|ty| format!("{:?}", ty))
                                    .unwrap_or_else(|| "?".into())
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    let r = ret_type
                        .as_ref()
                        .map(|ty| format!("{:?}", ty))
                        .unwrap_or_else(|| "?".into());
                    return Some(Hover {
                        contents: HoverContents::Scalar(MarkedString::String(format!(
                            "fn {}({}) -> {}",
                            name, p, r
                        ))),
                        range: None,
                    });
                }
            }
            if let crate::ast::Decl::Actor { name, .. } = decl {
                if name == word {
                    return Some(Hover {
                        contents: HoverContents::Scalar(MarkedString::String(format!(
                            "actor {}",
                            name
                        ))),
                        range: None,
                    });
                }
            }
            if let crate::ast::Decl::StateMachine { name, states, .. } = decl {
                if name == word {
                    return Some(Hover {
                        contents: HoverContents::Scalar(MarkedString::String(format!(
                            "state_machine {} (states: {})",
                            name,
                            states.join(", ")
                        ))),
                        range: None,
                    });
                }
            }
        }
        let kw = [
            "let", "fn", "fun", "actor", "agent", "if", "else", "match", "case", "for", "in",
            "spawn", "send", "ask", "perform", "handle", "emit", "return", "unit", "nil", "true",
            "false",
        ];
        if kw.contains(&word) {
            return Some(Hover {
                contents: HoverContents::Scalar(MarkedString::String(format!(
                    "keyword `{}`",
                    word
                ))),
                range: None,
            });
        }
        Some(Hover {
            contents: HoverContents::Scalar(MarkedString::String(format!("module type: {:?}", mt))),
            range: None,
        })
    }

    fn word_at<'a>(&self, line: &'a str, col: usize) -> Option<&'a str> {
        if col >= line.len() {
            return None;
        }
        let b = line.as_bytes();
        if !b[col].is_ascii_alphanumeric() && b[col] != b'_' {
            return None;
        }
        let mut s = col;
        while s > 0 && (b[s - 1].is_ascii_alphanumeric() || b[s - 1] == b'_') {
            s -= 1;
        }
        let mut e = col;
        while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') {
            e += 1;
        }
        Some(&line[s..e])
    }

    fn goto_def_local(&self, source: &str, position: Position, uri: &Url) -> Option<Location> {
        let line = position.line as usize;
        let col = position.character as usize;
        let target_line = source.lines().nth(line)?;
        let word = self.word_at(target_line, utf16_col_to_byte(target_line, col))?;
        let tokens = Lexer::new(source).lex().ok()?;
        let ast = Parser::new(tokens).parse_module().ok()?;
        let _ = TypeChecker::new().check_module(&ast).ok()?;
        for decl in &ast.decls {
            if let Some(loc) = self.find_decl(decl, word, uri) {
                return Some(loc);
            }
        }
        None
    }

    /// Follow import statements to find definitions in other files.
    fn goto_def_cross_file(
        &self,
        current_uri: &Url,
        word: &str,
        ast: &crate::ast::AstModule,
    ) -> Option<Location> {
        for decl in &ast.decls {
            if let crate::ast::Decl::Import { path, .. } = decl {
                let docs = self.documents.lock().unwrap();
                // Resolve relative to current file's directory
                if let Ok(mut resolved) = current_uri.to_file_path() {
                    resolved.pop();
                    resolved.push(path);
                    resolved.set_extension("nula");
                    if let Ok(imported_uri) = Url::from_file_path(&resolved) {
                        if let Some(doc) = docs.get(&imported_uri) {
                            if let Some(ref imported_ast) = doc.ast {
                                for d in &imported_ast.decls {
                                    if let Some(loc) = self.find_decl(d, word, &imported_uri) {
                                        return Some(loc);
                                    }
                                }
                            }
                        }
                    }
                }
                drop(docs);
            }
        }
        None
    }
    fn find_decl(&self, decl: &crate::ast::Decl, word: &str, uri: &Url) -> Option<Location> {
        use crate::ast::Decl;
        let loc = |s: &crate::types::Span| Location {
            uri: uri.clone(),
            range: Range {
                start: Position::new(
                    s.line().saturating_sub(1) as u32,
                    s.column().saturating_sub(1) as u32,
                ),
                end: Position::new(
                    s.line().saturating_sub(1) as u32,
                    s.column().saturating_sub(1) as u32,
                ),
            },
        };
        match decl {
            Decl::Function { name, span, .. }
            | Decl::Actor { name, span, .. }
            | Decl::Agent { name, span, .. }
            | Decl::Workflow { name, span, .. }
            | Decl::StateMachine { name, span, .. }
            | Decl::TypeAlias { name, span, .. }
                if name == word =>
            {
                Some(loc(span))
            }
            Decl::Module {
                name, decls, span, ..
            } => {
                if name == word {
                    return Some(loc(span));
                }
                for d in decls {
                    if let Some(l) = self.find_decl(d, word, uri) {
                        return Some(l);
                    }
                }
                None
            }
            _ => None,
        }
    }
    fn find_refs(&self, source: &str, position: Position, uri: &Url) -> Vec<Location> {
        let line = position.line as usize;
        let col = position.character as usize;
        let target_line = match source.lines().nth(line) {
            Some(l) => l,
            None => return vec![],
        };
        let word = match self.word_at(target_line, utf16_col_to_byte(target_line, col)) {
            Some(w) => w.to_string(),
            None => return vec![],
        };
        let tokens = match Lexer::new(source).lex() {
            Ok(t) => t,
            Err(_) => return vec![],
        };
        let ast = match Parser::new(tokens).parse_module() {
            Ok(a) => a,
            Err(_) => return vec![],
        };
        let _ = TypeChecker::new().check_module(&ast);
        let mut locs = Vec::new();
        for decl in &ast.decls {
            self.collect_refs(decl, &word, &mut locs, uri);
        }
        locs
    }
    fn collect_refs(
        &self,
        decl: &crate::ast::Decl,
        word: &str,
        locs: &mut Vec<Location>,
        uri: &Url,
    ) {
        use crate::ast::Decl;
        let loc = |s: &crate::types::Span| Location {
            uri: uri.clone(),
            range: Range {
                start: Position::new(
                    s.line().saturating_sub(1) as u32,
                    s.column().saturating_sub(1) as u32,
                ),
                end: Position::new(
                    s.line().saturating_sub(1) as u32,
                    s.column().saturating_sub(1) as u32,
                ),
            },
        };
        match decl {
            Decl::Function {
                name, body, span, ..
            } => {
                if name == word {
                    locs.push(loc(span));
                }
                self.refs_expr(body, word, locs);
            }
            Decl::Actor {
                name,
                behaviors,
                span,
                ..
            } => {
                if name == word {
                    locs.push(loc(span));
                }
                for b in behaviors {
                    self.refs_expr(&b.body, word, locs);
                }
            }
            Decl::StateMachine {
                name,
                entry_hooks,
                exit_hooks,
                span,
                ..
            } => {
                if name == word {
                    locs.push(loc(span));
                }
                for (_, body) in entry_hooks.iter().chain(exit_hooks.iter()) {
                    self.refs_expr(body, word, locs);
                }
            }
            Decl::Agent { name, span, .. }
            | Decl::Workflow { name, span, .. }
            | Decl::TypeAlias { name, span, .. }
                if name == word =>
            {
                locs.push(loc(span));
            }
            Decl::Module {
                name, decls, span, ..
            } => {
                if name == word {
                    locs.push(loc(span));
                }
                for d in decls {
                    self.collect_refs(d, word, locs, uri);
                }
            }
            _ => {}
        }
    }
    fn refs_expr(&self, expr: &crate::ast::Expr, word: &str, locs: &mut Vec<Location>) {
        use crate::ast::Expr;
        let loc = |s: &crate::types::Span| Location {
            uri: Url::parse("file:///current.nula").unwrap(),
            range: Range {
                start: Position::new(
                    s.line().saturating_sub(1) as u32,
                    s.column().saturating_sub(1) as u32,
                ),
                end: Position::new(
                    s.line().saturating_sub(1) as u32,
                    s.column().saturating_sub(1) as u32,
                ),
            },
        };
        match expr {
            Expr::Var(name, span) => {
                if name == word {
                    locs.push(loc(span));
                }
            }
            Expr::Binary { left, right, .. } => {
                self.refs_expr(left, word, locs);
                self.refs_expr(right, word, locs);
            }
            Expr::Let {
                name,
                value,
                body,
                span,
                ..
            } => {
                if name == word {
                    locs.push(loc(span));
                }
                self.refs_expr(value, word, locs);
                self.refs_expr(body, word, locs);
            }
            Expr::Block { exprs, .. } => {
                for e in exprs {
                    self.refs_expr(e, word, locs);
                }
            }
            Expr::App { func, args, .. } => {
                self.refs_expr(func, word, locs);
                for a in args {
                    self.refs_expr(a, word, locs);
                }
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                self.refs_expr(cond, word, locs);
                self.refs_expr(then_branch, word, locs);
                if let Some(ref eb) = else_branch {
                    self.refs_expr(eb, word, locs);
                }
            }
            Expr::Pipe { left, right, .. } => {
                self.refs_expr(left, word, locs);
                self.refs_expr(right, word, locs);
            }
            _ => {}
        }
    }

    fn doc_syms(&self, source: &str) -> Option<Vec<SymbolInformation>> {
        let tokens = Lexer::new(source).lex().ok()?;
        let ast = Parser::new(tokens).parse_module().ok()?;
        let mut syms = Vec::new();
        for decl in &ast.decls {
            self.collect_syms(decl, &mut syms);
        }
        Some(syms)
    }
    // `deprecated` is a required-but-deprecated field in lsp-types; we always
    // pass `None` (no symbols are marked deprecated), so silence the lint here.
    #[allow(deprecated)]
    fn collect_syms(&self, decl: &crate::ast::Decl, syms: &mut Vec<SymbolInformation>) {
        use crate::ast::Decl;
        let si = |name: &str, kind: SymbolKind, span: &crate::types::Span| SymbolInformation {
            name: name.to_string(),
            kind,
            tags: None,
            deprecated: None,
            location: Location {
                uri: Url::parse("file:///current.nula").unwrap(),
                range: Range {
                    start: Position::new(
                        span.line().saturating_sub(1) as u32,
                        span.column().saturating_sub(1) as u32,
                    ),
                    end: Position::new(
                        span.line().saturating_sub(1) as u32,
                        span.column().saturating_sub(1) as u32,
                    ),
                },
            },
            container_name: None,
        };
        match decl {
            Decl::Function { name, span, .. } => syms.push(si(name, SymbolKind::FUNCTION, span)),
            Decl::Actor {
                name,
                behaviors,
                span,
                ..
            } => {
                syms.push(si(name, SymbolKind::CLASS, span));
                for b in behaviors {
                    syms.push(si(
                        &format!("{}.{}", name, b.name),
                        SymbolKind::METHOD,
                        &b.span,
                    ));
                }
            }
            Decl::Agent { name, span, .. } => syms.push(si(name, SymbolKind::CLASS, span)),
            Decl::Workflow { name, span, .. } => syms.push(si(name, SymbolKind::CLASS, span)),
            Decl::StateMachine {
                name, events, span, ..
            } => {
                syms.push(si(name, SymbolKind::CLASS, span));
                for e in events {
                    syms.push(si(
                        &format!("{}.{}", name, e.name),
                        SymbolKind::EVENT,
                        &e.span,
                    ));
                }
            }
            Decl::TypeAlias { name, span, .. } => syms.push(si(name, SymbolKind::STRUCT, span)),
            Decl::Module {
                name, decls, span, ..
            } => {
                syms.push(si(name, SymbolKind::NAMESPACE, span));
                for d in decls {
                    self.collect_syms(d, syms);
                }
            }
            Decl::EffectDecl { name, span, .. } => syms.push(si(name, SymbolKind::INTERFACE, span)),
            _ => {}
        }
    }

    fn sig_help(source: &str, position: Position) -> Option<SignatureHelp> {
        let line = position.line as usize;
        let col = position.character as usize;
        let target_line = source.lines().nth(line)?;
        // LSP columns are UTF-16 code units; slice at a char boundary so
        // non-ASCII source cannot panic here.
        let prefix = &target_line[..utf16_col_to_byte(target_line, col)];
        let func_name = prefix
            .trim_end_matches(|c: char| c.is_whitespace() || c == '(' || c == ',')
            .rsplit(|c: char| c.is_whitespace() || c == '(' || c == ',')
            .next()?;
        if func_name.is_empty() || func_name == "let" || func_name == "if" {
            return None;
        }
        let comma_count = prefix.chars().filter(|&c| c == ',').count();
        let tokens = Lexer::new(source).lex().ok()?;
        let ast = Parser::new(tokens).parse_module().ok()?;
        let _ = TypeChecker::new().check_module(&ast).ok()?;
        for decl in &ast.decls {
            if let crate::ast::Decl::Function { name, params, .. } = decl {
                if name == func_name {
                    let label = format!(
                        "fn {}({})",
                        name,
                        params
                            .iter()
                            .map(|(n, t)| format!(
                                "{}: {}",
                                n,
                                t.as_ref()
                                    .map(|ty| format!("{:?}", ty))
                                    .unwrap_or_else(|| "?".into())
                            ))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    return Some(SignatureHelp {
                        signatures: vec![SignatureInformation {
                            label,
                            documentation: None,
                            parameters: None,
                            active_parameter: None,
                        }],
                        active_signature: Some(0),
                        active_parameter: Some(comma_count as u32),
                    });
                }
            }
        }
        None
    }

    fn fmt_source(&self, source: &str) -> String {
        let mut out = String::with_capacity(source.len());
        let mut indent: usize = 0;
        let w: usize = 4;
        let mut prev = false;
        for line in source.lines() {
            let t = line.trim();
            if t.is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                prev = true;
                continue;
            }
            let close = t
                .chars()
                .filter(|&c| c == '}' || c == ']' || c == ')')
                .count();
            let open = t
                .chars()
                .filter(|&c| c == '{' || c == '[' || c == '(')
                .count();
            let net = open as isize - close as isize;
            if close > 0 && close >= open {
                indent = indent.saturating_sub((close - open) * w);
            }
            if t.starts_with("in ") || t == "in" {
                indent = indent.saturating_sub(w);
            }
            if !prev && !out.is_empty() {
                out.push('\n');
            }
            for _ in 0..indent / w {
                out.push_str("    ");
            }
            out.push_str(t);
            if net > 0 {
                indent += net as usize * w;
            }
            if (t.ends_with("in") || t.ends_with("then")) && !t.contains('{') {
                indent += w;
            }
            prev = false;
        }
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out
    }

    fn sem_tokens(&self, source: &str) -> Vec<SemanticToken> {
        // Run capability analysis to find consumed linear variable spans.
        let consumed = self.find_consumed_spans(source);

        let mut tokens = Vec::new();
        let mut pl = 0u32;
        let mut pc = 0u32;
        let mut line = 0u32;
        let mut col = 0u32;
        let bytes = source.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let ch = bytes[i] as char;
            if ch == '\n' {
                line += 1;
                col = 0;
                i += 1;
                continue;
            }
            if ch.is_whitespace() {
                col += 1;
                i += 1;
                continue;
            }
            if ch == '-' && i + 1 < bytes.len() && bytes[i + 1] == b'-' {
                let end = source[i..].find('\n').unwrap_or(source.len() - i);
                tokens.push(SemanticToken {
                    delta_line: line - pl,
                    delta_start: if line == pl { col - pc } else { col },
                    length: end as u32,
                    token_type: 8,
                    token_modifiers_bitset: 0,
                });
                pl = line;
                pc = col;
                col += end as u32;
                i += end;
                continue;
            }
            if ch == '"' {
                if let Some(end) = source[i + 1..].find('"') {
                    let len = (end + 2) as u32;
                    tokens.push(SemanticToken {
                        delta_line: line - pl,
                        delta_start: if line == pl { col - pc } else { col },
                        length: len,
                        token_type: 5,
                        token_modifiers_bitset: 0,
                    });
                    pl = line;
                    pc = col;
                    col += len;
                    i += end + 2;
                    continue;
                }
            }
            if ch.is_ascii_alphabetic() || ch == '_' {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let word = &source[start..i];
                let len = (i - start) as u32;
                let kw = [
                    "let",
                    "fn",
                    "fun",
                    "actor",
                    "agent",
                    "workflow",
                    "if",
                    "else",
                    "match",
                    "case",
                    "for",
                    "in",
                    "spawn",
                    "send",
                    "ask",
                    "perform",
                    "handle",
                    "emit",
                    "return",
                    "break",
                    "unit",
                    "nil",
                    "true",
                    "false",
                    "iso",
                    "trn",
                    "ref",
                    "val",
                    "box",
                    "tag",
                    "lineariso",
                    "linear",
                    "type",
                    "effect",
                    "module",
                    "import",
                    "extern",
                    "self",
                    "and",
                    "or",
                    "not",
                ];
                let tt: u32 = if kw.contains(&word) { 0 } else { 2 };
                // Apply READONLY modifier if this variable was consumed (linear).
                let mut modifiers: u32 = 0;
                if tt == 2 {
                    if consumed.iter().any(|&(cs, ce)| cs <= start && start < ce) {
                        modifiers = 2; // READONLY bit
                    }
                }
                tokens.push(SemanticToken {
                    delta_line: line - pl,
                    delta_start: if line == pl { col - pc } else { col },
                    length: len,
                    token_type: tt,
                    token_modifiers_bitset: modifiers,
                });
                pl = line;
                pc = col;
                col += len;
                continue;
            }
            if ch.is_ascii_digit() {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b'.' {
                    i += 1;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let len = (i - start) as u32;
                tokens.push(SemanticToken {
                    delta_line: line - pl,
                    delta_start: if line == pl { col - pc } else { col },
                    length: len,
                    token_type: 6,
                    token_modifiers_bitset: 0,
                });
                pl = line;
                pc = col;
                col += len;
                continue;
            }
            if "=+-*/<>!|&^%.:;{},()[]".contains(ch) {
                tokens.push(SemanticToken {
                    delta_line: line - pl,
                    delta_start: if line == pl { col - pc } else { col },
                    length: 1,
                    token_type: 7,
                    token_modifiers_bitset: 0,
                });
                pl = line;
                pc = col;
                col += 1;
                i += 1;
                continue;
            }
            col += 1;
            i += 1;
        }
        tokens
    }

    /// Run the capability analysis pass and return the byte ranges of
    /// consumed linear/lineariso variable references.
    fn find_consumed_spans(&self, source: &str) -> Vec<(usize, usize)> {
        use crate::ast::Decl;
        use crate::effect_checker::{CapContext, CapabilityAnalyzer, flatten_decls};

        let mut lexer = crate::lexer::Lexer::new(source);
        let tokens = match lexer.lex() {
            Ok(t) => t,
            Err(_) => return vec![],
        };
        let mut parser = crate::parser::Parser::new(tokens);
        let ast = match parser.parse_module() {
            Ok(a) => a,
            Err(_) => return vec![],
        };

        let mut type_checker = crate::typechecker::TypeChecker::new();
        if type_checker.check_module(&ast).is_err() {
            return vec![];
        }

        let flat_decls = flatten_decls(&ast.decls);
        let mut cap_analyzer = CapabilityAnalyzer::new();
        let cap_ctx = CapContext::new();
        for decl in &flat_decls {
            if let Decl::Function { body, .. } = decl {
                let _ = cap_analyzer.infer_cap(&cap_ctx, body);
            }
        }

        cap_analyzer
            .consumed_spans
            .iter()
            .map(|s| (s.start as usize, s.end as usize))
            .collect()
    }

    fn code_actions(source: &str, range: Option<Range>, uri: &Url) -> Option<CodeActionResponse> {
        let mut actions = Vec::new();

        // Extract variable — only when a non-empty selection range is given.
        if let Some(ref sel) = range {
            if sel.start != sel.end {
                if let Some(action) = Self::extract_variable_action(source, sel, uri) {
                    actions.push(action);
                }
            }
        }

        // Existing: add type annotations for `let` bindings without explicit types.
        for (li, line) in source.lines().enumerate() {
            let t = line.trim();
            let ln = li as u32;
            if let Some(pos) = t.find("let ") {
                if !t[pos..].contains(':') {
                    let after = pos + 4;
                    let rest = &t[after..];
                    if let Some(end) = rest.find(|c: char| c == ' ' || c == '=') {
                        let vname = &rest[..end];
                        let Some(eq) = t.find('=') else { continue };
                        let rhs = t[eq + 1..].trim();
                        let ty = if rhs.parse::<i64>().is_ok() {
                            "Int"
                        } else if rhs.starts_with('"') {
                            "String"
                        } else if rhs == "true" || rhs == "false" {
                            "Bool"
                        } else {
                            "a"
                        };
                        let edit = TextEdit {
                            range: Range {
                                start: Position::new(ln, (pos + eq) as u32),
                                end: Position::new(ln, (pos + eq) as u32),
                            },
                            new_text: format!(" : {}", ty),
                        };
                        let mut changes = std::collections::HashMap::new();
                        changes.insert(uri.clone(), vec![edit]);
                        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                            title: format!("Add type annotation ': {}' for '{}'", ty, vname),
                            kind: Some(CodeActionKind::QUICKFIX),
                            diagnostics: None,
                            edit: Some(WorkspaceEdit {
                                changes: Some(changes),
                                ..WorkspaceEdit::default()
                            }),
                            command: None,
                            is_preferred: Some(false),
                            disabled: None,
                            data: None,
                        }));
                    }
                }
            }
        }
        if actions.is_empty() {
            None
        } else {
            Some(actions)
        }
    }

    /// Offer "Extract to variable" for a selected expression.
    ///
    /// Simple version: if the selection is on a line with `= expr`, replace
    /// from `=` to end of line with `let temp = expr in temp`.
    fn extract_variable_action(
        source: &str,
        range: &Range,
        uri: &Url,
    ) -> Option<CodeActionOrCommand> {
        let selected = Self::extract_selected_text(source, range)?;
        let sel = selected.trim();
        if sel.is_empty() || sel.len() == 1 {
            return None;
        }

        let line_num = range.start.line;
        let line = source.lines().nth(line_num as usize)?;

        // Look for `= expr` pattern on this line.
        if let Some(eq_pos) = line.find('=') {
            let rhs = line[eq_pos + 1..].trim();
            if rhs.is_empty() {
                return None;
            }
            let edit = TextEdit {
                range: Range {
                    start: Position::new(line_num, eq_pos as u32 + 1),
                    end: Position::new(line_num, line.len() as u32),
                },
                new_text: format!(" let temp = {} in temp", rhs),
            };
            let mut changes = std::collections::HashMap::new();
            changes.insert(uri.clone(), vec![edit]);
            return Some(CodeActionOrCommand::CodeAction(CodeAction {
                title: format!("Extract '{}' to variable", sel),
                kind: Some(CodeActionKind::REFACTOR_EXTRACT),
                edit: Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..WorkspaceEdit::default()
                }),
                is_preferred: Some(true),
                ..CodeAction::default()
            }));
        }
        None
    }

    /// Extract the text covered by an LSP range from the source.
    fn extract_selected_text(source: &str, range: &Range) -> Option<String> {
        let start = Self::position_to_byte_offset(source, &range.start)?;
        let end = Self::position_to_byte_offset(source, &range.end)?;
        if start >= end {
            return None;
        }
        Some(source[start..end].to_string())
    }

    /// Compute folding ranges by tracking brace depth line-by-line.
    fn compute_folding_ranges(source: &str) -> Vec<FoldingRange> {
        let mut ranges = Vec::new();
        let mut stack: Vec<(u32, u32)> = Vec::new();
        let lines: Vec<&str> = source.lines().collect();

        for (li, line) in lines.iter().enumerate() {
            let line_num = li as u32;
            let mut opens = 0i32;
            let mut closes = 0i32;
            let mut in_string = false;
            let bytes = line.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                let c = bytes[i];
                if in_string {
                    if c == b'"' && (i == 0 || bytes[i - 1] != b'\\') {
                        in_string = false;
                    }
                } else if c == b'"' {
                    in_string = true;
                } else if c == b'{' {
                    opens += 1;
                } else if c == b'}' {
                    closes += 1;
                }
                i += 1;
            }

            for _ in 0..closes {
                if let Some((start, _)) = stack.pop() {
                    if start < line_num {
                        ranges.push(FoldingRange {
                            start_line: start,
                            end_line: line_num,
                            start_character: None,
                            end_character: None,
                            kind: Some(FoldingRangeKind::Region),
                            collapsed_text: None,
                        });
                    }
                }
            }
            for _ in 0..opens {
                stack.push((line_num, stack.len() as u32));
            }
        }

        let eof_line = lines.len().saturating_sub(1) as u32;
        while let Some((start, _)) = stack.pop() {
            if start < eof_line {
                ranges.push(FoldingRange {
                    start_line: start,
                    end_line: eof_line,
                    start_character: None,
                    end_character: None,
                    kind: Some(FoldingRangeKind::Region),
                    collapsed_text: None,
                });
            }
        }

        ranges
    }
    fn compute_diagnostics(source: &str) -> (Vec<Diagnostic>, HashMap<usize, String>) {
        let mut diagnostics = Vec::new();

        // Lex
        let tokens = match Lexer::new(source).lex() {
            Ok(t) => t,
            Err(e) => {
                diagnostics.extend(nu_error_to_diagnostic(e));
                return (diagnostics, HashMap::new());
            }
        };

        // Parse
        let ast = match Parser::new(tokens).parse_module() {
            Ok(a) => a,
            Err(e) => {
                diagnostics.extend(nu_error_to_diagnostic(e));
                return (diagnostics, HashMap::new());
            }
        };

        // Type check
        if let Err(e) = TypeChecker::new().check_module(&ast) {
            diagnostics.extend(nu_error_to_diagnostic(e));
            return (diagnostics, HashMap::new());
        }

        // Effect check: same two-pass driver as the CLI frontend
        // (`run_frontend` in main.rs) — `check_module` flattens nested
        // `module {}` decls, registers function rows so callee effects
        // propagate to call sites (pass 1), then enforces declared rows
        // (pass 2). Stops at the first fatal error.
        let mut effect_checker = EffectChecker::new();
        if let Err(e) = effect_checker.check_module(&ast.decls) {
            diagnostics.extend(nu_error_to_diagnostic(e));
        }
        for msg in &effect_checker.diagnostics {
            diagnostics.push(Diagnostic {
                range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                severity: Some(DiagnosticSeverity::WARNING),
                code: None,
                code_description: None,
                source: Some("nulang-effect".to_string()),
                message: msg.clone(),
                related_information: None,
                tags: None,
                data: None,
            });
        }

        // Capability analysis over the flattened declaration list, so
        // functions nested in `module {}` blocks are checked like top-level
        // ones (mirroring the CLI frontend).
        let mut cap_analyzer = CapabilityAnalyzer::new();
        let cap_ctx = CapContext::new();
        for decl in crate::effect_checker::flatten_decls(&ast.decls) {
            if let crate::ast::Decl::Function { body, .. } = decl {
                if let Err(e) = cap_analyzer.infer_cap(&cap_ctx, body) {
                    diagnostics.extend(nu_error_to_diagnostic(e));
                }
            }
        }
        for msg in &cap_analyzer.diagnostics {
            diagnostics.push(Diagnostic {
                range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                severity: Some(DiagnosticSeverity::WARNING),
                code: None,
                code_description: None,
                source: Some("nulang-capability".to_string()),
                message: msg.clone(),
                related_information: None,
                tags: None,
                data: None,
            });
        }

        let type_map = Self::extract_type_map(source, &ast);
        (diagnostics, type_map)
    }

    /// Walk the AST and extract type information for `let` bindings.
    fn extract_type_map(source: &str, ast: &crate::ast::AstModule) -> HashMap<usize, String> {
        let mut map = HashMap::new();
        for decl in crate::effect_checker::flatten_decls(&ast.decls) {
            Self::extract_decl_types(decl, source, &mut map);
        }
        map
    }

    fn extract_decl_types(decl: &crate::ast::Decl, source: &str, map: &mut HashMap<usize, String>) {
        use crate::ast::Decl;
        match decl {
            Decl::Function { body, .. } => {
                Self::extract_expr_types(body, source, map);
            }
            Decl::Actor {
                behaviors,
                init,
                initializer,
                ..
            } => {
                for b in behaviors {
                    Self::extract_expr_types(&b.body, source, map);
                }
                for (_, e) in init {
                    Self::extract_expr_types(e, source, map);
                }
                if let Some((_, _, body)) = initializer {
                    Self::extract_expr_types(body, source, map);
                }
            }
            Decl::StateMachine {
                entry_hooks,
                exit_hooks,
                ..
            } => {
                for (_, body) in entry_hooks {
                    Self::extract_expr_types(body, source, map);
                }
                for (_, body) in exit_hooks {
                    Self::extract_expr_types(body, source, map);
                }
            }
            Decl::Workflow {
                items, compensate, ..
            } => {
                for item in items {
                    match item {
                        crate::ast::WorkflowItem::Step(s) => {
                            Self::extract_expr_types(&s.body, source, map);
                            if let Some(ref c) = s.compensate {
                                Self::extract_expr_types(c, source, map);
                            }
                        }
                        crate::ast::WorkflowItem::Parallel(steps) => {
                            for s in steps {
                                Self::extract_expr_types(&s.body, source, map);
                                if let Some(ref c) = s.compensate {
                                    Self::extract_expr_types(c, source, map);
                                }
                            }
                        }
                    }
                }
                if let Some(ref c) = compensate {
                    Self::extract_expr_types(c, source, map);
                }
            }
            _ => {}
        }
    }

    fn extract_expr_types(expr: &crate::ast::Expr, source: &str, map: &mut HashMap<usize, String>) {
        use crate::ast::Expr;
        match expr {
            Expr::Let {
                name,
                ty,
                value,
                body,
                span,
            } => {
                if let Some(ref t) = ty {
                    if let Some(pos) = Self::find_ident_position(source, span.start as usize, name)
                    {
                        map.insert(pos, t.to_string());
                    }
                }
                Self::extract_expr_types(value, source, map);
                Self::extract_expr_types(body, source, map);
            }
            Expr::LetRec { value, body, .. } => {
                Self::extract_expr_types(value, source, map);
                Self::extract_expr_types(body, source, map);
            }
            Expr::Block { exprs, .. } => {
                for e in exprs {
                    Self::extract_expr_types(e, source, map);
                }
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                Self::extract_expr_types(cond, source, map);
                Self::extract_expr_types(then_branch, source, map);
                if let Some(e) = else_branch {
                    Self::extract_expr_types(e, source, map);
                }
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                Self::extract_expr_types(scrutinee, source, map);
                for (_, guard, body) in arms {
                    if let Some(g) = guard {
                        Self::extract_expr_types(&g, source, map);
                    }
                    Self::extract_expr_types(body, source, map);
                }
            }
            Expr::Lambda { body, .. } => {
                Self::extract_expr_types(body, source, map);
            }
            Expr::App { func, args, .. } => {
                Self::extract_expr_types(func, source, map);
                for a in args {
                    Self::extract_expr_types(a, source, map);
                }
            }
            Expr::Binary { left, right, .. } => {
                Self::extract_expr_types(left, source, map);
                Self::extract_expr_types(right, source, map);
            }
            Expr::Unary { expr, .. } => {
                Self::extract_expr_types(expr, source, map);
            }
            Expr::Tuple(exprs, _) | Expr::Array(exprs, _) => {
                for e in exprs {
                    Self::extract_expr_types(e, source, map);
                }
            }
            Expr::Record(fields, _) => {
                for (_, e) in fields {
                    Self::extract_expr_types(e, source, map);
                }
            }
            Expr::FieldAccess { expr, .. } => {
                Self::extract_expr_types(expr, source, map);
            }
            Expr::Index { arr, idx, .. } => {
                Self::extract_expr_types(arr, source, map);
                Self::extract_expr_types(idx, source, map);
            }
            Expr::Assign { target, value, .. } => {
                Self::extract_expr_types(target, source, map);
                Self::extract_expr_types(value, source, map);
            }
            Expr::Spawn {
                actor_type,
                init,
                positional_args,
                ..
            } => {
                Self::extract_expr_types(actor_type, source, map);
                for (_, e) in init {
                    Self::extract_expr_types(e, source, map);
                }
                if let Some(ref args) = positional_args {
                    for a in args {
                        Self::extract_expr_types(a, source, map);
                    }
                }
            }
            Expr::Send { actor, args, .. } | Expr::Ask { actor, args, .. } => {
                Self::extract_expr_types(actor, source, map);
                for a in args {
                    Self::extract_expr_types(a, source, map);
                }
            }
            Expr::Receive { arms, after, .. } => {
                for (_, _, _, body) in arms {
                    Self::extract_expr_types(body, source, map);
                }
                if let Some((ref timeout, ref body)) = after {
                    Self::extract_expr_types(timeout, source, map);
                    Self::extract_expr_types(body, source, map);
                }
            }
            Expr::Emit { args, .. } | Expr::Perform { args, .. } => {
                for a in args {
                    Self::extract_expr_types(a, source, map);
                }
            }
            Expr::Handle { body, handlers, .. } => {
                Self::extract_expr_types(body, source, map);
                for h in handlers {
                    Self::extract_expr_types(&h.body, source, map);
                }
            }
            Expr::Migrate { actor, node, .. } => {
                Self::extract_expr_types(actor, source, map);
                Self::extract_expr_types(node, source, map);
            }
            Expr::CapAnnotate { expr, .. } | Expr::TypeAnnotate { expr, .. } => {
                Self::extract_expr_types(expr, source, map);
            }
            Expr::For { iterable, body, .. }
            | Expr::While {
                cond: iterable,
                body,
                ..
            } => {
                Self::extract_expr_types(iterable, source, map);
                Self::extract_expr_types(body, source, map);
            }
            Expr::Return(opt_expr, _) | Expr::Break(opt_expr, _) => {
                if let Some(ref e) = opt_expr {
                    Self::extract_expr_types(e, source, map);
                }
            }
            Expr::Pipe { left, right, .. } => {
                Self::extract_expr_types(left, source, map);
                Self::extract_expr_types(right, source, map);
            }
            // Leaf nodes: no sub-expressions to walk
            Expr::Literal(..) | Expr::Var(..) | Expr::SelfRef(..) => {}
        }
    }
    /// Find the byte offset of an identifier within the source, searching
    /// backwards from `near_offset`. Used to locate the position of a
    /// `let`-bound variable name (which appears before the `:` or `=` token
    /// whose span marks `near_offset`).
    fn find_ident_position(source: &str, near_offset: usize, name: &str) -> Option<usize> {
        let search_start = near_offset.saturating_sub(50);
        let search_end = source.len().min(near_offset + name.len());
        let search_region = &source[search_start..search_end];
        // Find the name in the search region, ensuring it's a whole identifier.
        // Collect into a Vec so we can pick the last (nearest to near_offset).
        let matches: Vec<usize> = search_region
            .match_indices(name)
            .filter(|&(pos, _)| {
                let abs_pos = search_start + pos;
                let before = if abs_pos > 0 {
                    source
                        .as_bytes()
                        .get(abs_pos - 1)
                        .map_or(false, |&b| !b.is_ascii_alphanumeric() && b != b'_')
                } else {
                    true
                };
                let after = source
                    .as_bytes()
                    .get(abs_pos + name.len())
                    .map_or(true, |&b| !b.is_ascii_alphanumeric() && b != b'_');
                before && after
            })
            .map(|(pos, _)| search_start + pos)
            .collect();
        matches.into_iter().next_back()
    }

    /// Convert an LSP Position to a byte offset within the source.
    fn position_to_byte_offset(source: &str, position: &Position) -> Option<usize> {
        let line = position.line as usize;
        let lines: Vec<&str> = source.lines().collect();
        let target_line = lines.get(line)?;
        let col_byte = utf16_col_to_byte(target_line, position.character as usize);
        let prev_bytes: usize = lines.iter().take(line).map(|l| l.len() + 1).sum();
        Some(prev_bytes + col_byte)
    }

    /// Find the nearest type entry at or before the given byte offset.
    fn find_type_at_offset(map: &HashMap<usize, String>, offset: usize) -> Option<&String> {
        map.iter()
            .filter(|(&k, _)| k <= offset)
            .max_by_key(|(&k, _)| k)
            .map(|(_, v)| v)
    }
}

/// Convert a `NuError` into zero or more LSP `Diagnostic`s.
/// For `NuError::Multiple`, each sub-error becomes its own diagnostic.
fn nu_error_to_diagnostic(err: NuError) -> Vec<Diagnostic> {
    match err {
        NuError::Multiple(errors) => {
            let mut diags = Vec::new();
            for err in errors {
                diags.extend(nu_error_to_diagnostic(err));
            }
            diags
        }
        other => vec![single_diagnostic(other)],
    }
}

/// Build a single LSP Diagnostic from a non-Multiple NuError.
fn single_diagnostic(err: NuError) -> Diagnostic {
    let (message, start_line, start_col, end_line, end_col) = match err {
        NuError::LexError { msg, span }
        | NuError::ParseError { msg, span, .. }
        | NuError::TypeError { msg, span, .. }
        | NuError::EffectError { msg, span, .. }
        | NuError::CapError { msg, span, .. }
        | NuError::FFIError { msg, span }
        | NuError::NotYetImplemented { feature: msg, span } => (
            msg,
            span.line(),
            span.column(),
            span.end_line(),
            span.end_column(),
        ),
        NuError::RuntimeError { msg, span }
        | NuError::VMError { msg, span }
        | NuError::PythonError { msg, span }
        | NuError::PackageError { msg, span } => (
            msg,
            span.line(),
            span.column(),
            span.end_line(),
            span.end_column(),
        ),
        NuError::Suspended(kind) => (format!("VM suspended: {}", kind), 1, 1, 1, 1),
        NuError::Multiple(_) => unreachable!("Multiple handled by caller"),
    };

    let start = Position::new(
        start_line.saturating_sub(1) as u32,
        start_col.saturating_sub(1) as u32,
    );
    let end = Position::new(
        end_line.saturating_sub(1) as u32,
        end_col.saturating_sub(1) as u32,
    );

    Diagnostic {
        range: Range::new(start, end),
        severity: Some(DiagnosticSeverity::ERROR),
        code: None,
        code_description: None,
        source: Some("nulang".to_string()),
        message,
        related_information: None,
        tags: None,
        data: None,
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
        annotations
            .into_iter()
            .map(|a| self.annotation_to_inlay(a))
            .collect()
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
                        let _var_name = &rest[..end];
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
                        // A malformed line with `)` before `(` must not panic
                        // the parameter slice below.
                        if rparen > lparen {
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
            }

            // Capability annotations (iso, val, trn, ref, box, tag)
            for cap in &["iso", "val", "trn", "ref", "box", "tag", "linear"] {
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
        if expr.is_empty() {
            return None;
        }

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
            || (expr.starts_with('\'') && expr.ends_with('\''))
        {
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
    function_names: Vec<String>,
    /// (name, [param_names]) for snippet generation.
    function_params: Vec<(String, Vec<String>)>,
    variant_names: Vec<String>,
    type_names: Vec<String>,
    /// (actor_name, [field_names]).
    actor_state_fields: Vec<(String, Vec<String>)>,
    /// (type_name, [field_names]).
    record_type_fields: Vec<(String, Vec<String>)>,
    /// (behavior_name, [param_names]) for snippet generation.
    behavior_params: Vec<(String, Vec<String>)>,
}

impl<'a> CompletionEngine<'a> {
    /// Nulang language keywords with markdown documentation.
    const KEYWORDS: &'static [(&'static str, &'static str)] = &[
        (
            "fn",
            "Declare a function.\n\n```nulang\nfn add(x: Int, y: Int) -> Int { x + y }\n```",
        ),
        (
            "let",
            "Bind a value to a name.\n\n```nulang\nlet x = 42\nlet y: Int = x + 1\n```",
        ),
        (
            "if",
            "Conditional expression.\n\n```nulang\nif x > 0 { \"positive\" } else { \"non-positive\" }\n```",
        ),
        (
            "else",
            "Else branch of a conditional.\n\n```nulang\nif x > 0 { \"positive\" } else { \"non-positive\" }\n```",
        ),
        (
            "match",
            "Pattern match expression.\n\n```nulang\nmatch x {\n  | Some(v) => v\n  | None => 0\n}\n```",
        ),
        (
            "effect",
            "Declare an effect.\n\n```nulang\neffect MyEffect {\n  op1: Int -> String\n}\n```",
        ),
        (
            "actor",
            "Declare an actor.\n\n```nulang\nactor Counter {\n  state count = 0\n  behavior inc() { self.count = self.count + 1 }\n}\n```",
        ),
        (
            "state_machine",
            "Declare a state machine.\n\n```nulang\nstate_machine Door {\n  state Closed\n  event open(): Open\n}\n```",
        ),
        (
            "type",
            "Declare a type alias, record, or variant.\n\n```nulang\ntype Point = { x: Int, y: Int }\ntype Option[T] = Some(T) | None\n```",
        ),
        (
            "module",
            "Declare a module.\n\n```nulang\nmodule MyModule {\n  fn foo() { 42 }\n}\n```",
        ),
        (
            "import",
            "Import a module.\n\n```nulang\nimport \"path/to/file.nula\"\nimport stdlib::json\n```",
        ),
        (
            "handle",
            "Handle effects.\n\n```nulang\nhandle perform IO.print(\"x\") with {\n  | IO.print(s) => { /* custom logic */ }\n  | return(x) => x\n}\n```",
        ),
        (
            "perform",
            "Perform an effect operation.\n\n```nulang\nperform IO.print(\"Hello\")\nperform Http.get(\"https://example.com\")\n```",
        ),
        (
            "resume",
            "Resume a handled effect continuation.\n\n```nulang\nhandle perform MyEffect.op() with {\n  | MyEffect.op() => resume(\"result\")\n}\n```",
        ),
        (
            "return",
            "Return a value from a function.\n\n```nulang\nfn foo() -> Int { return 42 }\n```",
        ),
        ("true", "Boolean `true` literal."),
        ("false", "Boolean `false` literal."),
        ("nil", "Nil / null value."),
        ("unit", "Unit value (void)."),
    ];

    /// Built-in effect names.
    const EFFECTS: &'static [&'static str] = &[
        "IO",
        "Net",
        "FS",
        "Spawn",
        "Send",
        "Receive",
        "Migrate",
        "STM",
        "Async",
        "Inference",
        "Cost",
        "Rand",
        "Time",
        "Actor",
        "Provider",
    ];

    /// Known stdlib modules for import path completion.
    const STDLIB_MODULES: &'static [&'static str] = &[
        "core", "set", "string", "list", "map", "math", "json", "http", "test",
    ];

    pub fn new(source: &'a str) -> Self {
        CompletionEngine {
            source,
            function_names: Vec::new(),
            function_params: Vec::new(),
            variant_names: Vec::new(),
            type_names: Vec::new(),
            actor_state_fields: Vec::new(),
            record_type_fields: Vec::new(),
            behavior_params: Vec::new(),
        }
    }

    /// Populate cached names and parameter info from a parsed AST.
    pub fn set_ast_info(&mut self, ast: &crate::ast::AstModule) {
        for decl in &ast.decls {
            match decl {
                crate::ast::Decl::Function { name, params, .. } => {
                    self.function_names.push(name.clone());
                    let param_names: Vec<String> = params.iter().map(|(n, _)| n.clone()).collect();
                    self.function_params.push((name.clone(), param_names));
                }
                crate::ast::Decl::Actor {
                    name,
                    behaviors,
                    state_fields,
                    ..
                } => {
                    self.type_names.push(name.clone());
                    let field_names: Vec<String> =
                        state_fields.iter().map(|(n, _, _, _)| n.clone()).collect();
                    self.actor_state_fields.push((name.clone(), field_names));
                    for b in behaviors {
                        self.function_names.push(b.name.clone());
                        let bparams: Vec<String> =
                            b.params.iter().map(|(n, _)| n.clone()).collect();
                        self.behavior_params.push((b.name.clone(), bparams));
                    }
                }
                crate::ast::Decl::VariantType { name, variants, .. } => {
                    self.type_names.push(name.clone());
                    for (vname, _) in variants {
                        self.variant_names.push(vname.clone());
                    }
                }
                crate::ast::Decl::TypeAlias { name, .. } => {
                    self.type_names.push(name.clone());
                }
                crate::ast::Decl::RecordType { name, fields, .. } => {
                    self.type_names.push(name.clone());
                    let rfields: Vec<String> = fields.iter().map(|(n, _)| n.clone()).collect();
                    self.record_type_fields.push((name.clone(), rfields));
                }
                _ => {}
            }
        }
    }

    // ------------------------------------------------------------------
    // Public entry point
    // ------------------------------------------------------------------

    /// Return completion items at the given LSP position.
    pub fn complete(&self, position: Position, document_dir: Option<&Path>) -> Vec<CompletionItem> {
        let offset = self.position_to_offset(position);

        // 1. Field access: `self.` or `expr.`
        if let Some(ident) = self.field_access_before(offset) {
            let prefix = self.prefix_at(offset);
            return self.complete_field_access(&ident, &prefix);
        }

        // 2. Import path: `import "` or `import stdlib::`
        if let Some(path_prefix) = self.import_context(offset) {
            return self.complete_import_path(&path_prefix, document_dir);
        }

        // 3. Regular identifier completion
        self.complete_ident(position)
    }

    // ------------------------------------------------------------------
    // Regular identifier completions
    // ------------------------------------------------------------------

    fn complete_ident(&self, position: Position) -> Vec<CompletionItem> {
        let offset = self.position_to_offset(position);
        let prefix = self.prefix_at(offset);
        let prefix_lower = prefix.to_lowercase();

        let mut items = Vec::new();

        // Local let bindings — highest priority (sort "0").
        for name in self.local_bindings() {
            if name.to_lowercase().starts_with(&prefix_lower) {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some("local binding".to_string()),
                    sort_text: Some(format!("0_{}", name)),
                    ..CompletionItem::default()
                });
            }
        }

        // Cached function names from AST — with snippet for params (sort "1").
        for name in &self.function_names {
            if name.to_lowercase().starts_with(&prefix_lower) {
                items.push(self.make_function_item(name));
            }
        }

        // Regex-based function names (fallback) — lower priority (sort "1b").
        for name in self.top_level_functions() {
            let nl = name.to_lowercase();
            if nl.starts_with(&prefix_lower)
                && !self.function_names.iter().any(|n| n.to_lowercase() == nl)
            {
                items.push(CompletionItem {
                    label: name,
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: Some("function".to_string()),
                    sort_text: Some(format!("1b_{}", name)),
                    ..CompletionItem::default()
                });
            }
        }

        // Cached type names (sort "2").
        for name in &self.type_names {
            if name.to_lowercase().starts_with(&prefix_lower) {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::CLASS),
                    detail: Some("type".to_string()),
                    sort_text: Some(format!("2_{}", name)),
                    ..CompletionItem::default()
                });
            }
        }

        // Cached variant constructors (sort "3").
        for name in &self.variant_names {
            if name.to_lowercase().starts_with(&prefix_lower) {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::CONSTRUCTOR),
                    detail: Some("variant".to_string()),
                    sort_text: Some(format!("3_{}", name)),
                    ..CompletionItem::default()
                });
            }
        }

        // Keywords — with markdown documentation (sort "4").
        for &(kw, doc) in Self::KEYWORDS {
            if kw.to_lowercase().starts_with(&prefix_lower) {
                items.push(CompletionItem {
                    label: kw.to_string(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    documentation: Some(Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: doc.to_string(),
                    })),
                    sort_text: Some(format!("4_{}", kw)),
                    ..CompletionItem::default()
                });
            }
        }

        // Built-in effects (sort "5").
        for &eff in Self::EFFECTS {
            let eff_lower = eff.to_lowercase();
            if eff_lower.starts_with(prefix_lower.as_str()) {
                items.push(CompletionItem {
                    label: eff.to_string(),
                    kind: Some(CompletionItemKind::ENUM_MEMBER),
                    detail: Some("built-in effect".to_string()),
                    sort_text: Some(format!("5_{}", eff)),
                    ..CompletionItem::default()
                });
            }
        }

        items
    }

    /// Build a function/behavior completion item with a snippet when params
    /// are known.
    fn make_function_item(&self, name: &str) -> CompletionItem {
        // Check behavior params first.
        if let Some((_, bparams)) = self.behavior_params.iter().find(|(n, _)| n == name) {
            let snippet = if bparams.is_empty() {
                name.to_string()
            } else {
                let placeholders: Vec<String> = bparams
                    .iter()
                    .enumerate()
                    .map(|(i, p)| format!("${{{}:{}}}", i + 1, p))
                    .collect();
                format!("{} {}", name, placeholders.join(" "))
            };
            return CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some("behavior".to_string()),
                insert_text: Some(snippet),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                sort_text: Some(format!("0b_{}", name)),
                ..CompletionItem::default()
            };
        }

        // Check function params.
        if let Some((_, fparams)) = self.function_params.iter().find(|(n, _)| n == name) {
            let snippet = if fparams.is_empty() {
                format!("{}()", name)
            } else {
                let placeholders: Vec<String> = fparams
                    .iter()
                    .enumerate()
                    .map(|(i, p)| format!("${{{}:{}}}", i + 1, p))
                    .collect();
                format!("{}({})", name, placeholders.join(", "))
            };
            return CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some("function".to_string()),
                insert_text: Some(snippet),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                sort_text: Some(format!("1_{}", name)),
                ..CompletionItem::default()
            };
        }

        // No params known — plain completion.
        CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some("function".to_string()),
            sort_text: Some(format!("1_{}", name)),
            ..CompletionItem::default()
        }
    }

    // ------------------------------------------------------------------
    // Field access completions (`self.` or `expr.`)
    // ------------------------------------------------------------------

    /// Check whether the cursor is positioned after `ident.` — return the
    /// identifier before the dot.
    fn field_access_before(&self, offset: usize) -> Option<String> {
        let bytes = self.source.as_bytes();
        // Walk back from offset past any identifier characters (the partial
        // field name).
        let mut pos = offset;
        while pos > 0 {
            let b = bytes[pos - 1];
            if b.is_ascii_alphanumeric() || b == b'_' {
                pos -= 1;
            } else {
                break;
            }
        }
        // Expect a dot right before the identifier.
        if pos == 0 || bytes[pos - 1] != b'.' {
            return None;
        }
        // Walk back from before the dot to collect the object identifier.
        let dot_pos = pos - 1;
        let mut ident_end = dot_pos;
        while ident_end > 0 && bytes[ident_end - 1].is_ascii_whitespace() {
            ident_end -= 1;
        }
        let mut ident_start = ident_end;
        while ident_start > 0 {
            let b = bytes[ident_start - 1];
            if b.is_ascii_alphanumeric() || b == b'_' {
                ident_start -= 1;
            } else {
                break;
            }
        }
        if ident_start < ident_end {
            Some(self.source[ident_start..ident_end].to_string())
        } else {
            None
        }
    }

    /// Offer field completions for the given object identifier.
    fn complete_field_access(&self, ident: &str, prefix: &str) -> Vec<CompletionItem> {
        let prefix_lower = prefix.to_lowercase();
        let mut items = Vec::new();

        // `self.` → actor state fields.
        if ident == "self" {
            for (actor_name, fields) in &self.actor_state_fields {
                for f in fields {
                    if f.to_lowercase().starts_with(&prefix_lower) {
                        items.push(CompletionItem {
                            label: f.clone(),
                            kind: Some(CompletionItemKind::FIELD),
                            detail: Some(format!("state field of {}", actor_name)),
                            sort_text: Some(format!("0_{}", f)),
                            ..CompletionItem::default()
                        });
                    }
                }
            }
            return items;
        }

        // Type-name-based record field access.
        for (type_name, fields) in &self.record_type_fields {
            if type_name.to_lowercase() == ident.to_lowercase() {
                for f in fields {
                    if f.to_lowercase().starts_with(&prefix_lower) {
                        items.push(CompletionItem {
                            label: f.clone(),
                            kind: Some(CompletionItemKind::FIELD),
                            detail: Some(format!("field of {}", type_name)),
                            sort_text: Some(format!("0_{}", f)),
                            ..CompletionItem::default()
                        });
                    }
                }
            }
        }

        items
    }

    // ------------------------------------------------------------------
    // Import path completions
    // ------------------------------------------------------------------

    /// Check whether the cursor is inside an import path string or after
    /// `stdlib::`, returning the prefix typed so far.
    fn import_context(&self, offset: usize) -> Option<String> {
        let line = self.current_line(offset)?;
        let line_trimmed = line.trim();

        // `import "path"`
        if let Some(rest) = line_trimmed.strip_prefix("import \"") {
            // Compute how far into the path the cursor is.
            let col_in_trimmed = offset.saturating_sub(
                self.source[..offset]
                    .rfind('\n')
                    .map(|i| i + 1)
                    .unwrap_or(0)
                    + (line.len() - line_trimmed.len()),
            );
            // `import "` is 8 chars.
            if col_in_trimmed >= 8 {
                let path_so_far = &rest[..(col_in_trimmed - 8).min(rest.len())];
                return Some(path_so_far.to_string());
            }
            return Some(String::new());
        }

        // `import stdlib::mod`
        if let Some(rest) = line_trimmed.strip_prefix("import stdlib::") {
            let col_in_trimmed = offset.saturating_sub(
                self.source[..offset]
                    .rfind('\n')
                    .map(|i| i + 1)
                    .unwrap_or(0)
                    + (line.len() - line_trimmed.len()),
            );
            let prefix_len = "import stdlib::".len();
            if col_in_trimmed >= prefix_len {
                let mod_so_far = &rest[..(col_in_trimmed - prefix_len).min(rest.len())];
                return Some(format!("stdlib::{}", mod_so_far));
            }
            return Some("stdlib::".to_string());
        }

        None
    }

    /// Offer import path completions.
    fn complete_import_path(
        &self,
        prefix: &str,
        document_dir: Option<&Path>,
    ) -> Vec<CompletionItem> {
        let mut items = Vec::new();

        // `stdlib::` prefix → suggest known stdlib modules.
        if let Some(mod_prefix) = prefix.strip_prefix("stdlib::") {
            let mod_prefix_lower = mod_prefix.to_lowercase();
            for &mod_name in Self::STDLIB_MODULES {
                if mod_name.to_lowercase().starts_with(&mod_prefix_lower) {
                    let full = format!("stdlib::{}", mod_name);
                    items.push(CompletionItem {
                        label: full.clone(),
                        kind: Some(CompletionItemKind::MODULE),
                        detail: Some("stdlib module".to_string()),
                        insert_text: Some(full),
                        sort_text: Some(format!("0_{}", full)),
                        ..CompletionItem::default()
                    });
                }
            }
            return items;
        }

        // For bare `import "` — suggest stdlib:: paths and local .nula files.
        for &mod_name in Self::STDLIB_MODULES {
            let full = format!("stdlib::{}", mod_name);
            if full.to_lowercase().starts_with(&prefix.to_lowercase()) {
                items.push(CompletionItem {
                    label: full.clone(),
                    kind: Some(CompletionItemKind::MODULE),
                    detail: Some("stdlib module".to_string()),
                    insert_text: Some(full),
                    sort_text: Some(format!("0_{}", full)),
                    ..CompletionItem::default()
                });
            }
        }

        // Scan for local .nula files when a document dir is known.
        if let Some(dir) = document_dir {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map_or(false, |e| e == "nula") {
                        if let Some(fname) = path.file_stem().and_then(|s| s.to_str()) {
                            let rel = format!("./{}", fname);
                            if rel.to_lowercase().starts_with(&prefix.to_lowercase()) {
                                items.push(CompletionItem {
                                    label: rel.clone(),
                                    kind: Some(CompletionItemKind::FILE),
                                    detail: Some("local module".to_string()),
                                    insert_text: Some(format!("./{}.nula", fname)),
                                    sort_text: Some(format!("1_{}", rel)),
                                    ..CompletionItem::default()
                                });
                            }
                        }
                    }
                }
            }
        }

        items
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    /// Get the text of the line containing `offset`.
    fn current_line(&self, offset: usize) -> Option<&str> {
        let start = self.source[..offset]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let end = self.source[offset..]
            .find('\n')
            .map(|i| offset + i)
            .unwrap_or(self.source.len());
        Some(&self.source[start..end])
    }

    /// Extract all `let` binding names from the source (simple heuristic
    /// for local variable completions).
    fn local_bindings(&self) -> Vec<String> {
        let mut names = Vec::new();
        for line in self.source.lines() {
            let trimmed = line.trim_start();
            if let Some(after_let) = trimmed.strip_prefix("let ") {
                if let Some(name) = after_let
                    .split(|c: char| c.is_whitespace() || c == ':' || c == '=')
                    .next()
                {
                    let name = name.trim();
                    if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        names.push(name.to_string());
                    }
                }
            }
        }
        names
    }

    /// Convert an LSP position to a byte offset in the source.
    fn position_to_offset(&self, position: Position) -> usize {
        let mut offset = 0usize;
        for (line_idx, line) in self.source.lines().enumerate() {
            if line_idx as u32 == position.line {
                return offset + utf16_col_to_byte(line, position.character as usize);
            }
            offset += line.len() + 1; // +1 for newline
        }
        self.source.len()
    }

    /// Extract the identifier fragment the user has typed so far.
    fn prefix_at(&self, offset: usize) -> String {
        let bytes = self.source.as_bytes();
        let mut offset = offset.min(bytes.len());
        while offset > 0 && !self.source.is_char_boundary(offset) {
            offset -= 1;
        }
        let mut start = offset;
        while start > 0 {
            let prev = bytes[start - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' {
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
                let name = after_fun.split_whitespace().next().unwrap_or("").trim();
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
        let cap_hints: Vec<_> = hints
            .iter()
            .filter(|h| label_to_string(&h.label).contains(":iso"))
            .collect();
        assert!(!cap_hints.is_empty());
    }

    #[test]
    fn test_effect_inlay_for_handler() {
        let source = "fun read() ! IO";
        let engine = InlayHintEngine::new(source);
        let hints = engine.generate_inlay_hints();
        let effect_hints: Vec<_> = hints
            .iter()
            .filter(|h| label_to_string(&h.label).contains("[IO]"))
            .collect();
        assert!(!effect_hints.is_empty());
    }

    #[test]
    fn test_no_inlay_when_explicit_type() {
        let source = "let x : Int = 42";
        let engine = InlayHintEngine::new(source);
        let hints = engine.generate_inlay_hints();
        // Should NOT generate a type inlay since type is already explicit
        let type_inlays: Vec<_> = hints
            .iter()
            .filter(|h| {
                let label = label_to_string(&h.label);
                label.starts_with(": ") && !label.contains(":iso")
            })
            .collect();
        assert!(
            type_inlays.is_empty(),
            "should not add inlay when type is explicit"
        );
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
            line: 0,
            character: 5,
            label: ": Int".to_string(),
            kind: AnnotationKind::Type,
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
        let items = engine.complete(
            Position {
                line: 0,
                character: 0,
            },
            None,
        );
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
        let items = engine.complete(
            Position {
                line: 0,
                character: 3,
            },
            None,
        );
        let labels = labels(&items);
        assert!(
            labels.contains(&"return"),
            "should offer 'return' for prefix 'ret'"
        );
        assert!(
            !labels.contains(&"let"),
            "'let' should not match prefix 'ret'"
        );
    }

    #[test]
    fn test_completion_top_level_functions() {
        let source = "fun foo()\nfun bar(x: Int)\nlet x = 1";
        let engine = CompletionEngine::new(source);
        let items = engine.complete(
            Position {
                line: 2,
                character: 0,
            },
            None,
        );
        let labels = labels(&items);
        assert!(labels.contains(&"foo"));
        assert!(labels.contains(&"bar"));
    }

    #[test]
    fn test_completion_effects() {
        let source = "";
        let engine = CompletionEngine::new(source);
        let items = engine.complete(
            Position {
                line: 0,
                character: 0,
            },
            None,
        );
        let labels = labels(&items);
        assert!(labels.contains(&"IO"));
        assert!(labels.contains(&"Migrate"));
        assert!(labels.contains(&"Inference"));
    }

    #[test]
    fn test_completion_case_insensitive() {
        // Effect names are matched case-insensitively by prefix.
        let source = "mi";
        let engine = CompletionEngine::new(source);
        let items = engine.complete(
            Position {
                line: 0,
                character: 2,
            },
            None,
        );
        let labels = labels(&items);
        assert!(
            labels.contains(&"Migrate"),
            "should match 'Migrate' for prefix 'mi'"
        );
    }

    // -- Crash-safety regression tests --

    #[test]
    fn test_code_action_let_binding_without_rhs() {
        // A half-typed `let` line with no `=` (e.g. `let x y`) must not
        // panic the code action provider; no quick fix can be offered
        // without a right-hand side.
        assert!(
            NulangLanguageServer::code_actions(
                "let x y",
                None,
                &Url::parse("file:///test.nula").unwrap(),
            )
            .is_none()
        );
        // A well-formed binding still produces a quick fix.
        assert!(
            NulangLanguageServer::code_actions(
                "let x = 42",
                None,
                &Url::parse("file:///test.nula").unwrap(),
            )
            .is_some()
        );
    }

    #[test]
    fn test_sig_help_non_ascii_line() {
        // UTF-16 columns must map to byte offsets on char boundaries: a
        // column inside the multibyte é must not panic the prefix slice.
        let source = "fun add(a, b) = a + b\nlet résumé = add(1, 2)";
        // UTF-16 column 6 on line 1 sits right after the é; as a raw byte
        // index it would land mid-character.
        let result = NulangLanguageServer::sig_help(source, Position::new(1, 6));
        assert!(result.is_none());
        // A column past the end of the line clamps to the line end.
        let result = NulangLanguageServer::sig_help(source, Position::new(1, 999));
        assert!(result.is_none());
    }

    #[test]
    fn test_position_to_offset_non_ascii_line() {
        // "café": é is one UTF-16 code unit but two bytes.
        let engine = CompletionEngine::new("let café = 1");
        // UTF-16 column 8 (right after "café") maps to byte offset 9.
        assert_eq!(engine.position_to_offset(Position::new(0, 8)), 9);
        // Completion at that offset must not panic: pre-fix the raw column
        // landed mid-character and prefix_at sliced inside é.
        let items = engine.complete(Position::new(0, 8), None);
        assert!(!items.is_empty(), "empty prefix should offer completions");
    }

    #[test]
    fn test_prefix_at_stops_on_char_boundary() {
        let engine = CompletionEngine::new("let caféx = 1");
        // Byte offset 10 is right after the x (é occupies bytes 7-8).
        assert_eq!(engine.prefix_at(10), "x");
        // A mid-character offset snaps back to a char boundary instead of
        // panicking: offset 8 (inside é) behaves like the start of é.
        assert_eq!(engine.prefix_at(8), "caf");
    }

    #[test]
    fn test_inlay_hints_rparen_before_lparen() {
        // A malformed `fun` line with `)` before `(` must not panic the
        // parameter slicer in collect_annotations.
        let engine = InlayHintEngine::new("fun foo) bar(");
        let hints = engine.generate_inlay_hints();
        assert!(hints.is_empty());
    }

    /// Regression: the LSP effect check is interprocedural, matching the CLI
    /// frontend — a function declared `! {}` that calls an IO function must
    /// produce an effect diagnostic in the editor, not just in `nulang run`.
    #[test]
    fn test_diagnostics_pure_fn_calling_io_fn() {
        let source = "fn do_io() -> Unit ! {IO} { perform IO.print(\"x\") }\n\
                      fn pure() -> Unit ! {} { do_io() }";
        let (diagnostics, _) = NulangLanguageServer::compute_diagnostics(source);
        assert!(
            diagnostics.iter().any(|d| {
                d.severity == Some(DiagnosticSeverity::ERROR) && d.message.contains("IO")
            }),
            "expected an effect error diagnostic mentioning IO, got: {:?}",
            diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    /// Regression: declarations nested in `module {}` blocks must be
    /// effect-checked just like top-level ones (the diagnostics pass
    /// flattens them, mirroring the CLI frontend).
    #[test]
    fn test_diagnostics_module_nested_effect_violation() {
        let source = "module M { fn pure() -> Unit ! {} { perform IO.print(\"x\") } }";
        let (diagnostics, _) = NulangLanguageServer::compute_diagnostics(source);
        assert!(
            diagnostics.iter().any(|d| {
                d.severity == Some(DiagnosticSeverity::ERROR) && d.message.contains("IO")
            }),
            "expected an effect error diagnostic for module-nested IO, got: {:?}",
            diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    /// Positive control: functions staying within their declared effect rows
    /// must produce no diagnostics at all.
    #[test]
    fn test_diagnostics_pure_functions_clean() {
        let source = "fn pure() -> Unit ! {} { unit }\n\
                      fn also_pure() -> Unit ! {} { pure() }\n\
                      fn do_io() -> Unit ! {IO} { perform IO.print(\"x\") }\n\
                      fn caller() -> Unit ! {IO} { do_io() }";
        let (diagnostics, _) = NulangLanguageServer::compute_diagnostics(source);
        assert!(
            diagnostics.is_empty(),
            "well-formed effectful/pure functions must be diagnostic-free, got: {:?}",
            diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_hover_shows_type_for_let_binding() {
        // Create a minimal NulangLanguageServer to test hover_at with type info
        let source = "fn main() { let x: Int = 42; x }";
        // compute_diagnostics now returns (diagnostics, type_map)
        let (_diagnostics, type_map) = NulangLanguageServer::compute_diagnostics(source);
        assert!(
            !type_map.is_empty(),
            "type_map should contain an entry for 'x'"
        );
        // The entry for x should show "Int"
        let has_int = type_map.values().any(|v| v == "Int");
        assert!(
            has_int,
            "type_map should contain 'Int' for x: {:?}",
            type_map
        );
    }

    #[test]
    fn test_hover_type_map_for_multiple_bindings() {
        let source = "fn main() { let x: Int = 42; let y: String = \"hi\"; x }";
        let (_diagnostics, type_map) = NulangLanguageServer::compute_diagnostics(source);
        assert!(
            type_map.values().any(|v| v == "Int"),
            "should have Int type"
        );
        assert!(
            type_map.values().any(|v| v == "String"),
            "should have String type"
        );
    }
}
