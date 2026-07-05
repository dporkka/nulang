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
pub mod typed_compiler;
pub mod runtime;
pub mod simd_analyzer;
pub mod simd_compiler;

#[cfg(test)]
mod tests;

pub use compiler::*;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use cranelift::prelude::*;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::Module;

// ---------------------------------------------------------------------------
// Hot Counter
// ---------------------------------------------------------------------------

/// Threshold: how many times a bytecode region must be interpreted
/// before it becomes eligible for JIT compilation.
pub const HOT_THRESHOLD: u64 = 1000;

static HOT_COUNTERS: OnceLock<Mutex<HashMap<usize, u64>>> = OnceLock::new();

fn get_hot_counters() -> &'static Mutex<HashMap<usize, u64>> {
    HOT_COUNTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record execution at offset. Returns true if region is hot.
pub fn record_and_check_hot(offset: usize) -> bool {
    let mut map = get_hot_counters().lock().unwrap();
    let count = map.entry(offset).or_insert(0);
    *count += 1;
    *count >= HOT_THRESHOLD
}

/// Reset hot counters.
pub fn reset_hot_counters() {
    let mut map = get_hot_counters().lock().unwrap();
    map.clear();
}

// ---------------------------------------------------------------------------
// JIT Session
// ---------------------------------------------------------------------------

/// Manages the Cranelift JIT compilation lifecycle.
///
/// - Creates and configures the `JITModule`
/// - Compiles bytecode regions to native functions
/// - Caches compiled function pointers by bytecode offset
pub struct JitSession {
    /// The Cranelift JIT module that owns compiled code memory.
    module: JITModule,
    /// Map from bytecode offset → compiled function name → pointer.
    compiled: HashMap<usize, *const u8>,
    /// Reusable function builder context.
    builder_context: FunctionBuilderContext,
    /// Reusable codegen context.
    ctx: codegen::Context,
}

impl JitSession {
    /// Create a new JIT session with the native target ISA.
    pub fn new() -> Self {
        let mut flag_builder = settings::builder();
        // Enable baseline SIMD support (SSE2 on x86_64, NEON on aarch64)
        let _ = flag_builder.set("enable_simd", "true");
        let isa_builder = cranelift_native::builder().unwrap_or_else(|msg| {
            panic!("host machine is not supported: {}", msg);
        });
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .unwrap();

        let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());

        // Register NaN-tag-aware runtime helpers so compiled code can call them.
        // Each helper is defined in `runtime.rs` with `#[no_mangle]` and `extern "C"`.
        let helpers: &[(&str, *const u8)] = &[
            ("nulang_iadd", crate::jit::runtime::nulang_iadd as *const u8),
            ("nulang_isub", crate::jit::runtime::nulang_isub as *const u8),
            ("nulang_imul", crate::jit::runtime::nulang_imul as *const u8),
            ("nulang_idiv", crate::jit::runtime::nulang_idiv as *const u8),
            ("nulang_imod", crate::jit::runtime::nulang_imod as *const u8),
            ("nulang_icmp_eq", crate::jit::runtime::nulang_icmp_eq as *const u8),
            ("nulang_icmp_lt", crate::jit::runtime::nulang_icmp_lt as *const u8),
            ("nulang_icmp_gt", crate::jit::runtime::nulang_icmp_gt as *const u8),
            ("nulang_icmp_le", crate::jit::runtime::nulang_icmp_le as *const u8),
            ("nulang_icmp_ge", crate::jit::runtime::nulang_icmp_ge as *const u8),
            ("nulang_fadd", crate::jit::runtime::nulang_fadd as *const u8),
            ("nulang_fsub", crate::jit::runtime::nulang_fsub as *const u8),
            ("nulang_fmul", crate::jit::runtime::nulang_fmul as *const u8),
            ("nulang_fdiv", crate::jit::runtime::nulang_fdiv as *const u8),
            ("nulang_fcmp_eq", crate::jit::runtime::nulang_fcmp_eq as *const u8),
            ("nulang_fcmp_lt", crate::jit::runtime::nulang_fcmp_lt as *const u8),
            ("nulang_fcmp_gt", crate::jit::runtime::nulang_fcmp_gt as *const u8),
            ("nulang_ineg", crate::jit::runtime::nulang_ineg as *const u8),
            ("nulang_iinc", crate::jit::runtime::nulang_iinc as *const u8),
            ("nulang_idec", crate::jit::runtime::nulang_idec as *const u8),
            ("nulang_not", crate::jit::runtime::nulang_not as *const u8),
            ("nulang_and", crate::jit::runtime::nulang_and as *const u8),
            ("nulang_or", crate::jit::runtime::nulang_or as *const u8),
            ("nulang_itof", crate::jit::runtime::nulang_itof as *const u8),
            ("nulang_ftoi", crate::jit::runtime::nulang_ftoi as *const u8),
        ];
        for (name, ptr) in helpers {
            builder.symbol(*name, *ptr);
        }

