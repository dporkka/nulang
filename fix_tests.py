import re

def replace_in_file(path, replacements):
    with open(path, 'r') as f:
        content = f.read()
    for old, new in replacements:
        content = re.sub(old, new, content)
    with open(path, 'w') as f:
        f.write(content)

replace_in_file('src/effect_checker.rs', [
    (r'Expr::Lambda \{\n\s*params:', 'Expr::Lambda {\n            param_caps: vec![],\n            params:'),
])

replace_in_file('src/hir.rs', [
    (r'let def = FunctionDef \{\n\s*name:', 'let def = FunctionDef {\n            name,\n            param_caps: vec![],'),
])

replace_in_file('src/hir_lower.rs', [
    (r'Decl::Function \{\n\s*name: "__main"\.to_string\(\),', 'Decl::Function {\n                name: "__main".to_string(),\n                param_caps: vec![],'),
])

replace_in_file('src/mir_codegen.rs', [
    (r'crate::hir::FunctionDef \{\n\s*name:', 'crate::hir::FunctionDef {\n                                                    name:\n                                                    param_caps: vec![],'),
    (r'let square_fn = crate::hir::FunctionDef \{\n\s*name:', 'let square_fn = crate::hir::FunctionDef {\n            name:\n            param_caps: vec![],'),
    (r'let main_fn = crate::hir::FunctionDef \{\n\s*name:', 'let main_fn = crate::hir::FunctionDef {\n            name:\n            param_caps: vec![],'),
])

# mir_codegen has specific formatting, let's just do a simpler replace
replace_in_file('src/mir_codegen.rs', [
    (r'name:\n\s*param_caps: vec\!\[\],', 'name:'), # undo bad replace
    (r'crate::hir::FunctionDef \{\n(\s*)name:', r'crate::hir::FunctionDef {\n\1param_caps: vec![],\n\1name:'),
])

replace_in_file('src/resolver.rs', [
    (r'Decl::Function \{\n\s*name:', 'Decl::Function {\n            name:\n            param_caps: vec![],'),
    (r'name:\n\s*param_caps: vec\!\[\],', 'param_caps: vec![],\n            name:'),
])

replace_in_file('src/typechecker.rs', [
    (r'Decl::Function \{\n\s*name:', 'Decl::Function {\n                param_caps: vec![],\n                name:'),
    (r'Expr::Lambda \{\n\s*params:', 'Expr::Lambda {\n            param_caps: vec![],\n            params:'),
    (r'Expr::LetRec \{\n\s*name:', 'Expr::LetRec {\n            param_caps: vec![],\n            name:'),
])

