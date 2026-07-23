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
//! - `simd_analyzer`: Detects loops that can be vectorized with SIMD.
//! - `simd_compiler`: Emits SIMD CLIF for vectorized array operations.
//! - `runtime.rs`: Runtime helper functions callable from JIT code for
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
//!
//! The function reads operands from `regs`, writes results back, and
//! returns via native `ret`. Control flow (jumps) is compiled to native
//! branches.

mod compiler;
pub mod helpers;
pub mod runtime;
pub mod simd_analyzer;
pub mod simd_compiler;
pub mod typed_compiler;

#[cfg(test)]
mod tests;

pub use compiler::*;

use std::collections::HashMap;

use cranelift::prelude::*;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::Module;

// ---------------------------------------------------------------------------
// Hot Counter
// ---------------------------------------------------------------------------

/// Threshold: how many times a bytecode region must be interpreted
/// before it becomes eligible for JIT compilation.
pub const HOT_THRESHOLD: u64 = 1000;

// ---------------------------------------------------------------------------
// JIT Session
// ---------------------------------------------------------------------------

/// Manages the Cranelift JIT compilation lifecycle.
///
/// - Creates and configures the `JITModule`
/// - Compiles bytecode regions to native functions
/// - Caches compiled function pointers by `(module_idx, bytecode offset)`
pub struct JitSession {
    /// The Cranelift JIT module that owns compiled code memory.
    module: JITModule,
    /// Map from `(module_idx, bytecode offset)` → (compiled function
    /// pointer, region length in instructions). The length is recorded at
    /// compile time so the VM can advance pc after a JIT run without
    /// re-scanning the instruction stream.
    compiled: HashMap<(usize, usize), (*const u8, usize)>,
    /// Hot counters keyed by `(module_idx, offset)` so identical offsets in
    /// different modules do not share (or pollute) each other's counts.
    /// Per-session rather than process-global: VMs never share counters,
    /// and the single-scheduler-thread invariant means no synchronization
    /// is needed — same as `compiled` and `typed_regions`.
    hot_counters: HashMap<(usize, usize), u64>,
    /// Regions compiled through the type-directed (guard-stripped) path in
    /// `typed_compiler`, i.e. where inferred register types were available.
    typed_regions: std::collections::HashSet<(usize, usize)>,
    /// Reusable function builder context.
    builder_context: FunctionBuilderContext,
    /// Reusable codegen context.
    ctx: codegen::Context,
}

impl JitSession {
    /// Create a new JIT session with the native target ISA.
    /// Returns `None` if the host platform is not supported or ISA finalization
    /// fails, printing a warning to stderr.
    pub fn new() -> Option<Self> {
        let mut flag_builder = settings::builder();
        // Enable baseline SIMD support (SSE2 on x86_64, NEON on aarch64)
        let _ = flag_builder.set("enable_simd", "true");
        let isa_builder = match cranelift_native::builder() {
            Ok(b) => b,
            Err(msg) => {
                eprintln!("JIT: host machine is not supported: {} — JIT disabled", msg);
                return None;
            }
        };
        let isa = match isa_builder.finish(settings::Flags::new(flag_builder)) {
            Ok(isa) => isa,
            Err(e) => {
                eprintln!("JIT: failed to finalize Cranelift ISA: {} — JIT disabled", e);
                return None;
            }
        };

        let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());

        // Register NaN-tag-aware runtime helpers so compiled code can call them.
        // Single source of truth: src/jit/helpers.rs define_helpers! macro.
        crate::jit::helpers::register_with_builder(&mut builder);

        let module = JITModule::new(builder);
        let ctx = module.make_context();

