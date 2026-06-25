//! Cranelift JIT Backend for Nulang.
//!
//! Provides tiered execution: bytecode is first interpreted, and hot regions
//! are lazily compiled to native code via Cranelift.
//!
//! # Architecture
//!
//! - `JitSession`: Owns the Cranelift JIT module, tracks hot counters, and
//!   manages compiled function pointers.
//! - `compiler`: Translates a bytecode region to Cranelift IR (CLIF).
//! - `typed_compiler`: Type-aware JIT that strips NaN-tag guards when types
//!   are known from the typechecker.
//! - `runtime`: Runtime helper functions callable from JIT code for
//!   NaN-tag-aware operations.
//!
//! # JIT Function Signature
//!
//! All JIT-compiled functions have the same C ABI signature:
//! ```c
//! void nulang_jit_func(uint64_t* regs, const uint64_t* constants);
//! ```
//! - `regs`: pointer to 256 u64 register file (read/write)
//! - `constants`: pointer to the constants pool (read-only)

mod compiler;
pub mod typed_compiler;
pub mod runtime;

#[cfg(test)]
mod tests;

pub use compiler::*;
pub use typed_compiler::*;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use cranelift::prelude::*;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};

// ---------------------------------------------------------------------------
// Hot Counter
// ---------------------------------------------------------------------------

/// Threshold: how many times a bytecode region must be interpreted
/// before it becomes eligible for JIT compilation.
pub const HOT_THRESHOLD: u64 = 1000;

static HOT_COUNTERS: AtomicU64 = AtomicU64::new(0);
static mut HOT_COUNTER_MAP: Option<HashMap<usize, u64>> = None;

/// Record execution at offset. Returns true if region is hot.
pub fn record_and_check_hot(offset: usize) -> bool {
    let prev = HOT_COUNTERS.fetch_add(1, Ordering::Relaxed);
    if prev < HOT_THRESHOLD { return false; }
    unsafe {
        if HOT_COUNTER_MAP.is_none() { HOT_COUNTER_MAP = Some(HashMap::new()); }
        let map = HOT_COUNTER_MAP.as_mut().unwrap();
        let count = map.entry(offset).or_insert(0);
        *count += 1;
        *count >= HOT_THRESHOLD
    }
}

/// Reset hot counters.
pub fn reset_hot_counters() {
    HOT_COUNTERS.store(0, Ordering::Relaxed);
    unsafe { HOT_COUNTER_MAP = None; }
}

// ---------------------------------------------------------------------------
// JIT Session
// ---------------------------------------------------------------------------

/// Manages the Cranelift JIT compilation lifecycle.
pub struct JitSession {
    module: JITModule,
    compiled: HashMap<usize, *const u8>,
    builder_context: FunctionBuilderContext,
    ctx: codegen::Context,
}

impl JitSession {
    pub fn new() -> Self {
        let flag_builder = settings::builder();
        let isa_builder = cranelift_native::builder()
            .unwrap_or_else(|msg| panic!("host machine is not supported: {}", msg));
        let isa = isa_builder.finish(settings::Flags::new(flag_builder)).unwrap();
        let builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        let module = JITModule::new(builder);
        JitSession {
            module,
            compiled: HashMap::new(),
            builder_context: FunctionBuilderContext::new(),
            ctx: module.make_context(),
        }
    }

    pub unsafe fn compile_region(
        &mut self,
        module_idx: usize,
        start_offset: usize,
        num_instrs: usize,
        instructions: &[crate::bytecode::Instruction],
    ) -> Option<JitFunctionPtr> {
        if let Some(&ptr) = self.compiled.get(&start_offset) {
            return Some(std::mem::transmute(ptr));
        }
        let func_name = format!("nulang_jit_{}_{}", module_idx, start_offset);
        match compiler::compile_bytecode_region(
            &mut self.module, &mut self.builder_context, &mut self.ctx,
            &func_name, start_offset, num_instrs, instructions,
        ) {
            Ok(ptr) => { self.compiled.insert(start_offset, ptr); Some(std::mem::transmute(ptr)) }
            Err(_) => None,
        }
    }

    pub fn is_compiled(&self, offset: usize) -> bool { self.compiled.contains_key(&offset) }
    pub fn get_compiled(&self, offset: usize) -> Option<JitFunctionPtr> {
        self.compiled.get(&offset).map(|&ptr| unsafe { std::mem::transmute(ptr) })
    }
    pub fn compiled_count(&self) -> usize { self.compiled.len() }
}

impl Default for JitSession { fn default() -> Self { Self::new() } }

// ---------------------------------------------------------------------------
// JIT Function Type
// ---------------------------------------------------------------------------

pub type JitFunctionPtr = extern "C" fn(*mut u64, *const u64);

// ---------------------------------------------------------------------------
// Tiered Execution
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TieredAction { Interpret, RanJit, CompiledAndRan }

pub fn tiered_execute_step(
    jit: &mut JitSession, module_idx: usize, pc: usize,
    instructions: &[crate::bytecode::Instruction],
    regs: &mut [u64; 256], constants: &[u64],
) -> TieredAction {
    if let Some(func) = jit.get_compiled(pc) {
        unsafe { func(regs.as_mut_ptr(), constants.as_ptr()); }
        return TieredAction::RanJit;
    }
    if record_and_check_hot(pc) {
        let region_len = find_compilable_region(pc, instructions);
        if region_len > 5 {
            if let Some(func) = unsafe { jit.compile_region(module_idx, pc, region_len, instructions) } {
                unsafe { func(regs.as_mut_ptr(), constants.as_ptr()); }
                return TieredAction::CompiledAndRan;
            }
        }
    }
    TieredAction::Interpret
}

pub(crate) fn find_compilable_region(
    offset: usize, instructions: &[crate::bytecode::Instruction],
) -> usize {
    let mut len = 0;
    for i in offset..instructions.len().min(offset + 500) {
        if !compiler::is_opcode_compilable(instructions[i].opcode) { break; }
        len += 1;
        if matches!(instructions[i].opcode, crate::bytecode::OpCode::Ret | crate::bytecode::OpCode::RetVal) { break; }
    }
    len
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::*;

    #[test] fn test_jit_session_creation() {
        let jit = JitSession::new();
        assert_eq!(jit.compiled_count(), 0);
    }

    #[test] fn test_hot_counter() {
        reset_hot_counters();
        assert!(!record_and_check_hot(0));
        for _ in 0..HOT_THRESHOLD { record_and_check_hot(42); }
        assert!(record_and_check_hot(42));
        reset_hot_counters();
    }

    #[test] fn test_find_compilable_region() {
        let instructions = vec![
            Instruction::new3(OpCode::IAdd, 0, 1, 2),
            Instruction::new3(OpCode::ISub, 0, 1, 2),
            Instruction::new0(OpCode::Ret),
        ];
        assert_eq!(find_compilable_region(0, &instructions), 3);
    }

    #[test] fn test_find_region_stops_at_unsupported() {
        let instructions = vec![
            Instruction::new3(OpCode::IAdd, 0, 1, 2),
            Instruction::new3(OpCode::Spawn, 0, 0, 0),
            Instruction::new3(OpCode::ISub, 0, 1, 2),
        ];
        assert_eq!(find_compilable_region(0, &instructions), 1);
    }
}
