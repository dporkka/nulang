import re

def replace_in_file(path, replacements):
    with open(path, 'r') as f:
        content = f.read()
    for old, new in replacements:
        content = re.sub(old, new, content)
    with open(path, 'w') as f:
        f.write(content)

# 1. Fix the double param_caps in ast.rs
replace_in_file('src/ast.rs', [
    (r'param_caps: Vec<Option<Capability>>,\n\s*param_caps: Vec<Option<Capability>>,', 'param_caps: Vec<Option<Capability>>,'),
])

# 2. Fix parser.rs initializations
replace_in_file('src/parser.rs', [
    (r'Ok\(Expr::Lambda \{\n\s*params,', 'Ok(Expr::Lambda {\n            params: params.clone(),\n            param_caps: vec![None; params.len()],'),
    (r'Ok\(Expr::LetRec \{\n\s*name,', 'Ok(Expr::LetRec {\n            name,\n            param_caps: vec![None; params.len()],'),
    (r'\} => Expr::LetRec \{\n\s*name,', '} => Expr::LetRec {\n                             name,\n                             param_caps: vec![None; params.len()],'),
    (r'value: Box::new\(Expr::Lambda \{\n\s*params,', 'value: Box::new(Expr::Lambda {\n                                params: params.clone(),\n                                param_caps: vec![None; params.len()],'),
])

# 3. Fix hir_lower.rs initializations and matches
replace_in_file('src/hir_lower.rs', [
    (r'hir::Decl::Function\(hir::FunctionDef \{\n\s*name:', 'hir::Decl::Function(hir::FunctionDef {\n                name,\n                param_caps: vec![],'),
    (r'let lambda = Expr::Lambda \{\n\s*params: lambda_params,', 'let lambda = Expr::Lambda {\n                    params: lambda_params.clone(),\n                    param_caps: vec![None; lambda_params.len()],'),
    (r'value: hir::RValue::Closure \{\n\s*params: lambda_params,', 'value: hir::RValue::Closure {\n                        params: lambda_params.clone(),\n                        param_caps: vec![],'),
    (r'value: hir::RValue::RecClosure \{\n\s*name: name\.clone\(\),\n\s*params: lambda_params,', 'value: hir::RValue::RecClosure {\n                        name: name.clone(),\n                        params: lambda_params.clone(),\n                        param_caps: vec![],'),
    (r'Expr::LetRec \{\n\s*name,\n\s*params,\n\s*value,\n\s*body: b,\n\s*span,\n\s*\} => \{', 'Expr::LetRec {\n            name,\n            params,\n            value,\n            body: b,\n            span,\n            .. \n        } => {'),
])

# 4. Fix typechecker.rs matches
replace_in_file('src/typechecker.rs', [
    (r'Expr::LetRec \{\n\s*name,\n\s*params,\n\s*value,\n\s*body,\n\s*span,\n\s*\} =>', 'Expr::LetRec {\n                name,\n                params,\n                value,\n                body,\n                span,\n                ..\n            } =>'),
])
