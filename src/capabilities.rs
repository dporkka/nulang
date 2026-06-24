//! Capability system for controlling reference sharing.

use crate::types::Capability;

/// Compute the upper bound (join) of two capabilities in the lattice.
///
/// Lattice (highest to lowest permission):
///   Iso (isolated - unique mutable)
///   Trn (transition - unique but can become iso/ref)
///   Ref (reference - shared read/write)
///   Val (value - shared read-only, deeply immutable)
///   Box (boxed - unique ownership)
///   Tag (tag - no access, just identity)
pub fn cap_join(a: Capability, b: Capability) -> Capability {
    use Capability::*;
    match (a, b) {
        // Tag is bottom
        (Tag, c) | (c, Tag) => c,
        // Iso is top
        (Iso, _) | (_, Iso) => Iso,
        // Val with anything stays Val (immutable)
        (Val, Val) => Val,
        (Val, _) | (_, Val) => Ref,
        // Box + Box = Box, otherwise Trn
        (Box, Box) => Box,
        (Box, _) | (_, Box) => Trn,
        // Trn + Trn = Trn, otherwise Ref
        (Trn, Trn) => Trn,
        (Trn, _) | (_, Trn) => Ref,
        // Ref + Ref = Ref
        (Ref, Ref) => Ref,
    }
}

/// Compute the lower bound (meet) of two capabilities.
pub fn cap_meet(a: Capability, b: Capability) -> Capability {
    use Capability::*;
    match (a, b) {
        (Iso, c) | (c, Iso) => c,
        (Tag, _) | (_, Tag) => Tag,
        (Box, Box) => Box,
        (Box, _) | (_, Box) => Tag,
        (Trn, Trn) => Trn,
        (Trn, Val) | (Val, Trn) => Val,
        (Trn, _) | (_, Trn) => Tag,
        (Val, Val) => Val,
        (Val, _) | (_, Val) => Val,
        (Ref, Ref) => Ref,
    }
}

/// Can a read be performed with the given capability?
pub fn can_read(cap: Capability) -> bool {
    use Capability::*;
    matches!(cap, Iso | Trn | Ref | Val | Box)
}

/// Can a write be performed with the given capability?
pub fn can_write(cap: Capability) -> bool {
    use Capability::*;
    matches!(cap, Iso | Trn | Ref)
}

/// Can the reference be aliased (shared)?
pub fn can_alias(cap: Capability) -> bool {
    use Capability::*;
    matches!(cap, Val | Ref | Tag)
}

/// Is the reference sendable across actors?
pub fn can_send(cap: Capability) -> bool {
    use Capability::*;
    matches!(cap, Iso | Val | Tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cap_join() {
        use Capability::*;
        assert_eq!(cap_join(Tag, Ref), Ref);
        assert_eq!(cap_join(Val, Val), Val);
        assert_eq!(cap_join(Val, Iso), Iso);
        assert_eq!(cap_join(Box, Box), Box);
        assert_eq!(cap_join(Box, Trn), Trn);
    }

    #[test]
    fn test_cap_meet() {
        use Capability::*;
        assert_eq!(cap_meet(Iso, Ref), Ref);
        assert_eq!(cap_meet(Val, Val), Val);
        assert_eq!(cap_meet(Box, Trn), Tag);
    }

    #[test]
    fn test_permissions() {
        use Capability::*;
        assert!(can_read(Ref));
        assert!(can_write(Ref));
        assert!(!can_write(Val));
        assert!(can_alias(Val));
        assert!(!can_alias(Box));
        assert!(can_send(Iso));
        assert!(can_send(Val));
        assert!(!can_send(Ref));
    }
}
