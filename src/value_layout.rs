//! Canonical NaN-boxed value layout for Nulang.
//!
//! This module owns the single source of truth for the bit layout used by the
//! VM, JIT runtime helpers, typed compiler, and Python marshalling layer.
//! Keeping the constants in one place prevents tag collisions and silent
//! divergence between interpretation and compiled code.
//!
//! # Layout
//!
//! All non-float values are encoded in the quiet-NaN payload of an f64:
//! - high 16 bits: type tag
//! - low 48 bits: payload
//!
//! Floats are stored as their raw IEEE-754 bit pattern. Any bit pattern that
//! is not a quiet NaN is a real float.

// ---------------------------------------------------------------------------
// Masks
// ---------------------------------------------------------------------------

/// Mask for the upper 16 tag bits.
pub const TAG_MASK: u64 = 0xFFFF_0000_0000_0000;

/// Mask for the lower 48 payload bits.
pub const PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// Bit 47 of the payload, used for sign-extending 48-bit signed integers.
pub const SIGN_BIT: u64 = 0x0000_8000_0000_0000;

// ---------------------------------------------------------------------------
// Type tags (stored in the upper 16 bits)
// ---------------------------------------------------------------------------

/// Tag for `nil`.
pub const TAG_NIL: u64 = 0x7FF8_0000_0000_0000;
/// Tag for `unit`.
pub const TAG_UNIT: u64 = 0x7FF9_0000_0000_0000;
/// Tag for booleans. Payload 0 = false, payload 1 = true.
pub const TAG_BOOL: u64 = 0x7FFA_0000_0000_0000;
/// Tag for integers. Payload is a 48-bit signed value.
pub const TAG_INT: u64 = 0x7FFB_0000_0000_0000;
/// Tag for heap pointers.
pub const TAG_PTR: u64 = 0x7FFC_0000_0000_0000;
/// Tag for actor references.
pub const TAG_ACTOR: u64 = 0x7FFD_0000_0000_0000;
/// Tag for interned string IDs.
pub const TAG_STRING: u64 = 0x7FFE_0000_0000_0000;
/// Tag for closure references.
pub const TAG_CLOSURE: u64 = 0x7FF7_0000_0000_0000;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Sign-extend a 48-bit signed payload to a full `i64`.
#[inline]
pub fn sext48(bits: u64) -> i64 {
    if bits & SIGN_BIT != 0 {
        (bits | 0xFFFF_0000_0000_0000) as i64
    } else {
        bits as i64
    }
}

/// Pack a 48-bit signed integer payload into a NaN-tagged raw value.
#[inline]
pub fn tag_int(payload: i64) -> u64 {
    TAG_INT | ((payload as u64) & PAYLOAD_MASK)
}

/// Pack a boolean into a NaN-tagged raw value.
#[inline]
pub fn tag_bool(b: bool) -> u64 {
    TAG_BOOL | (b as u64)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tags_are_unique() {
        let tags = [
            TAG_NIL,
            TAG_UNIT,
            TAG_BOOL,
            TAG_INT,
            TAG_PTR,
            TAG_ACTOR,
            TAG_STRING,
            TAG_CLOSURE,
        ];
        for i in 0..tags.len() {
            for j in (i + 1)..tags.len() {
                assert_ne!(tags[i], tags[j], "tags {} and {} collide", i, j);
            }
        }
    }

    #[test]
    fn test_tags_are_nan() {
        // All tags must be NaN values (exponent all 1s, mantissa non-zero)
        // so they are distinguishable from real floats.
        let exponent_mask: u64 = 0x7FF0_0000_0000_0000;
        let mantissa_mask: u64 = 0x000F_FFFF_FFFF_FFFF;
        let tags = [
            TAG_NIL,
            TAG_UNIT,
            TAG_BOOL,
            TAG_INT,
            TAG_PTR,
            TAG_ACTOR,
            TAG_STRING,
            TAG_CLOSURE,
        ];
        for tag in tags {
            assert!(
                (tag & exponent_mask) == exponent_mask && (tag & mantissa_mask) != 0,
                "tag {:#018x} is not a NaN",
                tag
            );
        }
    }

    #[test]
    fn test_sext48_positive() {
        assert_eq!(sext48(42), 42);
        assert_eq!(sext48(0), 0);
    }

    #[test]
    fn test_sext48_negative() {
        let bits: u64 = 0x0000_FFFF_FFFF_FFFF; // -1 in 48 bits
        assert_eq!(sext48(bits), -1);
    }

    #[test]
    fn test_tag_int_roundtrip() {
        for n in [0, 1, -1, i16::MAX as i64, i16::MIN as i64] {
            let raw = tag_int(n);
            let payload = raw & PAYLOAD_MASK;
            assert_eq!(sext48(payload), n);
            assert_eq!(raw & TAG_MASK, TAG_INT);
        }
    }
}
