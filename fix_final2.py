import re

def replace_in_file(path, replacements):
    with open(path, 'r') as f:
        content = f.read()
    for old, new in replacements:
        content = re.sub(old, new, content)
    with open(path, 'w') as f:
        f.write(content)

replace_in_file('src/hir_lower.rs', [
    (r'\.map\(\|p\| \(p\.name\.clone\(\), resolve_type\(&p\.ty\)\)\)', '.map(|(n, t)| (n.clone(), resolve_type(t)))'),
    (r'params\.into_iter\(\)\.map\(\|\(n, t\)\| crate::ast::Param::new\(n\.to_string\(\), Some\(t\)\)\)\.collect\(\)', 'params.into_iter().map(|(n, t)| (n.to_string(), Some(t))).collect()'),
    (r'vec!\[crate::ast::Param::new\("prompt", Some\(str_ty\.clone\(\)\)\)\]', 'vec![("prompt".to_string(), Some(str_ty.clone()))]'),
    (r'new_bound\.insert\(p\.clone\(\)\)', 'new_bound.insert(p.name.clone())'),
    (r'value_bound\.insert\(p\.clone\(\)\)', 'value_bound.insert(p.name.clone())'),
])

replace_in_file('src/lsp/mod.rs', [
    (r'let p\.name_len = p\.name\.len\(\) as u32;', 'let name_len = p.name.len() as u32;'),
    (r'col \+= p\.name_len \+ 2;', 'col += name_len + 2;'),
    (r'let type_map = doc\.type_map\.name\.clone\(\);', 'let type_map = doc.type_map.clone();'),
    (r'b\.params\.iter\(\)\.map\(\|p\| p\.name\.clone\(\)\)', 'b.params.iter().map(|(n, _)| n.clone())'),
    (r'fields\.iter\(\)\.map\(\|p\| p\.name\.clone\(\)\)', 'fields.iter().map(|(n, _)| n.clone())'),
    (r'n,\n\s*mt\.as_ref\(\)', 'p.name.clone(),\n                                p.ty.as_ref()'),
    (r'p,\n\s*p\.as_ref\(\)', 'p.name.clone(),\n                                p.ty.as_ref()'),
])

replace_in_file('src/fmt.rs', [
    (r'out\.push_str\(&b\.name\);', 'out.push_str(&p.name);'),
    (r'if let Some\(t\) = &b\.ty', 'if let Some(t) = &p.ty'),
])

replace_in_file('src/parser.rs', [
    (r'using_params: Vec<crate::ast::Param>', 'using_params: Vec<(String, Option<Type>)>'),
    (r'initializer: Option<\(String, Vec<crate::ast::Param>, Expr\)> = None;', 'initializer: Option<(String, Vec<(String, Option<Type>)>, Expr)> = None;'),
    (r'using_params,\n\s*ret_type', 'using_params: using_params.into_iter().map(|p| (p.name, p.ty)).collect(),\n            ret_type'),
    (r'initializer,\n\s*type_params', 'initializer: initializer.map(|(n, p, e)| (n, p.into_iter().map(|param| (param.name, param.ty)).collect(), e)),\n            type_params'),
    (r'params,\n\s*body,\n\s*span', 'params: params.into_iter().map(|p| (p.name, p.ty)).collect(),\n                        body,\n                        span'),
    (r'params,\n\s*value,\n\s*body', 'params: params.into_iter().map(|p| (p.name, p.ty)).collect(),\n            value,\n            body'),
    (r'match param_ty \{', 'match param_ty {'),
    (r'Some\(ty\) => params\.push\(\(param_name, ty\)\),', 'Some(ty) => params.push(crate::ast::Param { name: param_name, ty: Some(ty), cap: None }),'),
    (r'name, param_name', 'name, param_name'),
])

replace_in_file('src/typechecker.rs', [
    (r'if let Some\(t\) = param_ty \{', 'if let Some(t) = &p.ty {'),
])
