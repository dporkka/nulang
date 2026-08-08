import re

with open('src/ast.rs', 'r') as f:
    content = f.read()

# Replace in Lambda
content = re.sub(
    r'Lambda \{\n\s*params: Vec<\(String, Option<Type>\)>,\n',
    r'Lambda {\n        params: Vec<Param>,\n',
    content
)

# Replace in LetRec
content = re.sub(
    r'LetRec \{\n\s*name: String,\n\s*params: Vec<\(String, Option<Type>\)>,\n',
    r'LetRec {\n        name: String,\n        params: Vec<Param>,\n',
    content
)

# Replace in Function
content = re.sub(
    r'type_param_constraints: Vec<\(String, TypeVar, Vec<String>\)>,\n\s*params: Vec<\(String, Option<Type>\)>,\n',
    r'type_param_constraints: Vec<(String, TypeVar, Vec<String>)>,\n        params: Vec<Param>,\n',
    content
)

with open('src/ast.rs', 'w') as f:
    f.write(content)
