use crate::ast::{AstModule, Decl};
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::types::{NuError, NuResult, Span};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub fn resolve_imports(
    module: &mut AstModule,
    base_dir: &Path,
    visited: &mut HashSet<PathBuf>,
) -> NuResult<()> {
    let canonical_base = base_dir
        .canonicalize()
        .unwrap_or_else(|_| base_dir.to_path_buf());
    visited.insert(canonical_base.clone());

    let imports: Vec<(String, Vec<String>)> = module
        .decls
        .iter()
        .filter_map(|d| match d {
            Decl::Import { path, items, .. } => Some((path.clone(), items.clone())),
            _ => None,
        })
        .collect();

    for (import_path, items) in &imports {
        let resolved = resolve_path(&canonical_base, import_path);
        let resolved_canonical = resolved.canonicalize().unwrap_or_else(|_| resolved.clone());
        if visited.contains(&resolved_canonical) {
            continue;
        }

        let source = std::fs::read_to_string(&resolved).map_err(|e| NuError::RuntimeError {
            msg: format!("cannot read '{}': {}", import_path, e),
            span: Span::default(),
        })?;
        let tokens = Lexer::new(&source)
            .lex()
            .map_err(|e| NuError::RuntimeError {
                msg: format!("lex error in '{}': {}", import_path, e),
                span: Span::default(),
            })?;
        let mut imported =
            Parser::new(tokens)
                .parse_module()
                .map_err(|e| NuError::RuntimeError {
                    msg: format!("parse error in '{}': {}", import_path, e),
                    span: Span::default(),
                })?;

        let import_dir = resolved_canonical.parent().unwrap_or(&resolved_canonical);
        resolve_imports(&mut imported, import_dir, visited)?;

        let imported_decls = if items.is_empty() {
            imported.decls
        } else {
            filter_decls(imported.decls, items)
        };

        // Reject name collisions between what THIS import brings in and
        // what's already present (from an earlier import or the importing
        // file itself) up front, with a clear diagnostic pointing at the
        // conflicting import path. Without this check, two stdlib modules
        // that both define e.g. `empty`/`contains`/`remove` (map.nula and
        // set.nula both do) silently produce two top-level `Decl::Function`
        // entries with the same name; MIR's function-slot allocator isn't
        // built to handle that and fails much later with an opaque
        // "internal: MIR function slot 0 left unfilled" — a compiler crash
        // for user-visible input, not a real internal invariant violation.
        let existing_names: HashSet<&str> = module.decls.iter().filter_map(decl_name).collect();
        for decl in &imported_decls {
            if let Some(name) = decl_name(decl) {
                if existing_names.contains(name) {
                    return Err(NuError::RuntimeError {
                        msg: format!(
                            "import conflict: '{}' from '{}' collides with an already-imported \
                             or locally-declared name of the same name (qualified/aliased \
                             imports aren't available). Fix by renaming one of the conflicting \
                             declarations, or importing only the names you need with \
                             `import {} {{ specific_name }}`.",
                            name, import_path, import_path
                        ),
                        span: Span::default(),
                    });
                }
            }
        }

        let mut merged = imported_decls;
        merged.append(&mut module.decls);
        module.decls = merged;
    }
    module.decls.retain(|d| !matches!(d, Decl::Import { .. }));
    Ok(())
}

fn resolve_path(base: &Path, import: &str) -> PathBuf {
    // stdlib::set → resolve to STDLIB_DIR/set.nula or dev fallback
    if let Some(module) = import.strip_prefix("stdlib::") {
        // Try NULANG_STDLIB env var first
        if let Ok(dir) = std::env::var("NULANG_STDLIB") {
            return PathBuf::from(dir).join(format!("{}.nula", module));
        }
        // Try relative to executable
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                let candidate = exe_dir.join("stdlib").join(format!("{}.nula", module));
                if candidate.exists() {
                    return candidate;
                }
            }
        }
        // Development fallback: src/stdlib/ relative to CWD
        if let Ok(cwd) = std::env::current_dir() {
            let dev_path = cwd
                .join("src")
                .join("stdlib")
                .join(format!("{}.nula", module));
            if dev_path.exists() {
                return dev_path;
            }
        }
        // Last resort: relative to CWD
        return PathBuf::from(format!("src/stdlib/{}.nula", module));
    }
    let p = Path::new(import);
    let resolved = if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    };
    if resolved.extension().is_none() {
        resolved.with_extension("nula")
    } else {
        resolved
    }
}

fn decl_name(decl: &Decl) -> Option<&str> {
    match decl {
        Decl::Function { name, .. }
        | Decl::Actor { name, .. }
        | Decl::StateMachine { name, .. }
        | Decl::TypeAlias { name, .. }
        | Decl::RecordType { name, .. }
        | Decl::VariantType { name, .. }
        | Decl::EffectDecl { name, .. }
        | Decl::Module { name, .. }
        | Decl::Agent { name, .. }
        | Decl::Database { name, .. } => Some(name.as_str()),
        Decl::NamedHandler { name, .. } => Some(name.as_str()),
        Decl::CrdtDecl { name, .. } => Some(name.as_str()),
        Decl::Extern { .. }
        | Decl::Workflow { .. }
        | Decl::Import { .. }
        | Decl::Class { .. }
        | Decl::LetBinding { .. }
        | Decl::Impl { .. }
        | Decl::Given { .. } => None,
    }
}

fn filter_decls(decls: Vec<Decl>, items: &[String]) -> Vec<Decl> {
    if items.is_empty() {
        return decls;
    }
    let set: HashSet<&str> = items.iter().map(|s| s.as_str()).collect();
    decls
        .into_iter()
        .filter(|d| decl_name(d).map_or(false, |n| set.contains(n)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr, Literal};
    use crate::types::Span;
    fn sp() -> Span {
        Span::new(0, 0)
    }
    fn fn_decl(name: &str) -> Decl {
        Decl::Function {
            name: name.into(),
            type_params: vec![],
            type_param_constraints: vec![],
            params: vec![],
            default_values: vec![],
            using_params: vec![],
            ret_type: None,
            error_type: None,
            effect: None,
            cap: None,
            body: Expr::Literal(Literal::Int(0), sp()),
            annotations: vec![],
            public: false,
            span: sp(),
        }
    }

    #[test]
    fn test_filter_empty() {
        let decls = vec![fn_decl("f")];
        assert_eq!(filter_decls(decls, &[]).len(), 1);
    }

    #[test]
    fn test_filter_names() {
        let r = filter_decls(vec![fn_decl("a"), fn_decl("b")], &["a".into()]);
        assert_eq!(r.len(), 1);
        assert_eq!(decl_name(&r[0]), Some("a"));
    }
}
