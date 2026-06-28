//! Runtime helper functions callable from JIT-compiled code.

use crate::vm::Value;

const TAG_INT: u64 = 0x7FF9000000000000;

#[inline]
fn sext48(bits: u64) -> i64 {
    if bits & 0x0000800000000000 != 0 {
        (bits | 0xFFFF000000000000) as i64
    } else {
        bits as i64
    }
}

#[inline]
fn tag_int(n: i64) -> u64 {
    TAG_INT | ((n as u64) & 0x0000FFFFFFFFFFFF)
}

#[no_mangle]
pub extern "C" fn nulang_iadd(a: u64, b: u64) -> u64 {
    tag_int(sext48(a & 0x0000FFFFFFFFFFFF) + sext48(b & 0x0000FFFFFFFFFFFF))
}

#[no_mangle]
pub extern "C" fn nulang_isub(a: u64, b: u64) -> u64 {
    tag_int(sext48(a & 0x0000FFFFFFFFFFFF) - sext48(b & 0x0000FFFFFFFFFFFF))
}

#[no_mangle]
pub extern "C" fn nulang_imul(a: u64, b: u64) -> u64 {
    tag_int(sext48(a & 0x0000FFFFFFFFFFFF) * sext48(b & 0x0000FFFFFFFFFFFF))
}

#[no_mangle]
pub extern "C" fn nulang_idiv(a: u64, b: u64) -> u64 {
    let bv = sext48(b & 0x0000FFFFFFFFFFFF);
    if bv == 0 { return Value::nil().as_raw(); }
    tag_int(sext48(a & 0x0000FFFFFFFFFFFF) / bv)
}

#[no_mangle]
pub extern "C" fn nulang_imod(a: u64, b: u64) -> u64 {
    let bv = sext48(b & 0x0000FFFFFFFFFFFF);
    if bv == 0 { return Value::nil().as_raw(); }
    tag_int(sext48(a & 0x0000FFFFFFFFFFFF) % bv)
}

#[no_mangle]
pub extern "C" fn nulang_ineg(a: u64) -> u64 {
    tag_int(-sext48(a & 0x0000FFFFFFFFFFFF))
}

#[no_mangle]
pub extern "C" fn nulang_iinc(a: u64) -> u64 {
    tag_int(sext48(a & 0x0000FFFFFFFFFFFF) + 1)
}

#[no_mangle]
pub extern "C" fn nulang_idec(a: u64) -> u64 {
    tag_int(sext48(a & 0x0000FFFFFFFFFFFF) - 1)
}

#[no_mangle]
pub extern "C" fn nulang_icmp_eq(a: u64, b: u64) -> u64 {
    Value::bool(sext48(a & 0x0000FFFFFFFFFFFF) == sext48(b & 0x0000FFFFFFFFFFFF)).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_icmp_lt(a: u64, b: u64) -> u64 {
    Value::bool(sext48(a & 0x0000FFFFFFFFFFFF) < sext48(b & 0x0000FFFFFFFFFFFF)).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_icmp_gt(a: u64, b: u64) -> u64 {
    Value::bool(sext48(a & 0x0000FFFFFFFFFFFF) > sext48(b & 0x0000FFFFFFFFFFFF)).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_icmp_le(a: u64, b: u64) -> u64 {
    Value::bool(sext48(a & 0x0000FFFFFFFFFFFF) <= sext48(b & 0x0000FFFFFFFFFFFF)).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_icmp_ge(a: u64, b: u64) -> u64 {
    Value::bool(sext48(a & 0x0000FFFFFFFFFFFF) >= sext48(b & 0x0000FFFFFFFFFFFF)).as_raw()
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
    Value::float(sext48(a & 0x0000FFFFFFFFFFFF) as f64).as_raw()
}

#[no_mangle]
pub extern "C" fn nulang_ftoi(a: u64) -> u64 {
    Value::int(f64::from_bits(a) as i64).as_raw()
}
