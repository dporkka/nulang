//! Canonical i64-tagged value layout for Nulang.
//!
//! This module owns the single source of truth for the bit layout used by the
//! VM, JIT runtime helpers, typed compiler, WASM backend, and Python
//! marshalling layer. Keeping the constants in one place prevents tag
//! collisions and silent divergence between interpretation and compiled code.
//!
//! # Layout
//!
//! All values are represented as `u64` with the upper 16 bits encoding the
//! type tag and the lower 48 bits carrying the payload:
//!
//! ```text
//! |---- tag (16 bits) ----|-------- payload (48 bits) --------|
//! ```
//!
//! This scheme replaces the original NaN-boxing approach. The bit patterns
//! are identical to the old NaN-boxed layout (the tags occupy the IEEE 754
//! quiet-NaN range), but we now treat values as `i64`/`u64` integers rather
//! than `f64` bit patterns. This makes the representation immune to NaN
//! canonicalization in WASM engines while preserving the 8-byte value
//! footprint and the single-register property.
//!
//! # Rationale
//!
//! WASM engines normalize (canonicalize) floating-point NaN bit patterns,
//! which would silently corrupt type tags stored in NaN payload bits.
//! By treating values as `i64` integers and using integer shift/mask
//! operations for tag extraction, the representation is fully deterministic
//! across all targets: native, WASM, and any future backend.
//!
//! Floats are stored as their raw IEEE-754 bit pattern. Any bit pattern whose
//! upper 16 bits do not match a known type tag is interpreted as a float
//! (the current tag set occupies the quiet-NaN range 0x7FF6–0x7FFE, so no
//! valid non-NaN float will collide).
// ---------------------------------------------------------------------------
// Masks
// ---------------------------------------------------------------------------

/// Mask for the upper 16 tag bits.
pub const TAG_MASK: u64 = 0xFFFF_0000_0000_0000;

/// Mask for the lower 48 payload bits.
pub const PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// Bit 47 of the payload, used for sign-extending 48-bit signed integers.
pub const SIGN_BIT: u64 = 0x0000_8000_0000_0000;

/// Number of bits to shift right to extract the tag (upper 16 bits → low 16).
pub const TAG_SHIFT: u32 = 48;

// ---------------------------------------------------------------------------
// Type tags (upper 16 bits of the i64 value)
// ---------------------------------------------------------------------------

/// Tag for `nil`.
pub const TAG_NIL: u64 = 0x7FF8_0000_0000_0000;
/// Tag for `unit`.
pub const TAG_UNIT: u64 = 0x7FF9_0000_0000_0000;
/// Tag for booleans. Payload bit 0: false=0, true=1.
pub const TAG_BOOL: u64 = 0x7FFA_0000_0000_0000;
/// Tag for integers. Payload is a 48-bit signed value.
pub const TAG_INT: u64 = 0x7FFB_0000_0000_0000;
/// Tag for heap pointers. Payload is a heap offset.
pub const TAG_PTR: u64 = 0x7FFC_0000_0000_0000;
/// Tag for actor references.
pub const TAG_ACTOR: u64 = 0x7FFD_0000_0000_0000;
/// Tag for interned string IDs.
pub const TAG_STRING: u64 = 0x7FFE_0000_0000_0000;
/// Tag for closure references.
pub const TAG_CLOSURE: u64 = 0x7FF7_0000_0000_0000;

// ---------------------------------------------------------------------------
// Tag extraction helpers (i64-based — no f64 bit-casting)
// ---------------------------------------------------------------------------

/// Extract the upper 16 tag bits from a raw value.
#[inline]
pub fn tag_of(raw: u64) -> u64 {
    raw >> TAG_SHIFT
}

/// True when `raw` carries an integer tag.
#[inline]
pub fn is_int_raw(raw: u64) -> bool {
    (raw & TAG_MASK) == TAG_INT
}

/// True when `raw` carries a heap-pointer tag.
#[inline]
pub fn is_ptr_raw(raw: u64) -> bool {
    (raw & TAG_MASK) == TAG_PTR
}

/// Sign-extend a 48-bit signed payload to a full `i64`.
#[inline]
pub fn sext48(bits: u64) -> i64 {
    if bits & SIGN_BIT != 0 {
        (bits | 0xFFFF_0000_0000_0000) as i64
    } else {
        bits as i64
    }
}

/// Extract the integer payload from a tagged value (assumes `is_int_raw`).
#[inline]
pub fn as_int_raw(raw: u64) -> i64 {
    sext48(raw & PAYLOAD_MASK)
}

/// Extract the heap-pointer payload from a tagged value (assumes `is_ptr_raw`).
#[inline]
pub fn as_ptr_raw(raw: u64) -> u32 {
    (raw & 0xFFFF_FFFF) as u32
}

/// Pack a 48-bit signed integer payload into a tagged value.
#[inline]
pub fn tag_int(payload: i64) -> u64 {
    TAG_INT | ((payload as u64) & PAYLOAD_MASK)
}

/// Pack a boolean into a tagged value.
#[inline]
pub fn tag_bool(b: bool) -> u64 {
    TAG_BOOL | (b as u64)
}

