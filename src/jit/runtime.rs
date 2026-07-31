//! Runtime helper functions callable from JIT-compiled code.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

use crate::value_layout::{
    is_float_raw, sext48, tag_int, PAYLOAD_MASK, TAG_INT, TAG_MASK, TAG_PTR,
};
use crate::vm::Value;

// is_float_raw is now imported from crate::value_layout (integer bitmask, no FPU).

#[no_mangle]
pub extern "C" fn nulang_iadd(a: u64, b: u64) -> u64 {
    if is_float_raw(a) && is_float_raw(b) {
        Value::float(f64::from_bits(a) + f64::from_bits(b)).as_raw()
    } else {
        tag_int(sext48(a & PAYLOAD_MASK) + sext48(b & PAYLOAD_MASK))
    }
}

#[no_mangle]
pub extern "C" fn nulang_isub(a: u64, b: u64) -> u64 {
    if is_float_raw(a) && is_float_raw(b) {
        Value::float(f64::from_bits(a) - f64::from_bits(b)).as_raw()
    } else {
        tag_int(sext48(a & PAYLOAD_MASK) - sext48(b & PAYLOAD_MASK))
    }
}

#[no_mangle]
pub extern "C" fn nulang_imul(a: u64, b: u64) -> u64 {
    if is_float_raw(a) && is_float_raw(b) {
        Value::float(f64::from_bits(a) * f64::from_bits(b)).as_raw()
    } else {
        tag_int(sext48(a & PAYLOAD_MASK).wrapping_mul(sext48(b & PAYLOAD_MASK)))
    }
}

#[no_mangle]
pub extern "C" fn nulang_idiv(a: u64, b: u64) -> u64 {
    if is_float_raw(a) && is_float_raw(b) {
        let bv = f64::from_bits(b);
        if bv == 0.0 {
            return Value::nil().as_raw();
        }
        return Value::float(f64::from_bits(a) / bv).as_raw();
    }
    let bv = sext48(b & PAYLOAD_MASK);
    if bv == 0 {
        return Value::nil().as_raw();
    }
    tag_int(sext48(a & PAYLOAD_MASK) / bv)
}

#[no_mangle]
pub extern "C" fn nulang_imod(a: u64, b: u64) -> u64 {
    if is_float_raw(a) && is_float_raw(b) {
        let bv = f64::from_bits(b);
        if bv == 0.0 {
            return Value::nil().as_raw();
        }
        return Value::float(f64::from_bits(a) % bv).as_raw();
    }
    let bv = sext48(b & PAYLOAD_MASK);
    if bv == 0 {
        return Value::nil().as_raw();
    }
    tag_int(sext48(a & PAYLOAD_MASK) % bv)
}

/// Extract the integer payload like the interpreter's `as_int().unwrap_or(0)`:
/// non-int-tagged values contribute 0.
fn as_int_or_zero(v: u64) -> i64 {
    if (v & TAG_MASK) == TAG_INT {
        sext48(v & PAYLOAD_MASK)
    } else {
        0
    }
}

/// Extract the raw payload pointer from a NaN-boxed value, or null.
fn val_ptr(v: u64) -> *mut u8 {
    if (v & TAG_MASK) == TAG_PTR {
        (v & PAYLOAD_MASK) as *mut u8
    } else {
        std::ptr::null_mut()
    }
}

#[no_mangle]
pub extern "C" fn nulang_xor(a: u64, b: u64) -> u64 {
    tag_int(as_int_or_zero(a) ^ as_int_or_zero(b))
}

#[no_mangle]
pub extern "C" fn nulang_shl(a: u64, b: u64) -> u64 {
    let shift = (as_int_or_zero(b) as u64) & 0x3f;
    tag_int(as_int_or_zero(a) << shift)
}

#[no_mangle]
pub extern "C" fn nulang_shr(a: u64, b: u64) -> u64 {
    let shift = (as_int_or_zero(b) as u64) & 0x3f;
    tag_int(as_int_or_zero(a) >> shift)
}

#[no_mangle]
pub extern "C" fn nulang_bitand(a: u64, b: u64) -> u64 {
    tag_int(as_int_or_zero(a) & as_int_or_zero(b))
}

