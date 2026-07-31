//! Nulang source formatter — `nulang fmt <file>`.
//!
//! Parses a `.nula` file and pretty-prints it with canonical formatting.

use crate::ast::{BinOp, Decl, Expr, Literal, Pattern};
use crate::types::Type;
use std::path::Path;

use crate::types::{NuError, NuResult, Span};

/// Format a Nulang source string and return the formatted output.
pub fn format_source(source: &str) -> Result<String, String> {
    let mut lexer = crate::lexer::Lexer::new(source);
    let tokens = lexer.lex().map_err(|e| e.to_string())?;
    let mut parser = crate::parser::Parser::new(tokens);
    let ast = parser.parse_module().map_err(|e| e.to_string())?;

    let mut out = String::new();
    let mut first = true;
    for decl in &ast.decls {
        if !first {
            out.push('\n');
        }
        first = false;
        fmt_decl(&mut out, decl, 0);
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

/// Recursively format all `.nula` files under `dir`.
///
/// When `check_only` is true files are never modified; an error is returned
/// on the first file that *would* be reformatted instead.
pub fn format_directory(dir: &Path, check_only: bool) -> NuResult<()> {
    walk_format(dir, check_only)
}

fn walk_format(dir: &Path, check_only: bool) -> NuResult<()> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        NuError::vm_error(
            format!("Cannot read directory '{}': {}", dir.display(), e),
            Span::default(),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| {
            NuError::vm_error(
                format!("Cannot read entry in '{}': {}", dir.display(), e),
                Span::default(),
            )
        })?;
        let path = entry.path();
        if path.is_dir() {
            walk_format(&path, check_only)?;
        } else if path.extension().map_or(false, |ext| ext == "nula") {
            let source = std::fs::read_to_string(&path).map_err(|e| {
                NuError::vm_error(
                    format!("Cannot read '{}': {}", path.display(), e),
                    Span::default(),
                )
            })?;
            match format_source(&source) {
                Ok(formatted) => {
                    if formatted != source {
                        if check_only {
                            return Err(NuError::parse_error(
                                format!("Would reformat {}", path.display()),
                                Span::default(),
                            ));
                        }
                        std::fs::write(&path, formatted.as_bytes()).map_err(|e| {
                            NuError::vm_error(
                                format!("Cannot write '{}': {}", path.display(), e),
                                Span::default(),
                            )
                        })?;
                        println!("Formatted {}", path.display());
                    }
                }
                Err(e) => {
                    return Err(NuError::parse_error(
                        format!("{}: {}", path.display(), e),
                        Span::default(),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn fmt_decl(out: &mut String, decl: &Decl, indent: usize) {
    let sp = " ".repeat(indent);
    match decl {
        Decl::Function {
            name,
            params,
            ret_type,
            body,
            effect,
            ..
        } => {
            out.push_str(&format!("{}fn {}(", sp, name));
            for (i, (pn, pty)) in params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(pn);
                if let Some(t) = pty {
                    out.push_str(&format!(": {}", fmt_type(t)));
                }
            }
            out.push(')');
            if let Some(r) = ret_type {
                out.push_str(&format!(" -> {}", fmt_type(r)));
            }
            if let Some(e) = effect {
                out.push_str(&format!(" ! {}", e));
            }
            out.push_str(" {\n");
            fmt_expr(out, body, indent + 4);
            out.push_str(&format!("\n{}}}\n", sp));
        }
        Decl::VariantType {
            name,
            type_params,
            variants,
            ..
        } => {
            out.push_str(&format!("{}type {}", sp, name));
            if !type_params.is_empty() {
                out.push_str(&format!("[{}]", type_params.join(", ")));
            }
            out.push_str(" = ");
            for (i, (vn, vp)) in variants.iter().enumerate() {
                if i > 0 {
                    out.push_str(" | ");
                }
                out.push_str(vn);
                if let Some(p) = vp {
                    out.push_str(&format!("({})", fmt_type(p)));
                }
            }
            out.push('\n');
        }
        Decl::TypeAlias {
            name,
            type_params,
            body,
            ..
        } => {
            out.push_str(&format!("{}type {}", sp, name));
            if !type_params.is_empty() {
                out.push_str(&format!("[{}]", type_params.join(", ")));
            }
            out.push_str(&format!(" = {}\n", fmt_type(body)));
        }
        Decl::Actor {
            name,
            behaviors,
            state_fields,
            ..
        } => {
            out.push_str(&format!("{}actor {} {{\n", sp, name));
            for (fnm, _, fty, fdef) in state_fields {
                out.push_str(&format!("{}    state {}: {}", sp, fnm, fmt_type(fty)));
                out.push_str(" = ");
                fmt_expr(out, fdef, indent + 4);
                out.push('\n');
            }
            if !state_fields.is_empty() && !behaviors.is_empty() {
                out.push('\n');
            }
            for b in behaviors {
                out.push_str(&format!("{}    behavior {}(", sp, b.name));
                for (i, (pn, pty)) in b.params.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(pn);
                    if let Some(t) = pty {
                        out.push_str(&format!(": {}", fmt_type(t)));
                    }
                }
                out.push_str(") {\n");
                fmt_expr(out, &b.body, indent + 8);
                out.push_str(&format!("\n{}    }}\n", sp));
            }
            out.push_str(&format!("{}}}\n", sp));
        }
        _ => {
            out.push_str(&format!("{}// (unformatted: {:?})\n", sp, decl));
        }
    }
}

fn fmt_expr(out: &mut String, expr: &Expr, indent: usize) {
    let sp = " ".repeat(indent);
    match expr {
        Expr::Literal(lit, _) => match lit {
            Literal::Int(n) => out.push_str(&n.to_string()),
            Literal::Float(f) => out.push_str(&f.to_string()),
            Literal::String(s) => out.push_str(&format!("\"{}\"", s)),
            Literal::Bool(b) => out.push_str(&b.to_string()),
            Literal::Nil => out.push_str("nil"),
            Literal::Unit => out.push_str("unit"),
        },
        Expr::Var(name, _) => out.push_str(name),
        Expr::Let {
            name, value, body, ..
        } => {
            out.push_str(&format!("let {} = ", name));
            fmt_expr(out, value, indent);
            out.push_str(" in\n");
            fmt_expr(out, body, indent);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            out.push_str("if ");
            fmt_expr(out, cond, indent);
            out.push_str(" then ");
            fmt_expr(out, then_branch, indent);
            if let Some(e) = else_branch {
                out.push_str(" else ");
                fmt_expr(out, e, indent);
            }
        }
        Expr::App { func, args, .. } => {
            fmt_expr(out, func, indent);
            out.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                fmt_expr(out, a, indent);
            }
            out.push(')');
        }
        Expr::Lambda { params, body, .. } => {
            out.push_str("fn(");
            for (i, (pn, _)) in params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(pn);
            }
            out.push_str(") { ");
            fmt_expr(out, body, indent);
            out.push_str(" }");
        }
        Expr::Binary {
            op, left, right, ..
        } => {
            fmt_expr(out, left, indent);
            out.push_str(&format!(" {} ", op_sym(*op)));
            fmt_expr(out, right, indent);
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            out.push_str("match ");
            fmt_expr(out, scrutinee, indent);
            out.push_str(" {\n");
            for (pat, guard, body) in arms {
                out.push_str(&format!("{}    | ", sp));
                fmt_pat(out, pat);
                if let Some(g) = guard {
                    out.push_str(" if ");
                    fmt_expr(out, g, indent + 4);
                }
                out.push_str(" => ");
                fmt_expr(out, body, indent + 4);
                out.push('\n');
            }
            out.push_str(&format!("{}}}", sp));
        }
        Expr::Block { exprs, .. } => {
            out.push_str("{\n");
            for e in exprs {
                out.push_str(&format!("{}    ", sp));
                fmt_expr(out, e, indent + 4);
                out.push('\n');
            }
            out.push_str(&format!("{}}}", sp));
        }
        Expr::Perform {
            effect, op, args, ..
        } => {
            out.push_str(&format!("perform {}.{}(", effect, op));
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                fmt_expr(out, a, indent);
            }
            out.push(')');
        }
        Expr::Pipe { left, right, .. } => {
            fmt_expr(out, left, indent);
            out.push_str(" |> ");
            fmt_expr(out, right, indent);
        }
        Expr::FieldAccess { expr, field, .. } => {
            fmt_expr(out, expr, indent);
            out.push_str(&format!(".{}", field));
        }
        Expr::Tuple(elems, _) => {
            out.push('(');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                fmt_expr(out, e, indent);
            }
            out.push(')');
        }
        Expr::Record(fields, _) => {
            out.push_str("{ ");
            for (i, (nm, val)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format!("{}: ", nm));
                fmt_expr(out, val, indent);
            }
            out.push_str(" }");
        }
        Expr::RecordUpdate { base, fields, .. } => {
            out.push_str("{ ");
            fmt_expr(out, base, indent);
            out.push_str(" .. ");
            for (i, (nm, val)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format!("{} = ", nm));
                fmt_expr(out, val, indent);
            }
            out.push_str(" }");
        }
        Expr::Consume { expr, .. } => {
            out.push_str("consume ");
            fmt_expr(out, expr, indent);
        }
        Expr::Recover { body, .. } => {
            out.push_str("recover ");
            fmt_expr(out, body, indent);
        }
        _ => {
            out.push_str(&format!("/* {:?} */", expr));
        }
    }
}

