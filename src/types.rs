//! Shared type definitions used across all modules.

// ---------------------------------------------------------------------------
// Source location
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub line: u32,
    pub col: u32,
}

// ---------------------------------------------------------------------------
// Primitive types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Primitive {
    Int,
    Float,
    String,
    Bool,
    Unit,
}

// ---------------------------------------------------------------------------
// Capabilities (permission lattice)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Capability {
    Iso,    // isolated — unique, no aliases
    Trn,    // transition — unique, can become iso or ref
    Ref,    // reference — shared read/write
    Val,    // value — shared read-only, deeply immutable
    Box,    // boxed — unique ownership
    Tag,    // tag — no access, just identity
}

// ---------------------------------------------------------------------------
// Effect rows (algebraic effects)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum EffectRow {
    Closed(Vec<String>),
    Open(Vec<String>, u64),
}

// ---------------------------------------------------------------------------
// Core type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Prim(Primitive),
    Named(String),
    Tuple(Vec<Type>),
    Record(Vec<(String, Type)>),
    Variant(Vec<(String, Vec<Type>)>),
    Array(Box<Type>),
    Arrow(Box<Type>, Box<Type>),
    Generic(Vec<String>, Box<Type>),
    Var(u64),
    Unit,
    Actor(Box<Type>),
    Cap(Capability, Box<Type>),
    App(Box<Type>, Box<Type>),
    Effect(EffectRow, Box<Type>),
}

// ---------------------------------------------------------------------------
// Runtime value representation
// ---------------------------------------------------------------------------

/// 64-bit NaN-tagged value.
/// Uses the quiet-NaN payload to store pointers and special values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Value(pub u64);

impl Value {
    pub const TAG_MASK: u64 = 0xFFFF000000000000;
    pub const PTR_MASK: u64 = 0x0000FFFFFFFFFFFF;
    pub const INT_TAG: u64 = 0x0001000000000000;
    pub const FLOAT_TAG: u64 = 0x0002000000000000;
    pub const BOOL_TAG: u64 = 0x0003000000000000;
    pub const STR_TAG: u64 = 0x0004000000000000;
    pub const ACTOR_TAG: u64 = 0x0005000000000000;
    pub const HEAP_TAG: u64 = 0x0006000000000000;
    pub const NULL_TAG: u64 = 0x0007000000000000;
    pub const TRUE_VAL: u64 = Self::BOOL_TAG | 1;
    pub const FALSE_VAL: u64 = Self::BOOL_TAG | 0;

    pub fn int(n: i64) -> Self {
        Value(Self::INT_TAG | (n as u64 & Self::PTR_MASK))
    }

    pub fn float(f: f64) -> Self {
        Value(f.to_bits())
    }

    pub fn bool(b: bool) -> Self {
        Value(if b { Self::TRUE_VAL } else { Self::FALSE_VAL })
    }

    pub fn null() -> Self {
        Value(Self::NULL_TAG)
    }

    pub fn is_int(&self) -> bool {
        (self.0 & Self::TAG_MASK) == Self::INT_TAG
    }

    pub fn is_float(&self) -> bool {
        let bits = self.0;
        (bits & 0x7FF0000000000000) == 0x7FF0000000000000 && (bits & Self::PTR_MASK) != 0
    }

    pub fn is_bool(&self) -> bool {
        (self.0 & Self::TAG_MASK) == Self::BOOL_TAG
    }

    pub fn is_null(&self) -> bool {
        self.0 == Self::NULL_TAG
    }

    pub fn is_heap_ptr(&self) -> bool {
        (self.0 & Self::TAG_MASK) == Self::HEAP_TAG
    }

    pub fn is_actor_ref(&self) -> bool {
        (self.0 & Self::TAG_MASK) == Self::ACTOR_TAG
    }

    pub fn as_int(&self) -> Option<i64> {
        if self.is_int() {
            Some((self.0 & Self::PTR_MASK) as i64)
        } else {
            None
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        if !self.is_int() && !self.is_bool() && !self.is_null() && !self.is_heap_ptr() && !self.is_actor_ref() {
            Some(f64::from_bits(self.0))
        } else {
            None
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        if self.is_bool() {
            Some((self.0 & 1) != 0)
        } else {
            None
        }
    }

    pub fn heap_ptr(addr: usize) -> Self {
        Value(Self::HEAP_TAG | (addr as u64 & Self::PTR_MASK))
    }

    pub fn actor_ref(node_id: u16, local_id: u32, generation: u16) -> Self {
        let packed = ((node_id as u64) << 48) | ((local_id as u64) << 16) | (generation as u64);
        Value(Self::ACTOR_TAG | (packed & Self::PTR_MASK))
    }

    pub fn as_heap_ptr(&self) -> Option<usize> {
        if self.is_heap_ptr() {
            Some((self.0 & Self::PTR_MASK) as usize)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_value_int() {
        let v = Value::int(42);
        assert!(v.is_int());
        assert_eq!(v.as_int(), Some(42));
    }

    #[test]
    fn test_value_float() {
        let v = Value::float(3.14);
        assert_eq!(v.as_float(), Some(3.14));
    }

    #[test]
    fn test_value_bool() {
        let v = Value::bool(true);
        assert!(v.is_bool());
        assert_eq!(v.as_bool(), Some(true));
    }

    #[test]
    fn test_value_null() {
        let v = Value::null();
        assert!(v.is_null());
    }

    #[test]
    fn test_value_heap_ptr() {
        let v = Value::heap_ptr(0x1234);
        assert!(v.is_heap_ptr());
        assert_eq!(v.as_heap_ptr(), Some(0x1234));
    }
}
