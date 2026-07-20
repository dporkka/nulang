//! Documentation generator for Nulang source files.
//!
//! Line-based scanner (no parser involvement, so docs can be generated even
//! for sources that don't currently compile): consecutive `///` lines attach
//! to the declaration that immediately follows them, `//!` lines become the
//! module overview. Recognized declarations: `fn`, `actor`, `type`,
//! `workflow` (optionally preceded by `pub` / `persistent`). A blank line
//! between a doc comment and the declaration breaks the attachment.
//!
//! Output is a Markdown API reference per module; `write_project_docs`
//! combines all `.nula` files under a directory into a single `docs/api.md`.

use crate::types::{NuError, NuResult};

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// File extension treated as Nulang source.
const SOURCE_EXTENSION: &str = "nula";

/// Kind of a documented declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclKind {
    Function,
    Actor,
    Type,
    Workflow,
}

impl DeclKind {
    /// Markdown section title grouping declarations of this kind.
    fn section_title(self) -> &'static str {
        match self {
            DeclKind::Function => "Functions",
            DeclKind::Actor => "Actors",
            DeclKind::Type => "Types",
            DeclKind::Workflow => "Workflows",
        }
    }
}

/// A declaration with its doc comment and one-line signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocDecl {
    pub kind: DeclKind,
    pub name: String,
    /// Declaration header as written in source, cut at the body brace.
    pub signature: String,
    /// Attached `///` doc comment (empty when undocumented).
    pub doc: String,
}

/// Documentation extracted from one source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDoc {
    /// Module name used for the heading (typically the file stem).
    pub name: String,
    /// Module overview from `//!` comments (empty when absent).
    pub doc: String,
    pub decls: Vec<DocDecl>,
}

/// Extract documentation from one source text.
pub fn parse_module_doc(source: &str, name: &str) -> ModuleDoc {
    let mut module_doc_lines: Vec<String> = Vec::new();
    let mut pending_doc: Vec<String> = Vec::new();
    let mut decls = Vec::new();

    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("////") {
            // `////` is a regular comment, not a doc comment.
            pending_doc.clear();
            continue;
        }
        if let Some(doc) = trimmed.strip_prefix("///") {
            pending_doc.push(doc.trim().to_string());
            continue;
        }
        if let Some(doc) = trimmed.strip_prefix("//!") {
            module_doc_lines.push(doc.trim().to_string());
            continue;
        }
        if trimmed.is_empty() {
            // A blank line separates a doc comment from the declaration it
            // documents; module docs accumulate regardless of position.
            pending_doc.clear();
            continue;
        }
        if let Some((kind, decl_name, signature)) = parse_decl_line(trimmed) {
            decls.push(DocDecl {
                kind,
                name: decl_name,
                signature,
                doc: pending_doc.join("\n"),
            });
        }
        pending_doc.clear();
    }

    ModuleDoc {
        name: name.to_string(),
        doc: module_doc_lines.join("\n"),
        decls,
    }
}

/// Recognize a declaration header at the start of a trimmed line, returning
/// the kind, declared name, and one-line signature.
fn parse_decl_line(line: &str) -> Option<(DeclKind, String, String)> {
    let stripped = line.strip_prefix("pub ").unwrap_or(line);
    let stripped = stripped.strip_prefix("persistent ").unwrap_or(stripped);

    let (kind, rest) = if let Some(rest) = stripped.strip_prefix("fn ") {
        (DeclKind::Function, rest)
    } else if let Some(rest) = stripped.strip_prefix("actor ") {
        (DeclKind::Actor, rest)
    } else if let Some(rest) = stripped.strip_prefix("type ") {
        (DeclKind::Type, rest)
    } else if let Some(rest) = stripped.strip_prefix("workflow ") {
        (DeclKind::Workflow, rest)
    } else {
        return None;
    };

    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        return None;
    }

    // Signature: the header as written, cut at the body brace. Type
    // declarations keep the whole line (`type Point = { x: Int, ... }`
    // braces are the type body, not a declaration body). Known limitation
    // of line-based scanning: a record return type (`fn f() -> { x: Int }`)
    // truncates the signature at the record's `{`.
    let signature = match kind {
        DeclKind::Type => line,
        _ => line.split(" {").next().unwrap_or(line).trim_end(),
    };
    Some((kind, name, signature.to_string()))
}

