//! Compile-time type knowledge for code generation.
//!
//! Maps program values (registers or MIR locals) to statically-known types
//! so that backends (JIT, AOT) can emit unboxed native code instead of
//! NaN-tag-aware runtime operations.
//!
//! Shared between the JIT (`src/jit/typed_compiler.rs`) and the AOT
//! compiler (`src/aot/`).

use std::collections::HashMap;

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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TypeMetadata {
    /// Maps value index → known type (if any).
    pub reg_types: HashMap<usize, KnownType>,
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
        self.reg_types.insert(reg, ty);
    }

    /// Get the known type for a value index, defaulting to `Unknown`.
    pub fn get_type(&self, reg: usize) -> KnownType {
        self.reg_types
            .get(&reg)
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
        if let Some(&ty) = self.reg_types.get(&operand_reg) {
            self.reg_types.insert(dst, ty);
        }
    }

    /// Mark the destination as Bool (used after comparisons).
    pub fn set_bool_result(&mut self, dst: usize) {
        self.reg_types.insert(dst, KnownType::Bool);
    }

    /// Returns true if no value has a known type.
    pub fn is_empty(&self) -> bool {
        self.reg_types.is_empty()
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
