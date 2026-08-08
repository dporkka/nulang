import re

def replace_in_file(path, replacements):
    with open(path, 'r') as f:
        content = f.read()
    for old, new in replacements:
        content = re.sub(old, new, content)
    with open(path, 'w') as f:
        f.write(content)

replace_in_file('src/effect_checker.rs', [
    (r'for \(p, _\) in params', 'for p in params'),
    (r'for \(_, _\) in params', 'for _p in params'),
    (r'params\.iter\(\)\.map\(\|\(n, _\)\|\)', 'params.iter().map(|p|)'),
    (r'map\(\|\(n, _\)\| n\.clone\(\)\)', 'map(|p| p.name.clone())'),
    (r'if !new_bound\.contains\(p\)', 'if !new_bound.contains(&p.name)'),
    (r'new_bound\.push\(p\.clone\(\)\)', 'new_bound.push(p.name.clone())'),
])

replace_in_file('src/fmt.rs', [
    (r'for \(i, \(pn, pty\)\) in params\.iter\(\)\.enumerate\(\)', 'for (i, p) in params.iter().enumerate()'),
    (r'for \(i, \(pn, _\)\) in params\.iter\(\)\.enumerate\(\)', 'for (i, p) in params.iter().enumerate()'),
    (r'out\.push_str\(pn\)', 'out.push_str(&p.name)'),
    (r'if let Some\(t\) = pty', 'if let Some(t) = &p.ty'),
])

replace_in_file('src/hir_lower.rs', [
    (r'for \(param_name, param_ty\) in params', 'for p in params'),
    (r'if let Some\(ty\) = param_ty', 'if let Some(ty) = &p.ty'),
    (r'typed_params\.push\(\(param_name\.clone\(\), ty\.clone\(\)\)\)', 'typed_params.push((p.name.clone(), ty.clone()))'),
    (r'\.map\(\|\(n, t\)\| \(n\.clone\(\), resolve_type\(t\)\)\)', '.map(|p| (p.name.clone(), resolve_type(&p.ty)))'),
    (r'params: lambda_params,', 'params: lambda_params.into_iter().map(|(n, t)| crate::ast::Param::new(n, t)).collect(),'),
    (r'params: params\n\s*\.into_iter\(\)\n\s*\.map\(\|\(n, t\)\| \(n\.to_string\(\), Some\(t\)\)\)\n\s*\.collect\(\),', 'params: params.into_iter().map(|(n, t)| crate::ast::Param::new(n.to_string(), Some(t))).collect(),'),
    (r'params: vec!\[\("prompt"\.to_string\(\), Some\(str_ty\.clone\(\)\)\)\],', 'params: vec![crate::ast::Param::new("prompt", Some(str_ty.clone()))],'),
    (r'params: vec!\[\("msg"\.to_string\(\), None\)\],', 'params: vec![crate::ast::Param::new("msg", None)],'),
    (r'params: vec!\[\("url"\.to_string\(\), None\)\],', 'params: vec![crate::ast::Param::new("url", None)],'),
    (r'params: vec!\[\("x"\.to_string\(\), Some\(Type::int\(\)\)\)\],', 'params: vec![crate::ast::Param::new("x", Some(Type::int()))],'),
    (r'params: vec!\[\("x"\.to_string\(\), Some\(Type::float\(\)\)\)\],', 'params: vec![crate::ast::Param::new("x", Some(Type::float()))],'),
    (r'params: vec!\[\("address"\.to_string\(\), None\)\],', 'params: vec![crate::ast::Param::new("address", None)],'),
    (r'fn lambda_references\(name: &str, params: &\[\(String, Option<Type>\)\], body: &Expr\) -> bool', 'fn lambda_references(name: &str, params: &[crate::ast::Param], body: &Expr) -> bool'),
    (r'fn lambda_captures\(params: &\[\(String, Option<Type>\)\], body: &Expr\) -> Vec<String>', 'fn lambda_captures(params: &[crate::ast::Param], body: &Expr) -> Vec<String>'),
])

replace_in_file('src/typechecker.rs', [
    (r'for \(_, param_ty\) in params', 'for p in params'),
    (r'for \(_param_name, param_ty\) in params', 'for p in params'),
    (r'let pty = match param_ty', 'let pty = match &p.ty'),
    (r'new_ctx\.bind\(param_name\.0\.clone\(\), pty\.clone\(\), Capability::Ref, false\);', 'new_ctx.bind(p.name.clone(), pty.clone(), Capability::Ref, false);'),
    (r'params: &\[\(String, Option<Type>\)\]', 'params: &[crate::ast::Param]'),
])