/// Render one module's documentation as a Markdown section.
pub fn generate_markdown(module: &ModuleDoc) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "## `{}`\n", module.name);
    if !module.doc.is_empty() {
        let _ = writeln!(out, "{}\n", module.doc);
    }
    for kind in [
        DeclKind::Function,
        DeclKind::Actor,
        DeclKind::Type,
        DeclKind::Workflow,
    ] {
        let decls: Vec<&DocDecl> = module.decls.iter().filter(|d| d.kind == kind).collect();
        if decls.is_empty() {
            continue;
        }
        let _ = writeln!(out, "### {}\n", kind.section_title());
        for decl in decls {
            let _ = writeln!(out, "#### `{}`\n", decl.name);
            let _ = writeln!(out, "```nulang\n{}\n```\n", decl.signature);
            if !decl.doc.is_empty() {
                let _ = writeln!(out, "{}\n", decl.doc);
            }
        }
    }
    out
}

/// Generate the combined API reference for every `.nula` file under `root`
/// (recursively, skipping hidden directories and `target`).
pub fn generate_project_docs(root: &Path) -> NuResult<String> {
    let mut files = Vec::new();
    collect_source_files(root, &mut files)?;
    files.sort();

    let mut out = String::from("# API Reference\n\n_Generated by `nulang --doc`. Do not edit by hand._\n\n");
    for path in files {
        let source = fs::read_to_string(&path).map_err(|e| {
            NuError::RuntimeError(format!("docgen: cannot read {}: {}", path.display(), e))
        })?;
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("module");
        let module = parse_module_doc(&source, name);
        // Files without any docs or declarations add only an empty heading.
        if module.doc.is_empty() && module.decls.is_empty() {
            continue;
        }
        out.push_str(&generate_markdown(&module));
        out.push('\n');
    }
    Ok(out)
}

/// Scan `root` for `.nula` files and write the combined API reference to
/// `root/docs/api.md`, returning the output path.
pub fn write_project_docs(root: &Path) -> NuResult<PathBuf> {
    let markdown = generate_project_docs(root)?;
    let docs_dir = root.join("docs");
    fs::create_dir_all(&docs_dir).map_err(|e| {
        NuError::RuntimeError(format!(
            "docgen: cannot create {}: {}",
            docs_dir.display(),
            e
        ))
    })?;
    let out_path = docs_dir.join("api.md");
    fs::write(&out_path, markdown).map_err(|e| {
        NuError::RuntimeError(format!(
            "docgen: cannot write {}: {}",
            out_path.display(),
            e
        ))
    })?;
    Ok(out_path)
}

