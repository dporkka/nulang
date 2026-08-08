import re

def replace_in_file(path, replacements):
    with open(path, 'r') as f:
        content = f.read()
    for old, new in replacements:
        content = re.sub(old, new, content)
    with open(path, 'w') as f:
        f.write(content)

replace_in_file('src/hir_lower.rs', [
    (r'\.map\(\|p\| \(p\.name\.clone\(\), resolve_type\(&p\.ty\)\)\)', '.map(|p| (p.name.clone(), resolve_type(&p.ty)))'),
    (r'params\.into_iter\(\)\.map\(\|\(n, t\)\| crate::ast::Param::new\(n\.to_string\(\), Some\(t\)\)\)\.collect\(\)', 'params.into_iter().map(|(n, t)| crate::ast::Param::new(n.to_string(), Some(t))).collect()'),
    (r'params\.iter\(\)\.map\(\|\(n, _\)\| n\.clone\(\)\)', 'params.iter().map(|p| p.name.clone())'),
    (r'for \(p, _\) in params \{', 'for p in params {'),
])

replace_in_file('src/lsp/mod.rs', [
    (r'\.map\(\|\(n, t\)\| \{', '.map(|p| {'),
    (r'\.map\(\|\(n, t\)\| format!\(', '.map(|p| format!('),
    (r'for \(i, \(pname, ptype_ann\)\) in params\.iter\(\)\.enumerate\(\)', 'for (i, p) in params.iter().enumerate()'),
    (r'params\.iter\(\)\.map\(\|\(n, _\)\|\)', 'params.iter().map(|p|)'),
    (r'map\(\|\(n, _\)\| n\.clone\(\)\)', 'map(|p| p.name.clone())'),
    (r'\(n, t\)', 'p'),
    (r'pname', 'p.name'),
    (r'ptype_ann', 'p.ty'),
    (r'p\.clone\(\)', 'p.name.clone()'),
])

replace_in_file('src/parser.rs', [
    (r'canonical_params\[0\]\.0\.clone\(\)', 'canonical_params[0].name.clone()'),
    (r'\.map\(\|\(pname, _\)\| Pattern::Var\(pname\.clone\(\)\)\)', '.map(|p| Pattern::Var(p.name.clone()))'),
    (r'\.map\(\|\(pname, _\)\| \{', '.map(|p| {'),
    (r'for \(param_name, param_ty\) in raw_params', 'for p in raw_params'),
    (r'initializer: Option<\(String, Vec<\(String, Option<Type>\)>, Expr\)> = None;', 'initializer: Option<(String, Vec<crate::ast::Param>, Expr)> = None;'),
    (r'using_params: Vec<\(String, Option<Type>\)>,', 'using_params: Vec<crate::ast::Param>,'),
    (r'params,\n\s*body,\n', 'params,\n                        body,\n'),
])

replace_in_file('src/typechecker.rs', [
    (r'for \(param_name, param_ty\) in params', 'for p in params'),
    (r'if let Some\(t\) = param_ty', 'if let Some(t) = &p.ty'),
    (r'let pty = match param_ty', 'let pty = match &p.ty'),
    (r'new_ctx\.bind\(p\.name\.clone\(\), pty\.clone\(\), Capability::Ref, false\);', 'new_ctx.bind(p.name.clone(), pty.clone(), Capability::Ref, false);'),
    (r'let pty = match &p\.ty \{', 'let pty = match &p.ty {'),
])
