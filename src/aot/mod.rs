//! AOT (Ahead-of-Time) native code compilation backend.
//!
//! Compiles Nulang MIR modules to native code via Cranelift, leveraging
//! compile-time type information to emit unboxed operations.
//!
//! # Architecture
//!
//! - `codegen`: MIR → Cranelift CLIF compilation (per-function)
//! - This module: orchestrates module-level compilation, registers runtime
//!   helpers, and provides the execution entry point.
//!
//! # Current status
//!
//! Uses `cranelift_jit::JITModule` (same as the tiered JIT) rather than
//! true AOT object-file emission. This gives us native code without needing
//! a linker — the trampoline calls into the JIT module at startup.

pub mod codegen;

use cranelift::prelude::*;
use cranelift_frontend::FunctionBuilderContext;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::Module;

use crate::mir;
use crate::runtime::heap::TypeTag as HeapTypeTag;
use crate::types::{NuResult, Span};

/// Compiled AOT module ready for execution.
pub struct AotModule {
    /// The Cranelift JIT module that owns compiled code memory.
    #[allow(dead_code)]
    jit_module: JITModule,
    /// Reusable function builder context.
    #[allow(dead_code)]
    builder_context: FunctionBuilderContext,
    /// Compiled function pointers indexed by MIR function index.
    compiled_funcs: Vec<*const u8>,
    /// Actor behavior names, parallel to `compiled_behaviors`.
    behavior_names: Vec<String>,
    /// Compiled actor behavior pointers (native code), parallel to
    /// `behavior_names`. Empty when the module has no `actor` declarations.
    compiled_behaviors: Vec<*const u8>,
    /// Entry point index (the `__main` or `main` function).
    entry_idx: Option<usize>,
    /// Module-wide field name → slot index mapping for records.
    #[allow(dead_code)]
    field_map: std::collections::HashMap<String, u8>,
    /// Constant pool (String literals), for runtime string resolution.
    constants: Vec<crate::bytecode::Constant>,
}

impl AotModule {
    /// Compile a MIR module to native code for the specified target.
    pub fn compile(mir_module: &mir::Module) -> NuResult<Self> {
        Self::compile_for_target(mir_module, "native")
    }

    /// Compile a MIR module to native code for a specific target ISA.
    pub fn compile_for_target(mir_module: &mir::Module, target: &str) -> NuResult<Self> {
        // Set up Cranelift with the target ISA.
        let mut flag_builder = settings::builder();
        let _ = flag_builder.set("enable_simd", "true");
        let isa_builder = create_isa_builder(target)?;
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|e| crate::types::NuError::VMError {
                msg: format!("failed to finalize ISA for target '{}': {}", target, e),
                span: Span::default(),
            })?;