        Some(JitSession {
            module,
            compiled: HashMap::new(),
            hot_counters: HashMap::new(),
            typed_regions: std::collections::HashSet::new(),
            builder_context: FunctionBuilderContext::new(),
            ctx,
        })
    }

    /// Record one interpreted execution of the region at
    /// `(module_idx, offset)`. Returns true once the region has been
    /// interpreted at least `HOT_THRESHOLD` times, making it eligible for
    /// JIT compilation.
    pub fn record_and_check_hot(&mut self, module_idx: usize, offset: usize) -> bool {
        let count = self.hot_counters.entry((module_idx, offset)).or_insert(0);
        *count += 1;
        *count >= HOT_THRESHOLD
    }

    /// Reset all hot counters (used by tests that re-heat a region on an
    /// existing session).
    pub fn reset_hot_counters(&mut self) {
        self.hot_counters.clear();
    }

    /// Compile a bytecode region starting at `start_offset` with `num_instrs`
    /// instructions. Returns the compiled function pointer, or None if the
    /// region contains unsupported opcodes.
    ///
    /// # Safety
    /// The returned function pointer is valid for the lifetime of this
    /// `JitSession`. The bytecode must not be modified while JIT code is
    /// executing.
    pub unsafe fn compile_region(
        &mut self,
        module_idx: usize,
        start_offset: usize,
        num_instrs: usize,
        instructions: &[crate::bytecode::Instruction],
    ) -> Option<JitFunctionPtr> {
        // Check if already compiled
        if let Some(&(ptr, _)) = self.compiled.get(&(module_idx, start_offset)) {
            return Some(std::mem::transmute(ptr));
        }

        // Build the function
        let func_name = format!("nulang_jit_{}_{}", module_idx, start_offset);

        match compiler::compile_bytecode_region(
            &mut self.module,
            &mut self.builder_context,
            &mut self.ctx,
            &func_name,
            start_offset,
            num_instrs,
            instructions,
        ) {
            Ok(ptr) => {
                self.compiled
                    .insert((module_idx, start_offset), (ptr, num_instrs));
                Some(std::mem::transmute(ptr))
            }
            Err(_) => None,
        }
    }

    /// Compile a bytecode region with optional type-directed guard stripping.
    ///
    /// When `type_metadata` proves at least one register's type, the region
    /// goes through `typed_compiler::compile_bytecode_region_typed`, which
    /// emits direct CLIF for statically typed operations instead of
    /// NaN-tag-aware runtime helper calls. Absent/empty metadata — or any
    /// typed-compilation failure — falls back to the scalar
    /// [`JitSession::compile_region`], so this never compiles *less* code
    /// than the untyped path.
    ///
    /// # Safety
    /// Same safety requirements as `compile_region`.
    pub unsafe fn compile_region_typed(
        &mut self,
        module_idx: usize,
        start_offset: usize,
        num_instrs: usize,
        instructions: &[crate::bytecode::Instruction],
        type_metadata: Option<&crate::jit::typed_compiler::TypeMetadata>,
    ) -> Option<JitFunctionPtr> {
        // Check if already compiled
        if let Some(&(ptr, _)) = self.compiled.get(&(module_idx, start_offset)) {
            return Some(std::mem::transmute(ptr));
        }

        let has_known_types = type_metadata
            .map(|m| {
                m.reg_types
                    .values()
                    .any(|&t| t != crate::jit::typed_compiler::KnownType::Unknown)
            })
            .unwrap_or(false);

        if has_known_types {
            let func_name = format!("nulang_tjit_{}_{}", module_idx, start_offset);
            if let Ok(ptr) = typed_compiler::compile_bytecode_region_typed(
                &mut self.module,
                &mut self.builder_context,
                &mut self.ctx,
                &func_name,
                start_offset,
                num_instrs,
                instructions,
                type_metadata,
            ) {
                self.compiled
                    .insert((module_idx, start_offset), (ptr, num_instrs));
                self.typed_regions.insert((module_idx, start_offset));
                return Some(std::mem::transmute(ptr));
            }
            // Typed compilation failed: fall through to the scalar compiler.
        }

        self.compile_region(module_idx, start_offset, num_instrs, instructions)
    }

    /// Return the number of regions compiled through the type-directed path.
    pub fn typed_compiled_count(&self) -> usize {
        self.typed_regions.len()
    }

    /// Check whether a `(module_idx, offset)` region was compiled with
    /// type-directed guard stripping.
    pub fn is_typed_compiled(&self, module_idx: usize, offset: usize) -> bool {
        self.typed_regions.contains(&(module_idx, offset))
    }

    /// Check if a `(module_idx, offset)` region has already been compiled.
    pub fn is_compiled(&self, module_idx: usize, offset: usize) -> bool {
        self.compiled.contains_key(&(module_idx, offset))
    }

    /// Get the compiled function pointer for `(module_idx, offset)` (if compiled).
    ///
    /// # Safety
    /// The returned function pointer is valid only while this `JitSession` is
    /// alive and the original bytecode has not been modified.
    pub unsafe fn get_compiled(&self, module_idx: usize, offset: usize) -> Option<JitFunctionPtr> {
        self.compiled
            .get(&(module_idx, offset))
            .map(|&(ptr, _)| std::mem::transmute(ptr))
    }

    /// Number of bytecode instructions covered by the compiled region at
    /// `(module_idx, offset)`, recorded at compile time. The VM uses this
    /// to advance pc after a JIT run instead of re-scanning the
    /// instruction stream.
    pub fn compiled_region_len(&self, module_idx: usize, offset: usize) -> Option<usize> {
        self.compiled
            .get(&(module_idx, offset))
            .map(|&(_, len)| len)
    }

    /// Return the number of compiled regions.
    pub fn compiled_count(&self) -> usize {
        self.compiled.len()
    }


    /// Compile a SIMD-vectorizable bytecode region.
    /// First analyzes the region for vectorizable array loop patterns. If found,
    /// emits SIMD CLIF (I64x2/F64x2/I32x4/F32x4), falling back to the
    /// type-directed scalar compiler if SIMD emission fails. Returns `None`
    /// when the region has no vectorizable pattern at all.
    ///
    /// Currently **not wired into tiering**: `simd_analyzer` finds no
    /// trip-count hints in production and the emitter is unsound as-is (no
    /// register write-back, baked trip count), so the scalar/typed
    /// compilation paths are used until SIMD is reworked.
    ///
    /// # Safety
    /// Same safety requirements as `compile_region`.
    pub unsafe fn compile_region_simd(
        &mut self,
        module_idx: usize,
        start_offset: usize,
        num_instrs: usize,
        instructions: &[crate::bytecode::Instruction],
        type_metadata: Option<&crate::jit::typed_compiler::TypeMetadata>,
    ) -> Option<JitFunctionPtr> {
        use crate::jit::simd_analyzer::analyze_region;
        use crate::jit::simd_compiler::{compile_simd_region, is_simd_supported};

        // Check if already compiled
        if let Some(&(ptr, _)) = self.compiled.get(&(module_idx, start_offset)) {
            return Some(std::mem::transmute(ptr));
        }

        // Only attempt SIMD if host CPU supports it
        if !is_simd_supported() {
            return self.compile_region_typed(
                module_idx,
                start_offset,
                num_instrs,
                instructions,
                type_metadata,
            );
        }

        // Analyze for vectorizable patterns
        let simd_region = analyze_region(instructions, start_offset, num_instrs, type_metadata)?;

        let func_name = format!("nulang_simd_{}_{}", module_idx, start_offset);

        match compile_simd_region(
            &mut self.module,
            &mut self.builder_context,
            &mut self.ctx,
            &func_name,
            instructions,
            &simd_region,
        ) {
            Ok(ptr) => {
                self.compiled
                    .insert((module_idx, start_offset), (ptr, num_instrs));
                Some(std::mem::transmute(ptr))
            }
            Err(_) => self.compile_region_typed(
                module_idx,
                start_offset,
                num_instrs,
                instructions,
                type_metadata,
            ),
        }
    }
}

