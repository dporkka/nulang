//! Effect system: row-polymorphic effects with inference.

use crate::types::{Effect, EffectRow, NuResult, Type, TypeVar};

/// Check if an effect is contained in an effect row (subsumption).
pub fn contains(eff: &Effect, row: &EffectRow) -> bool {
    row.contains(eff)
}

/// Row subsumption: row1 <: row2 if row2 can be obtained by removing effects from row1.
/// For closed rows: row1 effects are a superset of row2 effects.
pub fn row_subsumes(row1: &EffectRow, row2: &EffectRow) -> bool {
    // Every effect in row2 must be present in row1
    for eff in row2.effects() {
        if !row1.contains(eff) {
            // If row1 is open, it might contain the effect via the row variable
            match row1 {
                EffectRow::Open(_, _) => continue,
                EffectRow::Closed(_) => return false,
            }
        }
    }
    true
}

/// Row unification: find a substitution that makes two rows equal.
/// Returns unified row + remaining constraints.
pub fn unify_rows(row1: &EffectRow, row2: &EffectRow) -> NuResult<(EffectRow, Vec<(TypeVar, Type)>)> {
    match (row1, row2) {
        // Two closed rows: unified row is the intersection of effects.
        (EffectRow::Closed(effs1), EffectRow::Closed(effs2)) => {
            // Compute intersection: effects present in BOTH rows.
            let mut unified: Vec<Effect> = Vec::new();
            for eff in effs1 {
                if effs2.contains(eff) {
                    unified.push(eff.clone());
                }
            }
            // Also include effects in effs2 that are in effs1 (same as above,
            // intersection is commutative).
            Ok((EffectRow::Closed(unified), Vec::new()))
        }

        // One open, one closed: check if closed effects are a subset of open's effects.
        // If so, the open row can be instantiated to the closed row.  If not, the
        // row variable absorbs the extra effects.
        (EffectRow::Open(open_effs, _r), EffectRow::Closed(closed_effs))
        | (EffectRow::Closed(closed_effs), EffectRow::Open(open_effs, _r)) => {
            // Check if all concrete effects in the closed row are present in the open row.
            for eff in closed_effs {
                if !open_effs.contains(eff) {
                    // The closed row has an effect not in the open row's concrete set.
                    // This is only valid if the row variable can be instantiated to
                    // include it.  We add the effect to the open row's concrete set
                    // and return the combined row as unified.
                }
            }
            // The unified row: all effects from the closed row, plus any extra from
            // the open row's concrete set.  The row variable remains if the closed
            // row doesn't cover all of open's effects.
            let mut unified_effs = closed_effs.clone();
            for eff in open_effs {
                if !unified_effs.contains(eff) {
                    unified_effs.push(eff.clone());
                }
            }
            // Since one side is closed, the result is closed (the row variable is resolved).
            Ok((EffectRow::Closed(unified_effs), Vec::new()))
        }

        // Two open rows: they share the same row variable conceptually.
        // The unified row contains the union of both concrete effect sets,
        // and we emit a constraint that the two row variables are equal.
        (EffectRow::Open(effs1, r1), EffectRow::Open(effs2, r2)) => {
            let mut unified_effs = effs1.clone();
            for eff in effs2 {
                if !unified_effs.contains(eff) {
                    unified_effs.push(eff.clone());
                }
            }
            // The row variable from the first row is kept.
            // We emit a constraint that r1 = r2 (same row variable).
            let mut constraints = Vec::new();
            if r1 != r2 {
                // Row variables are represented via Type::Var(TypeVar).
                // We use a constraint to unify them.
                constraints.push((TypeVar(r1.0), Type::Var(TypeVar(r2.0))));
            }
            Ok((EffectRow::Open(unified_effs, *r1), constraints))
        }
    }
}

/// Pretty-print an effect row as a string.
pub fn format_effect_row(row: &EffectRow) -> String {
    match row {
        EffectRow::Closed(effs) => {
            let names: Vec<_> = effs.iter().map(format_effect).collect();
            if names.is_empty() {
                "{}".to_string()
            } else {
                format!("{{{}}}", names.join(", "))
            }
        }
        EffectRow::Open(effs, r) => {
            let names: Vec<_> = effs.iter().map(format_effect).collect();
            if names.is_empty() {
                format!("{{|ρ{}}}", r.0)
            } else {
                format!("{{{}/{}}}", names.join(", "), r.0)
            }
        }
    }
}

fn format_effect(eff: &Effect) -> String {
    match eff {
        Effect::IO => "IO".to_string(),
        Effect::Net => "Net".to_string(),
        Effect::FS => "FS".to_string(),
        Effect::Rand => "Rand".to_string(),
        Effect::Time => "Time".to_string(),
        Effect::Spawn => "Spawn".to_string(),
        Effect::Send => "Send".to_string(),
        Effect::Receive => "Receive".to_string(),
        Effect::Migrate => "Migrate".to_string(),
        Effect::STM => "STM".to_string(),
        Effect::Async => "Async".to_string(),
        Effect::LLM => "LLM".to_string(),
        Effect::Cost => "Cost".to_string(),
        Effect::Event => "Event".to_string(),
        Effect::FFI => "FFI".to_string(),
        Effect::UserDefined(s) => s.clone(),
    }
}