        let mut jit_builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());

        // Register NaN-tag-aware runtime helpers.
        register_runtime_helpers(&mut jit_builder);

        let mut jit_module = JITModule::new(jit_builder);
        let mut builder_context = FunctionBuilderContext::new();

        // Pre-scan: build module-wide field name → slot index map and
        // constant pool for string literals.
        let mut field_map: std::collections::HashMap<String, u8> = std::collections::HashMap::new();
        let mut next_field_id: u8 = 0;
        let mut constants: Vec<crate::bytecode::Constant> = Vec::new();

        for func in &mir_module.functions {
            for block in &func.blocks {
                for stmt in &block.stmts {
                    collect_field_and_consts(
                        stmt,
                        &mut field_map,
                        &mut next_field_id,
                        &mut constants,
                    );
                }
            }
        }
        for func in &mir_module.behaviors {
            for block in &func.blocks {
                for stmt in &block.stmts {
                    collect_field_and_consts(
                        stmt,
                        &mut field_map,
                        &mut next_field_id,
                        &mut constants,
                    );
                }
            }
        }

        // Pass 1: declare all functions so forward references resolve.
        let mut func_ids: Vec<cranelift_module::FuncId> =
            Vec::with_capacity(mir_module.functions.len());
        // Unboxed variants for all-Int functions (same indices, empty for non-Int).
        let mut unboxed_ids: Vec<Option<cranelift_module::FuncId>> =
            vec![None; mir_module.functions.len()];

        for (idx, func) in mir_module.functions.iter().enumerate() {
            let func_name = format!("nulang_fn_{}", idx);
            let mut sig = jit_module.make_signature();
            for _ in &func.params {
                sig.params.push(AbiParam::new(types::I64));
            }
            sig.returns.push(AbiParam::new(types::I64));
            let fid = jit_module
                .declare_function(&func_name, cranelift_module::Linkage::Local, &sig)
                .map_err(|e| crate::types::NuError::VMError {
                    msg: format!("failed to declare '{}': {}", func.name, e),
                    span: Span::default(),
                })?;
            func_ids.push(fid);

            // If the function is all-Int, also declare an unboxed variant.
            if codegen::is_all_int(func) {
                let ub_name = format!("nulang_fn_{}_unboxed", idx);
                let mut ub_sig = jit_module.make_signature();
                for _ in &func.params {
                    ub_sig.params.push(AbiParam::new(types::I64));
                }
                ub_sig.returns.push(AbiParam::new(types::I64));
                let ub_fid = jit_module
                    .declare_function(&ub_name, cranelift_module::Linkage::Local, &ub_sig)
                    .map_err(|e| crate::types::NuError::VMError {
                        msg: format!("failed to declare unboxed '{}': {}", func.name, e),
                        span: Span::default(),
                    })?;
                unboxed_ids[idx] = Some(ub_fid);
            }
        }

        // Pass 2: compile each function body (boxed + optionally unboxed).
        let mut entry_idx: Option<usize> = None;

        for (idx, func) in mir_module.functions.iter().enumerate() {
            // For all-Int functions: compile unboxed body first, then
            // generate a boxing wrapper as the boxed entry point. The
            // original boxed body is never compiled.
            // For non-all-Int functions: compile boxed body as usual.
            if let Some(ub_fid) = unboxed_ids[idx] {
                // Compile unboxed variant (self-recursive calls resolve to ub_fid).
                let mut ctx2 = codegen::AotContext::new(&mut jit_module, &mut builder_context);
                ctx2.func_ids = func_ids.clone();
                ctx2.func_ids[idx] = ub_fid;
                ctx2.field_map = field_map.clone();
                ctx2.constants = constants.clone();
                codegen::compile_mir_function_body(
                    &mut ctx2,
                    func,
                    idx,
                    ub_fid,
                    codegen::CompileMode::Unboxed,
                )
                .map_err(|e| crate::types::NuError::VMError {
                    msg: format!("AOT compilation of unboxed '{}' failed: {}", func.name, e),
                    span: Span::default(),
                })?;

                // Compile boxing wrapper as the boxed function table entry.
                let mut ctx3 = codegen::AotContext::new(&mut jit_module, &mut builder_context);
                codegen::compile_boxing_wrapper(
                    &mut ctx3,
                    func.params.len(),
                    func_ids[idx],
                    ub_fid,
                )
                .map_err(|e| crate::types::NuError::VMError {
                    msg: format!("AOT boxing wrapper for '{}' failed: {}", func.name, e),
                    span: Span::default(),
                })?;
            } else {
                // Normal boxed compilation for non-all-Int functions.
                let mut ctx = codegen::AotContext::new(&mut jit_module, &mut builder_context);
                ctx.func_ids = func_ids.clone();
                ctx.field_map = field_map.clone();
                ctx.constants = constants.clone();
                codegen::compile_mir_function_body(
                    &mut ctx,
                    func,
                    idx,
                    func_ids[idx],
                    codegen::CompileMode::Boxed,
                )
                .map_err(|e| crate::types::NuError::VMError {
                    msg: format!("AOT compilation of '{}' failed: {}", func.name, e),
                    span: Span::default(),
                })?;
            }

            if func.name == "__main" || func.name == "main" {
                if entry_idx.is_none() || func.name == "__main" {
                    entry_idx = Some(idx);
                }
            }
        }

        // Pass: compile actor behaviors to native code, indexed by behavior
        // name. Behaviors are ordinary `Function`s (params + blocks); they
        // are never `Call` targets, so each compiles into its own native
        // entry point keyed by name. The actor runtime can later dispatch
        // messages straight to these pointers, bypassing the bytecode VM.
        let mut behavior_names: Vec<String> = Vec::new();
        let mut behavior_fids: Vec<cranelift_module::FuncId> = Vec::new();
        for (idx, func) in mir_module.behaviors.iter().enumerate() {
            let func_name = format!("nulang_behavior_{}", idx);
            let mut sig = jit_module.make_signature();
            for _ in &func.params {
                sig.params.push(AbiParam::new(types::I64));
            }
            sig.returns.push(AbiParam::new(types::I64));
            let fid = jit_module
                .declare_function(&func_name, cranelift_module::Linkage::Local, &sig)
                .map_err(|e| crate::types::NuError::VMError {
                    msg: format!("failed to declare behavior '{}': {}", func.name, e),
                    span: Span::default(),
                })?;
            let mut ctx = codegen::AotContext::new(&mut jit_module, &mut builder_context);
            ctx.func_ids = func_ids.clone();
            ctx.field_map = field_map.clone();
            ctx.constants = constants.clone();
            codegen::compile_mir_function_body(
                &mut ctx,
                func,
                idx,
                fid,
                codegen::CompileMode::Boxed,
            )
            .map_err(|e| crate::types::NuError::VMError {
                msg: format!("AOT compilation of behavior '{}' failed: {}", func.name, e),
                span: Span::default(),
            })?;
            behavior_names.push(func.name.clone());
            behavior_fids.push(fid);
        }
        jit_module
            .finalize_definitions()
            .map_err(|e| crate::types::NuError::VMError {
                msg: format!("failed to finalize JIT definitions: {}", e),
                span: Span::default(),
            })?;

        let compiled_funcs: Vec<*const u8> = func_ids
            .iter()
            .map(|fid| jit_module.get_finalized_function(*fid))
            .collect();
        let compiled_behaviors: Vec<*const u8> = behavior_fids
            .iter()
            .map(|fid| jit_module.get_finalized_function(*fid))
            .collect();

        Ok(AotModule {
            jit_module,
            builder_context,
            compiled_funcs,
            behavior_names,
            compiled_behaviors,
            entry_idx,
            field_map,
            constants,
        })
    }

    /// Look up a compiled behavior's native entry pointer by name.
    ///
    /// Returns `None` when the module has no behavior with that name. The
    /// returned pointer is a function with the AOT calling convention:
    /// `extern "C" fn(boxed_param_0, boxed_param_1, ...) -> u64`. It is only
    /// valid while the `AotModule` is alive (the pointer lives in the JIT
    /// code memory it owns).
    pub fn fn_ptr_for_behavior(&self, name: &str) -> Option<*const u8> {
        self.behavior_names
            .iter()
            .position(|n| n == name)
            .map(|idx| self.compiled_behaviors[idx])
    }

    /// The module's constant pool (string literals). AOT behavior dispatch
    /// sets these so `StateGet`/`StateSet` field names resolve.
    pub fn constants(&self) -> &[crate::bytecode::Constant] {
        &self.constants
    }

    /// Create a standalone actor of the type referenced by `behavior_idx`
    /// (the actor's first behavior's module index, per `spawn_behavior_idx`).
    /// Registers all of the actor's behaviors with the AOT adapter (in module
    /// order, so the actor's local behavior-table indices match module
    /// indices), applies `init` state overrides (name constant idx → value),
    /// and returns the new actor's id. The spawned actor is boxed and owned by
    /// this module's registry, and its raw pointer is registered in
    /// `AOT_ACTORS` so native `send` can deliver to it.
    pub fn spawn_actor(
        &self,
        behavior_idx: usize,
        init: Vec<(u64, crate::vm::Value)>,
    ) -> Option<u64> {
        let full = self.behavior_names.get(behavior_idx)?;
        let actor_name = full.split('.').next()?.to_string();
        let id = AOT_FRESH_ACTOR_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut actor = Box::new(crate::runtime::Actor::new(id, actor_name.clone(), 64));

        let prefix = format!("{}.", actor_name);
        for name in &self.behavior_names {
            if let Some(short) = name.strip_prefix(&prefix) {
                actor.register_behavior(short.to_string(), aot_behavior_adapter);
            }
        }

        for (name_idx, value) in init {
            let s = self
                .constants
                .get(name_idx as usize)
                .and_then(|c| match c {
                    crate::bytecode::Constant::String(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            actor.set_state_field(s, value);
        }

        let raw = &mut *actor as *mut crate::runtime::Actor;
        AOT_SPAWNED_ACTORS.with(|m| {
            m.borrow_mut().insert(id, actor);
        });
        AOT_ACTORS.with(|m| {
            m.borrow_mut().insert(id, raw);
        });
        Some(id)
    }

    /// Execute the module entry point and return the result as a u64 value.
    ///
    pub fn run(&self) -> NuResult<u64> {
        let idx = self.entry_idx.unwrap_or(0);
        let ptr = self
            .compiled_funcs
            .get(idx)
            .ok_or_else(|| crate::types::NuError::VMError {
                msg: "no compiled entry point".into(),
                span: Span::default(),
            })?;

        // Set up standalone heap for AOT runtime helpers.
        let mut heap = crate::runtime::heap::ActorHeap::new(1024 * 1024);
        heap.set_actor_id(0);
        crate::jit::runtime::aot_set_heap(heap);

        // Set up constant pool for string resolution.
        if !self.constants.is_empty() {
            unsafe {
                crate::jit::runtime::aot_set_constants(&self.constants);
            }
        }

        // Call the compiled function. Signature: extern "C" fn() -> u64
        // (for the entry point with no params).
        let func: extern "C" fn() -> u64 = unsafe { std::mem::transmute(*ptr) };
        let result = func();

        // Clean up.
        crate::jit::runtime::aot_clear_constants();
        let _ = crate::jit::runtime::aot_take_heap();

        Ok(result)
    }

    /// Emit assembly text for the compiled module.
    pub fn emit_assembly(&self) -> String {
        // For now, we'll just show the function names and basic info
        // Full assembly emission would require using cranelift_object or TextSectionBuilder
        let mut output = String::new();
        output.push_str(&format!("; AOT Module for target\n"));
        output.push_str(&format!("; Functions: {}\n", self.compiled_funcs.len()));
        for (idx, _) in self.compiled_funcs.iter().enumerate() {
            output.push_str(&format!("nulang_fn_{}:\n", idx));
            output.push_str("  ; [assembly would be emitted here]\n");
        }
        output
    }
}

// ---------------------------------------------------------------------------
// Native behavior dispatch
// ---------------------------------------------------------------------------
// Bridges the actor runtime's plain-fn behavior handler
// (`fn(&mut Actor, &[Value])`) to AOT-compiled native code. The compiled
// behavior functions are `extern "C" fn(boxed_param...) -> u64`; this adapter
// (a) installs an `ActorVmCallbacks` over the target actor so `StateGet` /
// `StateSet` / heap ops inside the native body route to it, and (b) packs the
// message payload into boxed args and calls the native pointer.
//
// The native target is supplied through a thread-local (set by the driver
// immediately before invoking the handler), mirroring how `set_jit_callbacks`
// feeds the VM's tiered JIT. This keeps the adapter a plain `fn` so it can sit
// in `Actor::behavior_table` without a closure.

thread_local! {
    /// Native behavior pointer the next `aot_behavior_adapter` call dispatches
    /// through. Null when no target is armed.
    static AOT_BEHAVIOR_TARGET: std::cell::RefCell<*const u8> =
        std::cell::RefCell::new(std::ptr::null());
}

/// Arm the thread-local native target for the next `aot_behavior_adapter`
/// invocation, and install the module constant pool so `StateGet`/`StateSet`
/// field-name string constants resolve. The driver must call this immediately
/// before dispatching a message to an AOT-compiled behavior.
pub fn set_aot_behavior_target(ptr: *const u8, constants: &[crate::bytecode::Constant]) {
    AOT_BEHAVIOR_TARGET.with(|c| *c.borrow_mut() = ptr);
    // SAFETY: `aot_set_constants` copies the slice; it stays valid for the
    // duration of the dispatched native call and is cleared afterwards.
    unsafe { crate::jit::runtime::aot_set_constants(constants) };
}

thread_local! {
    /// The `AotModule` whose `spawn_actor` resolves the next
    /// `nulang_aot_spawn` call (armed by the driver around dispatch).
    static AOT_SPAWN_CTX: std::cell::RefCell<*const AotModule> =
        std::cell::RefCell::new(std::ptr::null());
}

/// Arm the module the next `nulang_aot_spawn` (from native behavior code)
/// uses to create actors. The driver must call this before dispatching a
/// behavior that spawns, and `clear_aot_spawn_ctx` after.
pub fn set_aot_spawn_ctx(module: &AotModule) {
    AOT_SPAWN_CTX.with(|c| *c.borrow_mut() = module as *const AotModule);
}

/// Disarm the spawn context after a dispatched native behavior returns.
pub fn clear_aot_spawn_ctx() {
    AOT_SPAWN_CTX.with(|c| *c.borrow_mut() = std::ptr::null());
}

/// Native-code entry point for `RValue::Spawn`: creates a standalone actor of
/// the type whose first behavior is at module index `behavior_idx`, applying
/// any queued init pairs. Returns the new actor's id (boxed), or nil if no
/// spawn context is armed. Defined here (not in `jit/runtime.rs`) because it
/// needs `AotModule`; the JIT linker resolves it by symbol name at link time.
#[no_mangle]
pub unsafe extern "C" fn nulang_aot_spawn(behavior_idx: u64) -> u64 {
    let init = crate::jit::runtime::take_aot_spawn_init();
    let module = AOT_SPAWN_CTX.with(|c| *c.borrow());
    if module.is_null() {
        return crate::vm::Value::nil().as_raw();
    }
    match (*module).spawn_actor(behavior_idx as usize, init) {
        Some(id) => crate::vm::Value::actor_ref(id).as_raw(),
        None => crate::vm::Value::nil().as_raw(),
    }
}

thread_local! {
    /// Standalone actor registry: actor id → raw actor pointer. Populated by
    /// the AOT driver so `send` from native behavior code can deliver into a
    /// target actor's mailbox without a full `Runtime`.
    static AOT_ACTORS: std::cell::RefCell<std::collections::HashMap<u64, *mut crate::runtime::Actor>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

thread_local! {
    /// Ownership store for actors created by `AotModule::spawn_actor`. A
    /// spawned actor is boxed here (so its heap-allocated pointer is stable)
    /// and its raw pointer is also registered in `AOT_ACTORS` for `send`.
    static AOT_SPAWNED_ACTORS: std::cell::RefCell<std::collections::HashMap<u64, Box<crate::runtime::Actor>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Next id for a standalone-spawned actor, kept clear of the small ids the
/// tests use for manually-created actors.
static AOT_FRESH_ACTOR_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1_000_000);

/// Register a standalone actor so native `send` can deliver to its mailbox.
/// The pointer must stay valid until `unregister_aot_actor`.
pub fn register_aot_actor(actor: &mut crate::runtime::Actor) {
    AOT_ACTORS.with(|c| {
        c.borrow_mut().insert(actor.id, actor as *mut crate::runtime::Actor);
    });
}

/// Ids of every actor registered in the standalone send registry (both
/// driver-registered and spawned).
pub fn aot_actor_ids() -> Vec<u64> {
    AOT_ACTORS.with(|c| c.borrow().keys().copied().collect())
}

/// Read the actor pointer for an id owned by the standalone spawn registry.
pub fn aot_spawned_actor(id: u64) -> Option<*mut crate::runtime::Actor> {
    AOT_SPAWNED_ACTORS.with(|m| {
        m.borrow()
            .get(&id)
            .map(|b| &**b as *const crate::runtime::Actor as *mut crate::runtime::Actor)
    })
}

/// Remove a standalone actor from the native send registry.
pub fn unregister_aot_actor(id: u64) {
    AOT_ACTORS.with(|c| {
        c.borrow_mut().remove(&id);
    });
}

/// `Actor::register_behavior` handler that runs the actor's current message
/// through AOT-compiled native code, bypassing the bytecode VM.
///
/// Supports behaviors with 0 or 1 boxed params (the counter/state accessor
/// cases exercised by the AOT tests). Higher arities panic with a clear
/// message — extend the arity match as needed.
pub fn aot_behavior_adapter(actor: &mut crate::runtime::Actor, args: &[crate::vm::Value]) {
    let ptr = AOT_BEHAVIOR_TARGET.with(|c| *c.borrow());
    assert!(
        !ptr.is_null(),
        "aot_behavior_adapter: no native target armed (call set_aot_behavior_target first)"
    );
    // SAFETY: `actor` outlives the native call; `cb` holds a raw pointer to it
    // (mirroring `BytecodeRuntimeCallbacks`) so the `dyn ActorVmCallbacks` fat
    // pointer coerces to `'static` for the thread-local, and is cleared before
    // `cb` (and the borrow) ends.
    let mut cb = AotActorCallbacks {
        actor: actor as *mut crate::runtime::Actor,
    };
    unsafe { crate::jit::runtime::set_jit_callbacks(&mut cb) };
    let raw = args.iter().map(|v| v.as_raw()).collect::<Vec<_>>();
    // SAFETY (each arm): `ptr` is a finalized AOT behavior with this arity.
    match raw.len() {
        0 => {
            let f: extern "C" fn() -> u64 = unsafe { std::mem::transmute(ptr) };
            let _ = f();
        }
        1 => {
            let f: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(ptr) };
            let _ = f(raw[0]);
        }
        2 => {
            let f: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(ptr) };
            let _ = f(raw[0], raw[1]);
        }
        3 => {
            let f: extern "C" fn(u64, u64, u64) -> u64 = unsafe { std::mem::transmute(ptr) };
            let _ = f(raw[0], raw[1], raw[2]);
        }
        4 => {
            let f: extern "C" fn(u64, u64, u64, u64) -> u64 = unsafe { std::mem::transmute(ptr) };
            let _ = f(raw[0], raw[1], raw[2], raw[3]);
        }
        5 => {
            let f: extern "C" fn(u64, u64, u64, u64, u64) -> u64 =
                unsafe { std::mem::transmute(ptr) };
            let _ = f(raw[0], raw[1], raw[2], raw[3], raw[4]);
        }
        6 => {
            let f: extern "C" fn(u64, u64, u64, u64, u64, u64) -> u64 =
                unsafe { std::mem::transmute(ptr) };
            let _ = f(raw[0], raw[1], raw[2], raw[3], raw[4], raw[5]);
        }
        7 => {
            let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64) -> u64 =
                unsafe { std::mem::transmute(ptr) };
            let _ = f(raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6]);
        }
        8 => {
            let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64) -> u64 =
                unsafe { std::mem::transmute(ptr) };
            let _ = f(raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7]);
        }
        n => {
            crate::jit::runtime::clear_jit_callbacks();
            crate::jit::runtime::aot_clear_constants();
            panic!("aot_behavior_adapter: unsupported arity {} (add an arity arm)", n);
        }
    }
    crate::jit::runtime::clear_jit_callbacks();
    crate::jit::runtime::aot_clear_constants();
}