impl Default for JitSession {
    fn default() -> Self {
        Self::new().expect("JIT must be available for Default::default()")
    }
}

// ---------------------------------------------------------------------------
// JIT Function Type
// ---------------------------------------------------------------------------

/// Type of a JIT-compiled Nulang function.
///
/// Signature: `fn(regs: *mut u64, constants: *const u64)`
///
/// The function reads from `regs` (256 elements), performs operations,
/// writes results back to `regs`, and returns. Control flow is entirely
/// within the native code.
pub type JitFunctionPtr = extern "C" fn(*mut u64, *const u64);

// ---------------------------------------------------------------------------
// Tiered Execution
// ---------------------------------------------------------------------------


/// Find a contiguous region of compilable instructions starting at `offset`.
/// Returns the number of instructions in the region.
pub(crate) fn find_compilable_region(
    offset: usize,
    instructions: &[crate::bytecode::Instruction],
) -> usize {
    let mut len = 0;
    for i in offset..instructions.len().min(offset + 500) {
        if !compiler::is_opcode_compilable(instructions[i].opcode) {
            break;
        }
        // Stop *before* return instructions so the VM still executes the
        // return and pops the frame correctly after the JIT region.
        //
        // Also stop before any branch or halt: after a region runs, the VM
        // unconditionally advances pc by the region length, so a compiled
        // branch whose target lies outside the region would resume at the
        // wrong instruction. Restricting regions to straight-line code keeps
        // that pc-advance contract exact (branches themselves stay
        // interpreted; loop *bodies* still get compiled).
        if matches!(
            instructions[i].opcode,
            crate::bytecode::OpCode::Ret
                | crate::bytecode::OpCode::RetVal
                | crate::bytecode::OpCode::Jmp
                | crate::bytecode::OpCode::JmpT
                | crate::bytecode::OpCode::JmpF
                | crate::bytecode::OpCode::Halt
        ) {
            break;
        }
        len += 1;
    }
    len
}