#[no_mangle]
pub extern "C" fn nulang_bitor(a: u64, b: u64) -> u64 {
    tag_int(as_int_or_zero(a) | as_int_or_zero(b))
}

#[no_mangle]
pub extern "C" fn nulang_ineg(a: u64) -> u64 {
    if is_float_raw(a) {
        Value::float(-f64::from_bits(a)).as_raw()
    } else {
        tag_int(-sext48(a & PAYLOAD_MASK))
    }
}

#[no_mangle]
pub extern "C" fn nulang_iinc(a: u64) -> u64 {
    tag_int(sext48(a & PAYLOAD_MASK) + 1)
}

#[no_mangle]
pub extern "C" fn nulang_idec(a: u64) -> u64 {
    tag_int(sext48(a & PAYLOAD_MASK) - 1)
}

#[no_mangle]
pub extern "C" fn nulang_icmp_eq(a: u64, b: u64) -> u64 {
    if is_float_raw(a) && is_float_raw(b) {
        Value::bool((f64::from_bits(a) - f64::from_bits(b)).abs() < f64::EPSILON).as_raw()
    } else if (a & TAG_MASK) == TAG_INT && (b & TAG_MASK) == TAG_INT {
        Value::bool(sext48(a & PAYLOAD_MASK) == sext48(b & PAYLOAD_MASK)).as_raw()
    } else if is_float_raw(a) && (b & TAG_MASK) == TAG_INT {
        let bf = sext48(b & PAYLOAD_MASK) as f64;
        Value::bool((f64::from_bits(a) - bf).abs() < f64::EPSILON).as_raw()
    } else if (a & TAG_MASK) == TAG_INT && is_float_raw(b) {
        let af = sext48(a & PAYLOAD_MASK) as f64;
        Value::bool((af - f64::from_bits(b)).abs() < f64::EPSILON).as_raw()
    } else {
        Value::bool(a == b).as_raw()
    }
}

#[no_mangle]
pub extern "C" fn nulang_icmp_lt(a: u64, b: u64) -> u64 {
    if is_float_raw(a) && is_float_raw(b) {
        Value::bool(f64::from_bits(a) < f64::from_bits(b)).as_raw()
    } else if (a & TAG_MASK) == TAG_INT && (b & TAG_MASK) == TAG_INT {
        Value::bool(sext48(a & PAYLOAD_MASK) < sext48(b & PAYLOAD_MASK)).as_raw()
    } else if is_float_raw(a) && (b & TAG_MASK) == TAG_INT {
        Value::bool(f64::from_bits(a) < sext48(b & PAYLOAD_MASK) as f64).as_raw()
    } else if (a & TAG_MASK) == TAG_INT && is_float_raw(b) {
        Value::bool((sext48(a & PAYLOAD_MASK) as f64) < f64::from_bits(b)).as_raw()
    } else {
        Value::bool(a < b).as_raw()
    }
}

#[no_mangle]
pub extern "C" fn nulang_icmp_gt(a: u64, b: u64) -> u64 {
    if is_float_raw(a) && is_float_raw(b) {
        Value::bool(f64::from_bits(a) > f64::from_bits(b)).as_raw()
    } else if (a & TAG_MASK) == TAG_INT && (b & TAG_MASK) == TAG_INT {
        Value::bool(sext48(a & PAYLOAD_MASK) > sext48(b & PAYLOAD_MASK)).as_raw()
    } else if is_float_raw(a) && (b & TAG_MASK) == TAG_INT {
        Value::bool(f64::from_bits(a) > sext48(b & PAYLOAD_MASK) as f64).as_raw()
    } else if (a & TAG_MASK) == TAG_INT && is_float_raw(b) {
        Value::bool((sext48(a & PAYLOAD_MASK) as f64) > f64::from_bits(b)).as_raw()
    } else {
        Value::bool(a > b).as_raw()
    }
}

