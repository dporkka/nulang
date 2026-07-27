//! Compile-time type knowledge for code generation.
//!
//! Maps program values (registers or MIR locals) to statically-known types
//! so that backends (JIT, AOT) can emit unboxed native code instead of
//! NaN-tag-aware runtime operations.
//!
//! Shared between the JIT (`src/jit/typed_compiler.rs`) and the AOT
//! compiler (`src/aot/`).

/// Number of registers / value slots tracked.
pub const REG_COUNT: usize = 256;

/// The static type of a value known at compile time.
///
/// - `Int`: NaN-tagged integer → strip tag, use direct i64 ops.
/// - `Float`: Raw f64 bits → use direct f64 ops.
/// - `Bool`: NaN-tagged boolean → compare directly against tagged constants.
/// - `Unknown`: Fall back to runtime helpers / boxed representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownType {
    Int,
    Float,
    Bool,
    Unknown,
}

/// Static type information for a set of values (registers or MIR locals).
///
/// Uses a flat `[KnownType; 256]` array for O(1) access with no hashing.
/// When a type is known, backends can emit optimized native code instead of
/// calling NaN-tag-aware runtime helpers.
///
/// # Example
/// ```
/// use nulang::type_metadata::{TypeMetadata, KnownType};
///
/// let mut meta = TypeMetadata::new();
/// meta.set_type(0, KnownType::Int);   // R0 is known Int
/// meta.set_type(1, KnownType::Float); // R1 is known Float
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct TypeMetadata {
    /// Per-register known type. Index ≥ `REG_COUNT` is a programmer error.
    pub reg_types: [KnownType; REG_COUNT],
}

impl Default for TypeMetadata {
    fn default() -> Self {
        Self {
            reg_types: [KnownType::Unknown; REG_COUNT],
        }
    }
}

impl std::ops::Index<usize> for TypeMetadata {
    type Output = KnownType;
    fn index(&self, index: usize) -> &KnownType {
        &self.reg_types[index]
    }
}

impl std::ops::IndexMut<usize> for TypeMetadata {
    fn index_mut(&mut self, index: usize) -> &mut KnownType {
        &mut self.reg_types[index]
    }
}

/// Convert a language-level `Type` to a `KnownType` for code generation.
///
/// Only primitive types are statically known; polymorphic, compound, and
/// effectful types all map to `Unknown`.
pub fn type_to_known_type(ty: &crate::types::Type) -> KnownType {
    match ty {
        crate::types::Type::Primitive(p) => match p {
            crate::types::PrimitiveType::Int => KnownType::Int,
            crate::types::PrimitiveType::Float => KnownType::Float,
            crate::types::PrimitiveType::Bool => KnownType::Bool,
            _ => KnownType::Unknown,
        },
        _ => KnownType::Unknown,
    }
}

impl TypeMetadata {
    /// Create an empty type metadata map (all values are Unknown).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the known type for a value index.
    pub fn set_type(&mut self, reg: usize, ty: KnownType) {
        self.reg_types[reg] = ty;
    }

    /// Get the known type for a value index, defaulting to `Unknown`.
    pub fn get_type(&self, reg: usize) -> KnownType {
        self.reg_types
            .get(reg)
            .copied()
            .unwrap_or(KnownType::Unknown)
    }

    /// Check whether both operands have the same known type.
    pub fn both_known(&self, r1: usize, r2: usize, expected: KnownType) -> bool {
        self.get_type(r1) == expected && self.get_type(r2) == expected
    }

    /// Check whether a single value has the expected known type.
    pub fn is_known(&self, reg: usize, expected: KnownType) -> bool {
        self.get_type(reg) == expected
    }

    /// Mark the destination as having a known type after an operation.
    ///
    /// For arithmetic: the result type is usually the same as the operand type.
    /// For comparisons: the result is always Bool.
    pub fn propagate_result(&mut self, dst: usize, operand_reg: usize) {
        let ty = self.reg_types[operand_reg];
        if ty != KnownType::Unknown {
            self.reg_types[dst] = ty;
        }
    }

    /// Mark the destination as Bool (used after comparisons).
    pub fn set_bool_result(&mut self, dst: usize) {
        self.reg_types[dst] = KnownType::Bool;
    }

    /// Returns true if no value has a known type.
    pub fn is_empty(&self) -> bool {
        self.reg_types.iter().all(|&t| t == KnownType::Unknown)
    }

    /// Build TypeMetadata from an iterator of (register_index, Type) pairs.
    ///
    /// Converts language-level `Type` values to `KnownType` by stripping
    /// away polymorphic wrappers: only primitive `Int`, `Float`, and `Bool`
    /// are statically known; everything else becomes `Unknown`.
    pub fn from_mir_locals<'a>(
        locals: impl Iterator<Item = (usize, &'a crate::types::Type)>,
    ) -> Self {
        let mut meta = TypeMetadata::new();
        for (reg, ty) in locals {
            let known = type_to_known_type(ty);
            if known != KnownType::Unknown {
                meta.set_type(reg, known);
            }
        }
        meta
    }
}
