//! Runtime helper functions callable from JIT-compiled code.

use crate::bytecode::Constant;
use crate::value_layout::{
    is_float_raw, sext48, tag_int, PAYLOAD_MASK, TAG_INT, TAG_MASK, TAG_PTR, TAG_STRING,
};
use crate::vm::Value;
use std::cell::UnsafeCell;
use std::ffi::CStr;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

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
    } else if (a & TAG_MASK) == TAG_STRING
        || (a & TAG_MASK) == TAG_PTR
        || (b & TAG_MASK) == TAG_STRING
        || (b & TAG_MASK) == TAG_PTR
    {
        // String equality must compare content, not raw bits.
        // Only when BOTH resolve to strings do we compare text.
        let eq = match (resolve_jit_string(a), resolve_jit_string(b)) {
            (Some(sa), Some(sb)) => sa == sb,
            _ => false,
        };
        Value::bool(eq).as_raw()
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
// Constant-pool thread-local for JIT runtime helpers (string comparison)
// ---------------------------------------------------------------------------

/// Pointer-length pair for the current module's constant pool, stored as
/// two usize values to avoid zero-initialization UB in the thread-local.
#[derive(Clone, Copy)]
struct ConstantsPtr(*const Constant, usize);

impl ConstantsPtr {
    const NULL: Self = ConstantsPtr(std::ptr::null(), 0);

    /// # Safety
    /// The slice must be valid for the duration of the JIT execution.
    unsafe fn as_slice(self) -> &'static [Constant] {
        if self.0.is_null() {
            &[]
        } else {
            std::slice::from_raw_parts(self.0, self.1)
        }
    }
}

thread_local! {
    static JIT_CONSTANTS: UnsafeCell<ConstantsPtr> = UnsafeCell::new(ConstantsPtr::NULL);
}

/// Set the current module's constant pool for JIT runtime helpers.
///
/// # Safety
/// The slice must remain valid until `clear_jit_constants` is called.
pub unsafe fn set_jit_constants(constants: &[Constant]) {
    JIT_CONSTANTS.with(|cell| {
        *cell.get() = ConstantsPtr(constants.as_ptr(), constants.len());
    });
}

pub fn clear_jit_constants() {
    JIT_CONSTANTS.with(|cell| unsafe {
        *cell.get() = ConstantsPtr::NULL;
    });
}