#[no_mangle]
pub extern "C" fn nulang_icmp_le(a: u64, b: u64) -> u64 {
    if is_float_raw(a) && is_float_raw(b) {
        Value::bool(f64::from_bits(a) <= f64::from_bits(b)).as_raw()
    } else if (a & TAG_MASK) == TAG_INT && (b & TAG_MASK) == TAG_INT {
        Value::bool(sext48(a & PAYLOAD_MASK) <= sext48(b & PAYLOAD_MASK)).as_raw()
    } else if is_float_raw(a) && (b & TAG_MASK) == TAG_INT {
        Value::bool(f64::from_bits(a) <= sext48(b & PAYLOAD_MASK) as f64).as_raw()
    } else if (a & TAG_MASK) == TAG_INT && is_float_raw(b) {
        Value::bool((sext48(a & PAYLOAD_MASK) as f64) <= f64::from_bits(b)).as_raw()
    } else {
        Value::bool(a <= b).as_raw()
    }
}

#[no_mangle]
pub extern "C" fn nulang_icmp_ge(a: u64, b: u64) -> u64 {
    if is_float_raw(a) && is_float_raw(b) {
        Value::bool(f64::from_bits(a) >= f64::from_bits(b)).as_raw()
    } else if (a & TAG_MASK) == TAG_INT && (b & TAG_MASK) == TAG_INT {
        Value::bool(sext48(a & PAYLOAD_MASK) >= sext48(b & PAYLOAD_MASK)).as_raw()
    } else if is_float_raw(a) && (b & TAG_MASK) == TAG_INT {
        Value::bool(f64::from_bits(a) >= sext48(b & PAYLOAD_MASK) as f64).as_raw()
    } else if (a & TAG_MASK) == TAG_INT && is_float_raw(b) {
        Value::bool((sext48(a & PAYLOAD_MASK) as f64) >= f64::from_bits(b)).as_raw()
    } else {
        Value::bool(a >= b).as_raw()
    }
}

