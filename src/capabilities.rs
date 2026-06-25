//! Reference capability system for Nulang.
//!
//! Based on Pony's reference capabilities. Compile-time only, erased at runtime.

use crate::types::Capability;

/// Full capability lattice operations.
pub use crate::types::Capability as Cap;

/// Check if a value with capability `value_cap` can be used where `required_cap` is needed.
pub fn check(value_cap: Capability, required_cap: Capability) -> bool {
    value_cap.is_subtype_of(required_cap)
}

/// Check if a value can be sent to another actor (must be iso, val, tag, or lineariso).
pub fn check_sendable(cap: Capability) -> bool {
    cap.is_sendable()
}

/// Recover iso from trn (or preserve LinearIso).
pub fn recover_iso(cap: Capability) -> Option<Capability> {
    match cap {
        Capability::Trn => Some(Capability::Iso),
        // LinearIso is already a form of unique ownership, no recovery needed
        Capability::LinearIso => Some(Capability::LinearIso),
        _ => None,
    }
}

/// Recover val from ref (requires no aliases exist - checked by compiler).
pub fn recover_val(cap: Capability) -> Option<Capability> {
    match cap {
        Capability::Ref => Some(Capability::Val),
        _ => None,
    }
}

/// Read a reference through a box (box can read iso, lineariso, trn, ref, val as box).
pub fn read_as_box(cap: Capability) -> Option<Capability> {
    match cap {
        Capability::Iso | Capability::LinearIso | Capability::Trn | Capability::Ref | Capability::Val | Capability::Box => {
            Some(Capability::Box)
        }
        Capability::Tag => None,
    }
}

/// Convert capability to human-readable string.
pub fn format_cap(cap: Capability) -> &'static str {
    match cap {
        Capability::Iso => "iso",
        Capability::LinearIso => "lineariso",
        Capability::Trn => "trn",
        Capability::Ref => "ref",
        Capability::Val => "val",
        Capability::Box => "box",
        Capability::Tag => "tag",
    }
}
