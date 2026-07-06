//! Runtime helper functions callable from JIT-compiled code.

use crate::vm::Value;
use crate::value_layout::{sext48, tag_int, PAYLOAD_MASK};

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
    tag_int(sext48(a & PAYLOAD_MASK) * sext48(b & PAYLOAD_MASK))
}

#[no_mangle]
pub extern "C" fn nulang_idiv(a: u64, b: u64) -> u64 {
    let bv = sext48(b & PAYLOAD_MASK);
    if bv == 0 { return Value::nil().as_raw(); }
    tag_int(sext48(a & PAYLOAD_MASK) / bv)
}

#[no_mangle]
pub extern "C" fn nulang_imod(a: u64, b: u64) -> u64 {
    let bv = sext48(b & PAYLOAD_MASK);
    if bv == 0 { return Value::nil().as_raw(); }
    tag_int(sext48(a & PAYLOAD_MASK) % bv)
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
    Value::float(f64::from_bits(a) / f64::from_bits(b)).as_raw()
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