#[no_mangle]
pub extern "C" fn nulang_fadd(a: u64, b: u64) -> u64 {
    Value::float(f64::from_bits(a) + f64::from_bits(b)).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_fsub(a: u64, b: u64) -> u64 {
    Value::float(f64::from_bits(a) - f64::from_bits(b)).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_fmul(a: u64, b: u64) -> u64 {
    Value::float(f64::from_bits(a) * f64::from_bits(b)).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_fdiv(a: u64, b: u64) -> u64 {
    let bv = f64::from_bits(b);
    if bv == 0.0 {
        return Value::nil().as_raw();
    }
    Value::float(f64::from_bits(a) / bv).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_fcmp_eq(a: u64, b: u64) -> u64 {
    Value::bool((f64::from_bits(a) - f64::from_bits(b)).abs() < f64::EPSILON).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_fcmp_lt(a: u64, b: u64) -> u64 {
    Value::bool(f64::from_bits(a) < f64::from_bits(b)).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_fcmp_gt(a: u64, b: u64) -> u64 {
    Value::bool(f64::from_bits(a) > f64::from_bits(b)).as_raw()
}

fn is_truthy(v: u64) -> bool {
    v != Value::nil().as_raw() && v != Value::bool(false).as_raw() && v != Value::int(0).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_not(a: u64) -> u64 {
    Value::bool(is_truthy(a) == false).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_and(a: u64, b: u64) -> u64 {
    Value::bool(is_truthy(a) && is_truthy(b)).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_or(a: u64, b: u64) -> u64 {
    Value::bool(is_truthy(a) || is_truthy(b)).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_itof(a: u64) -> u64 {
    Value::float(sext48(a & PAYLOAD_MASK) as f64).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_ftoi(a: u64) -> u64 {
    Value::int(f64::from_bits(a) as i64).as_raw()
}

/// Float negate, matching the interpreter's `as_float().unwrap_or(0.0)`:
/// any NaN bit pattern (i.e. any tagged value) negates to -0.0.
#[no_mangle]
pub extern "C" fn nulang_fneg(a: u64) -> u64 {
    let f = f64::from_bits(a);
    let v = if f.is_nan() { 0.0 } else { f };
    Value::float(-v).as_raw()
}

// -----------------------------------------------------------------------
// Actor callback thread-local for JIT runtime helpers
// -----------------------------------------------------------------------

/// Raw pair representing a `*mut dyn ActorVmCallbacks` fat pointer.
/// Stored as two usize values to avoid zero-initialization UB.
#[derive(Clone, Copy)]
struct CbPair(usize, usize);

impl CbPair {
    const NULL: Self = CbPair(0, 0);

    /// # Safety
    /// Transmutes `*mut dyn ActorVmCallbacks` (a fat pointer: data ptr +
    /// vtable ptr) to `(usize, usize)`. Relies on the de-facto fat pointer
    /// layout used by all Tier-1 Rust targets (x86_64, aarch64).
    fn from_ptr(ptr: *mut dyn crate::vm::ActorVmCallbacks) -> Self {
        unsafe { std::mem::transmute(ptr) }
    }

    /// # Safety
    /// Reconstructs the fat pointer. The caller must ensure the original
    /// `&mut dyn ActorVmCallbacks` is alive and `&mut` provenance restored.
    fn to_ptr(self) -> *mut dyn crate::vm::ActorVmCallbacks {
        unsafe { std::mem::transmute(self) }
    }

    fn is_null(self) -> bool {
        self.0 == 0 && self.1 == 0
    }
}

thread_local! {
    static JIT_CALLBACKS: UnsafeCell<CbPair> = UnsafeCell::new(CbPair::NULL);
}

pub unsafe fn set_jit_callbacks(cb: *mut dyn crate::vm::ActorVmCallbacks) {
    JIT_CALLBACKS.with(|cell| {
        *cell.get() = CbPair::from_ptr(cb);
    });
}

pub fn clear_jit_callbacks() {
    JIT_CALLBACKS.with(|cell| unsafe {
        *cell.get() = CbPair::NULL;
    });
}

// ---------------------------------------------------------------------------
// JIT Safepoint: reduction-count preemption for long-running JIT regions
// ---------------------------------------------------------------------------

/// How many JIT region entries a behavior may execute before yielding
/// back to the scheduler. Reset at each behavior invocation.
pub const JIT_SAFEPOINT_BUDGET: u64 = 1000;

/// Dummy counter that never reaches 0 — acts as fallback when no runtime
/// actor is executing JIT code. `i64::MAX` is used because the safepoint
/// check is a signed `≤ 0` comparison.
static JIT_SAFEPOINT_DUMMY: AtomicU64 = AtomicU64::new(i64::MAX as u64);

/// Process-global pointer to the current actor's `jit_safepoint_counter`.
/// Set/cleared by the runtime. `AtomicPtr` so JIT code can load it via a
/// single embedded address constant without indirection through a
/// thread-local.
pub static JIT_SAFEPOINT_PTR: AtomicPtr<u64> =
    AtomicPtr::new(&JIT_SAFEPOINT_DUMMY as *const AtomicU64 as *mut u64);

pub fn set_jit_safepoint_ptr(ptr: *mut u64) {
    JIT_SAFEPOINT_PTR.store(ptr, Ordering::Release);
}

pub fn clear_jit_safepoint_ptr() {
    JIT_SAFEPOINT_PTR.store(
        &JIT_SAFEPOINT_DUMMY as *const AtomicU64 as *mut u64,
        Ordering::Release,
    );
}

/// Bytecode offset where the JIT yielded, or `u64::MAX` if no yield is
/// pending. Set by JIT-compiled code (inline store), consumed by
/// `try_jit_execute`. Single-scheduler-thread invariant: no CAS needed.
pub static JIT_YIELD_PC: AtomicU64 = AtomicU64::new(u64::MAX);

pub fn take_jit_yield_pc() -> Option<usize> {
    let old = JIT_YIELD_PC.swap(u64::MAX, Ordering::AcqRel);
    if old == u64::MAX {
        None
    } else {
        Some(old as usize)
    }
}

/// Called from JIT-compiled code when the safepoint budget is exhausted.
///
/// Stores the bytecode offset where execution should resume (relative to
/// region start) and returns 1 (must yield).
///
/// # Safety
/// Called only from JIT-compiled code on the scheduler thread.
#[no_mangle]
pub unsafe extern "C" fn nulang_safepoint_yield(resume_offset: u64) -> u64 {
    JIT_YIELD_PC.store(resume_offset, Ordering::Release);
    1 // must yield
}

unsafe fn with_callbacks<R>(f: impl FnOnce(&mut dyn crate::vm::ActorVmCallbacks) -> R) -> R {
    JIT_CALLBACKS.with(|cell| {
        let pair = *cell.get();
        assert!(!pair.is_null(), "JIT_CALLBACKS not set");
        f(&mut *pair.to_ptr())
    })
}

use crate::runtime::heap::{ActorHeap, TypeTag as HeapTypeTag};

/// # Safety
/// `regs` must point to a valid `[u64; 256]` array. Called only from
/// JIT-compiled code that follows the `regs_ptr` ABI contract.
#[no_mangle]
pub unsafe extern "C" fn nulang_arr_store(
    regs: *mut u64,
    arr_reg: u32,
    idx_reg: u32,
    src_reg: u32,
) {
    let arr_ptr_val = *regs.add(arr_reg as usize);
    let idx_val = *regs.add(idx_reg as usize);
    let val = Value::from_raw(*regs.add(src_reg as usize));
    let arr_ptr = val_ptr(arr_ptr_val);
    if arr_ptr.is_null() {
        return;
    }
    let idx = as_int_or_zero(idx_val) as usize;
    with_callbacks(|cb| {
        if let Some(len) = cb.array_len(arr_ptr) {
            if idx < len {
                if let Some(ptr) = val.as_ptr() {
                    cb.retain_ref(ptr);
                }
                let slot = (arr_ptr as *mut Value).add(idx);
                let old = *slot;
                *slot = val;
                if let Some(old_ptr) = old.as_ptr() {
                    cb.drop_ref(old_ptr);
                }
            }
        }
    });
}

/// # Safety
/// `regs` must point to a valid `[u64; 256]` array.
#[no_mangle]
pub unsafe extern "C" fn nulang_arr_len(regs: *mut u64, arr_reg: u32, dst_reg: u32) {
    let arr_ptr_val = *regs.add(arr_reg as usize);
    let arr_ptr = val_ptr(arr_ptr_val);
    let len = if !arr_ptr.is_null() {
        let header = &*ActorHeap::header_of(arr_ptr);
        if header.type_tag == HeapTypeTag::Array {
            header.size.saturating_sub(ActorHeap::HEADER_SIZE) / std::mem::size_of::<Value>()
        } else {
            0
        }
    } else {
        0
    };
    *regs.add(dst_reg as usize) = tag_int(len as i64);
}

/// # Safety
/// `regs` must point to a valid `[u64; 256]` array.
#[no_mangle]
pub unsafe extern "C" fn nulang_field_load(regs: *mut u64, obj_reg: u32, idx: u32, dst_reg: u32) {
    let obj_ptr_val = *regs.add(obj_reg as usize);
    let obj_ptr = val_ptr(obj_ptr_val);
    let val = if !obj_ptr.is_null() {
        let header = &*ActorHeap::header_of(obj_ptr);
        if header.type_tag == HeapTypeTag::Tuple {
            let payload_size = header.size.saturating_sub(ActorHeap::HEADER_SIZE);
            let len = payload_size / std::mem::size_of::<Value>();
            if (idx as usize) < len {
                *((obj_ptr as *const Value).add(idx as usize))
            } else {
                Value::nil()
            }
        } else {
            Value::nil()
        }
    } else {
        Value::nil()
    };
    *regs.add(dst_reg as usize) = val.as_raw();
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_jit_helpers_linked() {
        // Force the linker to retain the JIT runtime helpers by taking
        // their addresses. Without this, the linker may strip them since
        let _ = super::nulang_arr_store as unsafe extern "C" fn(_, _, _, _);
        let _ = super::nulang_arr_len as unsafe extern "C" fn(_, _, _);
        let _ = super::nulang_field_load as unsafe extern "C" fn(_, _, _, _);
        let _ = super::nulang_safepoint_yield as unsafe extern "C" fn(u64) -> u64;
    }
}
