//! Escape Analysis for the Nulang JIT Compiler
//!
//! This module implements bytecode-level escape analysis for Nulang's register-based VM.
//! It determines whether heap-allocated objects can be safely allocated on the stack
//! instead, based on how they are used within a function.
//!
//! # Analysis Levels
//!
//! - **NoEscape**: Object is only used within its allocating function → stack allocable.
//! - **ArgEscape**: Object escapes through a function argument → limited optimization.
//! - **GlobalEscape**: Object escapes globally (returned, stored in heap, sent to actor)
//!   → must remain heap-allocated.
//!
//! # Algorithm
//!
//! The analysis is a single forward pass over the bytecode that tracks the escape
//! status of each register. When an allocated object's register is used in a way
//! that causes it to escape (return, field store, array store, function call, send),
//! its status is upgraded. The analysis is conservative: if in doubt, it marks as
//! GlobalEscape.

use crate::bytecode::{Instruction, OpCode};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// EscapeStatus
// ---------------------------------------------------------------------------

/// The escape status of a heap-allocated object.
///
/// This enum represents the three levels of escape that the analysis tracks,
/// from most constrained (NoEscape) to least constrained (GlobalEscape).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EscapeStatus {
    /// The object does not escape — it is only used within its allocating function.
    /// Can be stack-allocated.
    NoEscape,
    /// The object escapes through an argument to a function call (but not globally).
    /// Limited optimization possible.
    ArgEscape,
    /// The object escapes globally — stored in heap, returned, or passed to unknown code.
    /// Must be heap-allocated.
    GlobalEscape,
}

impl EscapeStatus {
    /// Upgrade this status to another status, taking the more conservative of the two.
    ///
    /// The ordering is: NoEscape < ArgEscape < GlobalEscape.
    pub fn upgrade(self, other: EscapeStatus) -> EscapeStatus {
        match (self, other) {
            (EscapeStatus::GlobalEscape, _) | (_, EscapeStatus::GlobalEscape) => {
                EscapeStatus::GlobalEscape
            }
            (EscapeStatus::ArgEscape, _) | (_, EscapeStatus::ArgEscape) => EscapeStatus::ArgEscape,
            _ => EscapeStatus::NoEscape,
        }
    }
}

// ---------------------------------------------------------------------------
// EscapeAnalysis — results container
// ---------------------------------------------------------------------------

/// Holds the results of analyzing a function's bytecode for object escape.
///
/// This structure is produced by `EscapeAnalyzer::analyze_function` and contains
/// per-register escape status, allocation site information, and summary counters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EscapeAnalysis {
    /// Maps register index → escape status for values allocated in that register.
    pub reg_escape: HashMap<u8, EscapeStatus>,
    /// Maps register index → the opcode offset where it was allocated.
    pub reg_alloc_site: HashMap<u8, usize>,
    /// Number of objects that can be stack-allocated (NoEscape count).
    pub stack_allocable: usize,
    /// Total number of allocations analyzed.
    pub total_allocs: usize,
}

impl EscapeAnalysis {
    /// Create a new empty `EscapeAnalysis`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Finalize the analysis by computing summary counters from the per-register state.
    fn finalize(&mut self) {
        self.total_allocs = self.reg_escape.len();
        self.stack_allocable = self
            .reg_escape
            .values()
            .filter(|&&s| s == EscapeStatus::NoEscape)
            .count();
    }
}

// ---------------------------------------------------------------------------
// EscapeAnalyzer — the analysis engine
// ---------------------------------------------------------------------------

/// The bytecode-level escape analysis engine.
///
/// `EscapeAnalyzer` performs a forward dataflow analysis over a slice of
/// `Instruction`s to determine the escape status of each heap allocation.
///
/// # Example
///
/// ```
/// use nulang::bytecode::{Instruction, OpCode};
/// use nulang::escape_analysis::EscapeAnalyzer;
///
/// let code = vec![
///     Instruction::new2(OpCode::RecMk, 0, 0),   // r0 = new record
///     Instruction::new2(OpCode::RetVal, 0, 0),   // return r0 → escapes!
/// ];
///
/// let analyzer = EscapeAnalyzer::new();
/// let result = analyzer.analyze_function(&code);
/// assert_eq!(result.total_allocs, 1);
/// ```
pub struct EscapeAnalyzer {
    /// Current escape status per *allocation-site* register.
    ///
    /// Keys are the original registers where allocations occurred.
    /// Values are the current (most conservative) escape status.
    /// This map is NOT affected by Move/Dup/Swap — allocation sites are stable.
    reg_status: HashMap<u8, EscapeStatus>,
    /// Register aliases: dst → src (Move/Dup tracking).
    ///
    /// When a register `dst` is copied from `src`, it aliases `src`.
    /// All tracked allocations in `src` are also considered live in `dst`.
    reg_aliases: HashMap<u8, u8>,
    /// Maps *current* register → *allocation-site* register.
    ///
    /// This tracks where each live register's value was originally allocated,
    /// even after swaps. For directly allocated values, current == origin.
    alloc_origin: HashMap<u8, u8>,
}

impl Default for EscapeAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl EscapeAnalyzer {
    /// Create a new `EscapeAnalyzer` with empty tracking state.
    pub fn new() -> Self {
        EscapeAnalyzer {
            reg_status: HashMap::new(),
            reg_aliases: HashMap::new(),
            alloc_origin: HashMap::new(),
        }
    }

    // ------------------------------------------------------------------
    // Core analysis entry point
    // ------------------------------------------------------------------

    /// Analyze a function's bytecode to determine escape status of allocations.
    ///
    /// This is the main entry point. It resets internal state, performs a single
    /// forward pass over the instructions, and returns the collected results.
    ///
    /// # Arguments
    ///
    /// * `instructions` — The bytecode of the function to analyze.
    ///
    /// # Returns
    ///
    /// An `EscapeAnalysis` containing per-register escape status and summary counters.
    pub fn analyze_function(mut self, instructions: &[Instruction]) -> EscapeAnalysis {
        self.reg_status.clear();
        self.reg_aliases.clear();
        self.alloc_origin.clear();

        let mut result = EscapeAnalysis::new();

        for (offset, instr) in instructions.iter().enumerate() {
            self.process_instruction(instr, offset, &mut result);
        }

        result.finalize();
        result
    }

