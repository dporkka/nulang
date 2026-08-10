//! WIT (WASM Interface Type) world generator.
//!
//! Maps Nulang effect signatures to WIT interfaces so compiled Nulang
//! actors are valid WASI 0.2+ components pluggable into any compliant host.
//!
//! Each Nulang effect module (IO, Timer, Signal, Provider, etc.) becomes
//! a WIT `import` interface. The host provides matching `export` functions.
//! The component's effect `perform` calls compile to WIT import calls.
//!
//! ## Example
//!
//! A Nulang actor with effects `{IO, Timer}` generates:
//! ```wit
//! package nulang:generated;
//! world actor {
//!     import io: interface {
//!         print: func(msg: string);
//!         read: func() -> string;
//!     }
//!     import timer: interface {
//!         sleep: func(ms: u64);
//!     }
//!     export init: func() -> s64;
//!     export handle-message: func(msg: list<u8>) -> s64;
//!     export checkpoint: func() -> list<u8>;
//! }
//! ```

use std::collections::BTreeSet;

/// A single operation in a WIT interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitOp {
    pub name: String,
    pub params: Vec<(String, String)>, // (name, WIT type)
    pub result: Option<String>,        // WIT return type
}

/// A WIT interface (e.g., `interface io { ... }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitInterface {
    pub name: String,
    pub ops: Vec<WitOp>,
}

/// A complete WIT world definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitWorld {
    pub package: String,
    pub world_name: String,
    pub imports: Vec<WitInterface>,
    pub exports: Vec<WitOp>,
}

/// Mapping from Nulang effect names to their WIT interface definitions.
///
/// This is the canonical registry of built-in effects that can cross the
/// WASM component boundary. Custom effects are not yet supported.
pub fn builtin_effect_wit_interfaces() -> Vec<WitInterface> {
    vec![
        WitInterface {
            name: "io".into(),
            ops: vec![
                WitOp {
                    name: "print".into(),
                    params: vec![("msg".into(), "string".into())],
                    result: None,
                },
                WitOp {
                    name: "read".into(),
                    params: vec![],
                    result: Some("string".into()),
                },
            ],
        },
        WitInterface {
            name: "timer".into(),
            ops: vec![WitOp {
                name: "sleep".into(),
                params: vec![("ms".into(), "u64".into())],
                result: None,
            }],
        },
        WitInterface {
            name: "random".into(),
            ops: vec![WitOp {
                name: "u64".into(),
                params: vec![],
                result: Some("u64".into()),
            }],
        },
        WitInterface {
            name: "signal".into(),
            ops: vec![
                WitOp {
                    name: "wait".into(),
                    params: vec![("name".into(), "string".into())],
                    result: None,
                },
                WitOp {
                    name: "notify".into(),
                    params: vec![("name".into(), "string".into())],
                    result: None,
                },
            ],
        },
        WitInterface {
            name: "provider".into(),
            ops: vec![WitOp {
                name: "ask".into(),
                params: vec![
                    ("provider".into(), "string".into()),
                    ("prompt".into(), "string".into()),
                ],
                result: Some("string".into()),
            }],
        },
    ]
}

/// Generate a WIT world for the given set of Nulang effect names.
///
/// `effects` is a set of effect module names (e.g., `{"io", "timer"}`).
/// Only built-in effects with known WIT mappings are included; unknown
/// effects are silently skipped.
pub fn generate_wit_world(effects: &BTreeSet<String>) -> WitWorld {
    let all_interfaces = builtin_effect_wit_interfaces();
    let imports: Vec<WitInterface> = all_interfaces
        .into_iter()
        .filter(|iface| effects.contains(&iface.name))
        .collect();

    WitWorld {
        package: "nulang:generated".into(),
        world_name: "actor".into(),
        imports,
        exports: vec![
            WitOp {
                name: "init".into(),
                params: vec![],
                result: Some("s64".into()),
            },
            WitOp {
                name: "handle-message".into(),
                params: vec![("msg".into(), "list<u8>".into())],
                result: Some("s64".into()),
            },
            WitOp {
                name: "checkpoint".into(),
                params: vec![],
                result: Some("list<u8>".into()),
            },
        ],
    }
}

