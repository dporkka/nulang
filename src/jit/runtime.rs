//! Runtime helper functions callable from JIT-compiled code.

use crate::value_layout::{sext48, tag_int, PAYLOAD_MASK, TAG_INT, TAG_MASK};
use crate::vm::Value;

#[no_mangle]
pub extern "C" fn nulang_iadd(a: u64, b: u64) -> u64 {
    tag_int(sext48(a & PAYLOAD_MASK) + sext48(b & PAYLOAD_MASK))
}

#[no_mangle]
pub extern "C" fn nulang_isub(a: u64, b: u64) -> u64 {
    tag_int(sext48(a & PAYLOAD_MASK) - sext48(b & PAYLOAD_MASK))
}

#[no_mangle]
pub extern "C" fn nulang_imul(a: u64, b: u64) -> u64 {
    // wrapping_mul: 48-bit operands can overflow i64 when multiplied; the
    // result is masked to 48 bits by tag_int (matches interpreter IMul).
    tag_int(sext48(a & PAYLOAD_MASK).wrapping_mul(sext48(b & PAYLOAD_MASK)))
}

#[no_mangle]
pub extern "C" fn nulang_idiv(a: u64, b: u64) -> u64 {
    let bv = sext48(b & PAYLOAD_MASK);
    if bv == 0 {
        return Value::nil().as_raw();
    }
    tag_int(sext48(a & PAYLOAD_MASK) / bv)
}

#[no_mangle]
pub extern "C" fn nulang_imod(a: u64, b: u64) -> u64 {
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
    tag_int(-sext48(a & PAYLOAD_MASK))
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
    Value::bool(sext48(a & PAYLOAD_MASK) == sext48(b & PAYLOAD_MASK)).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_icmp_lt(a: u64, b: u64) -> u64 {
    Value::bool(sext48(a & PAYLOAD_MASK) < sext48(b & PAYLOAD_MASK)).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_icmp_gt(a: u64, b: u64) -> u64 {
    Value::bool(sext48(a & PAYLOAD_MASK) > sext48(b & PAYLOAD_MASK)).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_icmp_le(a: u64, b: u64) -> u64 {
    Value::bool(sext48(a & PAYLOAD_MASK) <= sext48(b & PAYLOAD_MASK)).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_icmp_ge(a: u64, b: u64) -> u64 {
    Value::bool(sext48(a & PAYLOAD_MASK) >= sext48(b & PAYLOAD_MASK)).as_raw()
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
    // Guard the zero divisor like `nulang_idiv`: the interpreter's FDiv
    // yields nil instead of inf/NaN (src/vm.rs OpCode::FDiv).
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
    Value::bool(!is_truthy(a)).as_raw()
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
