//! Nulang source formatter — `nulang fmt <file>`.
//!
//! Parses a `.nula` file and pretty-prints it with canonical formatting.

use crate::ast::{BinOp, Decl, Expr, Literal, Pattern};
use crate::types::Type;
use std::path::Path;

use crate::types::{NuError, NuResult, Span};

/// Format a Nulang source string and return the formatted output.
/// Returns an error if any construct is not yet supported by the formatter
/// (rather than silently dropping or corrupting it).
pub fn format_source(source: &str) -> Result<String, String> {
    let mut lexer = crate::lexer::Lexer::new(source);
    let tokens = lexer.lex().map_err(|e| e.to_string())?;
    let mut parser = crate::parser::Parser::new(tokens);
    let ast = parser.parse_module().map_err(|e| e.to_string())?;

    let mut out = String::new();
    let mut first = true;
    let mut had_unhandled = false;
    for decl in &ast.decls {
        if !first {
            out.push('\n');
        }
        first = false;
        fmt_decl(&mut out, decl, 0, &mut had_unhandled);
    }
    if had_unhandled {
        return Err("file contains constructs not yet supported by the formatter (e.g. workflow, agent, let-binding, class, impl). The file was not modified.".to_string());
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

fn fmt_decl(out: &mut String, decl: &Decl, indent: usize, had_unhandled: &mut bool) {
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
            for (i, p) in params.iter().enumerate() {
                let pn = &p.name;
                let pty = &p.ty;
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
            fmt_block_body(out, body, indent + 4, had_unhandled);
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
                fmt_expr(out, fdef, indent + 4, had_unhandled);
                out.push('\n');
            }
            if !state_fields.is_empty() && !behaviors.is_empty() {
                out.push('\n');
            }
            for (i, b) in behaviors.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                out.push_str(&format!("{}    behavior {}(", sp, b.name));
                for (j, p) in b.params.iter().enumerate() {
                    if j > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&p.name);
                    if let Some(t) = &p.ty {
                        out.push_str(&format!(": {}", fmt_type(t)));
                    }
                }
                out.push_str(") {\n");
                fmt_block_body(out, &b.body, indent + 8, had_unhandled);
                out.push_str(&format!("\n{}    }}\n", sp));
            }
            out.push_str(&format!("{}}}\n", sp));
        }
        _ => {
            *had_unhandled = true;
            out.push_str(&format!("{}// (unformatted decl)\n", sp));
        }
    }
}

