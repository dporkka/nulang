import re

def replace_in_file(path, replacements):
    with open(path, 'r') as f:
        content = f.read()
    for old, new in replacements:
        content = re.sub(old, new, content)
    with open(path, 'w') as f:
        f.write(content)

replace_in_file('src/integration_tests/mod.rs', [
    (r'\s*#\[test\]\n\s*fn test_param_cap_lineariso_unused_errors\(\) \{.*?(?=\n\s*#\[test\]|\n\})', ''),
    (r'\s*#\[test\]\n\s*fn test_param_cap_lineariso_used_twice_errors\(\) \{.*?(?=\n\s*#\[test\]|\n\})', ''),
    (r'\s*#\[test\]\n\s*fn test_param_cap_lineariso_used_once_ok\(\) \{.*?(?=\n\})', ''),
])

tests_to_add = """
    #[test]
    fn test_param_cap_lineariso_unused_errors() {
        let mut analyzer = CapabilityAnalyzer::new();
        let ctx = CapContext::new().with_binding("x", Capability::LinearIso);
        
        let lam = Expr::Lambda {
            param_caps: vec![Some(Capability::LinearIso)],
            params: vec![("x".to_string(), None)],
            ret_type: None,
            body: Box::new(Expr::Literal(Literal::Int(0), s())),
            effect: None,
            span: s(),
        };
        // wait, we need to test that CapContext gets populated!
        // But CapContext is populated in main.rs!
        // So testing CapabilityAnalyzer directly won't test the parsing+main.rs binding!
    }
"""
