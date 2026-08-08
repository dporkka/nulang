import re

def replace_in_file(path, replacements):
    with open(path, 'r') as f:
        content = f.read()
    for old, new in replacements:
        content = re.sub(old, new, content)
    with open(path, 'w') as f:
        f.write(content)

replace_in_file('src/ast.rs', [
    (r'params: Vec<\(String, Option<Type>\)>,\n\s*ret_type: Option<Type>,',
     'params: Vec<(String, Option<Type>)>,\n        param_caps: Vec<Option<Capability>>,\n        ret_type: Option<Type>,'),
    (r'params: Vec<\(String, Option<Type>\)>,\n\s*value: Box<Expr>,',
     'params: Vec<(String, Option<Type>)>,\n        param_caps: Vec<Option<Capability>>,\n        value: Box<Expr>,'),
    (r'params: Vec<\(String, Option<Type>\)>,\n\s*/// Default values',
     'params: Vec<(String, Option<Type>)>,\n        param_caps: Vec<Option<Capability>>,\n        /// Default values')
])

replace_in_file('src/hir.rs', [
    (r'pub params: Vec<\(String, Type\)>,\n\s*pub dict_params',
     'pub params: Vec<(String, Type)>,\n    pub param_caps: Vec<Capability>,\n    pub dict_params'),
    (r'params: Vec<\(String, Type\)>,\n\s*body: Box<Body>,',
     'params: Vec<(String, Type)>,\n        param_caps: Vec<Capability>,\n        body: Box<Body>,'),
])

