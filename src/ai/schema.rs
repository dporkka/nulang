//! JSON-schema generation for `@tool`-annotated Nulang functions.
//!
//! Converts Nulang type signatures into the JSON Schema subset used by
//! `ToolSchema.parameters` so that LLM providers can request tool calls.

use serde_json::Map;

use crate::ai::request::ToolSchema;
use crate::types::{PrimitiveType, Type};

/// Convert a Nulang type into a JSON Schema value.
///
/// Supported shapes:
/// - Primitives: Int, Float, Bool, String, Unit
/// - Records: `{ type: "object", properties: {...} }`
/// - Arrays / Lists: `{ type: "array", items: ... }`
/// - Tuples: `{ type: "array", prefixItems: [...] }`
/// - Variants: `{ oneOf: [...] }`
/// - References: unwrap to the inner type
pub fn type_to_json_schema(ty: &Type) -> serde_json::Value {
    match ty {
        Type::Primitive(PrimitiveType::Int) => serde_json::json!({"type": "integer"}),
        Type::Primitive(PrimitiveType::Float) => serde_json::json!({"type": "number"}),
        Type::Primitive(PrimitiveType::Bool) => serde_json::json!({"type": "boolean"}),
        Type::Primitive(PrimitiveType::String) => serde_json::json!({"type": "string"}),
        Type::Primitive(PrimitiveType::Unit) => serde_json::json!({"type": "null"}),
        Type::Primitive(PrimitiveType::Nil) => serde_json::json!({"type": "null"}),
        Type::Primitive(PrimitiveType::Never) | Type::Primitive(PrimitiveType::Address) => {
            serde_json::json!({})
        }
        Type::Record(fields) => {
            let mut properties = Map::new();
            let mut required = Vec::new();
            for (name, field_ty) in fields {
                properties.insert(name.clone(), type_to_json_schema(field_ty));
                required.push(name.clone());
            }
            serde_json::json!({
                "type": "object",
                "properties": properties,
                "required": required,
            })
        }
        Type::Array(inner) => serde_json::json!({
            "type": "array",
            "items": type_to_json_schema(inner),
        }),
        Type::Tuple(elems) => serde_json::json!({
            "type": "array",
            "prefixItems": elems.iter().map(type_to_json_schema).collect::<Vec<_>>(),
        }),
        Type::Variant(variants) => {
            let one_of: Vec<serde_json::Value> = variants
                .iter()
                .map(|(name, inner)| {
                    if let Some(inner_ty) = inner {
                        serde_json::json!({
                            "type": "object",
                            "properties": {
                                name: type_to_json_schema(inner_ty),
                            },
                            "required": [name],
                        })
                    } else {
                        serde_json::json!({
                            "type": "string",
                            "enum": [name],
                        })
                    }
                })
                .collect();
            serde_json::json!({ "oneOf": one_of })
        }
        Type::Reference { inner, .. } => type_to_json_schema(inner),
        Type::App { constructor, args } => {
            // Common constructors: List[T], Array[T], Option[T]
            if let Type::Var(_) | Type::Primitive(_) | Type::App { .. } = constructor.as_ref() {
                // Cannot determine a concrete schema; fall through to unknown.
                return serde_json::json!({});
            }
            let constructor_name = type_name(constructor);
            match constructor_name.as_deref() {
                Some("List") | Some("Array") if !args.is_empty() => serde_json::json!({
                    "type": "array",
                    "items": type_to_json_schema(&args[0]),
                }),
                Some("Option") if !args.is_empty() => serde_json::json!({
                    "anyOf": [
                        {"type": "null"},
                        type_to_json_schema(&args[0]),
                    ],
                }),
                _ => serde_json::json!({}),
            }
        }
        Type::Function { .. } => serde_json::json!({}),
        Type::Actor { .. } => serde_json::json!({}),
        Type::Scheme { body, .. } => type_to_json_schema(body),
        Type::Var(_) => serde_json::json!({}),
    }
}

/// Best-effort name extraction for type-application constructors.
fn type_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Primitive(p) => Some(format!("{:?}", p)),
        Type::Var(_) => None,
        Type::App { constructor, .. } => type_name(constructor),
        Type::Reference { inner, .. } => type_name(inner),
        _ => None,
    }
}

/// Build a `ToolSchema` from a Nulang function signature.
///
/// `params` must contain the explicit parameter types; `ret` is validated to be
/// present but is not included in the tool schema (providers only need argument
/// schemas).
pub fn function_to_tool_schema(
    name: &str,
    description: &str,
    params: &[(String, Type)],
    _ret: &Type,
) -> ToolSchema {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for (param_name, param_ty) in params {
        properties.insert(param_name.clone(), type_to_json_schema(param_ty));
        required.push(param_name.clone());
    }

    ToolSchema {
        name: name.to_string(),
        description: description.to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_primitive_schemas() {
        assert_eq!(type_to_json_schema(&Type::int()), serde_json::json!({"type": "integer"}));
        assert_eq!(type_to_json_schema(&Type::float()), serde_json::json!({"type": "number"}));
        assert_eq!(type_to_json_schema(&Type::bool()), serde_json::json!({"type": "boolean"}));
        assert_eq!(type_to_json_schema(&Type::string()), serde_json::json!({"type": "string"}));
    }

    #[test]
    fn test_array_schema() {
        let ty = Type::Array(Box::new(Type::int()));
        let schema = type_to_json_schema(&ty);
        assert_eq!(schema["type"], "array");
        assert_eq!(schema["items"], serde_json::json!({"type": "integer"}));
    }

    #[test]
    fn test_record_schema() {
        let ty = Type::Record(vec![
            ("x".to_string(), Type::int()),
            ("y".to_string(), Type::int()),
        ]);
        let schema = type_to_json_schema(&ty);
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["x"], serde_json::json!({"type": "integer"}));
        assert_eq!(schema["properties"]["y"], serde_json::json!({"type": "integer"}));
        assert!(schema["required"].as_array().unwrap().contains(&serde_json::json!("x")));
        assert!(schema["required"].as_array().unwrap().contains(&serde_json::json!("y")));
    }

    #[test]
    fn test_function_to_tool_schema() {
        let params = vec![
            ("a".to_string(), Type::int()),
            ("b".to_string(), Type::int()),
        ];
        let tool = function_to_tool_schema("add", "Add two integers.", &params, &Type::int());
        assert_eq!(tool.name, "add");
        assert_eq!(tool.description, "Add two integers.");
        assert_eq!(tool.parameters["type"], "object");
        assert_eq!(tool.parameters["properties"]["a"], serde_json::json!({"type": "integer"}));
        assert_eq!(tool.parameters["properties"]["b"], serde_json::json!({"type": "integer"}));
        assert!(tool.parameters["required"].as_array().unwrap().contains(&serde_json::json!("a")));
        assert!(tool.parameters["required"].as_array().unwrap().contains(&serde_json::json!("b")));
    }
}
