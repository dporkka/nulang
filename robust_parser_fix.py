import re

def replace_in_file(path, replacements):
    with open(path, 'r') as f:
        content = f.read()
    for old, new in replacements:
        content = re.sub(old, new, content)
    with open(path, 'w') as f:
        f.write(content)

replace_in_file('src/parser.rs', [
    (r'fn parse_params\(&mut self\) -> NuResult<Vec<\(String, Option<Type>\)>> \{', 'fn parse_params(&mut self) -> NuResult<(Vec<(String, Option<Type>)>, Vec<Option<Capability>>)> {'),
    (r'let mut params = Vec::new\(\);\n\s*self\.skip_newlines\(\);', 'let mut params = Vec::new();\n        let mut param_caps = Vec::new();\n        self.skip_newlines();'),
    (r'let name = self\.expect_ident\("parameter name"\)\?;', 'let cap = self.try_parse_capability();\n            let name = self.expect_ident("parameter name")?;'),
    (r'params\.push\(\(name, ty\)\);', 'params.push((name, ty));\n            param_caps.push(cap);'),
    (r'Ok\(params\)\n\s*\}', 'Ok((params, param_caps))\n    }'),

    (r'fn parse_params_with_defaults\(\n\s*&mut self,\n\s*\) -> NuResult<\(Vec<\(String, Option<Type>\)>, Vec<Option<Expr>>\)> \{', 'fn parse_params_with_defaults(\n        &mut self,\n    ) -> NuResult<(Vec<(String, Option<Type>)>, Vec<Option<Capability>>, Vec<Option<Expr>>)> {'),
    (r'let mut defaults = Vec::new\(\);\n\s*self\.skip_newlines\(\);', 'let mut defaults = Vec::new();\n        let mut param_caps = Vec::new();\n        self.skip_newlines();'),
    (r'defaults\.push\(default\);', 'defaults.push(default);\n            param_caps.push(cap);'),
    (r'Ok\(\(params, defaults\)\)\n\s*\}', 'Ok((params, param_caps, defaults))\n    }'),

    # Fix callsites
    (r'let \(params, default_values\) = self\.parse_params_with_defaults\(\)\?;', 'let (params, param_caps, default_values) = self.parse_params_with_defaults()?;'),
    (r'let \(up, _\) = self\.parse_params_with_defaults\(\)\?;', 'let (up, _up_caps, _) = self.parse_params_with_defaults()?;'),
    (r'let params = self\.parse_params\(\)\?;', 'let (params, param_caps) = self.parse_params()?;'),
    (r'let raw_params = self\.parse_params\(\)\?;', 'let (raw_params, _caps) = self.parse_params()?;'),
])

replace_in_file('src/ast.rs', [
    (r'pub struct Behavior \{\n\s*pub name: String,\n\s*pub params: Vec<\(String, Option<Type>\)>,', 'pub struct Behavior {\n    pub name: String,\n    pub params: Vec<(String, Option<Type>)>,    pub param_caps: Vec<Option<Capability>>,'),
    (r'pub struct StateMachineEvent \{\n\s*pub name: String,\n\s*pub params: Vec<\(String, Option<Type>\)>,', 'pub struct StateMachineEvent {\n    pub name: String,\n    pub params: Vec<(String, Option<Type>)>,    pub param_caps: Vec<Option<Capability>>,'),
])