        let module = JITModule::new(builder);
        let ctx = module.make_context();

        JitSession {
            module,
            compiled: HashMap::new(),
            builder_context: FunctionBuilderContext::new(),
            ctx,
        }
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
        if let Some(&ptr) = self.compiled.get(&start_offset) {
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
                self.compiled.insert(start_offset, ptr);
                Some(std::mem::transmute(ptr))
            }
            Err(_) => None,
        }
    }

    /// Check if a bytecode offset has already been compiled.
    pub fn is_compiled(&self, offset: usize) -> bool {
        self.compiled.contains_key(&offset)
    }

    /// Get the compiled function pointer for an offset (if compiled).
    pub fn get_compiled(&self, offset: usize) -> Option<JitFunctionPtr> {
        self.compiled
            .get(&offset)
            .map(|&ptr| unsafe { std::mem::transmute(ptr) })
    }

    /// Return the number of compiled regions.
    pub fn compiled_count(&self) -> usize {
        self.compiled.len()
    }

    /// Compile a SIMD-vectorizable bytecode region.
    ///
    /// First analyzes the region for vectorizable array loop patterns. If found,
    /// emits SIMD CLIF (I64x2/F64x2/I32x4/F32x4). Falls back to the scalar
    /// typed compiler if the region is not vectorizable.
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
        if let Some(&ptr) = self.compiled.get(&start_offset) {
            return Some(std::mem::transmute(ptr));
        }

        // Only attempt SIMD if host CPU supports it
        if !is_simd_supported() {
            return self.compile_region(module_idx, start_offset, num_instrs, instructions);
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
                self.compiled.insert(start_offset, ptr);
                Some(std::mem::transmute(ptr))
            }
            Err(_) => self.compile_region(module_idx, start_offset, num_instrs, instructions),
        }
    }
}

impl Default for JitSession {
    fn default() -> Self {
        Self::new()
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

/// Execute a bytecode instruction, recording it for hotness tracking.
///
/// This should be called from the interpreter loop before each instruction.
/// When a region becomes hot, the caller should JIT compile it and switch
/// to native execution.
pub fn tiered_execute_step(
    jit: &mut JitSession,
    module_idx: usize,
    pc: usize,
    instructions: &[crate::bytecode::Instruction],
    regs: &mut [u64; 256],
    constants: &[u64],
    type_metadata: Option<&crate::jit::typed_compiler::TypeMetadata>,
) -> TieredAction {
    // Check if already compiled
    if let Some(func) = jit.get_compiled(pc) {
        // Execute JIT-compiled code
        func(regs.as_mut_ptr(), constants.as_ptr());
        return TieredAction::RanJit;
    }

    // Record execution for hotness
    if record_and_check_hot(pc) {
        // Try to compile from PC to the end of the function or a unsupported opcode
        let region_len = find_compilable_region(pc, instructions);
        if region_len > 5 {
            // Try SIMD-vectorized compilation first, fall back to scalar
            if let Some(func) = unsafe {
                jit.compile_region_simd(module_idx, pc, region_len, instructions, type_metadata)
            } {
                func(regs.as_mut_ptr(), constants.as_ptr());
                return TieredAction::CompiledSimdAndRan;
            }
        }
    }

    TieredAction::Interpret
}

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
        len += 1;
        // Stop at return instructions
        if matches!(
            instructions[i].opcode,
            crate::bytecode::OpCode::Ret | crate::bytecode::OpCode::RetVal
        ) {
            break;
        }
    }
    len
}

/// Action taken by the tiered execution system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TieredAction {
    /// The instruction should be interpreted (not hot enough or unsupported).
    Interpret,
    /// JIT-compiled code was executed.
    RanJit,
    /// The region was JIT compiled and then executed (scalar).
    CompiledAndRan,
    /// The region was SIMD-vectorized, compiled, and executed.
    CompiledSimdAndRan,
}


