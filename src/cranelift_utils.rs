//! Shared Cranelift CLIF emission helpers used by both JIT and AOT backends.
//!
//! This module eliminates ~60 lines of duplicated constants and helper functions
//! between `src/jit/typed_compiler.rs` and `src/aot/codegen.rs`.

use cranelift::prelude::*;
use cranelift_frontend::FunctionBuilder;
use crate::value_layout::{PAYLOAD_MASK, SIGN_BIT, TAG_BOOL, TAG_INT, TAG_NIL};

// ---------------------------------------------------------------------------
// Constants — cast once here, import everywhere
// ---------------------------------------------------------------------------

pub const TAG_INT_I64: i64 = TAG_INT as i64;
pub const TAG_BOOL_I64: i64 = TAG_BOOL as i64;
pub const TAG_NIL_I64: i64 = TAG_NIL as i64;
pub const PAYLOAD_MASK_I64: i64 = PAYLOAD_MASK as i64;
pub const SIGN_BIT_I64: i64 = SIGN_BIT as i64;
pub const SIGN_EXTEND: i64 = 0xFFFF_0000_0000_0000u64 as i64;

// ---------------------------------------------------------------------------
// Payload extraction
// ---------------------------------------------------------------------------

/// Extract the 48-bit payload from a tagged i64 value.
///
/// Emits: `band(raw, PAYLOAD_MASK)` — zeroes out the upper 16 tag bits.
#[inline]
pub fn emit_extract_payload(builder: &mut FunctionBuilder, raw: Value) -> Value {
    let mask = builder.ins().iconst(types::I64, PAYLOAD_MASK_I64);
    builder.ins().band(raw, mask)
}

// ---------------------------------------------------------------------------
// Sign extension
// ---------------------------------------------------------------------------

/// Sign-extend a 48-bit payload to 64 bits.
///
/// Emits the equivalent of the runtime `sext48` function directly in CLIF:
/// ```clif
/// payload   = band(raw, PAYLOAD_MASK)
/// sign_bit  = band(raw, SIGN_BIT)
/// is_neg    = icmp ne, sign_bit, 0
/// extended  = bor(payload, 0xFFFF_0000_0000_0000)
/// result    = select is_neg, extended, payload
/// ```
///
/// This is a key optimization: instead of a runtime helper call, the sign
/// extension happens inline with ~5 CLIF instructions.
#[inline]
pub fn emit_sext48(builder: &mut FunctionBuilder, raw: Value) -> Value {
    let payload = emit_extract_payload(builder, raw);
    let sign_mask = builder.ins().iconst(types::I64, SIGN_BIT_I64);
    let sign_bit = builder.ins().band(raw, sign_mask);
    let zero = builder.ins().iconst(types::I64, 0);
    let is_negative = builder.ins().icmp(IntCC::NotEqual, sign_bit, zero);
    let sign_extend_const = builder.ins().iconst(types::I64, SIGN_EXTEND);
    let extended = builder.ins().bor(payload, sign_extend_const);
    builder.ins().select(is_negative, extended, payload)
}

// ---------------------------------------------------------------------------
// Tagging
// ---------------------------------------------------------------------------

/// Re-tag an i64 value into a NaN-tagged integer.
///
/// Emits: `bor(TAG_INT, band(value, PAYLOAD_MASK))`
#[inline]
pub fn emit_tag_int(builder: &mut FunctionBuilder, value: Value) -> Value {
    let tag = builder.ins().iconst(types::I64, TAG_INT_I64);
    let mask = builder.ins().iconst(types::I64, PAYLOAD_MASK_I64);
    let masked = builder.ins().band(value, mask);
    builder.ins().bor(tag, masked)
}

/// Tag a boolean comparison result (i8 from icmp/fcmp) as a NaN-tagged Bool.
///
/// Emits: `select cond, TAG_BOOL|1, TAG_BOOL|0`
/// Uses the more efficient 2-iconst + select pattern (from aot/codegen.rs).
#[inline]
pub fn emit_tag_bool(builder: &mut FunctionBuilder, cond: Value) -> Value {
    let true_val = builder.ins().iconst(types::I64, TAG_BOOL_I64 | 1);
    let false_val = builder.ins().iconst(types::I64, TAG_BOOL_I64 | 0);
    builder.ins().select(cond, true_val, false_val)
}