// TieredAction is defined in `crate::backends` so the VM can reference it
// without importing the JIT module. Re-export for backward compatibility.
pub use crate::backends::TieredAction;

// ---------------------------------------------------------------------------
// JitBackend trait impl — adapts the Cranelift JIT to the backend trait
// ---------------------------------------------------------------------------

impl crate::backends::JitBackend for JitSession {
    fn is_compiled(&self, module_idx: usize, pc: usize) -> bool {
        self.compiled.contains_key(&(module_idx, pc))
    }

    fn record_and_check_hot(&mut self, module_idx: usize, pc: usize) -> bool {
        let count = self.hot_counters.entry((module_idx, pc)).or_insert(0);
        *count += 1;
        *count >= HOT_THRESHOLD
    }

    fn compiled_region_len(&self, module_idx: usize, pc: usize) -> Option<usize> {
        self.compiled.get(&(module_idx, pc)).map(|&(_, len)| len)
    }

    fn compiled_count(&self) -> usize {
        self.compiled.len()
    }

    fn typed_compiled_count(&self) -> usize {
        self.typed_regions.len()
    }

    fn reset_hot_counters(&mut self) {
        self.hot_counters.clear();
    }

    fn tiered_execute_step_typed(
        &mut self,
        module_idx: usize,
        pc: usize,
        module: &crate::bytecode::CodeModule,
        regs: &mut [u64; 256],
        constants: &[u64],
    ) -> crate::backends::TieredAction {
        let instructions = &module.instructions;

        // Check if already compiled
        if let Some(func) = unsafe { self.get_compiled(module_idx, pc) } {
            func(regs.as_mut_ptr(), constants.as_ptr());
            return crate::backends::TieredAction::RanJit;
        }

        // Record execution for hotness
        if self.record_and_check_hot(module_idx, pc) {
            let region_len = find_compilable_region(pc, instructions);
            if region_len >= 3 {
                let meta = typed_compiler::infer_reg_types(module, pc);
                let meta_ref = if meta.reg_types.is_empty() {
                    None
                } else {
                    Some(&meta)
                };
                if let Some(func) = unsafe {
                    self.compile_region_typed(module_idx, pc, region_len, instructions, meta_ref)
                } {
                    func(regs.as_mut_ptr(), constants.as_ptr());
                    return crate::backends::TieredAction::RanJit;
                }
            }
        }

        crate::backends::TieredAction::Interpret
    }
}