/// Recursively collect `.nula` files under `dir`, skipping hidden
/// directories and build output.
fn collect_source_files(dir: &Path, files: &mut Vec<PathBuf>) -> NuResult<()> {
    let entries = fs::read_dir(dir).map_err(|e| {
        NuError::RuntimeError(format!("docgen: cannot list {}: {}", dir.display(), e))
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| {
            NuError::RuntimeError(format!("docgen: cannot list {}: {}", dir.display(), e))
        })?;
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "target" {
                continue;
            }
            collect_source_files(&path, files)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some(SOURCE_EXTENSION) {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique scratch dir per test (the suite runs tests in parallel, and a
    /// re-run must not see a previous run's leftover files).
    fn fresh_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("nulang_docgen_test_{}_{}", std::process::id(), tag));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn test_doc_comment_attaches_to_function() {
        let source = "/// Adds two integers.\npub fn add(x: Int, y: Int) -> Int {\n    x + y\n}\n";
        let module = parse_module_doc(source, "math");
        assert_eq!(module.decls.len(), 1);
        let decl = &module.decls[0];
        assert_eq!(decl.kind, DeclKind::Function);
        assert_eq!(decl.name, "add");
        assert_eq!(decl.signature, "pub fn add(x: Int, y: Int) -> Int");
        assert_eq!(decl.doc, "Adds two integers.");
    }

    #[test]
    fn test_module_doc_comment_collected() {
        let source =
            "//! Math utilities.\n//!\n//! Safe to use anywhere.\nfn pi() -> Float { 3.14 }\n";
        let module = parse_module_doc(source, "math");
        assert_eq!(module.doc, "Math utilities.\n\nSafe to use anywhere.");
        assert_eq!(module.decls.len(), 1);
    }

    #[test]
    fn test_all_decl_kinds_extracted() {
        let source = "\
/// A counter actor.
actor Counter {
    state count: Int = 0
}

/// A 2D point.
type Point = { x: Int, y: Int }

/// Order fulfillment saga.
workflow Fulfill {
    step reserve { perform IO.print(\"reserve\") }
}

persistent actor Store {
    state items: Int = 0
}
";
        let module = parse_module_doc(source, "shop");
        let kinds: Vec<DeclKind> = module.decls.iter().map(|d| d.kind).collect();
        assert_eq!(
            kinds,
            [
                DeclKind::Actor,
                DeclKind::Type,
                DeclKind::Workflow,
                DeclKind::Actor
            ]
        );
        assert_eq!(module.decls[0].name, "Counter");
        assert_eq!(module.decls[1].name, "Point");
        assert_eq!(module.decls[1].signature, "type Point = { x: Int, y: Int }");
        assert_eq!(module.decls[2].name, "Fulfill");
        assert_eq!(module.decls[3].name, "Store");
    }

    #[test]
    fn test_generic_and_persistent_headers() {
        let source = "/// Identity.\nfn id[T](x: T) -> T { x }\n";
        let module = parse_module_doc(source, "util");
        assert_eq!(module.decls[0].name, "id");
        assert_eq!(module.decls[0].signature, "fn id[T](x: T) -> T");
    }

    #[test]
    fn test_undocumented_decl_listed_with_empty_doc() {
        let source = "fn helper() { 1 }\n";
        let module = parse_module_doc(source, "util");
        assert_eq!(module.decls.len(), 1);
        assert_eq!(module.decls[0].doc, "");
    }

    #[test]
    fn test_blank_line_breaks_doc_attachment() {
        let source = "/// Not attached.\n\nfn f() { 1 }\n";
        let module = parse_module_doc(source, "util");
        assert_eq!(module.decls[0].doc, "");
    }

    #[test]
    fn test_regular_comments_ignored() {
        let source = "// plain comment\n//// also plain\nfn f() { 1 }\n";
        let module = parse_module_doc(source, "util");
        assert_eq!(module.doc, "");
        assert_eq!(module.decls[0].doc, "");
    }

    #[test]
    fn test_multiline_doc_comment_joined() {
        let source = "/// First line.\n/// Second line.\nfn f() { 1 }\n";
        let module = parse_module_doc(source, "util");
        assert_eq!(module.decls[0].doc, "First line.\nSecond line.");
    }

    #[test]
    fn test_generate_markdown_structure() {
        let source = "\
//! Math utilities.

/// Adds two integers.
fn add(x: Int, y: Int) -> Int { x + y }

/// A counter actor.
actor Counter {
    state count: Int = 0
}

fn undocumented() { 1 }
";
        let module = parse_module_doc(source, "math");
        let md = generate_markdown(&module);
        assert!(md.contains("## `math`"));
        assert!(md.contains("Math utilities."));
        assert!(md.contains("### Functions"));
        assert!(md.contains("### Actors"));
        assert!(!md.contains("### Types"));
        assert!(!md.contains("### Workflows"));
        assert!(md.contains("#### `add`"));
        assert!(md.contains("```nulang\nfn add(x: Int, y: Int) -> Int\n```"));
        assert!(md.contains("Adds two integers."));
        assert!(md.contains("#### `undocumented`"));
        assert!(md.contains("#### `Counter`"));
        // Functions section must come before Actors.
        assert!(md.find("### Functions").unwrap() < md.find("### Actors").unwrap());
    }

    #[test]
    fn test_write_project_docs_emits_api_md() {
        let dir = fresh_dir("project");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("target")).unwrap();
        fs::write(
            dir.join("src/math.nula"),
            "//! Math module.\n/// Doubles a value.\nfn double(x: Int) -> Int { x * 2 }\n",
        )
        .unwrap();
        fs::write(dir.join("empty.nula"), "// nothing documented here\n").unwrap();
        // Build output must be skipped even if it holds source files.
        fs::write(dir.join("target/ignored.nula"), "fn junk() { 0 }\n").unwrap();

        let out_path = write_project_docs(&dir).unwrap();
        assert_eq!(out_path, dir.join("docs/api.md"));
        let md = fs::read_to_string(&out_path).unwrap();
        assert!(md.contains("# API Reference"));
        assert!(md.contains("## `math`"));
        assert!(md.contains("Math module."));
        assert!(md.contains("#### `double`"));
        assert!(md.contains("Doubles a value."));
        // Undocumented-only files and skipped dirs contribute nothing.
        assert!(!md.contains("empty"));
        assert!(!md.contains("junk"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_generate_project_docs_io_error_is_nu_error() {
        let missing = fresh_dir("missing").join("does_not_exist");
        let err = generate_project_docs(&missing).unwrap_err();
        match err {
            NuError::RuntimeError(msg) => assert!(msg.contains("docgen")),
            other => panic!("expected RuntimeError, got {:?}", other),
        }
    }
}
