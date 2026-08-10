//! Live variable analysis over CIR.
//!
//! Computes, for each `SuspendAndYield`, the set of variables that must be
//! preserved across the suspension boundary (the `live_vars` of the resume
//! block minus the resume variable), then synthesizes the `SaveFrame` /
//! `RestoreFrame` statements that the codegen lowers to frame save/restore
//! in Wasm linear memory.

use crate::cir::{
    compute_frame_layout, CirBlock, CirExpr, CirFunction, CirStmt, CirTerminator, VarId,
};
use crate::cir_lower::{FRAME_PTR_VAR, RESULT_VAR};
use std::collections::BTreeSet;

/// Compute live variables for every suspension boundary and populate
/// `SuspendAndYield.live_vars`, plus `SaveFrame`/`RestoreFrame` statements.
pub fn compute_live_vars(func: &mut CirFunction) {
    let n = func.blocks.len();
    if n == 0 {
        return;
    }

    // Per-block use/def (vars, in stable order).
    let mut uses: Vec<BTreeSet<VarId>> = Vec::with_capacity(n);
    let mut defs: Vec<BTreeSet<VarId>> = Vec::with_capacity(n);
    for block in &func.blocks {
        let (u, d) = block_use_def(block);
        uses.push(u);
        defs.push(d);
    }

    // Successors by block id (ids are dense: block id == index).
    let succs: Vec<Vec<usize>> = (0..n)
        .map(|i| successors(&func.blocks[i].terminator))
        .collect();

    // Fixed-point liveness: out[B] = ⋃ in[S], in[B] = use[B] ∪ (out[B] \ def[B]).
    let mut in_sets: Vec<BTreeSet<VarId>> = vec![BTreeSet::new(); n];
    let mut out_sets: Vec<BTreeSet<VarId>> = vec![BTreeSet::new(); n];
    loop {
        let mut changed = false;
        // Iterate in reverse for faster convergence (does not affect result).
        for i in (0..n).rev() {
            // out[B] = ⋃ in[S]
            let mut out: BTreeSet<VarId> = BTreeSet::new();
            for &s in &succs[i] {
                out.extend(in_sets[s].iter().copied());
            }
            // in[B] = use[B] ∪ (out[B] \ def[B])
            let mut inp: BTreeSet<VarId> = uses[i].clone();
            inp.extend(out.difference(&defs[i]).copied());
            if inp != in_sets[i] || out != out_sets[i] {
                in_sets[i] = inp;
                out_sets[i] = out;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Populate SuspendAndYield.live_vars and synthesize SaveFrame /
    // RestoreFrame. Collect first, then apply, to avoid aliasing issues with
    // simultaneous &mut borrows of two blocks.
    let mut save_frames: Vec<(usize, Vec<VarId>, Vec<usize>)> = Vec::new();
    // (block idx, vars, offsets, resume_var)
    let mut restore_frames: Vec<(usize, Vec<VarId>, Vec<usize>, VarId)> = Vec::new();

    for (i, block) in func.blocks.iter_mut().enumerate() {
        if let CirTerminator::SuspendAndYield {
            resume_block,
            resume_var,
            live_vars,
            ..
        } = &mut block.terminator
        {
            let r = resume_block.0 as usize;
            let mut live: Vec<VarId> = in_sets[r]
                .iter()
                .copied()
                .filter(|v| *v != *resume_var && *v != FRAME_PTR_VAR && *v != RESULT_VAR)
                .collect();
            live.sort();
            *live_vars = live.clone();

            // Frame layout: header 16 bytes + 8 bytes per live var.
            let layout = compute_frame_layout(&live);
            let offsets: Vec<usize> = live.iter().map(|v| layout.var_offsets[v]).collect();
            save_frames.push((i, live.clone(), offsets.clone()));

            // Restore at the top of the resume block. Each resume block has
            // exactly one incoming suspend (the splitter allocates a fresh
            // resume id per suspension), so no dedup check is needed.
            restore_frames.push((r, live, offsets, *resume_var));
        }
    }

    for (i, vars, offsets) in save_frames {
        func.blocks[i].stmts.push(CirStmt::SaveFrame {
            vars,
            offsets,
            frame_ptr: FRAME_PTR_VAR,
        });
    }
    for (i, vars, offsets, resume_var) in restore_frames {
        func.blocks[i].stmts.insert(
            0,
            CirStmt::RestoreFrame {
                vars,
                offsets,
                frame_ptr: FRAME_PTR_VAR,
            },
        );
        // The resume value arrives via the shared resume function's RESULT_VAR
        // local; copy it into the MIR destination local of the suspending
        // assignment.
        func.blocks[i]
            .stmts
            .insert(1, CirStmt::Assign { dst: resume_var, src: CirExpr::Var(RESULT_VAR) });
    }
}

/// Collect variables used (read) in a block.
fn block_use_def(block: &CirBlock) -> (BTreeSet<VarId>, BTreeSet<VarId>) {
    let mut uses = BTreeSet::new();
    let mut defs = BTreeSet::new();

    for stmt in &block.stmts {
        match stmt {
            // MIR/CIR maintain SSA-like single-def-per-block; the `!defs.contains(dst)`
            // guard prevents a second definition of `dst` within the same block from
            // falsely registering the source's prior-def read as a use of this def.
            CirStmt::Assign { dst, src } => {
                if !defs.contains(dst) {
                    expr_uses(src, &mut uses);
                }
                defs.insert(*dst);
            }
            CirStmt::Emit { args, .. } => {
                for a in args {
                    expr_uses(a, &mut uses);
                }
            }
            CirStmt::SaveFrame { vars, .. } => {
                uses.extend(vars.iter().copied());
            }
            CirStmt::RestoreFrame { vars, .. } => {
                defs.extend(vars.iter().copied());
            }
        }
    }

    match &block.terminator {
        CirTerminator::Return(Some(e)) => expr_uses(e, &mut uses),
        CirTerminator::Branch { cond, .. } => expr_uses(cond, &mut uses),
        CirTerminator::SuspendAndYield { args, .. } => {
            for a in args {
                expr_uses(a, &mut uses);
            }
        }
        CirTerminator::Resume(e) => expr_uses(e, &mut uses),
        _ => {}
    }

    (uses, defs)
}

/// Collect variables referenced by an expression.
fn expr_uses(e: &CirExpr, out: &mut BTreeSet<VarId>) {
    match e {
        CirExpr::Var(v) => {
            out.insert(*v);
        }
        CirExpr::BinaryOp { lhs, rhs, .. } => {
            expr_uses(lhs, out);
            expr_uses(rhs, out);
        }
        CirExpr::UnaryOp { operand, .. } => expr_uses(operand, out),
        CirExpr::Call { args, .. } => {
            for a in args {
                expr_uses(a, out);
            }
        }
        CirExpr::ArrayLen { arr } => expr_uses(arr, out),
        CirExpr::ArrayLoad { arr, idx } => {
            expr_uses(arr, out);
            expr_uses(idx, out);
        }
        _ => {}
    }
}

/// CFG successors of a terminator (block ids are dense, so ids == indices).
fn successors(term: &CirTerminator) -> Vec<usize> {
    match term {
        CirTerminator::Return(_) | CirTerminator::Resume(_) => Vec::new(),
        CirTerminator::Jump(t) => vec![t.0 as usize],
        CirTerminator::Branch {
            then_block,
            else_block,
            ..
        } => vec![then_block.0 as usize, else_block.0 as usize],
        CirTerminator::SuspendAndYield { resume_block, .. } => vec![resume_block.0 as usize],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cir::{CirBlock, CirExpr, CirFunction, CirStmt, CirTerminator, EffectKind, BlockId, VarId};

    /// Build a minimal 2-block CIR: block 0 suspends (LLM.ask with one live var),
    /// block 1 (resume) returns the result.
    fn build_two_block_cir() -> CirFunction {
        let v0 = VarId(0); // param: suspension arg
        let v1 = VarId(1); // const live across suspend
        let v2 = VarId(2); // resume dst

        CirFunction {
            name: "test_suspend".into(),
            locals: (0..256).map(|i| crate::cir::CirLocal { id: VarId(i) }).collect(),
            entry_block: BlockId(0),
            blocks: vec![
                CirBlock {
                    id: BlockId(0),
                    stmts: vec![
                        CirStmt::Assign { dst: v1, src: CirExpr::ConstI64(42) },
                    ],
                    terminator: CirTerminator::SuspendAndYield {
                        effect: EffectKind::LlmAsk,
                        args: vec![CirExpr::Var(v0)],
                        resume_block: BlockId(1),
                        resume_var: v2,
                        live_vars: Vec::new(), // populated by compute_live_vars
                    },
                },
                CirBlock {
                    id: BlockId(1),
                    stmts: vec![
                        // resume block uses v1 (live) and v2 (resume value)
                        CirStmt::Assign {
                            dst: VarId(3),
                            src: CirExpr::BinaryOp {
                                op: crate::cir::BinaryOp::Add,
                                lhs: Box::new(CirExpr::Var(v1)),
                                rhs: Box::new(CirExpr::Var(v2)),
                            },
                        },
                    ],
                    terminator: CirTerminator::Return(Some(CirExpr::Var(VarId(3)))),
                },
            ],
        }
    }

    #[test]
    fn test_compute_live_vars_populates_suspend() {
        let mut cir = build_two_block_cir();
        compute_live_vars(&mut cir);

        // Block 0 should have SaveFrame appended
        let block0 = &cir.blocks[0];
        let has_save = block0.stmts.iter().any(|s| matches!(s, CirStmt::SaveFrame { .. }));
        assert!(has_save, "block 0 should have SaveFrame after liveness analysis");

        // Block 0's terminator should have live_vars populated
        if let CirTerminator::SuspendAndYield { live_vars, .. } = &block0.terminator {
            assert!(!live_vars.is_empty(), "live_vars should be non-empty");
            // v1 (const 42) is live across the suspend
            assert!(live_vars.contains(&VarId(1)), "const var should be live");
        } else {
            panic!("expected SuspendAndYield terminator");
        }

        // Block 1 should have RestoreFrame + resume Assign
        let block1 = &cir.blocks[1];
        let has_restore = block1.stmts.iter().any(|s| matches!(s, CirStmt::RestoreFrame { .. }));
        assert!(has_restore, "resume block should have RestoreFrame");
        let has_resume_assign = block1.stmts.iter().any(|s| matches!(s,
            CirStmt::Assign { dst, src: CirExpr::Var(resume_src) }
            if *dst == VarId(2) && *resume_src == RESULT_VAR
        ));
        assert!(has_resume_assign, "resume block should assign from RESULT_VAR");
    }

    #[test]
    fn test_compute_live_vars_no_suspend_noop() {
        // A single block with no suspension should remain unchanged
        let mut cir = CirFunction {
            name: "no_suspend".into(),
            locals: vec![],
            entry_block: BlockId(0),
            blocks: vec![
                CirBlock {
                    id: BlockId(0),
                    stmts: vec![
                        CirStmt::Assign { dst: VarId(0), src: CirExpr::ConstI64(1) },
                    ],
                    terminator: CirTerminator::Return(Some(CirExpr::Var(VarId(0)))),
                },
            ],
        };
        let blocks_before = cir.blocks[0].stmts.len();
        compute_live_vars(&mut cir);
        // No SaveFrame/RestoreFrame should be added
        assert_eq!(cir.blocks[0].stmts.len(), blocks_before,
            "non-suspending function should not get SaveFrame/RestoreFrame");
    }
}