/// Minimal `ActorVmCallbacks` that routes AOT actor operations (state access,
/// heap allocation) to a single `Actor`. Used by `aot_behavior_adapter` so
/// `StateGet`/`StateSet` and object allocation inside a native behavior body
/// target the right actor. Spawn/Send are unsupported in the standalone
/// native path (they need the full `Runtime`).
struct AotActorCallbacks {
    /// Raw pointer to the actor, kept alive by the caller across the native
    /// call. Mirrors `BytecodeRuntimeCallbacks` (raw `*mut Runtime`) so the
    /// fat pointer stored in `JIT_CALLBACKS` is `'static`.
    actor: *mut crate::runtime::Actor,
}

impl std::fmt::Debug for AotActorCallbacks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AotActorCallbacks(actor={:p})", self.actor)
    }
}

impl crate::vm::ActorVmCallbacks for AotActorCallbacks {
    fn current_actor_id(&self) -> Option<u64> {
        // SAFETY: `actor` is the caller's live `&mut Actor`.
        Some(unsafe { (*self.actor).id })
    }

    fn alloc(&mut self, size: usize, type_tag: HeapTypeTag) -> Option<*mut u8> {
        // SAFETY: `actor` is the caller's live `&mut Actor`.
        unsafe { (*self.actor).heap.alloc(size, type_tag) }
    }

