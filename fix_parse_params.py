import re

with open('src/parser.rs', 'r') as f:
    content = f.read()

# 1. Update parse_params signature
content = content.replace(
    'fn parse_params(&mut self) -> NuResult<Vec<(String, Option<Type>)>> {',
    'fn parse_params(&mut self) -> NuResult<(Vec<(String, Option<Type>)>, Vec<Option<Capability>>)> {'
)

# 2. Update parse_params body
content = content.replace(
    'let mut params = Vec::new();\n        self.skip_newlines();\n        while self.peek_kind() != &TokenKind::RParen && !self.is_at_end() {\n            let name = self.expect_ident("parameter name")?;',
    'let mut params = Vec::new();\n        let mut param_caps = Vec::new();\n        self.skip_newlines();\n        while self.peek_kind() != &TokenKind::RParen && !self.is_at_end() {\n            let cap = self.try_parse_capability();\n            let name = self.expect_ident("parameter name")?;'
)
content = content.replace(
    'params.push((name, ty));\n            self.skip_newlines();\n            if !self.consume_if(&TokenKind::Comma) {\n                break;\n            }\n            self.skip_newlines();\n        }\n        Ok(params)\n    }',
    'params.push((name, ty));\n            param_caps.push(cap);\n            self.skip_newlines();\n            if !self.consume_if(&TokenKind::Comma) {\n                break;\n            }\n            self.skip_newlines();\n        }\n        Ok((params, param_caps))\n    }'
)

# 3. Update parse_params_with_defaults signature
content = content.replace(
    'fn parse_params_with_defaults(\n        &mut self,\n    ) -> NuResult<(Vec<(String, Option<Type>)>, Vec<Option<Expr>>)> {',
    'fn parse_params_with_defaults(\n        &mut self,\n    ) -> NuResult<(Vec<(String, Option<Type>)>, Vec<Option<Capability>>, Vec<Option<Expr>>)> {'
)

# 4. Update parse_params_with_defaults body
content = content.replace(
    'let mut params = Vec::new();\n        let mut defaults = Vec::new();\n        self.skip_newlines();\n        while self.peek_kind() != &TokenKind::RParen && !self.is_at_end() {\n            let name = self.expect_ident("parameter name")?;',
    'let mut params = Vec::new();\n        let mut defaults = Vec::new();\n        let mut param_caps = Vec::new();\n        self.skip_newlines();\n        while self.peek_kind() != &TokenKind::RParen && !self.is_at_end() {\n            let cap = self.try_parse_capability();\n            let name = self.expect_ident("parameter name")?;'
)
content = content.replace(
    'params.push((name, ty));\n            defaults.push(default);',
    'params.push((name, ty));\n            defaults.push(default);\n            param_caps.push(cap);'
)
content = content.replace(
    'Ok((params, defaults))\n    }',
    'Ok((params, param_caps, defaults))\n    }'
)

# 5. Fix callsites
content = content.replace(
    'let (params, default_values) = self.parse_params_with_defaults()?;',
    'let (params, param_caps, default_values) = self.parse_params_with_defaults()?;'
)
content = content.replace(
    'let (using_params, _) = self.parse_params_with_defaults()?;',
    'let (using_params, _using_caps, _) = self.parse_params_with_defaults()?;'
)
content = content.replace(
    'let params = self.parse_params()?;',
    'let (params, param_caps) = self.parse_params()?;'
)
content = content.replace(
    'let raw_params = self.parse_params()?;',
    'let (raw_params, _caps) = self.parse_params()?;'
)

with open('src/parser.rs', 'w') as f:
    f.write(content)