/// Render a WIT world to its text representation.
pub fn render_wit(world: &WitWorld) -> String {
    let mut out = String::new();

    // Package header
    out.push_str(&format!("package {};\n", world.package));
    out.push_str(&format!("world {} {{\n", world.world_name));

    // Imports
    for iface in &world.imports {
        out.push_str(&format!("    import {}: interface {{\n", iface.name));
        for op in &iface.ops {
            let params: Vec<String> = op
                .params
                .iter()
                .map(|(n, t)| format!("{}: {}", n, t))
                .collect();
            let params_str = params.join(", ");
            match &op.result {
                Some(ret) => out.push_str(&format!(
                    "        {}: func({}) -> {};\n",
                    op.name, params_str, ret
                )),
                None => out.push_str(&format!("        {}: func({});\n", op.name, params_str)),
            }
        }
        out.push_str("    }\n");
    }

    // Exports
    for op in &world.exports {
        let params: Vec<String> = op
            .params
            .iter()
            .map(|(n, t)| format!("{}: {}", n, t))
            .collect();
        let params_str = params.join(", ");
        match &op.result {
            Some(ret) => out.push_str(&format!(
                "    export {}: func({}) -> {};\n",
                op.name, params_str, ret
            )),
            None => out.push_str(&format!("    export {}: func({});\n", op.name, params_str)),
        }
    }

    out.push_str("}\n");
    out
}

/// Extract effect names from a Nulang source program's effect row.
///
/// This is a simplified parser that scans the source for `perform Effect.op`
/// patterns and returns the set of effect module names used.
pub fn extract_effects_from_source(source: &str) -> BTreeSet<String> {
    let mut effects = BTreeSet::new();

    // Look for `perform <Effect>.<op>(...)` patterns
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("perform ") {
            if let Some(dot_pos) = rest.find('.') {
                let effect_name = rest[..dot_pos].trim().to_lowercase();
                // Map Nulang effect names to WIT interface names
                let wit_name = match effect_name.as_str() {
                    "io" => "io",
                    "timer" => "timer",
                    "random" => "random",
                    "signal" => "signal",
                    "provider" | "inference" => "provider",
                    _ => continue, // unknown effect, skip
                };
                effects.insert(wit_name.to_string());
            }
        }
    }

    effects
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_wit_empty() {
        let effects = BTreeSet::new();
        let world = generate_wit_world(&effects);
        assert!(world.imports.is_empty());
        assert_eq!(world.exports.len(), 3);
    }

    #[test]
    fn test_generate_wit_io_only() {
        let effects: BTreeSet<String> = ["io".into()].into();
        let world = generate_wit_world(&effects);
        assert_eq!(world.imports.len(), 1);
        assert_eq!(world.imports[0].name, "io");
    }

    #[test]
    fn test_generate_wit_io_and_timer() {
        let effects: BTreeSet<String> = ["io".into(), "timer".into()].into();
        let world = generate_wit_world(&effects);
        assert_eq!(world.imports.len(), 2);
    }

    #[test]
    fn test_render_wit_io_only() {
        let effects: BTreeSet<String> = ["io".into()].into();
        let world = generate_wit_world(&effects);
        let wit = render_wit(&world);
        assert!(wit.contains("package nulang:generated;"));
        assert!(wit.contains("import io: interface {"));
        assert!(wit.contains("print: func(msg: string);"));
        assert!(wit.contains("export init: func() -> s64;"));
    }

    #[test]
    fn test_extract_effects() {
        let source = r#"
            fn main() {
                perform IO.print("hello");
                perform Timer.sleep(100);
            }
        "#;
        let effects = extract_effects_from_source(source);
        assert!(effects.contains("io"));
        assert!(effects.contains("timer"));
        assert_eq!(effects.len(), 2);
    }

    #[test]
    fn test_extract_effects_empty() {
        let effects = extract_effects_from_source("fn main() { 42 }");
        assert!(effects.is_empty());
    }
}