    fn drop_ref(&mut self, ptr: *mut u8) {
        // SAFETY: both raw pointers are valid; `ptr` is from this actor's heap.
        unsafe { (*self.actor).orca_gc.drop_local_ref(&mut (*self.actor).heap, ptr) };
    }

    fn retain_ref(&mut self, ptr: *mut u8) {
        // SAFETY: both raw pointers are valid; `ptr` is from this actor's heap.
        unsafe { (*self.actor).orca_gc.local_ref(&(*self.actor).heap, ptr) };
    }

    fn array_len(&self, ptr: *mut u8) -> Option<usize> {
        // SAFETY: `ptr` is a valid heap pointer from this actor's heap.
        unsafe {
            let header = &*crate::runtime::heap::ActorHeap::header_of(ptr);
            if header.type_tag == HeapTypeTag::Array {
                let payload =
                    header.size.saturating_sub(crate::runtime::heap::ActorHeap::HEADER_SIZE);
                Some(payload / std::mem::size_of::<crate::vm::Value>())
            } else {
                None
            }
        }
    }

    fn get_state_field(&self, field: &str) -> crate::vm::Value {
        // SAFETY: `actor` is the caller's live `&mut Actor`.
        unsafe {
            (*self.actor)
                .get_state_field(field)
                .unwrap_or(crate::vm::Value::nil())
        }
    }

