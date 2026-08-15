//! Virtual machine implementation for Nulang.
//!
//! This module implements the register-based bytecode interpreter (VM) and
//! NaN-boxed value representation. It also contains the compiler backend
//! (Cranelift-based JIT and AOT helpers) used by the actor runtime and
//! standalone runner.

pub mod actor_runtime;
pub mod backend;
pub mod monomorphize;

use std::collections::HashMap;
use std::ffi::{c_char, CStr, CString};

use crate::bytecode::{CodeModule, Constant, Instruction, OpCode};
use crate::types::effects::EffectMask;
use crate::types::{Capability, NuError, NuResult, Span};
use crate::value_layout::{
    float_bits, int48_in_range, is_float_raw, sext48, tag_int, INT48_MAX, INT48_MIN,
    PAYLOAD_MASK, TAG_ACTOR, TAG_BOOL, TAG_CLOSURE, TAG_INT, TAG_MASK, TAG_NIL, TAG_PTR,
    TAG_STRING, TAG_UNIT,
};

// ---------------------------------------------------------------------------
// Value representation (NaN-boxing)
// ---------------------------------------------------------------------------

/// A VM value.
///
/// Values use NaN-boxing: the upper 16 bits encode the type tag, the lower
/// 48 bits carry the payload. This keeps values in a single machine word and
/// makes equality checks cheap, at the cost of a 48-bit integer range.
///
/// See `src/value_layout.rs` for the bit-level constants shared with the
/// JIT/AOT/WASM backends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Value {
    raw: u64,
}

/// Default callback object used when no actor callbacks are installed.
///
/// Implements `VmCallbacks` with no-op behavior so the VM can run standalone
/// without a host runtime. Tests override this via `set_actor_callbacks`.
pub struct StandaloneVmCallbacks {
    pub gc: crate::gc::GarbageCollector,
    pub io_output: Option<std::rc::Rc<std::cell::RefCell<Vec<String>>>>,
}

impl StandaloneVmCallbacks {
    pub fn new() -> Self {
        Self {
            gc: crate::gc::GarbageCollector::new(),
            io_output: None,
        }
    }
}

impl Default for StandaloneVmCallbacks {
    fn default() -> Self {
        Self::new()
    }
}

impl actor_runtime::VmCallbacks for StandaloneVmCallbacks {
    fn alloc(&mut self, size: usize, tag: actor_runtime::HeapTypeTag) -> NuResult<*mut u8> {
        Ok(self.gc.alloc(size, tag))
    }
    fn release(&mut self, ptr: *mut u8) {
        self.gc.free_object(ptr)
    }
    fn retain(&mut self, _ptr: *mut u8) {}
    fn local_ref_dropped(&mut self) {}
    fn io_print(&mut self, s: &str) {
        if let Some(sink) = &self.io_output {
            sink.borrow_mut().push(s.to_string());
        }
    }
}

/// Trait for distributed-runtime callbacks (node identity, migration, remote
/// ask, gossip). Default implementations are no-ops returning zero values.
pub trait DistributedVmCallbacks: std::any::Any {
    fn node_id(&self) -> u64;
    fn migrate(&mut self, actor_id: u64, target_node_id: u64);
    fn remote_ask(&mut self, target_actor: u64, behavior: &str, args: &[Value], timeout_ms: u64) -> Value;
    fn gossip(&mut self, message: &str) -> Value;
}

/// Value layout constants re-exported for tests and backends.
pub const CLOSURE_ENV_FLAG: u64 = 0x0000_8000_0000_0000;
pub const CLOSURE_ENV_IDX_MASK: u64 = 0x0000_7FFF_FFFF_FFFF;

impl Value {
    /// Create an integer value.
    ///
    /// The payload is a 48-bit signed integer (range [`INT48_MIN`],
    /// [`INT48_MAX`]). Values outside that range are silently masked to 48
    /// bits; callers that can produce larger magnitudes (e.g. integer
    /// arithmetic) must range-check first and raise an overflow error.
    pub fn int(n: i64) -> Self {
        debug_assert!(
            int48_in_range(n),
            "Value::int payload {n} out of 48-bit range [{INT48_MIN}, {INT48_MAX}]"
        );
        // Store directly in the 48-bit payload.
        let payload = (n as u64) & PAYLOAD_MASK;
        Value {
            raw: TAG_INT | payload,
        }
    }

