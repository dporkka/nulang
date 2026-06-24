use crate::types::{Effect, EffectRow, NuError, NuResult, Span};

/// Compute whether `sub` is a subset of `sup` as an effect row.
///
/// A closed row {e1, e2} is a subset of {e1, e2, e3}.
/// A row with a tail variable is a superset of any row whose concrete
/// effects are all contained in it.
pub fn effect_row_subset(sub: &EffectRow, sup: &EffectRow) -> bool {
    match (sub, sup) {
        (EffectRow::Closed(es), EffectRow::Closed(fs)) => {
            es.iter().all(|e| fs.contains(e))
        }
        (EffectRow::Closed(es), EffectRow::Open(fs, _)) => {
            es.iter().all(|e| fs.contains(e))
        }
        // A row with a free tail variable can only be a subset of another
        // row with the *same* tail variable or a superset that has the
        // same free tail.
        (EffectRow::Open(es, v1), EffectRow::Open(fs, v2)) => {
            es.iter().all(|e| fs.contains(e)) && v1 == v2
        }
        // Open on the left, closed on the right: not a subset unless
        // the tail variable is empty (we can't prove that).
        (EffectRow::Open(_, _), EffectRow::Closed(_)) => false,
    }
}

/// Compute the union of two effect rows.
pub fn effect_row_union(a: &EffectRow, b: &EffectRow) -> EffectRow {
    match (a, b) {
        (EffectRow::Closed(es), EffectRow::Closed(fs)) => {
            let mut result = es.clone();
            for f in fs.iter() {
                if !result.contains(f) {
                    result.push(f.clone());
                }
            }
            EffectRow::Closed(result)
        }
        (EffectRow::Open(es, v), EffectRow::Closed(fs))
        | (EffectRow::Closed(fs), EffectRow::Open(es, v)) => {
            let mut result = es.clone();
            for f in fs.iter() {
                if !result.contains(f) {
                    result.push(f.clone());
                }
            }
            EffectRow::Open(result, *v)
        }
        (EffectRow::Open(es, v1), EffectRow::Open(fs, _)) => {
            let mut result = es.clone();
            for f in fs.iter() {
                if !result.contains(f) {
                    result.push(f.clone());
                }
            }
            // Keep the left tail variable
            EffectRow::Open(result, *v1)
        }
    }
}

/// Remove a handled effect from a row. If the row is closed and
/// contains the effect, it is removed. If it is open, we can't
/// statically remove since the tail could include it at runtime.
pub fn effect_row_diff(row: &EffectRow, handled: &Effect) -> EffectRow {
    match row {
        EffectRow::Closed(es) => {
            let filtered: Vec<_> = es.iter().filter(|e| *e != handled).cloned().collect();
            EffectRow::Closed(filtered)
        }
        // If the row is open we conservatively keep it.  The runtime
        // effect handler will catch it if present.
        open @ EffectRow::Open(..) => open.clone(),
    }
}

/// Verify that concrete effects are well-formed (exist in the effect table).
pub fn validate_effect_row(row: &EffectRow, _table: &EffectTable, span: Span) -> NuResult<()> {
    // For MVP we accept any effect name that is syntactically valid.
    // A production implementation would cross-reference a registry.
    match row {
        EffectRow::Closed(es) if es.is_empty() => Ok(()),
        EffectRow::Closed(_es) => {
            // TODO: check each effect name against the registry
            Ok(())
        }
        EffectRow::Open(_es, _v) => {
            // TODO: check concrete part against registry
            Ok(())
        }
    }
}

/// Global effect table — maps effect names to their operation signatures.
#[derive(Debug, Clone)]
pub struct EffectTable {
    pub entries: Vec<(String, Vec<(String, Vec<Type>, Type)>)>,
}

use crate::types::Type;

impl EffectTable {
    pub fn new() -> Self {
        EffectTable { entries: Vec::new() }
    }

    pub fn add(&mut self, name: impl Into<String>, ops: Vec<(String, Vec<Type>, Type)>) {
        self.entries.push((name.into(), ops));
    }

    pub fn lookup(&self, name: &str) -> Option<&[(String, Vec<Type>, Type)]> {
        self.entries.iter().find(|(n, _)| n == name).map(|(_, ops)| ops.as_slice())
    }
}