    fn set_state_field(&mut self, field: &str, value: crate::vm::Value) {
        // SAFETY: `actor` is the caller's live `&mut Actor`.
        unsafe { (*self.actor).set_state_field(field, value) };
    }

    fn spawn_actor(
        &mut self,
        _module: &crate::bytecode::CodeModule,
        _behavior_idx: usize,
        _init: Vec<(String, crate::vm::Value)>,
    ) -> crate::vm::Value {
        crate::vm::Value::actor_ref(0)
    }

    fn try_receive(&mut self) -> Option<(u16, crate::vm::Value)> {
        // SAFETY: `actor` is the caller's live `&mut Actor`; mailbox access
        // runs on the owning thread (the standalone driver's dispatcher).
        unsafe { (*self.actor).mailbox.pop() }.map(|msg| {
            let first = msg
                .payload
                .first()
                .copied()
                .unwrap_or(crate::vm::Value::nil());
            (msg.behavior_id, first)
        })
    }

    fn try_receive_match(
        &mut self,
        behavior_ids: &[u16],
    ) -> Option<(usize, Vec<crate::vm::Value>)> {
        // SAFETY: `actor` is the caller's live `&mut Actor`; mailbox access
        // runs on the owning thread (the standalone driver's dispatcher).
        unsafe { (*self.actor).mailbox.receive_match(behavior_ids) }
            .map(|(pos, payload)| (pos, payload.to_vec()))
    }