    /// Borrowing variant — analyzes without consuming `self`.
    ///
    /// This is useful when the analyzer needs to be reused across multiple functions.
    pub fn analyze_function_borrowed(&mut self, instructions: &[Instruction]) -> EscapeAnalysis {
        self.reg_status.clear();
        self.reg_aliases.clear();
        self.alloc_origin.clear();

        let mut result = EscapeAnalysis::new();

        for (offset, instr) in instructions.iter().enumerate() {
            self.process_instruction(instr, offset, &mut result);
        }

        result.finalize();
        result
    }

    // ------------------------------------------------------------------
    // Per-instruction processing
    // ------------------------------------------------------------------

    /// Process a single instruction, updating tracking state and results.
    fn process_instruction(
        &mut self,
        instr: &Instruction,
        offset: usize,
        result: &mut EscapeAnalysis,
    ) {
        use OpCode::*;

        match instr.opcode {
            // -- Allocation opcodes ------------------------------------
            Alloc | RecMk | TupleMk | ArrAlloc | Closure => {
                let dst_reg = instr.op2;
                self.reg_status.insert(dst_reg, EscapeStatus::NoEscape);
                self.reg_aliases.remove(&dst_reg); // dst is freshly defined
                self.alloc_origin.insert(dst_reg, dst_reg); // alloc site = itself
                result.reg_escape.insert(dst_reg, EscapeStatus::NoEscape);
                result.reg_alloc_site.insert(dst_reg, offset);
            }

            // -- Register moves & duplication ---------------------------
            Move => {
                let src = instr.op1;
                let dst = instr.op2;
                self.propagate_register(src, dst);
            }
            Dup => {
                let src = instr.op1;
                let dst = instr.op2;
                self.propagate_register(src, dst);
            }

            // -- Field store (object.field = value) --------------------
            // The *value* (op3) being stored escapes into the heap object.
            // The container object (op1) does not escape.
            FieldS | RecS => {
                let _obj_reg = instr.op1;
                let _field_idx = instr.op2;
                let val_reg = instr.op3;
                self.mark_escape_if_tracked(val_reg, EscapeStatus::GlobalEscape, result);
            }

            // -- Array store (arr[idx] = value) ------------------------
            ArrStore => {
                let _arr_reg = instr.op1;
                let _idx_reg = instr.op2;
                let src_reg = instr.op3;
                self.mark_escape_if_tracked(src_reg, EscapeStatus::GlobalEscape, result);
            }

            // -- Function calls ----------------------------------------
            // Conservative: any tracked argument register gets ArgEscape.
            Call => {
                let func_reg = instr.op1;
                let arg_reg = instr.op2;
                let _dst_reg = instr.op3;
                // Mark function register (could be a closure object)
                self.mark_escape_if_tracked(func_reg, EscapeStatus::ArgEscape, result);
                // Mark argument register
                self.mark_escape_if_tracked(arg_reg, EscapeStatus::ArgEscape, result);
                // Result register gets whatever status the call produces (unknown → conservative)
                // We do not track the destination as an allocation here.
            }

            ClosureCall => {
                let closure_reg = instr.op1;
                let arg_reg = instr.op2;
                let _dst_reg = instr.op3;
                self.mark_escape_if_tracked(closure_reg, EscapeStatus::ArgEscape, result);
                self.mark_escape_if_tracked(arg_reg, EscapeStatus::ArgEscape, result);
            }

            TailCall => {
                let func_reg = instr.op1;
                let arg_reg = instr.op2;
                // Tail call arguments escape to the called function.
                self.mark_escape_if_tracked(func_reg, EscapeStatus::ArgEscape, result);
                self.mark_escape_if_tracked(arg_reg, EscapeStatus::ArgEscape, result);
            }

            // -- Returns (object returned to caller) --------------------
            Ret | RetVal => {
                let result_reg = instr.op1;
                self.mark_escape_if_tracked(result_reg, EscapeStatus::GlobalEscape, result);
            }

            // -- Actor send (message escapes to another actor) ----------
            Send => {
                let _target_reg = instr.op1;
                let msg_reg = instr.op2;
                self.mark_escape_if_tracked(msg_reg, EscapeStatus::GlobalEscape, result);
            }

            // -- Spawn (actor creation — init value may escape) ---------
            Spawn => {
                let _behavior_idx = instr.op1;
                let init_reg = instr.op2;
                // The init value is passed to the new actor's behavior.
                self.mark_escape_if_tracked(init_reg, EscapeStatus::GlobalEscape, result);
            }

            // -- Capture store (value escapes into closure environment) --
            // CapStore: op1=closure_reg, op2=idx, op3=val_reg
            // The value being stored escapes into the closure's capture env.
            CapStore => {
                let _closure_reg = instr.op1;
                let _idx = instr.op2;
                let val_reg = instr.op3;
                self.mark_escape_if_tracked(val_reg, EscapeStatus::GlobalEscape, result);
            }

            // -- Ask (request-response — argument escapes) --------------
            Ask => {
                let _addr_reg = instr.op1;
                let _behavior_id = instr.op2;
                // Ask sends a request; arguments escape.
                self.mark_escape_if_tracked(instr.op1, EscapeStatus::GlobalEscape, result);
            }

            // -- Remote send (RSend) — value escapes across nodes -------
            RSend => {
                let _addr_reg = instr.op1;
                self.mark_escape_if_tracked(instr.op2, EscapeStatus::GlobalEscape, result);
            }

            // -- Remote spawn (RSpawn) — init escapes -------------------
            RSpawn => {
                let _node_id = instr.op1;
                let init_reg = instr.op2;
                self.mark_escape_if_tracked(init_reg, EscapeStatus::GlobalEscape, result);
            }

            // -- Remote ask (RAsk) — argument escapes -------------------
            RAsk => {
                self.mark_escape_if_tracked(instr.op1, EscapeStatus::GlobalEscape, result);
            }

            // -- Copy (deep copy) — the source escapes into the copy ----
            Copy => {
                let _ref_cap = instr.op1;
                let src = instr.op2;
                let _dst = instr.op3;
                self.mark_escape_if_tracked(src, EscapeStatus::GlobalEscape, result);
            }

            // -- Drop — the object is explicitly deallocated ------------
            // This does not change escape status; the object is already being dropped.
            Drop => {
                // No escape implication — dropping is a local operation.
            }

            // -- Field loads / array loads / reads ----------------------
            // Reading from an object does not cause escape of the *source* object.
            // The destination register may receive an escaped object, but we do not
            // track values *from outside* the function.
            FieldL | RecL | ArrLoad | TupleL | CapLoad | IsTag => {
                // These all use op3 as destination.
                // Remove any alias/origin for dst; it's now an unknown value.
                self.reg_aliases.remove(&instr.op3);
                self.alloc_origin.remove(&instr.op3);
            }
            ArrLen | Unpack => {
                // These use op2 as destination.
                self.reg_aliases.remove(&instr.op2);
                self.alloc_origin.remove(&instr.op2);
            }

            // -- Store to local register --------------------------------
            // Store copies a value into a local. The source may escape if the
            // destination is later used in an escaping context.
            Store => {
                let src = instr.op1;
                let dst = instr.op2;
                self.propagate_register(src, dst);
            }

            // -- Load from local register -------------------------------
            // Load copies from local to register. Track it as an alias.
            Load => {
                let src = instr.op1;
                let dst = instr.op2;
                self.propagate_register(src, dst);
            }

            // -- Swap — exchange two registers --------------------------
            Swap => {
                let r1 = instr.op1;
                let r2 = instr.op2;
                self.swap_registers(r1, r2);
            }

            // -- Pop — pops a value from the call stack -----------------
            Pop => {
                let dst = instr.op1;
                // Popped value comes from caller; we don't track it.
                self.reg_aliases.remove(&dst);
                self.alloc_origin.remove(&dst);
            }

            // -- CapUp / CapDown (capability changes) -------------------
            // The register's value may be modified in capability.
            // Capability changes don't inherently cause escape,
            // but CapDown to box might indicate aliasing.
            // Conservative: no escape status change.
            CapUp | CapDown => {}

            // -- CapSend — marks value as sendable across actors --------
            CapSend => {
                let reg = instr.op1;
                // CapSend implies the value is about to be sent.
                self.mark_escape_if_tracked(reg, EscapeStatus::GlobalEscape, result);
            }

            // -- Perform (effect operation) — arguments may escape ------
            Perform => {
                let _eff_id = instr.op1;
                let _op_id = instr.op2;
                // Effect operations may escape their arguments.
                self.mark_escape_if_tracked(instr.op3, EscapeStatus::ArgEscape, result);
            }

            // -- Resume (effect handler resume) — value escapes to continuation
            Resume => {
                let val_reg = instr.op1;
                self.mark_escape_if_tracked(val_reg, EscapeStatus::GlobalEscape, result);
            }

            // -- Handle — install effect handler ------------------------
            // The handler table index is a constant, not a value reference.
            Handle => {
                // No tracked values involved.
            }

            // -- Unwind — unwind effect handler -------------------------
            Unwind => {
                // No tracked values involved.
            }

            // -- Actor self — just a constant, no escape ---------------
            SelfOp => {
                // No tracked values involved.
            }

            // -- Receive — receive a message ----------------------------
            Receive => {
                // Receiving a message does not cause escape of tracked objects.
            }

            // -- Monitor / Demonitor / Link / Unlink / Exit -------------
            Monitor | Demon | Link | Unlink | Exit => {
                let target_reg = instr.op1;
                self.mark_escape_if_tracked(target_reg, EscapeStatus::GlobalEscape, result);
            }

            // -- Yield — just a scheduling hint ------------------------
            Yield => {
                // No effect on escape analysis.
            }

            // -- Migrate — actor migration causes escape ----------------
            Migrate => {
                let addr_reg = instr.op1;
                self.mark_escape_if_tracked(addr_reg, EscapeStatus::GlobalEscape, result);
            }

            // -- Gossip — cluster state gossip --------------------------
            Gossip => {
                // No tracked values.
            }

            // -- NodeId — just a constant --------------------------------
            NodeId => {
                // No tracked values.
            }

            // -- Arithmetic, comparison, logic — local operations -------
            // These operate on registers but do not cause escape of
            // tracked heap objects (they may read them but not store/return).
            IAdd | ISub | IMul | IDiv | IMod | INeg | IInc | IDec | IPow | IToF
            | FAdd | FSub | FMul | FDiv | FNeg | FMod | FToI | FToS | ICmpEq
            | ICmpLt | ICmpGt | ICmpLe | ICmpGe | FCmpEq | FCmpLt | FCmpGt
            | SCmpEq | Not | And | Or | SConcat | Const0 | Const1 | Const2
            | ConstM1 | ConstU | ConstL => {
                // Pure local operations — no escape.
            }

            // -- Nop / Halt / Panic / Debugger breakpoints --------------
            Nop | Halt | Panic | DbgBreak | DbgPrint | DbgStack => {
                // No effect on escape analysis.
            }

            // -- String & IO operations --------------------------------
            // Printing and file operations may cause escape through external effects.
            SPrint | SRead | FOpen | FRead | FWrite | FClose | Print => {
                // IO operations could be considered as escape through side effects.
                // For simplicity, we conservatively consider the printed value as escaping.
                match instr.opcode {
                    SPrint | Print => {
                        let val_reg = instr.op1;
                        self.mark_escape_if_tracked(val_reg, EscapeStatus::GlobalEscape, result);
                    }
                    FWrite => {
                        let val_reg = instr.op2;
                        self.mark_escape_if_tracked(val_reg, EscapeStatus::GlobalEscape, result);
                    }
                    _ => {}
                }
            }

            // -- Metadata operations ------------------------------------
            MetaType | MetaCap => {
                // These read type/capability metadata — no escape of the value itself.
            }

            // -- Tuple field store (RecS covers this too) ---------------
            // Already handled above in FieldS/RecS.

            // -- Jump instructions — control flow does not affect escape --
            Jmp | JmpT | JmpF | Switch => {
                // Control flow is handled by the single-pass analysis.
                // For a more precise analysis, a full CFG-based dataflow
                // analysis would be needed. For MVP, the single forward
                // pass is sufficient and conservative.
            }

            // -- Free variable capture declaration ----------------------
            // FreeVar declares a variable that will be captured by a closure.
            // This is a declaration, not an operation that causes escape.
            FreeVar => {
                // No escape effect at declaration time.
            }

            // -- Capability check — no escape effect --------------------
            CapChk => {
                // No tracked values involved in escape.
            }
        }
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Mark a register as escaping with the given status, resolving aliases.
    ///
    /// If the register itself is tracked, it is updated directly.
    /// If the register aliases a tracked source, the source is updated.
    /// Uses `alloc_origin` to ensure `result.reg_escape` is keyed by the
    /// original allocation-site register.
    fn mark_escape_if_tracked(
        &mut self,
        reg: u8,
        status: EscapeStatus,
        result: &mut EscapeAnalysis,
    ) {
        // Resolve aliases: if `reg` is an alias of `src`, update `src`.
        let target = self.resolve_alias(reg);

        if let Some(current) = self.reg_status.get_mut(&target) {
            *current = current.upgrade(status);
            // Find the original allocation register for consistent result keys.
            let origin = self.alloc_origin.get(&target).copied()
                .unwrap_or(target);
            result.reg_escape.insert(origin, *current);
        }
    }

    /// Propagate the escape status from `src` to `dst` (for Move, Dup, Load, Store).
    ///
    /// If `src` tracks an allocation, `dst` becomes an alias of `src` and
    /// inherits its current escape status. If `dst` previously tracked its own
    /// allocation, that tracking is removed (the register has been overwritten).
    fn propagate_register(&mut self, src: u8, dst: u8) {
        // dst is being overwritten — remove any previous origin for it.
        self.reg_aliases.remove(&dst);
        self.alloc_origin.remove(&dst);

        let resolved_src = self.resolve_alias(src);

        if self.reg_status.contains_key(&resolved_src) {
            // dst now aliases the source register's allocation.
            self.reg_aliases.insert(dst, resolved_src);
            // dst's origin is the same as src's origin (or src itself).
            let origin = self.alloc_origin.get(&resolved_src).copied()
                .unwrap_or(resolved_src);
            self.alloc_origin.insert(dst, origin);
        }
        // else: source is not tracked — dst is now an unknown value.
    }

    /// Swap the contents of two registers.
    ///
    /// After `swap r1, r2`, r1 holds what r2 held and vice versa.
    /// We swap the `alloc_origin` entries and `reg_aliases` entries,
    /// but `reg_status` is NOT swapped — it is keyed by stable
    /// allocation-site registers.
    fn swap_registers(&mut self, r1: u8, r2: u8) {
        // Swap alloc_origin entries
        let o1 = self.alloc_origin.remove(&r1);
        let o2 = self.alloc_origin.remove(&r2);
        if let Some(o) = o2 {
            self.alloc_origin.insert(r1, o);
        }
        if let Some(o) = o1 {
            self.alloc_origin.insert(r2, o);
        }

        // Swap direct alias entries for r1 and r2
        let alias_r1 = self.reg_aliases.remove(&r1);
        let alias_r2 = self.reg_aliases.remove(&r2);
        if let Some(a) = alias_r2 {
            self.reg_aliases.insert(r1, a);
        }
        if let Some(a) = alias_r1 {
            self.reg_aliases.insert(r2, a);
        }

        // Update any other aliases that point TO r1 or r2
        let keys_to_update: Vec<u8> = self
            .reg_aliases
            .iter()
            .filter(|(_, &v)| v == r1 || v == r2)
            .map(|(&k, _)| k)
            .collect();

        for k in keys_to_update {
            if let Some(v) = self.reg_aliases.get(&k).copied() {
                let new_v = if v == r1 { r2 } else { r1 };
                self.reg_aliases.insert(k, new_v);
            }
        }
    }

    /// Resolve a register through the alias chain to find the root tracked register.
    ///
    /// Uses path compression for efficiency.
    fn resolve_alias(&mut self, reg: u8) -> u8 {
        let mut current = reg;
        let mut chain = Vec::new();

        // Follow the alias chain
        while let Some(&next) = self.reg_aliases.get(&current) {
            if next == current {
                break;
            }
            chain.push(current);
            current = next;
            // Prevent infinite loops
            if chain.len() > 100 {
                break;
            }
        }

        // Path compression: point all intermediate nodes directly to root
        for node in chain {
            self.reg_aliases.insert(node, current);
        }

        current
    }
}

// ---------------------------------------------------------------------------
// analyze_region — analyze a subset of instructions
// ---------------------------------------------------------------------------

/// Analyze a contiguous region of bytecode for escape analysis.
///
/// This function is designed for JIT compilation, where only a hot region
/// (trace or basic block) needs to be analyzed rather than the full function.
///
/// # Arguments
///
/// * `instructions` — The full instruction array.
/// * `start_offset` — The starting index in `instructions`.
/// * `num_instrs` — The number of instructions to analyze from the start.
///
/// # Returns
///
/// An `EscapeAnalysis` for the specified region.
///
/// # Example
///
/// ```
/// use nulang::bytecode::{Instruction, OpCode};
/// use nulang::escape_analysis::analyze_region;
///
/// let code = vec![
///     Instruction::new2(OpCode::RecMk, 0, 0),   // r0 = new record
///     Instruction::new2(OpCode::RetVal, 0, 0),   // return r0
/// ];
///
/// let region = analyze_region(&code, 0, 2);
/// assert_eq!(region.total_allocs, 1);
/// ```
pub fn analyze_region(
    instructions: &[Instruction],
    start_offset: usize,
    num_instrs: usize,
) -> EscapeAnalysis {
    let end = (start_offset + num_instrs).min(instructions.len());
    let region = &instructions[start_offset..end];

    let analyzer = EscapeAnalyzer::new();
    analyzer.analyze_function(region)
}

// ---------------------------------------------------------------------------
// Helper: is_allocation_opcode
// ---------------------------------------------------------------------------

/// Check whether an opcode allocates a new heap object.
///
/// Returns `true` for opcodes that create new heap-allocated values:
/// `Alloc`, `RecMk`, `TupleMk`, `ArrAlloc`, and `Closure`.
///
/// # Example
///
/// ```
/// use nulang::bytecode::OpCode;
/// use nulang::escape_analysis::is_allocation_opcode;
///
/// assert!(is_allocation_opcode(OpCode::RecMk));
/// assert!(is_allocation_opcode(OpCode::TupleMk));
/// assert!(!is_allocation_opcode(OpCode::Move));
/// ```
pub fn is_allocation_opcode(opcode: OpCode) -> bool {
    matches!(
        opcode,
        OpCode::Alloc | OpCode::RecMk | OpCode::TupleMk | OpCode::ArrAlloc | OpCode::Closure
    )
}

// ---------------------------------------------------------------------------
// Helper: can_stack_alloc
// ---------------------------------------------------------------------------

/// Check whether an escape status permits stack allocation.
///
/// Only `EscapeStatus::NoEscape` objects can be safely stack-allocated.
/// `ArgEscape` and `GlobalEscape` objects must remain heap-allocated.
///
/// # Example
///
/// ```
/// use nulang::escape_analysis::{EscapeStatus, can_stack_alloc};
///
/// assert!(can_stack_alloc(EscapeStatus::NoEscape));
/// assert!(!can_stack_alloc(EscapeStatus::ArgEscape));
/// assert!(!can_stack_alloc(EscapeStatus::GlobalEscape));
/// ```
pub fn can_stack_alloc(status: EscapeStatus) -> bool {
    status == EscapeStatus::NoEscape
}

// ---------------------------------------------------------------------------
// Helper: summarize_analysis
// ---------------------------------------------------------------------------

/// Produce a human-readable summary of an `EscapeAnalysis`.
///
/// Useful for debugging and JIT compiler logging.
pub fn summarize_analysis(analysis: &EscapeAnalysis) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "EscapeAnalysis: {}/{} stack-allocable ({}%)",
        analysis.stack_allocable,
        analysis.total_allocs,
        if analysis.total_allocs > 0 {
            (analysis.stack_allocable * 100) / analysis.total_allocs
        } else {
            0
        }
    ));

    for (reg, status) in &analysis.reg_escape {
        let site = analysis
            .reg_alloc_site
            .get(reg)
            .map(|s| s.to_string())
            .unwrap_or_else(|| "?".to_string());
        let indicator = match status {
            EscapeStatus::NoEscape => "STACK",
            EscapeStatus::ArgEscape => "ARG  ",
            EscapeStatus::GlobalEscape => "HEAP ",
        };
        lines.push(format!("  r{} @ instr {} → {} ({:?})", reg, site, indicator, status));
    }

    lines.join("\n")
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod escape_analysis_tests {
    use super::*;
    use crate::bytecode::{Instruction, OpCode};

    // -- Test helpers --------------------------------------------------------

    /// Build a single instruction with opcode and up to 3 operands.
    fn i0(op: OpCode) -> Instruction {
        Instruction::new0(op)
    }
    fn i1(op: OpCode, a: u8) -> Instruction {
        Instruction::new1(op, a)
    }
    fn i2(op: OpCode, a: u8, b: u8) -> Instruction {
        Instruction::new2(op, a, b)
    }
    fn i3(op: OpCode, a: u8, b: u8, c: u8) -> Instruction {
        Instruction::new3(op, a, b, c)
    }

    // -- 1. Local-only use → NoEscape ---------------------------------------

    #[test]
    fn test_no_escape_local_use() {
        // r0 = new record
        // r1 = field_load r0[0]  (reading r0 — local only)
        // ret
        let code = vec![
            i2(OpCode::RecMk, 2, 0), // r0 = new record (2 fields)
            i3(OpCode::RecL, 0, 0, 1), // r1 = r0.field_0 (read only)
            i1(OpCode::Ret, 0),      // return (not returning r0)
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 1);
        assert_eq!(result.stack_allocable, 1);
        assert_eq!(result.reg_escape.get(&0), Some(&EscapeStatus::NoEscape));
    }

    // -- 2. Object returned → GlobalEscape ----------------------------------

    #[test]
    fn test_escape_via_return() {
        // r0 = new record
        // ret r0
        let code = vec![
            i2(OpCode::RecMk, 2, 0), // r0 = new record
            i1(OpCode::RetVal, 0),   // return r0 → escapes!
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 1);
        assert_eq!(result.stack_allocable, 0);
        assert_eq!(
            result.reg_escape.get(&0),
            Some(&EscapeStatus::GlobalEscape)
        );
    }

    // -- 3. Object stored in another's field → GlobalEscape -----------------

    #[test]
    fn test_escape_via_field_set() {
        // r0 = new record        (the container — local)
        // r1 = new record        (the value — stored into r0's field → escapes)
        // r0.field_0 = r1
        // ret
        let code = vec![
            i2(OpCode::RecMk, 2, 0), // r0 = new record (container)
            i2(OpCode::RecMk, 2, 1), // r1 = new record (value to store)
            i3(OpCode::RecS, 0, 0, 1), // r0.field_0 = r1 → r1 escapes!
            i1(OpCode::Ret, 0),      // return
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 2);
        assert_eq!(result.stack_allocable, 1); // only r0 is stack-allocable
        assert_eq!(result.reg_escape.get(&0), Some(&EscapeStatus::NoEscape));
        assert_eq!(
            result.reg_escape.get(&1),
            Some(&EscapeStatus::GlobalEscape)
        );
    }

    // -- 4. Object stored in array → GlobalEscape ---------------------------

    #[test]
    fn test_escape_via_array_store() {
        // r0 = new array
        // r1 = new record        (to be stored in array)
        // r0[idx] = r1           → r1 escapes into heap
        // ret
        let code = vec![
            i2(OpCode::ArrAlloc, 0, 0), // r0 = new array (size in r0... use r0 as size for test)
            // Actually we need a size. Let's use r5 for size, r0 for array.
        ];
        // Redo with better register assignment:
        let code = vec![
            i2(OpCode::ArrAlloc, 5, 0), // r0 = new array (size from r5)
            i2(OpCode::TupleMk, 2, 1), // r1 = new tuple
            i3(OpCode::ArrStore, 0, 2, 1), // r0[r2] = r1 → r1 escapes!
            i1(OpCode::Ret, 0),         // return
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 2);
        assert_eq!(result.stack_allocable, 1); // only the array (r0) is stack-local
        assert_eq!(result.reg_escape.get(&1), Some(&EscapeStatus::GlobalEscape));
    }

    // -- 5. Object sent to actor → GlobalEscape -----------------------------

    #[test]
    fn test_escape_via_send() {
        // r0 = new record
        // send target=r1, msg=r0   → r0 escapes to another actor
        // ret
        let code = vec![
            i2(OpCode::RecMk, 2, 0), // r0 = new record
            i2(OpCode::Send, 1, 0),  // send r0 to actor at r1 → r0 escapes!
            i1(OpCode::Ret, 0),      // return
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 1);
        assert_eq!(result.stack_allocable, 0);
        assert_eq!(
            result.reg_escape.get(&0),
            Some(&EscapeStatus::GlobalEscape)
        );
    }

    // -- 6. Object passed to function call → ArgEscape ----------------------

    #[test]
    fn test_escape_via_call() {
        // r0 = new record
        // call func=r2, arg=r0, dst=r3  → r0 escapes as argument
        // ret
        let code = vec![
            i2(OpCode::RecMk, 2, 0), // r0 = new record
            i3(OpCode::Call, 2, 0, 3), // call r2(r0), result → r3 → r0 ArgEscapes!
            i1(OpCode::Ret, 0),      // return
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 1);
        assert_eq!(result.stack_allocable, 0);
        assert_eq!(result.reg_escape.get(&0), Some(&EscapeStatus::ArgEscape));
    }

    // -- 7. Object created, read, discarded → NoEscape ----------------------

    #[test]
    fn test_no_escape_pure_local() {
        // r0 = new record
        // r1 = load r0.field_0    (read)
        // r2 = r0 + r1           (not a real opcode; use Move to show local use)
        // ret (not returning r0)
        let code = vec![
            i2(OpCode::RecMk, 2, 0), // r0 = new record
            i3(OpCode::RecL, 0, 0, 1), // r1 = r0.field_0 (read, no escape)
            i2(OpCode::Move, 0, 2),  // r2 = r0 (alias, still local)
            i1(OpCode::Ret, 9),      // return something else entirely
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 1);
        assert_eq!(result.stack_allocable, 1);
        assert_eq!(result.reg_escape.get(&0), Some(&EscapeStatus::NoEscape));
    }

    // -- 8. Move/Dup correctly tracks aliases -------------------------------

    #[test]
    fn test_alias_tracking() {
        // r0 = new record
        // r1 = r0               (Move — r1 aliases r0)
        // ret r1                → r0 (via alias) escapes!
        let code = vec![
            i2(OpCode::RecMk, 2, 0), // r0 = new record
            i2(OpCode::Move, 0, 1),  // r1 = r0 (alias)
            i1(OpCode::RetVal, 1),   // return r1 → r0 escapes through alias!
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 1);
        assert_eq!(result.stack_allocable, 0);
        assert_eq!(
            result.reg_escape.get(&0),
            Some(&EscapeStatus::GlobalEscape)
        );
        // r1 is not a separate allocation, just an alias
        assert!(!result.reg_escape.contains_key(&1));
    }

    // -- 9. Multiple allocations with different fates -----------------------

    #[test]
    fn test_multiple_allocations() {
        // r0 = new record        (local, never escapes)
        // r1 = new tuple         (returned, escapes)
        // r2 = new array         (local)
        // r3 = new record        (sent, escapes)
        // ret r1
        let code = vec![
            i2(OpCode::RecMk, 2, 0), // r0 = new record (local)
            i2(OpCode::TupleMk, 3, 1), // r1 = new tuple (will escape)
            i2(OpCode::ArrAlloc, 5, 2), // r2 = new array (local)
            i2(OpCode::RecMk, 1, 3), // r3 = new record (will escape via send)
            i2(OpCode::Send, 4, 3),  // send r3 → r3 escapes!
            i1(OpCode::RetVal, 1),   // return r1 → r1 escapes!
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 4);
        assert_eq!(result.stack_allocable, 2); // r0 and r2
        assert_eq!(result.reg_escape.get(&0), Some(&EscapeStatus::NoEscape));
        assert_eq!(
            result.reg_escape.get(&1),
            Some(&EscapeStatus::GlobalEscape)
        );
        assert_eq!(result.reg_escape.get(&2), Some(&EscapeStatus::NoEscape));
        assert_eq!(
            result.reg_escape.get(&3),
            Some(&EscapeStatus::GlobalEscape)
        );
    }

    // -- 10. Mixed escape — some escape, some don't -------------------------

    #[test]
    fn test_mixed_escape() {
        // r0 = new record        (local)
        // r1 = new record        (field-stored → global escape)
        // r2 = new tuple         (passed to call → arg escape)
        // r3 = new array         (local)
        // r0.field_0 = r1        → r1 global escape
        // call func=r4, arg=r2   → r2 arg escape
        // ret (no return value)
        let code = vec![
            i2(OpCode::RecMk, 2, 0), // r0 = container record
            i2(OpCode::RecMk, 2, 1), // r1 = value record
            i2(OpCode::TupleMk, 2, 2), // r2 = tuple
            i2(OpCode::ArrAlloc, 5, 3), // r3 = array (local)
            i3(OpCode::RecS, 0, 0, 1), // r0.field_0 = r1 → r1 global escape
            i3(OpCode::Call, 4, 2, 5), // call r4(r2) → r2 arg escape
            i1(OpCode::Ret, 0),      // return (nothing)
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 4);
        assert_eq!(result.stack_allocable, 2); // r0 and r3
        assert_eq!(result.reg_escape.get(&0), Some(&EscapeStatus::NoEscape));
        assert_eq!(
            result.reg_escape.get(&1),
            Some(&EscapeStatus::GlobalEscape)
        );
        assert_eq!(result.reg_escape.get(&2), Some(&EscapeStatus::ArgEscape));
        assert_eq!(result.reg_escape.get(&3), Some(&EscapeStatus::NoEscape));
    }

    // -- 11. Empty function — no allocations --------------------------------

    #[test]
    fn test_empty_function() {
        let code: Vec<Instruction> = vec![i1(OpCode::Ret, 0)];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 0);
        assert_eq!(result.stack_allocable, 0);
        assert!(result.reg_escape.is_empty());
        assert!(result.reg_alloc_site.is_empty());
    }

    // -- 12. Closure creation analyzed correctly ----------------------------

    #[test]
    fn test_closure_creation() {
        // r0 = new closure       (local, not returned)
        // r1 = load r0.capture[0] (read from closure)
        // ret (not returning r0)
        let code = vec![
            i2(OpCode::Closure, 0, 0), // r0 = new closure (func_idx=0)
            i3(OpCode::CapLoad, 0, 0, 1), // r1 = r0.capture[0]
            i1(OpCode::Ret, 0),        // return
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 1);
        assert_eq!(result.stack_allocable, 1);
        assert_eq!(result.reg_escape.get(&0), Some(&EscapeStatus::NoEscape));
    }

    // -- 13. Closure that escapes via return --------------------------------

    #[test]
    fn test_closure_escape_via_return() {
        // r0 = new closure
        // ret r0 → closure escapes
        let code = vec![
            i2(OpCode::Closure, 0, 0), // r0 = new closure
            i1(OpCode::RetVal, 0),     // return r0 → escapes!
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 1);
        assert_eq!(result.stack_allocable, 0);
        assert_eq!(
            result.reg_escape.get(&0),
            Some(&EscapeStatus::GlobalEscape)
        );
    }

    // -- 14. Tuple with field load only → NoEscape --------------------------

    #[test]
    fn test_tuple_no_escape() {
        // r0 = new tuple(3)
        // r1 = tuple_load r0[1]
        // ret (not returning r0)
        let code = vec![
            i2(OpCode::TupleMk, 3, 0), // r0 = new tuple of arity 3
            i3(OpCode::TupleL, 0, 1, 1), // r1 = r0[1]
            i1(OpCode::Ret, 0),        // return
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 1);
        assert_eq!(result.stack_allocable, 1);
        assert_eq!(result.reg_escape.get(&0), Some(&EscapeStatus::NoEscape));
    }

    // -- 15. Spawn causes escape --------------------------------------------

    #[test]
    fn test_spawn_escape() {
        // r0 = new record        (will be passed to spawned actor)
        // spawn behavior=0, init=r0 → r0 escapes
        // ret
        let code = vec![
            i2(OpCode::RecMk, 1, 0), // r0 = new record
            i2(OpCode::Spawn, 0, 0), // spawn actor with init=r0 → r0 escapes!
            i1(OpCode::Ret, 0),      // return
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 1);
        assert_eq!(result.stack_allocable, 0);
        assert_eq!(
            result.reg_escape.get(&0),
            Some(&EscapeStatus::GlobalEscape)
        );
    }

    // -- 16. analyze_region function ----------------------------------------

    #[test]
    fn test_analyze_region() {
        let code = vec![
            i2(OpCode::RecMk, 2, 0), // r0 = new record     (instr 0)
            i3(OpCode::RecS, 0, 0, 1), // r0.field = r1     (instr 1)
            i2(OpCode::TupleMk, 2, 2), // r2 = new tuple    (instr 2)
            i1(OpCode::RetVal, 2),   // return r2           (instr 3)
        ];

        // Analyze only instructions 2-3 (the tuple creation and return)
        let region = analyze_region(&code, 2, 2);

        assert_eq!(region.total_allocs, 1); // only the tuple
        assert_eq!(region.reg_escape.get(&2), Some(&EscapeStatus::GlobalEscape));
        // r0 was allocated before the region, so not counted here
        assert!(!region.reg_escape.contains_key(&0));
    }

    // -- 17. is_allocation_opcode helper ------------------------------------

    #[test]
    fn test_is_allocation_opcode() {
        assert!(is_allocation_opcode(OpCode::Alloc));
        assert!(is_allocation_opcode(OpCode::RecMk));
        assert!(is_allocation_opcode(OpCode::TupleMk));
        assert!(is_allocation_opcode(OpCode::ArrAlloc));
        assert!(is_allocation_opcode(OpCode::Closure));

        assert!(!is_allocation_opcode(OpCode::Move));
        assert!(!is_allocation_opcode(OpCode::Call));
        assert!(!is_allocation_opcode(OpCode::RetVal));
        assert!(!is_allocation_opcode(OpCode::IAdd));
        assert!(!is_allocation_opcode(OpCode::FieldS));
    }

    // -- 18. can_stack_alloc helper -----------------------------------------

    #[test]
    fn test_can_stack_alloc() {
        assert!(can_stack_alloc(EscapeStatus::NoEscape));
        assert!(!can_stack_alloc(EscapeStatus::ArgEscape));
        assert!(!can_stack_alloc(EscapeStatus::GlobalEscape));
    }

    // -- 19. Dup instruction alias tracking ---------------------------------

    #[test]
    fn test_dup_alias_tracking() {
        // r0 = new record
        // r1 = dup r0           (r1 aliases r0)
        // r2 = dup r1           (r2 aliases r0 through r1)
        // send target=r3, msg=r2 → r0 escapes through r2 alias chain
        let code = vec![
            i2(OpCode::RecMk, 2, 0), // r0 = new record
            i2(OpCode::Dup, 0, 1),   // r1 = dup r0
            i2(OpCode::Dup, 1, 2),   // r2 = dup r1
            i2(OpCode::Send, 3, 2),  // send r2 → r0 escapes through alias chain!
            i1(OpCode::Ret, 0),      // return
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 1);
        assert_eq!(result.stack_allocable, 0);
        assert_eq!(
            result.reg_escape.get(&0),
            Some(&EscapeStatus::GlobalEscape)
        );
    }

    // -- 20. EscapeStatus::upgrade ------------------------------------------

    #[test]
    fn test_escape_status_upgrade() {
        assert_eq!(
            EscapeStatus::NoEscape.upgrade(EscapeStatus::NoEscape),
            EscapeStatus::NoEscape
        );
        assert_eq!(
            EscapeStatus::NoEscape.upgrade(EscapeStatus::ArgEscape),
            EscapeStatus::ArgEscape
        );
        assert_eq!(
            EscapeStatus::ArgEscape.upgrade(EscapeStatus::NoEscape),
            EscapeStatus::ArgEscape
        );
        assert_eq!(
            EscapeStatus::ArgEscape.upgrade(EscapeStatus::GlobalEscape),
            EscapeStatus::GlobalEscape
        );
        assert_eq!(
            EscapeStatus::GlobalEscape.upgrade(EscapeStatus::NoEscape),
            EscapeStatus::GlobalEscape
        );
    }

    // -- 21. Borrowed analyzer reuse ----------------------------------------

    #[test]
    fn test_analyzer_reuse() {
        let mut analyzer = EscapeAnalyzer::new();

        // First function
        let code1 = vec![
            i2(OpCode::RecMk, 2, 0),
            i1(OpCode::RetVal, 0),
        ];
        let result1 = analyzer.analyze_function_borrowed(&code1);
        assert_eq!(result1.reg_escape.get(&0), Some(&EscapeStatus::GlobalEscape));

        // Second function — analyzer state should be fresh
        let code2 = vec![
            i2(OpCode::RecMk, 2, 0),
            i1(OpCode::Ret, 0),
        ];
        let result2 = analyzer.analyze_function_borrowed(&code2);
        assert_eq!(result2.reg_escape.get(&0), Some(&EscapeStatus::NoEscape));
    }

    // -- 22. Summarize analysis ---------------------------------------------

    #[test]
    fn test_summarize_analysis() {
        let analysis = EscapeAnalysis {
            reg_escape: {
                let mut m = HashMap::new();
                m.insert(0, EscapeStatus::NoEscape);
                m.insert(1, EscapeStatus::GlobalEscape);
                m
            },
            reg_alloc_site: {
                let mut m = HashMap::new();
                m.insert(0, 0);
                m.insert(1, 1);
                m
            },
            stack_allocable: 1,
            total_allocs: 2,
        };

        let summary = summarize_analysis(&analysis);
        assert!(summary.contains("1/2 stack-allocable"));
        assert!(summary.contains("r0"));
        assert!(summary.contains("r1"));
    }

    // -- 23. ClosureCall escape tracking ------------------------------------

    #[test]
    fn test_closure_call_escape() {
        // r0 = new closure
        // closure_call r0(r1) → r0 (the closure object) is the function being called
        // ret
        let code = vec![
            i2(OpCode::Closure, 0, 0),  // r0 = new closure
            i3(OpCode::ClosureCall, 0, 1, 2), // call r0(r1), result → r2
            i1(OpCode::Ret, 0),         // return
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        // The closure object (r0) is the function being called → ArgEscape
        assert_eq!(result.reg_escape.get(&0), Some(&EscapeStatus::ArgEscape));
    }

    // -- 24. TailCall escape tracking ---------------------------------------

    #[test]
    fn test_tail_call_escape() {
        // r0 = new record
        // tailcall func=r1, arg=r0 → r0 escapes as argument
        let code = vec![
            i2(OpCode::RecMk, 2, 0), // r0 = new record
            i2(OpCode::TailCall, 1, 0), // tailcall r1(r0) → r0 arg escapes!
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.reg_escape.get(&0), Some(&EscapeStatus::ArgEscape));
    }

    // -- 25. Swap register tracking -----------------------------------------

    #[test]
    fn test_swap_registers() {
        // r0 = new record        (tracked, NoEscape)
        // r1 = new tuple         (tracked, NoEscape)
        // swap r0, r1            (now r0 holds tuple, r1 holds record)
        // ret r1                 → the record (now in r1) escapes
        let code = vec![
            i2(OpCode::RecMk, 2, 0), // r0 = new record
            i2(OpCode::TupleMk, 2, 1), // r1 = new tuple
            i2(OpCode::Swap, 0, 1),  // swap r0, r1
            i1(OpCode::RetVal, 1),   // return r1 → the original record escapes
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 2);
        // After swap, the original record is in r1 which is returned → GlobalEscape
        assert_eq!(
            result.reg_escape.get(&0),
            Some(&EscapeStatus::GlobalEscape)
        );
    }

    // -- 26. CapStore causes escape -----------------------------------------

    #[test]
    fn test_capstore_escape() {
        // r0 = new record        (local value)
        // r1 = new closure       (container)
        // capstore r1[0] = r0    → r0 escapes into closure env
        // ret
        let code = vec![
            i2(OpCode::RecMk, 2, 0), // r0 = new record
            i2(OpCode::Closure, 0, 1), // r1 = new closure
            i3(OpCode::CapStore, 1, 0, 0), // r1.capture[0] = r0 → r0 escapes!
            i1(OpCode::Ret, 0),      // return
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 2);
        assert_eq!(result.reg_escape.get(&0), Some(&EscapeStatus::GlobalEscape));
    }

    // -- 27. Print causes escape through IO ---------------------------------

    #[test]
    fn test_print_escape() {
        // r0 = new record
        // print r0               → r0 escapes through IO
        // ret
        let code = vec![
            i2(OpCode::RecMk, 2, 0), // r0 = new record
            i1(OpCode::Print, 0),    // print r0 → r0 escapes!
            i1(OpCode::Ret, 0),      // return
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.total_allocs, 1);
        assert_eq!(result.stack_allocable, 0);
        assert_eq!(
            result.reg_escape.get(&0),
            Some(&EscapeStatus::GlobalEscape)
        );
    }

    // -- 28. Region bounds checking -----------------------------------------

    #[test]
    fn test_analyze_region_bounds() {
        let code = vec![
            i2(OpCode::RecMk, 2, 0), // instr 0
            i1(OpCode::Ret, 0),      // instr 1
        ];

        // Request more instructions than available
        let region = analyze_region(&code, 0, 100);
        assert_eq!(region.total_allocs, 1);

        // Start past the end
        let region = analyze_region(&code, 5, 2);
        assert_eq!(region.total_allocs, 0);
    }

    // -- 29. Default construction -------------------------------------------

    #[test]
    fn test_default_construction() {
        let analysis = EscapeAnalysis::new();
        assert_eq!(analysis.total_allocs, 0);
        assert_eq!(analysis.stack_allocable, 0);
        assert!(analysis.reg_escape.is_empty());
        assert!(analysis.reg_alloc_site.is_empty());

        let analyzer = EscapeAnalyzer::new();
        let _result = analyzer.analyze_function(&[]);

        let default_analyzer: EscapeAnalyzer = Default::default();
        let _result = default_analyzer.analyze_function(&[]);
    }

    // -- 30. Allocation site tracking ---------------------------------------

    #[test]
    fn test_allocation_site_tracking() {
        let code = vec![
            i1(OpCode::Nop, 0),        // instr 0
            i2(OpCode::RecMk, 2, 0), // instr 1: r0 allocated here
            i1(OpCode::Nop, 0),        // instr 2
            i2(OpCode::TupleMk, 2, 1), // instr 3: r1 allocated here
        ];

        let analyzer = EscapeAnalyzer::new();
        let result = analyzer.analyze_function(&code);

        assert_eq!(result.reg_alloc_site.get(&0), Some(&1));
        assert_eq!(result.reg_alloc_site.get(&1), Some(&3));
    }
}
