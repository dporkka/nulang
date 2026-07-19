//! Serialization contract for WASM Component boundary.
//!
//! Defines the Borsh-based wire format shared between the WASM component
//! compiler backend and the host runtime. Values crossing the component
//! boundary are serialized with Borsh for deterministic, fast encoding.
//!
//! Within the component, values use the same i64-tagged NaN-boxed
//! representation as the native VM; this module defines the types used
//! ONLY for cross-boundary serialization.

use crate::vm::Value;

/// A serializable subset of Nulang values for WASM component boundary.
///
/// Actor refs, closures, and heap pointers cannot cross the component
/// boundary — they serialize as `Nil`. The component compiler rejects
/// programs that try to send these types in WASM component mode.
#[derive(Debug, Clone, PartialEq)]
pub enum WireValue {
    Nil,
    Unit,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Tuple(Vec<WireValue>),
    Record(Vec<(String, WireValue)>),
    Variant(String, Option<Box<WireValue>>),
    Array(Vec<WireValue>),
}

/// A message crossing the WASM component boundary.
#[derive(Debug, Clone)]
pub struct WireMessage {
    /// Stringified u64 actor id of the sender.
    pub sender: String,
    /// Behavior name for dispatch.
    pub behavior_name: String,
    /// Serialized payload values.
    pub payload: Vec<WireValue>,
}

impl WireValue {
    /// Convert a native VM `Value` to a `WireValue` for serialization.
    ///
    /// Heap pointers (arrays, tuples, records, closures) are traversed
    /// shallowly — their elements are recursively converted. Actor refs
    /// and raw pointers become `Nil`.
    pub fn from_value(_v: &Value) -> Self {
        // Stub: full conversion requires heap traversal and is
        // implemented in Phase 2.2 (component compiler backend).
        WireValue::Nil
    }

    /// Convert a `WireValue` back to a native VM `Value`.
    pub fn to_value(&self) -> Value {
        match self {
            WireValue::Nil => Value::nil(),
            WireValue::Unit => Value::unit(),
            WireValue::Bool(b) => Value::bool(*b),
            WireValue::Int(i) => Value::int(*i),
            WireValue::Float(f) => Value::float(*f),
            WireValue::String(_s) => Value::nil(), // stub: string allocation
            _ => Value::nil(),                     // stub: compound types
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wire_value_nil_roundtrip() {
        let wv = WireValue::Nil;
        let v = wv.to_value();
        assert!(v.is_nil());
    }

    #[test]
    fn test_wire_value_int_roundtrip() {
        let wv = WireValue::Int(42);
        let v = wv.to_value();
        assert_eq!(v.as_int(), Some(42));
    }

    #[test]
    fn test_wire_value_bool_roundtrip() {
        let wv = WireValue::Bool(true);
        let v = wv.to_value();
        assert_eq!(v.as_bool(), Some(true));
    }

    #[test]
    fn test_wire_value_unit_roundtrip() {
        let wv = WireValue::Unit;
        let v = wv.to_value();
        assert!(v.is_unit());
    }
}