    fn send_message(
        &mut self,
        target: crate::vm::Value,
        behavior_id: u16,
        args: &[crate::vm::Value],
    ) {
        let Some(target_id) = target.as_actor_id() else {
            return;
        };
        // SAFETY: registry entries are registered by the driver and unregistered
        // before the actor drops; the pointer is valid for this dispatch.
        let target_actor = AOT_ACTORS.with(|c| c.borrow().get(&target_id).copied());
        let Some(target_actor) = target_actor else {
            return;
        };
        unsafe {
            let _ = (*target_actor).mailbox.push_local(crate::runtime::Message {
                behavior_id,
                payload: std::sync::Arc::new(args.to_vec()),
                sender: (*self.actor).id,
                priority: crate::runtime::MessagePriority::Normal,
                trace_id: None,
            });
        }
    }
}

/// Create an ISA builder for the specified target.
fn create_isa_builder(target: &str) -> NuResult<isa::Builder> {
    use target_lexicon::Triple;

    match target {
        "native" => cranelift_native::builder().map_err(|msg| crate::types::NuError::VMError {
            msg: format!("host machine not supported: {}", msg),
            span: Span::default(),
        }),
        "ptx" | "nvptx64" => {
            // PTX (NVIDIA GPU) target
            let triple: Triple =
                "nvptx64-nvidia-cuda"
                    .parse()
                    .map_err(|e| crate::types::NuError::VMError {
                        msg: format!("invalid PTX triple: {}", e),
                        span: Span::default(),
                    })?;
            isa::lookup(triple).map_err(|e| crate::types::NuError::VMError {
                msg: format!("PTX target not supported: {}", e),
                span: Span::default(),
            })
        }
        "riscv64" | "riscv" => {
            // RISC-V 64-bit target
            let triple: Triple = "riscv64gc-unknown-none-elf".parse().map_err(|e| {
                crate::types::NuError::VMError {
                    msg: format!("invalid RISC-V triple: {}", e),
                    span: Span::default(),
                }
            })?;
            isa::lookup(triple).map_err(|e| crate::types::NuError::VMError {
                msg: format!("RISC-V target not supported: {}", e),
                span: Span::default(),
            })
        }
        _ => Err(crate::types::NuError::VMError {
            msg: format!(
                "unknown target '{}' (expected native | ptx | riscv64)",
                target
            ),
            span: Span::default(),
        }),
    }
}

