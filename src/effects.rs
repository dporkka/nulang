//! Effect system implementation.

use crate::types::{EffectRow, Type};

/// Represents a resolved effect operation.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectOp {
    pub name: String,
    pub params: Vec<Type>,
    pub ret: Type,
}

/// Table of known effects and their operations.
#[derive(Debug, Clone)]
pub struct EffectTable {
    effects: Vec<(String, Vec<EffectOp>)>,
}

impl EffectTable {
    pub fn new() -> Self {
        EffectTable { effects: Vec::new() }
    }

    pub fn register(&mut self, name: String, ops: Vec<EffectOp>) {
        self.effects.push((name, ops));
    }

    pub fn lookup(&self, effect: &str, op: &str) -> Option<&EffectOp> {
        self.effects.iter()
            .find(|(name, _)| name == effect)
            .and_then(|(_, ops)| ops.iter().find(|o| o.name == op))
    }

    /// Check if effect row `sub` is a subset of `sup`.
    pub fn is_subset(&self, sub: &EffectRow, sup: &EffectRow) -> bool {
        use crate::types::EffectRow::*;
        match (sub, sup) {
            (Closed(a), Closed(b)) => a.iter().all(|e| b.contains(e)),
            (Closed(a), Open(b, _)) => a.iter().all(|e| b.contains(e)),
            (Open(a, _), Open(b, _)) if a == b => true,
            (Open(a, va), Open(b, vb)) => {
                a.iter().all(|e| b.contains(e)) && va == vb
            }
            _ => false,
        }
    }

    /// Combine two effect rows (union).
    pub fn combine(&self, a: &EffectRow, b: &EffectRow) -> EffectRow {
        use crate::types::EffectRow::*;
        match (a, b) {
            (Closed(x), Closed(y)) => {
                let mut union = x.clone();
                for e in y {
                    if !union.contains(e) {
                        union.push(e.clone());
                    }
                }
                Closed(union)
            }
            (Open(x, v), Closed(y)) | (Closed(y), Open(x, v)) => {
                let mut union = x.clone();
                for e in y {
                    if !union.contains(e) {
                        union.push(e.clone());
                    }
                }
                Open(union, *v)
            }
            (Open(x, v), Open(y, _)) => {
                let mut union = x.clone();
                for e in y {
                    if !union.contains(e) {
                        union.push(e.clone());
                    }
                }
                Open(union, *v)
            }
        }
    }
}

impl Default for EffectTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Primitive::*;

    #[test]
    fn test_effect_table() {
        let mut table = EffectTable::new();
        table.register("IO".to_string(), vec![
            EffectOp { name: "print".to_string(), params: vec![Type::Prim(String)], ret: Type::Unit },
        ]);
        assert!(table.lookup("IO", "print").is_some());
        assert!(table.lookup("IO", "read").is_none());
    }

    #[test]
    fn test_subset() {
        let table = EffectTable::new();
        let a = EffectRow::Closed(vec!["IO".to_string()]);
        let b = EffectRow::Closed(vec!["IO".to_string(), "Net".to_string()]);
        assert!(table.is_subset(&a, &b));
        assert!(!table.is_subset(&b, &a));
    }
}