impl Default for EffectTable {
    fn default() -> Self {
        let mut table = EffectTable::new();

        // --- IO Effect ---
        table.add("IO", vec![
            ("print".into(), vec![Type::String], Type::Unit),
            ("readLine".into(), vec![], Type::String),
            ("flush".into(), vec![], Type::Unit),
        ]);

        // --- FileSystem Effect ---
        table.add("FileSystem", vec![
            ("readFile".into(), vec![Type::String], Type::String),
            ("writeFile".into(), vec![Type::String, Type::String], Type::Unit),
            ("appendFile".into(), vec![Type::String, Type::String], Type::Unit),
            ("deleteFile".into(), vec![Type::String], Type::Unit),
            ("exists".into(), vec![Type::String], Type::Bool),
        ]);

        // --- Network Effect ---
        table.add("Network", vec![
            ("httpGet".into(), vec![Type::String], Type::String),
            ("httpPost".into(), vec![Type::String, Type::String], Type::String),
            ("tcpConnect".into(), vec![Type::String, Type::Int], Type::Int),
        ]);

        // --- Random Effect ---
        table.add("Random", vec![
            ("intRange".into(), vec![Type::Int, Type::Int], Type::Int),
            ("float".into(), vec![], Type::Float),
            ("bool".into(), vec![], Type::Bool),
        ]);

        // --- Time Effect ---
        table.add("Time", vec![
            ("now".into(), vec![], Type::Int),
            ("sleep".into(), vec![Type::Int], Type::Unit),
            ("timeout".into(), vec![Type::Int], Type::Bool),
        ]);

        // --- Spawn Effect ---
        table.add("Spawn", vec![
            ("spawn".into(), vec![Type::Int], Type::Int),
        ]);

        // --- LLM Effect ---
        table.add("LLM", vec![
            ("generate".into(), vec![Type::String], Type::String),
            ("chat".into(), vec![Type::String, Type::String], Type::String),
            ("embed".into(), vec![Type::String], Type::Array(Box::new(Type::Float))),
        ]);

        table
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_effect_table_default() {
        let table = EffectTable::default();
        let io = table.lookup("IO").expect("IO should exist");
        assert!(!io.is_empty());
        assert!(io.iter().any(|(n, _, _)| n == "print"));
    }

    #[test]
    fn test_effect_row_union() {
        let a = EffectRow::Closed(vec![
            Effect::new("IO"),
            Effect::new("FileSystem"),
        ]);
        let b = EffectRow::Closed(vec![
            Effect::new("IO"),
            Effect::new("Network"),
        ]);
        let u = effect_row_union(&a, &b);
        match u {
            EffectRow::Closed(es) => {
                assert_eq!(es.len(), 3);
                assert!(es.iter().any(|e| e.name == "IO"));
                assert!(es.iter().any(|e| e.name == "FileSystem"));
                assert!(es.iter().any(|e| e.name == "Network"));
            }
            other => panic!("Expected Closed, got {:?}", other),
        }
    }

    #[test]
    fn test_effect_row_subset() {
        let sub = EffectRow::Closed(vec![
            Effect::new("IO"),
        ]);
        let sup = EffectRow::Closed(vec![
            Effect::new("IO"),
            Effect::new("FileSystem"),
        ]);
        assert!(effect_row_subset(&sub, &sup));
        assert!(!effect_row_subset(&sup, &sub));
    }

    #[test]
    fn test_effect_row_diff() {
        let row = EffectRow::Closed(vec![
            Effect::new("IO"),
            Effect::new("FileSystem"),
        ]);
        let diff = effect_row_diff(&row, &Effect::new("IO"));
        match diff {
            EffectRow::Closed(es) => {
                assert_eq!(es.len(), 1);
                assert_eq!(es[0].name, "FileSystem");
            }
            other => panic!("Expected Closed, got {:?}", other),
        }
    }

    #[test]
    fn test_union_with_open_row() {
        let closed = EffectRow::Closed(vec![Effect::new("IO")]);
        let open = EffectRow::Open(vec![Effect::new("FileSystem")], 1);
        let u = effect_row_union(&closed, &open);
        match u {
            EffectRow::Open(es, 1) => {
                assert_eq!(es.len(), 2);
            }
            other => panic!("Expected Open(_, 1), got {:?}", other),
        }
    }

    #[test]
    fn test_open_subset_of_open_same_var() {
        let a = EffectRow::Open(vec![Effect::new("IO")], 1);
        let b = EffectRow::Open(vec![Effect::new("IO"), Effect::new("FileSystem")], 1);
        assert!(effect_row_subset(&a, &b));
        assert!(!effect_row_subset(&b, &a));
    }

    #[test]
    fn test_open_not_subset_of_closed() {
        let open = EffectRow::Open(vec![Effect::new("IO")], 1);
        let closed = EffectRow::Closed(vec![Effect::new("IO"), Effect::new("FileSystem")]);
        assert!(!effect_row_subset(&open, &closed));
    }
}