/// Resolve a raw u64 value to its string content (for comparison).
/// Returns None for non-string values or when the constant pool is unavailable.
fn resolve_jit_string(raw: u64) -> Option<String> {
    if (raw & TAG_MASK) == TAG_STRING {
        // Interned string: look up in the thread-local constant pool.
        let id = (raw & PAYLOAD_MASK) as u32;
        JIT_CONSTANTS.with(|cell| unsafe {
            let cp = (*cell.get()).as_slice();
            match cp.get(id as usize) {
                Some(Constant::String(s)) => Some(s.clone()),
                _ => None,
            }
        })
    } else if (raw & TAG_MASK) == TAG_PTR {
        let ptr = (raw & PAYLOAD_MASK) as *mut u8;
        if ptr.is_null() {
            return None;
        }
        // SAFETY: ptr is a valid ActorHeap allocation with a header.
        // We check the type tag to ensure it's a string.
        unsafe {
            let header = &*ActorHeap::header_of(ptr);
            if header.type_tag != HeapTypeTag::String {
                return None;
            }
            Some(
                CStr::from_ptr(ptr as *const std::ffi::c_char)
                    .to_string_lossy()
                    .into_owned(),
            )
        }
    } else {
        None
    }
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

// ---------------------------------------------------------------------------
// AOT standalone execution context
// ---------------------------------------------------------------------------

thread_local! {
    /// Standalone heap for AOT execution when no actor runtime is active.
    static AOT_HEAP: std::cell::RefCell<Option<crate::runtime::heap::ActorHeap>> =
        std::cell::RefCell::new(None);
    /// Standalone constant pool for AOT execution.
    static AOT_CONSTANTS: std::cell::RefCell<Option<Vec<crate::bytecode::Constant>>> =
        std::cell::RefCell::new(None);
}

/// Set up a standalone heap for AOT execution.
pub fn aot_set_heap(heap: crate::runtime::heap::ActorHeap) {
    AOT_HEAP.with(|cell| {
        *cell.borrow_mut() = Some(heap);
    });
}

/// Take the standalone heap, returning it to the caller.
pub fn aot_take_heap() -> Option<crate::runtime::heap::ActorHeap> {
    AOT_HEAP.with(|cell| cell.borrow_mut().take())
}

/// Set standalone constants for AOT execution.
///
/// # Safety
/// The slice must remain valid until `aot_clear_constants` is called.
pub unsafe fn aot_set_constants(constants: &[crate::bytecode::Constant]) {
    AOT_CONSTANTS.with(|cell| {
        *cell.borrow_mut() = Some(constants.to_vec());
    });
}

/// Clear standalone constants.
pub fn aot_clear_constants() {
    AOT_CONSTANTS.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

/// Allocate via callbacks or fall back to standalone AOT heap.
/// Check if JIT callbacks are set, and if so, use them.
pub(crate) unsafe fn try_with_callbacks<R>(
    f: impl FnOnce(&mut dyn crate::vm::ActorVmCallbacks) -> R,
) -> Option<R> {
    JIT_CALLBACKS.with(|cell| {
        let pair = *cell.get();
        if pair.is_null() {
            None
        } else {
            Some(f(&mut *pair.to_ptr()))
        }
    })
}

/// Allocate via callbacks or fall back to standalone AOT heap.
unsafe fn alloc_obj(size: usize, type_tag: HeapTypeTag) -> Option<*mut u8> {
    if let Some(ptr) = try_with_callbacks(|cb| cb.alloc(size, type_tag)) {
        return ptr;
    }
    AOT_HEAP.with(|cell| {
        cell.borrow_mut()
            .as_mut()
            .and_then(|heap| heap.alloc(size, type_tag))
    })
}

/// Retain a reference via callbacks or AOT heap directly.
unsafe fn retain_obj(ptr: *mut u8) {
    if try_with_callbacks(|cb| {
        cb.retain_ref(ptr);
        true
    })
    .is_some()
    {
        return;
    }
    if !ptr.is_null() {
        let header = &mut *ActorHeap::header_of(ptr);
        header.ref_count += 1;
    }
}

/// Drop a reference via callbacks or AOT heap directly.
unsafe fn drop_obj(ptr: *mut u8) {
    if try_with_callbacks(|cb| {
        cb.drop_ref(ptr);
        true
    })
    .is_some()
    {
        return;
    }
    if !ptr.is_null() {
        let header = &mut *ActorHeap::header_of(ptr);
        if header.ref_count > 0 {
            header.ref_count -= 1;
        }
        if header.ref_count == 0 {
            AOT_HEAP.with(|cell| {
                if let Some(ref mut heap) = *cell.borrow_mut() {
                    heap.free(ptr);
                }
            });
        }
    }
}

// ---------------------------------------------------------------------------
// AOT value-based runtime helpers
// ---------------------------------------------------------------------------

/// Allocate a heap object with `slot_count` slots of type `type_tag`.
/// Returns tagged pointer or nil.
#[no_mangle]
pub unsafe extern "C" fn nulang_alloc_obj(slot_count: u64, type_tag_raw: u32) -> u64 {
    let count = slot_count as usize;
    let tag: HeapTypeTag = match type_tag_raw {
        1 => HeapTypeTag::Array,
        3 => HeapTypeTag::Record,
        6 => HeapTypeTag::Tuple,
        2 => HeapTypeTag::String,
        _ => return Value::nil().as_raw(),
    };
    let size = count.checked_mul(std::mem::size_of::<Value>()).unwrap_or(0);
    if let Some(ptr) = alloc_obj(size, tag) {
        let slots = std::slice::from_raw_parts_mut(ptr as *mut Value, count);
        for slot in slots.iter_mut() {
            *slot = Value::nil();
        }
        Value::ptr(ptr).as_raw()
    } else {
        Value::nil().as_raw()
    }
}

/// Read slot `idx` from a heap object (record, tuple, or array).
/// Returns nil if the object is not a valid heap object or idx is out of range.
#[no_mangle]
pub unsafe extern "C" fn nulang_obj_get(obj: u64, idx: u64) -> u64 {
    let obj_ptr = val_ptr(obj);
    if obj_ptr.is_null() {
        return Value::nil().as_raw();
    }
    let header = &*ActorHeap::header_of(obj_ptr);
    let payload_size = header.size.saturating_sub(ActorHeap::HEADER_SIZE);
    let len = payload_size / std::mem::size_of::<Value>();
    let i = idx as usize;
    if i < len {
        (*((obj_ptr as *const Value).add(i))).as_raw()
    } else {
        Value::nil().as_raw()
    }
}

/// Write `val` into slot `idx` of a heap object, with proper refcounting.
#[no_mangle]
pub unsafe extern "C" fn nulang_obj_set(obj: u64, idx: u64, val: u64) {
    let obj_ptr = val_ptr(obj);
    if obj_ptr.is_null() {
        return;
    }
    let val = Value::from_raw(val);
    let header = &*ActorHeap::header_of(obj_ptr);
    let payload_size = header.size.saturating_sub(ActorHeap::HEADER_SIZE);
    let len = payload_size / std::mem::size_of::<Value>();
    let i = idx as usize;
    if i < len {
        if let Some(ptr) = val.as_ptr() {
            retain_obj(ptr);
        }
        let slot = (obj_ptr as *mut Value).add(i);
        let old = *slot;
        *slot = val;
        if let Some(old_ptr) = old.as_ptr() {
            drop_obj(old_ptr);
        }
    }
}

/// Get element count of a heap object (record, tuple, or array).
/// Returns tagged int.
#[no_mangle]
pub unsafe extern "C" fn nulang_obj_len(obj: u64) -> u64 {
    let obj_ptr = val_ptr(obj);
    if obj_ptr.is_null() {
        return Value::int(0).as_raw();
    }
    let header = &*ActorHeap::header_of(obj_ptr);
    let payload_size = header.size.saturating_sub(ActorHeap::HEADER_SIZE);
    let len = payload_size / std::mem::size_of::<Value>();
    Value::int(len as i64).as_raw()
}

/// Shallow copy a record (copies all slots, retains each).
/// Returns tagged pointer or nil.
#[no_mangle]
pub unsafe extern "C" fn nulang_rec_copy(obj: u64) -> u64 {
    let src_ptr = val_ptr(obj);
    if src_ptr.is_null() {
        return Value::nil().as_raw();
    }
    let header = &*ActorHeap::header_of(src_ptr);
    if header.type_tag != HeapTypeTag::Record {
        return Value::nil().as_raw();
    }
    let payload_size = header.size.saturating_sub(ActorHeap::HEADER_SIZE);
    let slot_count = payload_size / std::mem::size_of::<Value>();
    if let Some(dst_ptr) = alloc_obj(payload_size, HeapTypeTag::Record) {
        let src_slots = std::slice::from_raw_parts(src_ptr as *const Value, slot_count);
        let dst_slots = std::slice::from_raw_parts_mut(dst_ptr as *mut Value, slot_count);
        for i in 0..slot_count {
            let val = src_slots[i];
            if let Some(ptr) = val.as_ptr() {
                retain_obj(ptr);
            }
            dst_slots[i] = val;
        }
        Value::ptr(dst_ptr).as_raw()
    } else {
        Value::nil().as_raw()
    }
}

/// String equality: compare two Nulang values as strings.
/// Returns tagged bool.
#[no_mangle]
pub unsafe extern "C" fn nulang_str_eq(a: u64, b: u64) -> u64 {
    let sa = resolve_string_coerce(a);
    let sb = resolve_string_coerce(b);
    let eq = match (sa, sb) {
        (Some(sa), Some(sb)) => sa == sb,
        _ => false,
    };
    Value::bool(eq).as_raw()
}

/// String concatenation: allocate a new heap string.
/// Returns tagged pointer or nil.
#[no_mangle]
pub fn resolve_string_coerce(raw: u64) -> Option<String> {
    let val = crate::vm::Value::from_raw(raw);
    if val.is_int() {
        return Some(val.as_int().unwrap().to_string());
    }
    if val.is_float() {
        return Some(val.as_float().unwrap().to_string());
    }
    if val.is_bool() {
        return Some(val.as_bool().unwrap().to_string());
    }
    if (raw & TAG_MASK) == TAG_STRING {
        // String constant from the module pool: content lives in the JIT or
        // AOT constant pool, keyed by the payload index.
        let id = (raw & PAYLOAD_MASK) as u32;
        let from_jit = JIT_CONSTANTS.with(|cell| unsafe {
            let cp = (*cell.get()).as_slice();
            cp.get(id as usize).and_then(|c| match c {
                crate::bytecode::Constant::String(s) => Some(s.clone()),
                _ => None,
            })
        });
        if from_jit.is_some() {
            return from_jit;
        }
        return AOT_CONSTANTS.with(|cell| {
            let guard = cell.borrow();
            if let Some(ref constants) = *guard {
                constants.get(id as usize).and_then(|c| match c {
                    crate::bytecode::Constant::String(s) => Some(s.clone()),
                    _ => None,
                })
            } else {
                None
            }
        });
    }
    if (raw & TAG_MASK) == TAG_PTR {
        let ptr = (raw & PAYLOAD_MASK) as *mut u8;
        if ptr.is_null() {
            return None;
        }
        unsafe {
            let header = &*ActorHeap::header_of(ptr);
            if header.type_tag != HeapTypeTag::String {
                return None;
            }
            return Some(
                std::ffi::CStr::from_ptr(ptr as *const std::ffi::c_char)
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    None
}

#[no_mangle]
pub unsafe extern "C" fn nulang_str_concat(a: u64, b: u64) -> u64 {
    let sa = resolve_string_coerce(a);
    let sb = resolve_string_coerce(b);
    let result = match (sa, sb) {
        (Some(sa), Some(sb)) => format!("{}{}", sa, sb),
        (Some(s), None) => s,
        (None, Some(s)) => s,
        _ => return Value::nil().as_raw(),
    };
    let bytes = result.into_bytes();
    if let Some(ptr) = alloc_obj(bytes.len() + 1, HeapTypeTag::String) {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        *ptr.add(bytes.len()) = 0;
        Value::ptr(ptr).as_raw()
    } else {
        Value::nil().as_raw()
    }
}

/// Power operation: a^b for tagged integers.
/// Returns tagged int or nil (for negative exponent / overflow).
#[no_mangle]
pub extern "C" fn nulang_pow(a: u64, b: u64) -> u64 {
    let base = as_int_or_zero(a);
    let exp = as_int_or_zero(b);
    if exp < 0 {
        return Value::nil().as_raw();
    }
    // 0^0 = 1 in Nulang (matching Rust's checked_pow and math convention for discrete exponentiation)
    let result = base
        .checked_pow(exp as u32)
        .map(|r| Value::int(r).as_raw())
        .unwrap_or_else(|| Value::nil().as_raw());
    result
}

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

// ---------------------------------------------------------------------------
// AOT actor runtime helpers
// ---------------------------------------------------------------------------
// Called from AOT-compiled code when the function body contains actor
// operations (SelfRef, StateGet, StateSet).  They go through the same
// `ActorVmCallbacks` trait the VM uses, stored in the JIT_CALLBACKS
// thread-local (set before each AOT invocation).  Outside an actor
// context (`try_with_callbacks` returns None) they degrade gracefully:
// SelfRef/StateGet return nil, StateSet is a no-op.

/// Return the current actor's ID as a tagged i64, or nil outside an actor.
#[no_mangle]
pub unsafe extern "C" fn nulang_aot_self_ref() -> u64 {
    try_with_callbacks(|cb| match cb.current_actor_id() {
        Some(id) => Value::int(id as i64).as_raw(),
        None => Value::nil().as_raw(),
    })
    .unwrap_or_else(|| Value::nil().as_raw())
}

/// Read a field from the current actor's durable state.
///
/// `field_name_raw` is a TAG_STRING constant resolved via
/// `resolve_string_coerce`. Returns nil when no actor is active or the
/// field is absent.
#[no_mangle]
pub unsafe extern "C" fn nulang_aot_state_get(field_name_raw: u64) -> u64 {
    let field = resolve_string_coerce(field_name_raw).unwrap_or_default();
    try_with_callbacks(|cb| cb.get_state_field(&field).as_raw())
        .unwrap_or_else(|| Value::nil().as_raw())
}

/// Write a field on the current actor's durable state.
///
/// `field_name_raw` is a TAG_STRING constant; `value` is the new
/// NaN-tagged value to store. No-op outside an actor.
#[no_mangle]
pub unsafe extern "C" fn nulang_aot_state_set(field_name_raw: u64, value: u64) {
    let field = resolve_string_coerce(field_name_raw).unwrap_or_default();
    try_with_callbacks(|cb| cb.set_state_field(&field, Value::from_bits(value)));
}

// ---------------------------------------------------------------------------
// AOT fire-and-forget message send
// ---------------------------------------------------------------------------
// `send actor behavior(args...)` in an AOT-compiled behavior lowers to a call
// to one of these arity-matched helpers (0..8 payload args). The helper packs
// the boxed args and routes through the current callbacks' `send_message`,
// which delivers to the target actor's mailbox (scheduler path) or a
// registered standalone actor (AOT dispatch path). Outside an actor context
// it is a no-op, matching the bytecode VM's outside-an-actor contract.

macro_rules! define_aot_send {
    ($name:ident, $($arg:ident),*) => {
        /// Send a fire-and-forget actor message from AOT-compiled code.
        #[no_mangle]
        pub unsafe extern "C" fn $name(target_raw: u64, behavior_raw: u64 $(, $arg: u64)*) {
            let args = [$(Value::from_bits($arg)),*];
            let _ = try_with_callbacks(|cb| {
                cb.send_message(Value::from_bits(target_raw), behavior_raw as u16, &args);
                true
            });
        }
    };
}

define_aot_send!(nulang_aot_send_0,);
define_aot_send!(nulang_aot_send_1, a0);
define_aot_send!(nulang_aot_send_2, a0, a1);
define_aot_send!(nulang_aot_send_3, a0, a1, a2);
define_aot_send!(nulang_aot_send_4, a0, a1, a2, a3);
define_aot_send!(nulang_aot_send_5, a0, a1, a2, a3, a4);
define_aot_send!(nulang_aot_send_6, a0, a1, a2, a3, a4, a5);
define_aot_send!(nulang_aot_send_7, a0, a1, a2, a3, a4, a5, a6);
define_aot_send!(nulang_aot_send_8, a0, a1, a2, a3, a4, a5, a6, a7);

// ---------------------------------------------------------------------------
// AOT event emission
// ---------------------------------------------------------------------------
// `emit Event(args)` in an AOT-compiled behavior lowers to an arity-matched
// `nulang_aot_emit_N` call. The helper resolves the event name (a TAG_STRING
// constant from the module pool), packs the boxed args, and routes through the
// current callbacks' `emit_event`, which records the event on the target actor
// (`actor.event_log`) exactly as the bytecode `Emit` opcode does. Outside an
// actor context it is a no-op.

macro_rules! define_aot_emit {
    ($name:ident, $($arg:ident),*) => {
        /// Emit an event from AOT-compiled code.
        #[no_mangle]
        pub unsafe extern "C" fn $name(event_raw: u64 $(, $arg: u64)*) {
            let event = resolve_string_coerce(event_raw).unwrap_or_default();
            let args = [$(Value::from_bits($arg)),*];
            let _ = try_with_callbacks(|cb| {
                cb.emit_event(&event, &args);
                true
            });
        }
    };
}

define_aot_emit!(nulang_aot_emit_0,);
define_aot_emit!(nulang_aot_emit_1, a0);
define_aot_emit!(nulang_aot_emit_2, a0, a1);
define_aot_emit!(nulang_aot_emit_3, a0, a1, a2);
define_aot_emit!(nulang_aot_emit_4, a0, a1, a2, a3);
define_aot_emit!(nulang_aot_emit_5, a0, a1, a2, a3, a4);
define_aot_emit!(nulang_aot_emit_6, a0, a1, a2, a3, a4, a5);
define_aot_emit!(nulang_aot_emit_7, a0, a1, a2, a3, a4, a5, a6);
define_aot_emit!(nulang_aot_emit_8, a0, a1, a2, a3, a4, a5, a6, a7);

// ---------------------------------------------------------------------------
// AOT selective receive
// ---------------------------------------------------------------------------
// `receive { | Behavior(params) => ... }` in an AOT-compiled behavior lowers
// to an arity-matched `nulang_aot_receive_match_N` call with the candidate
// behavior ids as raw u64s. The helper scans the current actor's mailbox (via
// the callbacks' `try_receive_match`), returns the matched arm index as a
// boxed Int — or the arm count when nothing matched, mirroring the VM's
// ReceiveMatch contract — and stashes the payload for
// `nulang_aot_receive_payload`, which the codegen calls once per parameter
// slot. Timed receive (`after ms`) behaves as untimed in AOT: no suspension.

thread_local! {
    /// Payload of the most recent AOT receive match, read by
    /// `nulang_aot_receive_payload`.
    static AOT_RECEIVE_PAYLOAD: std::cell::RefCell<Vec<Value>> =
        std::cell::RefCell::new(Vec::new());
}

thread_local! {
    /// Pending `(name_const_idx, value)` init pairs for the next AOT spawn,
    /// pushed by `nulang_aot_spawn_push` and drained by `nulang_aot_spawn`.
    static AOT_SPAWN_INIT: std::cell::RefCell<Vec<(u64, Value)>> =
        std::cell::RefCell::new(Vec::new());
}

/// Queue one `(name, value)` init pair for the next `nulang_aot_spawn`.
/// `name_idx` is the position of the field name in the module constant pool.
#[no_mangle]
pub unsafe extern "C" fn nulang_aot_spawn_push(name_idx: u64, value: u64) {
    AOT_SPAWN_INIT.with(|c| {
        c.borrow_mut().push((name_idx, Value::from_bits(value)));
    });
}

/// Drain the queued spawn init pairs (used by `aot::nulang_aot_spawn`).
pub fn take_aot_spawn_init() -> Vec<(u64, Value)> {
    AOT_SPAWN_INIT.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

macro_rules! define_aot_receive {
    ($name:ident, $($id:ident),*) => {
        /// Selective receive from AOT-compiled code.
        #[no_mangle]
        pub unsafe extern "C" fn $name($($id: u64),*) -> u64 {
            let ids: Vec<u16> = vec![$($id as u16),*];
            match try_with_callbacks(|cb| cb.try_receive_match(&ids)).flatten() {
                Some((idx, payload)) => {
                    AOT_RECEIVE_PAYLOAD.with(|c| *c.borrow_mut() = payload);
                    Value::int(idx as i64).as_raw()
                }
                None => Value::int(ids.len() as i64).as_raw(),
            }
        }
    };
}

define_aot_receive!(nulang_aot_receive_match_1, id0);
define_aot_receive!(nulang_aot_receive_match_2, id0, id1);
define_aot_receive!(nulang_aot_receive_match_3, id0, id1, id2);
define_aot_receive!(nulang_aot_receive_match_4, id0, id1, id2, id3);
define_aot_receive!(nulang_aot_receive_match_5, id0, id1, id2, id3, id4);
define_aot_receive!(nulang_aot_receive_match_6, id0, id1, id2, id3, id4, id5);
define_aot_receive!(nulang_aot_receive_match_7, id0, id1, id2, id3, id4, id5, id6);
define_aot_receive!(nulang_aot_receive_match_8, id0, id1, id2, id3, id4, id5, id6, id7);

/// Read the i-th payload value of the most recent AOT receive match (boxed),
/// or nil when out of range.
#[no_mangle]
pub unsafe extern "C" fn nulang_aot_receive_payload(idx: u64) -> u64 {
    AOT_RECEIVE_PAYLOAD.with(|c| {
        c.borrow()
            .get(idx as usize)
            .map(|v| v.as_raw())
            .unwrap_or_else(|| Value::nil().as_raw())
    })
}

/// Legacy pop-any receive (`RValue::Receive`): pops the next mailbox message
/// and returns its first payload value (boxed), or nil when the mailbox is
/// empty or no actor is active. The behavior id is discarded — the MIR
/// contract only consumes the first payload value.
#[no_mangle]
pub unsafe extern "C" fn nulang_aot_receive_pop() -> u64 {
    try_with_callbacks(|cb| cb.try_receive())
        .flatten()
        .map(|(_, first)| first.as_raw())
        .unwrap_or_else(|| Value::nil().as_raw())
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_jit_helpers_linked() {
        // Force the linker to retain the JIT runtime helpers by taking
        // their addresses. Without this, the linker may strip them since
        // they are only called from JIT-compiled code.
        let _ = super::nulang_arr_store as unsafe extern "C" fn(_, _, _, _);
        let _ = super::nulang_arr_len as unsafe extern "C" fn(_, _, _);
        let _ = super::nulang_field_load as unsafe extern "C" fn(_, _, _, _);
        let _ = super::nulang_safepoint_yield as unsafe extern "C" fn(u64) -> u64;
        let _ = super::nulang_alloc_obj as unsafe extern "C" fn(u64, u32) -> u64;
        let _ = super::nulang_obj_get as unsafe extern "C" fn(u64, u64) -> u64;
        let _ = super::nulang_obj_set as unsafe extern "C" fn(u64, u64, u64);
        let _ = super::nulang_obj_len as unsafe extern "C" fn(u64) -> u64;
        let _ = super::nulang_rec_copy as unsafe extern "C" fn(u64) -> u64;
        let _ = super::nulang_str_eq as unsafe extern "C" fn(u64, u64) -> u64;
        let _ = super::nulang_str_concat as unsafe extern "C" fn(u64, u64) -> u64;
        let _ = super::nulang_pow as extern "C" fn(u64, u64) -> u64;
        let _ = super::nulang_aot_self_ref as unsafe extern "C" fn() -> u64;
        let _ = super::nulang_aot_state_get as unsafe extern "C" fn(u64) -> u64;
        let _ = super::nulang_aot_state_set as unsafe extern "C" fn(u64, u64);
        let _ = super::nulang_aot_send_0 as unsafe extern "C" fn(u64, u64);
        let _ = super::nulang_aot_send_1 as unsafe extern "C" fn(u64, u64, u64);
        let _ = super::nulang_aot_send_2 as unsafe extern "C" fn(u64, u64, u64, u64);
        let _ = super::nulang_aot_send_8 as unsafe extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64);
        let _ = super::nulang_aot_receive_match_1 as unsafe extern "C" fn(u64) -> u64;
        let _ = super::nulang_aot_receive_match_2 as unsafe extern "C" fn(u64, u64) -> u64;
        let _ = super::nulang_aot_receive_payload as unsafe extern "C" fn(u64) -> u64;
        let _ = super::nulang_aot_receive_pop as unsafe extern "C" fn() -> u64;
        let _ = super::nulang_aot_spawn_push as unsafe extern "C" fn(u64, u64);
        let _ = super::nulang_aot_emit_0 as unsafe extern "C" fn(u64);
        let _ = super::nulang_aot_emit_1 as unsafe extern "C" fn(u64, u64);
        let _ = super::nulang_aot_emit_8
            as unsafe extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64);
    }
}