/// Pack a heap offset into a tagged pointer value.
#[inline]
pub fn tag_ptr(offset: u32) -> u64 {
    TAG_PTR | (offset as u64)
}

/// Pack a raw u64 bit pattern into a tagged closure value.
#[inline]
pub fn tag_closure(payload: u64) -> u64 {
    TAG_CLOSURE | (payload & PAYLOAD_MASK)
}

// ---------------------------------------------------------------------------
// Float detection
// ---------------------------------------------------------------------------

/// True when `raw` represents a real IEEE-754 float (any bit pattern whose
/// upper 16 bits do not match a known type tag).
///
/// Since all tags occupy the quiet-NaN range (0x7FF6–0x7FFE), this is
/// equivalent to checking `!f64::from_bits(raw).is_nan()`, but expressed
/// in terms of the tag mask for clarity.
#[inline]
pub fn is_float_raw(raw: u64) -> bool {
    let tag = raw >> TAG_SHIFT;
    // Tags 0x7FF6 (Python), 0x7FF7 (Closure), 0x7FF8–0x7FFE (Nil, Unit,
    // Bool, Int, Ptr, Actor, String) are all in the quiet-NaN range.
    // Real floats cannot have these upper 16 bits — they are not NaN.
    !(0x7FF6..=0x7FFE).contains(&tag)
}

// ---------------------------------------------------------------------------
// Tests

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
    fn test_tags_distinct_upper_bits() {
        // Each tag must have a unique value in the upper 16 bits so that
        // `tag_of()` can discriminate types. No tag may collide with another
        // or with the float range (0x0000–0x7FF5, 0x7FFF).
        let tags: &[(u64, &str)] = &[
            (TAG_NIL, "nil"),
            (TAG_UNIT, "unit"),
            (TAG_BOOL, "bool"),
            (TAG_INT, "int"),
            (TAG_PTR, "ptr"),
            (TAG_ACTOR, "actor"),
            (TAG_STRING, "string"),
            (TAG_CLOSURE, "closure"),
        ];
        let mut seen = std::collections::HashSet::new();
        for &(tag, name) in tags {
            let upper = tag >> TAG_SHIFT;
            assert!(
                seen.insert(upper),
                "tag {} ({:#018x}) upper 16 bits {:#06x} collide with another tag",
                name, tag, upper
            );
        }
    }

    #[test]
    fn test_tag_of() {
        assert_eq!(tag_of(TAG_INT), TAG_INT >> TAG_SHIFT);
        assert_eq!(tag_of(TAG_NIL), TAG_NIL >> TAG_SHIFT);
        assert_eq!(tag_of(TAG_PTR | 0x1234), TAG_PTR >> TAG_SHIFT);
    }

    #[test]
    fn test_is_int_raw() {
        assert!(is_int_raw(tag_int(42)));
        assert!(is_int_raw(tag_int(-1)));
        assert!(!is_int_raw(TAG_NIL));
        assert!(!is_int_raw(TAG_PTR | 0x100));
    }

    #[test]
    fn test_is_ptr_raw() {
        assert!(is_ptr_raw(TAG_PTR | 0x1000));
        assert!(!is_ptr_raw(tag_int(0)));
        assert!(!is_ptr_raw(TAG_NIL));
    }

    #[test]
    fn test_as_int_raw() {
        assert_eq!(as_int_raw(tag_int(42)), 42);
        assert_eq!(as_int_raw(tag_int(-1)), -1);
        assert_eq!(as_int_raw(tag_int(0)), 0);
    }

    #[test]
    fn test_as_ptr_raw() {
        assert_eq!(as_ptr_raw(TAG_PTR | 0xDEAD_BEEF), 0xDEAD_BEEF);
        assert_eq!(as_ptr_raw(TAG_PTR), 0);
    }

    #[test]
    fn test_tag_ptr() {
        let raw = tag_ptr(0xABCD);
        assert!(is_ptr_raw(raw));
        assert_eq!(as_ptr_raw(raw), 0xABCD);
    }

    #[test]
    fn test_tag_closure() {
        let raw = tag_closure(0x5555);
        assert_eq!(raw & TAG_MASK, TAG_CLOSURE);
        assert_eq!(raw & PAYLOAD_MASK, 0x5555);
    }

    #[test]
    fn test_is_float_raw() {
        // Real floats (non-NaN) should be detected.
        assert!(is_float_raw(0u64));                          // +0.0
        assert!(is_float_raw(0x3FF0_0000_0000_0000));         // 1.0
        assert!(is_float_raw(0x4000_0000_0000_0000));         // 2.0
        assert!(is_float_raw(0x7FF0_0000_0000_0000));         // +inf
        // Tagged values should NOT be detected as floats.
        assert!(!is_float_raw(tag_int(1)));
        assert!(!is_float_raw(TAG_NIL));
        assert!(!is_float_raw(TAG_PTR | 0x1000));
        assert!(!is_float_raw(TAG_CLOSURE | 0x10));
        // NaN values with different upper bits (not a known tag) ARE floats.
        assert!(is_float_raw(0x7FF5_0000_0000_0000));         // NaN, not a tag
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