fn fmt_expr(out: &mut String, expr: &Expr, indent: usize, had_unhandled: &mut bool) {
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
        Expr::SelfRef(_) => out.push_str("self"),
        Expr::Let {
            name, value, body, ..
        } => {
            out.push_str(&format!("let {} = ", name));
            fmt_expr(out, value, indent, had_unhandled);
            out.push_str(" in\n");
            fmt_expr(out, body, indent, had_unhandled);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            out.push_str("if ");
            fmt_expr(out, cond, indent, had_unhandled);
            out.push_str(" then ");
            fmt_expr(out, then_branch, indent, had_unhandled);
            if let Some(e) = else_branch {
                out.push_str(" else ");
                fmt_expr(out, e, indent, had_unhandled);
            }
        }
        Expr::App { func, args, .. } => {
            fmt_expr(out, func, indent, had_unhandled);
            out.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                fmt_expr(out, a, indent, had_unhandled);
            }
            out.push(')');
        }
        Expr::Lambda { params, body, .. } => {
            out.push_str("fn(");
            for (i, p) in params.iter().enumerate() {
                let pn = &p.name;
                let _ = &p.ty;
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(pn);
            }
            out.push_str(") { ");
            fmt_expr(out, body, indent, had_unhandled);
            out.push_str(" }");
        }
        Expr::Binary {
            op, left, right, ..
        } => {
            fmt_expr(out, left, indent, had_unhandled);
            out.push_str(&format!(" {} ", op_sym(*op)));
            fmt_expr(out, right, indent, had_unhandled);
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            out.push_str("match ");
            fmt_expr(out, scrutinee, indent, had_unhandled);
            out.push_str(" {\n");
            for (pat, guard, body) in arms {
                out.push_str(&format!("{}    | ", sp));
                fmt_pat(out, pat);
                if let Some(g) = guard {
                    out.push_str(" if ");
                    fmt_expr(out, g, indent + 4, had_unhandled);
                }
                out.push_str(" => ");
                fmt_expr(out, body, indent + 4, had_unhandled);
                out.push('\n');
            }
            out.push_str(&format!("{}}}", sp));
        }
        Expr::Block { exprs, .. } => {
            out.push_str("{\n");
            for e in exprs {
                out.push_str(&format!("{}    ", sp));
                fmt_expr(out, e, indent + 4, had_unhandled);
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
                fmt_expr(out, a, indent, had_unhandled);
            }
            out.push(')');
        }
        Expr::Pipe { left, right, .. } => {
            fmt_expr(out, left, indent, had_unhandled);
            out.push_str(" |> ");
            fmt_expr(out, right, indent, had_unhandled);
        }
        Expr::FieldAccess { expr, field, .. } => {
            fmt_expr(out, expr, indent, had_unhandled);
            out.push_str(&format!(".{}", field));
        }
        Expr::Tuple(elems, _) => {
            out.push('(');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                fmt_expr(out, e, indent, had_unhandled);
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
                fmt_expr(out, val, indent, had_unhandled);
            }
            out.push_str(" }");
        }
        Expr::RecordUpdate { base, fields, .. } => {
            out.push_str("{ ");
            fmt_expr(out, base, indent, had_unhandled);
            out.push_str(" .. ");
            for (i, (nm, val)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format!("{} = ", nm));
                fmt_expr(out, val, indent, had_unhandled);
            }
            out.push_str(" }");
        }
        Expr::Consume { expr, .. } => {
            out.push_str("consume ");
            fmt_expr(out, expr, indent, had_unhandled);
        }
        Expr::Recover { body, .. } => {
            out.push_str("recover ");
            fmt_expr(out, body, indent, had_unhandled);
        }
        Expr::Return(value, _) => {
            out.push_str("return");
            if let Some(v) = value {
                out.push(' ');
                fmt_expr(out, v, indent, had_unhandled);
            }
        }
        Expr::Break(_, _) => out.push_str("break"),
        Expr::Array(elems, _) => {
            out.push('[');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                fmt_expr(out, e, indent, had_unhandled);
            }
            out.push(']');
        }
        Expr::Index { arr, idx, .. } => {
            fmt_expr(out, arr, indent, had_unhandled);
            out.push('[');
            fmt_expr(out, idx, indent, had_unhandled);
            out.push(']');
        }
        Expr::Unary { op, expr, .. } => {
            out.push_str(op_sym_unary(*op));
            fmt_expr(out, expr, indent, had_unhandled);
        }
        Expr::Assign { target, value, .. } => {
            fmt_expr(out, target, indent, had_unhandled);
            out.push_str(" = ");
            fmt_expr(out, value, indent, had_unhandled);
        }
        Expr::While { cond, body, .. } => {
            out.push_str("while ");
            fmt_expr(out, cond, indent, had_unhandled);
            out.push_str(" {\n");
            fmt_block_body(out, body, indent + 4, had_unhandled);
            out.push_str(&format!("\n{}}}", sp));
        }
        Expr::For {
            var,
            iterable,
            body,
            ..
        } => {
            out.push_str(&format!("for {} in ", var));
            fmt_expr(out, iterable, indent, had_unhandled);
            out.push_str(" {\n");
            fmt_block_body(out, body, indent + 4, had_unhandled);
            out.push_str(&format!("\n{}}}", sp));
        }
        Expr::LetRec {
            name,
            params,
            value,
            body,
            ..
        } => {
            out.push_str(&format!("let rec {}(", name));
            for (i, p) in params.iter().enumerate() {
                let pn = &p.name;
                let _ = &p.ty;
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(pn);
            }
            out.push_str(") = ");
            fmt_expr(out, value, indent, had_unhandled);
            out.push_str(" in\n");
            fmt_expr(out, body, indent, had_unhandled);
        }
        Expr::Send {
            actor,
            behavior,
            args,
            ..
        } => {
            out.push_str("send ");
            fmt_expr(out, actor, indent, had_unhandled);
            out.push_str(&format!(" {}(", behavior));
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                fmt_expr(out, a, indent, had_unhandled);
            }
            out.push(')');
        }
        Expr::Ask {
            actor,
            behavior,
            args,
            ..
        } => {
            out.push_str("ask ");
            fmt_expr(out, actor, indent, had_unhandled);
            out.push_str(&format!(" {}(", behavior));
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                fmt_expr(out, a, indent, had_unhandled);
            }
            out.push(')');
        }
        Expr::Spawn { .. } => {
            // Complex formatting deferred; preserve original via catch-all behavior
            *had_unhandled = true;
            out.push_str("/* unformatted spawn */");
        }
        Expr::Handle { .. } => {
            *had_unhandled = true;
            out.push_str("/* unformatted handle */");
        }
        Expr::Receive { .. }
        | Expr::Emit { .. }
        | Expr::Migrate { .. }
        | Expr::CapAnnotate { .. }
        | Expr::TypeAnnotate { .. } => {
            *had_unhandled = true;
            out.push_str("/* unformatted receive */");
        }
        Expr::Defer {
            expr, error_only, ..
        } => {
            if *error_only {
                out.push_str("errdefer ");
            } else {
                out.push_str("defer ");
            }
            fmt_expr(out, expr, indent, had_unhandled);
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
            Literal::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            _ => out.push_str("_"),
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
        Pattern::Record(fields) => {
            out.push('{');
            for (i, (name, pat)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(name);
                out.push_str(": ");
                fmt_pat(out, pat);
            }
            out.push('}');
        }
        Pattern::Alias(name, inner) => {
            out.push_str(name);
            out.push_str(" @ ");
            fmt_pat(out, inner);
        }
    }
}

/// Format a function/behavior body, unwrapping blocks to avoid double braces.
fn fmt_block_body(out: &mut String, body: &Expr, indent: usize, had_unhandled: &mut bool) {
    if let Expr::Block { exprs, .. } = body {
        let sp = " ".repeat(indent);
        for (i, e) in exprs.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&sp);
            fmt_expr(out, e, indent, had_unhandled);
        }
    } else {
        fmt_expr(out, body, indent, had_unhandled);
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

fn op_sym_unary(op: crate::ast::UnOp) -> &'static str {
    match op {
        crate::ast::UnOp::Neg => "-",
        crate::ast::UnOp::Not => "!",
        crate::ast::UnOp::Deref => "*",
        crate::ast::UnOp::Ref(_) => "ref",
    }
}