fn fmt_pat(out: &mut String, pat: &Pattern) {
    match pat {
        Pattern::Wild => out.push('_'),
        Pattern::Var(name) => out.push_str(name),
        Pattern::Lit(lit) => match lit {
            Literal::Int(n) => out.push_str(&n.to_string()),
            Literal::String(s) => out.push_str(&format!("\"{}\"", s)),
            _ => out.push_str(&format!("{:?}", lit)),
        },
        Pattern::Variant(name, Some(inner)) => {
            out.push_str(&format!("{}(", name));
            fmt_pat(out, inner);
            out.push(')');
        }
        Pattern::Variant(name, None) => out.push_str(name),
        Pattern::Tuple(elems) => {
            out.push('(');
            for (i, p) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                fmt_pat(out, p);
            }
            out.push(')');
        }
        _ => out.push_str(&format!("{:?}", pat)),
    }
}

fn op_sym(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Pow => "**",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::Le => "<=",
        BinOp::Ge => ">=",
        BinOp::And => "and",
        BinOp::Or => "or",
        BinOp::BitAnd => "&",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
        BinOp::Assign => "=",
        BinOp::Range => "..",
        BinOp::Pipe => "|>",
    }
}

fn fmt_type(ty: &Type) -> String {
    format!("{}", ty)
}
