//! Source-level migration tool for Nulang programs.
//!
//! Rewrites deprecated constructs per RFC 0004:
//!   `agent Name = { ... }`    → `actor Name { ... }` with `import nlc.ai`
//!   `workflow Name = { ... }` → `actor Name { ... }` with `import nlc.workflow`
//!   `database Name = { ... }` → `actor Name { ... }` with `import nlc.storage`
//!   `perform LLM.ask(prompt)` → `perform Provider.ask("inference", prompt)`
//!
//! Usage:
//!   nulang migrate <file.nula>         — rewrite in place
//!   nulang migrate --check <file.nula> — report only, no changes
//!   nulang migrate --nbc <file.nbc>    — bytecode v1→v2 migration

use crate::format::migrate::{migrate_nbc, peek_format_version};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// A source-level rewrite: replace `from` with `to`, and optionally add an import.
struct Rewrite {
    name: &'static str,
    from: &'static str,
    to: &'static str,
    add_import: Option<&'static str>,
}

fn rewrites() -> Vec<Rewrite> {
    vec![
        Rewrite { name: "agent → actor", from: "agent ", to: "actor ", add_import: Some("import nlc.ai;") },
        Rewrite { name: "workflow → actor", from: "workflow ", to: "actor ", add_import: Some("import nlc.workflow;") },
        Rewrite { name: "database → actor", from: "database ", to: "actor ", add_import: Some("import nlc.storage;") },
        Rewrite { name: "LLM.ask → Provider.ask", from: "perform LLM.ask(", to: r#"perform Provider.ask("inference", "#, add_import: None },
    ]
}

/// Apply all rewrites to `source`. Returns (new_source, applied_names).
fn apply_rewrites(source: &str) -> (String, Vec<&'static str>) {
    let mut result = source.to_string();
    let mut applied = Vec::new();
    let mut imports: HashSet<&'static str> = HashSet::new();

    for rw in rewrites() {
        if rw.from.len() <= 5 && result.contains(rw.from) {
            // For keyword replacements, need word-boundary-aware substitution.
            // Simple approach: only replace at start of line or after whitespace.
            let mut new = String::with_capacity(result.len());
            let mut i = 0;
            let bytes = result.as_bytes();
            let from_bytes = rw.from.as_bytes();
            while i < bytes.len() {
                let can_match = i == 0 || bytes[i - 1] == b' ' || bytes[i - 1] == b'\t' || bytes[i - 1] == b'\n';
                if can_match && bytes[i..].starts_with(from_bytes) {
                    new.push_str(rw.to);
                    i += from_bytes.len();
                    if !applied.contains(&rw.name) {
                        applied.push(rw.name);
                        if let Some(imp) = rw.add_import {
                            imports.insert(imp);
                        }
                    }
                } else {
                    new.push(bytes[i] as char);
                    i += 1;
                }
            }
            result = new;
        } else if rw.from.len() > 5 && result.contains(rw.from) {
            // Direct string replacement for longer patterns
            let new = result.replace(rw.from, rw.to);
            if new != result {
                applied.push(rw.name);
                if let Some(imp) = rw.add_import {
                    imports.insert(imp);
                }
                result = new;
            }
        }
    }

    if !imports.is_empty() {
        let mut sorted: Vec<&str> = imports.into_iter().collect();
        sorted.sort();
        let import_block = sorted.join("\n");
        result = format!("{}\n{}", import_block, result);
    }

    (result, applied)
}

/// Run migration on a source file. If `check_only`, report without writing.
pub fn migrate_source(path: &Path, check_only: bool) -> Result<(), String> {
    let source = fs::read_to_string(path)
        .map_err(|e| format!("Cannot read '{}': {}", path.display(), e))?;

    let (new_source, applied) = apply_rewrites(&source);

    if applied.is_empty() {
        println!("{}: no changes needed", path.display());
        return Ok(());
    }

    println!("{}:", path.display());
    for rule in &applied {
        println!("  + {}", rule);
    }

    if check_only {
        println!("  (dry run — file not modified)");
    } else {
        fs::write(path, &new_source)
            .map_err(|e| format!("Cannot write '{}': {}", path.display(), e))?;
        println!("  (written)");
    }

    Ok(())
}

/// Run bytecode migration from file at `path` to target version.
pub fn migrate_nbc_file(path: &Path, target_version: u32) -> Result<(), String> {
    let bytes = fs::read(path)
        .map_err(|e| format!("Cannot read '{}': {}", path.display(), e))?;

    let current_version = peek_format_version(&bytes)
        .ok_or_else(|| format!("'{}' is not a valid .nbc artifact", path.display()))?;

    if current_version >= target_version {
        println!(
            "{}: already at format version {} (target {})",
            path.display(), current_version, target_version
        );
        return Ok(());
    }

    let migrated = migrate_nbc(&bytes, target_version)
        .map_err(|e| format!("Migration failed: {:?}", e))?;

    let out_path = {
        let stem = path.file_stem().unwrap().to_string_lossy();
        let parent = path.parent().unwrap_or(Path::new("."));
        parent.join(format!("{}.v{}.nbc", stem, target_version))
    };

    fs::write(&out_path, &migrated)
        .map_err(|e| format!("Cannot write '{}': {}", out_path.display(), e))?;

    println!(
        "{}: v{} → v{} written to {}",
        path.display(), current_version, target_version, out_path.display()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_migration() {
        let source = "agent Assistant = {\n    state name: String = \"bot\"\n}";
        let (result, applied) = apply_rewrites(source);
        assert!(applied.iter().any(|a| a.contains("agent")));
        assert!(result.contains("actor Assistant {"));
        assert!(result.contains("import nlc.ai;"));
    }

    #[test]
    fn test_workflow_migration() {
        let source = "workflow Payment = {\n    step validate {}\n}";
        let (result, applied) = apply_rewrites(source);
        assert!(applied.iter().any(|a| a.contains("workflow")));
        assert!(result.contains("actor Payment {"));
        assert!(result.contains("import nlc.workflow;"));
    }

    #[test]
    fn test_llm_ask_migration() {
        let source = "perform LLM.ask(\"hello world\")";
        let (result, applied) = apply_rewrites(source);
        assert!(applied.iter().any(|a| a.contains("LLM")));
        assert!(result.contains(r#"perform Provider.ask("inference", "#));
    }

    #[test]
    fn test_no_change() {
        let source = "actor Counter {\n    state count: Int = 0\n}";
        let (result, applied) = apply_rewrites(source);
        assert!(applied.is_empty());
        assert_eq!(result, source);
    }

    #[test]
    fn test_database_migration() {
        let source = "database Users = {\n    table accounts {}\n}";
        let (result, applied) = apply_rewrites(source);
        assert!(applied.iter().any(|a| a.contains("database")));
        assert!(result.contains("actor Users {"));
        assert!(result.contains("import nlc.storage;"));
    }
}