/// Register all runtime helper symbols with the JIT builder.
/// Single source of truth: `src/jit/helpers.rs` `define_helpers!` macro.
fn register_runtime_helpers(builder: &mut JITBuilder) {
    crate::jit::helpers::register_with_builder(builder);
}

/// Scan MIR statements to collect field names and string constants.
fn collect_field_and_consts(
    stmt: &mir::Stmt,
    field_map: &mut std::collections::HashMap<String, u8>,
    next_field_id: &mut u8,
    constants: &mut Vec<crate::bytecode::Constant>,
) {
    match stmt {
        mir::Stmt::Assign { op, .. } => {
            collect_rvalue_field_and_consts(op, field_map, next_field_id, constants);
        }
        mir::Stmt::StoreFieldNamed { field, .. } => {
            field_map.entry(field.clone()).or_insert_with(|| {
                let id = *next_field_id;
                *next_field_id = next_field_id.saturating_add(1);
                id
            });
        }
        mir::Stmt::StateSet { field, .. } => {
            let c = crate::bytecode::Constant::String(field.clone());
            if !constants.contains(&c) {
                constants.push(c);
            }
        }
        _ => {}
    }
}

fn collect_rvalue_field_and_consts(
    rv: &mir::RValue,
    field_map: &mut std::collections::HashMap<String, u8>,
    next_field_id: &mut u8,
    constants: &mut Vec<crate::bytecode::Constant>,
) {
    match rv {
        mir::RValue::Const(c) => {
            if let crate::bytecode::Constant::String(_) = c {
                // Add string constant to pool, returning index
                constants.push(c.clone());
            }
        }
        mir::RValue::Record(fields)
        | mir::RValue::RecordUpdate {
            overrides: fields, ..
        } => {
            for (name, _) in fields {
                field_map.entry(name.clone()).or_insert_with(|| {
                    let id = *next_field_id;
                    *next_field_id = next_field_id.saturating_add(1);
                    id
                });
            }
        }
        mir::RValue::LoadFieldNamed { field, .. } => {
            field_map.entry(field.clone()).or_insert_with(|| {
                let id = *next_field_id;
                *next_field_id = next_field_id.saturating_add(1);
                id
            });
        }
        mir::RValue::Spawn { init, .. } => {
            for (name, rv) in init {
                field_map.entry(name.clone()).or_insert_with(|| {
                    let id = *next_field_id;
                    *next_field_id = next_field_id.saturating_add(1);
                    id
                });
                // Intern the init field name so native spawn code can resolve
                // it back to a string via the constant pool.
                let c = crate::bytecode::Constant::String(name.clone());
                if !constants.contains(&c) {
                    constants.push(c);
                }
                collect_rvalue_field_and_consts(rv, field_map, next_field_id, constants);
            }
        }
        mir::RValue::StateGet { field } => {
            let c = crate::bytecode::Constant::String(field.clone());
            if !constants.contains(&c) {
                constants.push(c);
            }
        }
        _ => {}
    }
}
