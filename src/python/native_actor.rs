// This file contains the NativeActorPool implementation for running
// Python code within Nulang actors.

use crate::vm::Value;
use std::sync::Mutex;

/// Pool of Python interpreters, one per thread.
pub struct NativeActorPool {
    size: usize,
    interpreters: Mutex<Vec<()>>, // Placeholder for Python interpreters
}

impl NativeActorPool {
    /// Create a new pool with the given number of interpreters.
    pub fn new(size: usize) -> Self {
        NativeActorPool {
            size,
            interpreters: Mutex::new(Vec::new()),
        }
    }

    /// Evaluate a Python expression and return the result as a Value.
    pub fn eval(&self, _expr: &str) -> Result<Value, String> {
        // Python support removed in v0.14 — return nil for compatibility
        Ok(Value::nil())
    }

    /// Ensure Python is available (no-op when Python support is disabled).
    pub fn ensure_python() {}
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ensure_python() {
        NativeActorPool::ensure_python();
    }

    fn is_safe_to_marshal(val: &Value) -> bool {
        // Primitive scalar values are safe; heap references and opaque
        // handles are not.
        val.is_int() || val.is_bool() || val.is_float() || val.is_nil() || val.is_unit()
    }

    #[test]
    fn test_pool_creation() {
        let pool = NativeActorPool::new(1);
        assert_eq!(pool.size, 1);
    }

    #[test]
    fn test_marshal_safety() {
        // Test that Value types are safe to marshal
        let int_val = Value::int(42);
        assert!(is_safe_to_marshal(&int_val));

        let bool_val = Value::bool(true);
        assert!(is_safe_to_marshal(&bool_val));

        let unit_val = Value::unit();
        assert!(is_safe_to_marshal(&unit_val));

        let nil_val = Value::nil();
        assert!(is_safe_to_marshal(&nil_val));

        // Test that pointer values are not safe to marshal
        let ptr_val = Value::ptr(std::ptr::null_mut::<u8>());
        assert!(!is_safe_to_marshal(&ptr_val));

        // Test that actor references are not safe to marshal
        let actor_val = Value::actor_ref(1);
        assert!(!is_safe_to_marshal(&actor_val));

        // Test that closure references are not safe to marshal
        let closure_val = Value::closure(0);
        assert!(!is_safe_to_marshal(&closure_val));
    }

    #[test]
    fn test_eval_returns_nil() {
        // Python eval returns nil when Python support is disabled
        let pool = NativeActorPool::new(1);
        let result = pool.eval("True and False");
        assert!(result.is_ok());
        assert!(result.unwrap().is_nil());
    }
}