    /// Create a float value.
    ///
    /// NaN results are canonicalized to `CANONICAL_NAN_BITS` so they can never
    /// alias a type tag in the boxed representation (see `value_layout`).
    pub fn float(f: f64) -> Self {
        Value {
            raw: float_bits(f),
        }
    }

    /// Create a boolean value.
    pub fn bool(b: bool) -> Self {
        Value {
            raw: TAG_BOOL | (b as u64),
        }
    }

    /// The `nil` value.
    pub fn nil() -> Self {
        Value { raw: TAG_NIL }
    }

    /// The `unit` value.
    pub fn unit() -> Self {
        Value { raw: TAG_UNIT }
    }

    /// Create an actor reference.
    pub fn actor_ref(id: u64) -> Self {
        Value {
            raw: TAG_ACTOR | (id & PAYLOAD_MASK),
        }
    }

    /// Create a heap pointer value.
    pub fn ptr(offset: *mut u8) -> Self {
        Value {
            raw: TAG_PTR | ((offset as u64) & PAYLOAD_MASK),
        }
    }

    /// Create an interned-string value from a constant-pool index.
    pub fn string(idx: u32) -> Self {
        Value {
            raw: TAG_STRING | (idx as u64),
        }
    }

    /// Create a closure value from a function index (immediate closure).
    pub fn closure(func_idx: u64) -> Self {
        Value {
            raw: TAG_CLOSURE | (func_idx & PAYLOAD_MASK),
        }
    }

    /// Create an env-carrying closure value from a closure-env table index.
    pub fn closure_env(env_idx: u64) -> Self {
        Value {
            raw: TAG_CLOSURE | CLOSURE_ENV_FLAG | (env_idx & CLOSURE_ENV_IDX_MASK),
        }
    }

    /// True when this value is `nil`.
    pub fn is_nil(&self) -> bool {
        self.raw == TAG_NIL
    }

    /// True when this value is `unit`.
    pub fn is_unit(&self) -> bool {
        self.raw == TAG_UNIT
    }

    /// True when this value is a boolean.
    pub fn is_bool(&self) -> bool {
        (self.raw & TAG_MASK) == TAG_BOOL
    }

    /// True when this value is an integer.
    pub fn is_int(&self) -> bool {
        (self.raw & TAG_MASK) == TAG_INT
    }

    /// True when this value is a float.
    pub fn is_float(&self) -> bool {
        is_float_raw(self.raw)
    }

    /// True when this value is an interned string.
    pub fn is_string(&self) -> bool {
        (self.raw & TAG_MASK) == TAG_STRING
    }

    /// True when this value is a heap pointer.
    pub fn is_ptr(&self) -> bool {
        (self.raw & TAG_MASK) == TAG_PTR
    }

    /// True when this value is an actor reference.
    pub fn is_actor_ref(&self) -> bool {
        (self.raw & TAG_MASK) == TAG_ACTOR
    }

    /// True when this value is a closure.
    pub fn is_closure(&self) -> bool {
        (self.raw & TAG_MASK) == TAG_CLOSURE
    }

    /// Extract the integer payload, or None when not an integer.
    pub fn as_int(&self) -> Option<i64> {
        if self.is_int() {
            Some(sext48(self.raw & PAYLOAD_MASK))
        } else {
            None
        }
    }

    /// Extract the float payload, or None when not a float.
    pub fn as_float(&self) -> Option<f64> {
        if self.is_float() {
            Some(f64::from_bits(self.raw))
        } else {
            None
        }
    }

    /// Extract the boolean payload, or None when not a boolean.
    pub fn as_bool(&self) -> Option<bool> {
        if self.is_bool() {
            Some((self.raw & 1) == 1)
        } else {
            None
        }
    }

    /// Extract the heap pointer, or None when not a pointer.
    pub fn as_ptr(&self) -> Option<*mut u8> {
        if self.is_ptr() {
            Some((self.raw & PAYLOAD_MASK) as *mut u8)
        } else {
            None
        }
    }

