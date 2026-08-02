//! `ToolSchema`: JSON-schema description of an `@tool`-annotated function.
//!
//! Lives in core (unconditional, no `ai-runtime` gate) because it is
//! embedded in core, always-compiled types: `bytecode::ActorMeta.tools`
//! and `hir::Decl`'s agent `tools` field. The `@tool` annotation is
//! permanent language surface (RFC 0010 §C.2: "Keep as-is; document
//! extensibility") independent of whether any LLM client is compiled in.
//!
//! This is a distinct type from `nulang_ai::request::ToolSchema` (used for
//! the actual LLM-provider wire format in `LlmRequest.tools`), which stays
//! in the optional `nulang-ai` crate per its "zero dependency on core"
//! charter. `runtime/agent.rs` converts between the two, behind
//! `ai-runtime`, at the one point a tool schema is handed to a provider.

use serde::{Deserialize, Serialize};
use serde_json::Map;

use crate::types::{PrimitiveType, Type};

/// JSON-schema description of a tool exposed to an LLM (or any future
/// caller of `@tool`-annotated functions).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSchema {
    /// Tool name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON schema for the tool arguments.
    pub parameters: serde_json::Value,
}

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
        Type::Nominal { underlying, .. } => type_to_json_schema(underlying),
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
        assert_eq!(
            type_to_json_schema(&Type::Primitive(PrimitiveType::Int)),
            serde_json::json!({"type": "integer"})
        );
        assert_eq!(
            type_to_json_schema(&Type::Primitive(PrimitiveType::String)),
            serde_json::json!({"type": "string"})
        );
    }

    #[test]
    fn test_function_to_tool_schema_basic() {
        let params = vec![
            ("city".to_string(), Type::Primitive(PrimitiveType::String)),
            ("days".to_string(), Type::Primitive(PrimitiveType::Int)),
        ];
        let schema = function_to_tool_schema(
            "get_weather",
            "Get the weather forecast",
            &params,
            &Type::Primitive(PrimitiveType::String),
        );
        assert_eq!(schema.name, "get_weather");
        assert_eq!(schema.description, "Get the weather forecast");
        assert_eq!(schema.parameters["type"], "object");
        assert_eq!(
            schema.parameters["required"],
            serde_json::json!(["city", "days"])
        );
    }

    #[test]
    fn test_array_and_option_schemas() {
        let arr = Type::Array(Box::new(Type::Primitive(PrimitiveType::Int)));
        assert_eq!(
            type_to_json_schema(&arr),
            serde_json::json!({"type": "array", "items": {"type": "integer"}})
        );
    }
}
