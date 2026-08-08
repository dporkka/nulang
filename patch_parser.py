import re

with open('src/parser.rs', 'r') as f:
    content = f.read()

# Replace return type of parse_params
content = content.replace(
    'fn parse_params(&mut self) -> NuResult<Vec<(String, Option<Type>)>>',
    'fn parse_params(&mut self) -> NuResult<Vec<crate::ast::Param>>'
)

# In parse_params, parse capability
content = content.replace(
    'let name = self.expect_ident("parameter name")?;',
    'let cap = self.try_parse_capability();\n            let name = self.expect_ident("parameter name")?;'
)

content = content.replace(
    'params.push((name, ty));',
    'params.push(crate::ast::Param { name, ty, cap });'
)

# Replace return type of parse_params_with_defaults
content = content.replace(
    'fn parse_params_with_defaults(\n        &mut self,\n    ) -> NuResult<(Vec<(String, Option<Type>)>, Vec<Option<Expr>>)>',
    'fn parse_params_with_defaults(\n        &mut self,\n    ) -> NuResult<(Vec<crate::ast::Param>, Vec<Option<Expr>>)>'
)

with open('src/parser.rs', 'w') as f:
    f.write(content)