    /// Extract the actor id, or None when not an actor reference.
    pub fn as_actor_id(&self) -> Option<u64> {
        if self.is_actor_ref() {
            Some(self.raw & PAYLOAD_MASK)
        } else {
            None
        }
    }

    /// Extract the interned-string constant index, or None.
    pub fn as_string_id(&self) -> Option<u32> {
        if self.is_string() {
            Some((self.raw & PAYLOAD_MASK) as u32)
        } else {
            None
        }
    }

    /// Coarse type name used in runtime error messages.
    pub fn type_name(&self) -> &'static str {
        if self.is_nil() {
            "Nil"
        } else if self.is_unit() {
            "Unit"
        } else if self.is_int() {
            "Int"
        } else if self.is_float() {
            "Float"
        } else if self.is_bool() {
            "Bool"
        } else if self.is_string() {
            "String"
        } else if self.is_closure() {
            "Closure"
        } else if self.is_actor_ref() {
            "Actor"
        } else if self.is_ptr() {
            "Pointer"
        } else {
            "Unknown"
        }
    }

    /// Raw bit access (for JIT/AOT interop and serialization).
    pub fn as_raw(&self) -> u64 {
        self.raw
    }

    /// Rebuild a value from raw bits (used by JIT/AOT and serialization).
    pub fn from_raw(raw: u64) -> Self {
        Value { raw }
    }

    /// Bit-pattern access used by marshalling and serialization code.
    pub fn to_bits(&self) -> u64 {
        self.raw
    }

    /// Human-readable representation used for string coercion and error
    /// messages. Heap pointers are shown as a debug placeholder (the VM does
    /// not chase pointers here; `string_operand` handles heap strings).
    pub fn to_string_repr(&self) -> String {
        if self.is_nil() {
            "nil".to_string()
        } else if self.is_unit() {
            "()".to_string()
        } else if let Some(n) = self.as_int() {
            format!("{}", n)
        } else if let Some(f) = self.as_float() {
            if f.fract() == 0.0 && f.is_finite() {
                format!("{:.1}", f)
            } else {
                format!("{}", f)
            }
        } else if let Some(b) = self.as_bool() {
            format!("{}", b)
        } else if let Some(id) = self.as_actor_id() {
            format!("actor({})", id)
        } else if self.is_ptr() {
            format!("<ptr {:p}>", (self.raw & PAYLOAD_MASK) as *const u8)
        } else if self.is_string() {
            format!("<str#{}>", self.raw & PAYLOAD_MASK)
        } else if self.is_closure() {
            format!("<closure#{}>", self.raw & PAYLOAD_MASK)
        } else {
            "<unknown>".to_string()
        }
    }
}

/// Runtime error for integer arithmetic whose result does not fit in the
/// 48-bit payload range.
///
/// SPEC2 §3.2.2 defines `Int` as a 64-bit signed integer; the VM's tagged
/// encoding currently carries only 48-bit payloads, so overflow of that range
/// is reported as a runtime error instead of silently wrapping (the wider
/// encoding / bignum support is a known limitation).
fn int_overflow_error(op: &str, a: i64, b: i64) -> NuError {
    NuError::runtime_error(
        format!(
            "integer overflow: `{}` on {} and {} exceeds the 48-bit range \
             [{}, {}] supported by the VM encoding \
             (spec: Int is i64; wider encoding is a known limitation)",
            op, a, b, INT48_MIN, INT48_MAX
        ),
        Span::default(),
    )
}

/// Runtime error for arithmetic on operands of the wrong type.
fn arith_type_error(op: &str, a: Value, b: Value) -> NuError {
    NuError::runtime_error(
        format!(
            "type error: arithmetic `{}` requires numeric operands, got {} and {}",
            op,
            a.type_name(),
            b.type_name()
        ),
        Span::default(),
    )
}

/// Convert a bytecode constant into a runtime value.
pub(crate) fn constant_to_value(c: &Constant) -> Value {
    match c {
        Constant::Int(n) => Value::int(*n),
        Constant::Float(f) => Value::float(*f),
        Constant::Bool(b) => Value::bool(*b),
        Constant::Nil => Value::nil(),
        Constant::Unit => Value::unit(),
        Constant::String(_) => Value::nil(), // strings resolved via string pool
        Constant::StringId(idx) => Value::string(*idx),
    }
}
