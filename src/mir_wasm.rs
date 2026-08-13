//! WASM backend: compiles MIR directly to WebAssembly bytecode.
//!
//! Lowers `mir::Module` → `.wasm` binary via `wasm-encoder`. Values are
//! represented as `i64` using the i64-tagged encoding from `value_layout`.
//!
//! # Effect handling
//!
//! Built-in effects (`IO.print`, `IO.read`, etc.) compile to host imports.
//! User-defined effect handlers (`EnterHandle`/`PopHandler`/`Resume`) are
//! stubbed — they need the CPS transform or WasmFX for full support.

use crate::mir::{self, BlockId, FuncRef, LocalId, RValue, Stmt, Terminator};
use crate::types::NuResult;
use crate::value_layout;
use std::collections::HashMap;
use wasm_encoder::*;
// ── Import / type index constants (used by import/type builders) ───
// Note: import indices count all imports, but function indices only
// count function imports. The memory import (index 0) is NOT a function,
// so function indices start at 0 while import indices start at 1.

const IMPORT_ALLOC_IDX: u32 = 0; // function index of nulang_alloc

/// Function index of `env.nulang_dispatch` — generic effect dispatch
/// (i32, i32, i32, i32) -> i64 (result-length return).
const IMPORT_NULANG_DISPATCH: u32 = 1;

/// Linear-memory base of the host's effect-result ring buffer. Must match
/// `ActorCtx::ring_buffer_base` in nulang-cloud's wasmtime-actor-pool
/// (0x1000). The host writes the dispatch result here and returns its
/// length; the guest reads it back from this fixed address.
pub(crate) const RING_BUFFER_BASE: u32 = 0x1000;

/// Function index of `env.io_print` — used in `Call` instructions.
const IMPORT_IO_PRINT: u32 = 3;
/// Function index of `env.io_read` — used in `Call` instructions.
const IMPORT_IO_READ: u32 = 4;
/// Function index of `env.str_concat` — string concatenation (i64, i64) -> i64.
const IMPORT_STR_CONCAT: u32 = 5;
/// Function index of `env.str_eq` — string content equality (i64, i64) -> i64.
const IMPORT_STR_EQ: u32 = 6;
/// Function index of `env.pow` — integer exponentiation (i64, i64) -> i64.
const IMPORT_POW: u32 = 7;
/// Function index of `env.arith_add` — float/int add (i64, i64) -> i64.
const IMPORT_ARITH_ADD: u32 = 8;
const IMPORT_ARITH_SUB: u32 = 9;
const IMPORT_ARITH_MUL: u32 = 10;
const IMPORT_ARITH_DIV: u32 = 11;
const IMPORT_ARITH_MOD: u32 = 12;
/// Function index of `env.arith_cmp` — float/int comparison (i64, i64, i64) -> i64.
const IMPORT_ARITH_CMP: u32 = 13;
/// Function index of `env.arith_neg` — unary negation (i64) -> i64.
const IMPORT_ARITH_NEG: u32 = 14;
/// Function index of `env.arith_fneg` — VM FNeg semantics (i64) -> i64.
/// Kept after the existing imports so older indices remain stable.
const IMPORT_ARITH_FNEG: u32 = 21;
/// Function index of `env.arr_load` — bounds-checked array load (i64, i64) -> i64.
const IMPORT_ARR_LOAD: u32 = 15;
/// Function index of `env.ffi_call_0` — foreign call (lib, sym, sig) -> i64.
const IMPORT_FFI_CALL_0: u32 = 16;
const IMPORT_FFI_CALL_1: u32 = 17;
const IMPORT_FFI_CALL_2: u32 = 18;
const IMPORT_FFI_CALL_3: u32 = 19;
const IMPORT_FFI_CALL_4: u32 = 20;
/// Number of function imports. Module-defined functions start at this index.
const FUNC_IMPORT_COUNT: u32 = 22;

const TY_VOID_TO_I64: u32 = 0;

/// (i64, i64) -> i64 — used by `env.str_concat`.
const TY_I64I64_TO_I64: u32 = 2;
/// (i64) -> i64 — used by `env.arith_neg`.
const TY_I64_TO_I64: u32 = 1;

/// (i64, i64, i64) -> i64 — used by `env.arith_cmp`.
const TY_I64I64I64_TO_I64: u32 = 3;
const TY_I32I32_TO_I64: u32 = 4;
const TY_FIXED_COUNT: u32 = 5;

// ── WasmBackend ──────────────────────────────────────────────────────

pub struct WasmBackend {
    types: TypeSection,
    imports: ImportSection,
    functions: FunctionSection,
    exports: ExportSection,
    codes: CodeSection,
    data: DataSection,
    /// Accumulated data-segment bytes for interned strings.
    string_data: Vec<u8>,
    /// String content → (offset in data segment, length).
    interned: HashMap<String, (u32, u32)>,
    func_index_map: HashMap<usize, u32>,
    next_func_idx: u32,
    func_types: HashMap<Vec<ValType>, u32>,
    next_type_idx: u32,
    /// Module-wide record field name → slot index map. Built by pre-scanning
    /// the MIR; used so `Record` literals and `LoadFieldNamed` agree on slot
    /// positions.
    field_map: HashMap<String, u8>,
    /// Foreign function declarations, indexed by `RValue::FFICall.idx`.
    foreign_functions: Vec<mir::ForeignFunction>,
}

/// Resolve a MIR local to its defining constant, if it is assigned exactly
/// once from `RValue::Const`. Used to pre-compute effect-dispatch JSON at
/// compile time — the WASM backend requires constant effect args (dynamic
/// args are a loud compile error, not a silent nil).
fn resolve_const(func: &mir::Function, local: mir::LocalId) -> Option<crate::bytecode::Constant> {
    use crate::mir::Stmt;
    let mut found = None;
    for block in &func.blocks {
        for stmt in &block.stmts {
            if let Stmt::Assign { dst, op } = stmt {
                if *dst == local {
                    // Multiple assignments (a `var` mutated along the way)
                    // mean the value is not a compile-time constant.
                    if found.is_some() {
                        return None;
                    }
                    if let RValue::Const(c) = op {
                        found = Some(c.clone());
                    } else {
                        return None;
                    }
                }
            }
        }
    }
    found
}

/// JSON-encode one constant for an effect-dispatch payload.
fn json_arg(c: &crate::bytecode::Constant) -> Option<String> {
    use crate::bytecode::Constant;
    match c {
        Constant::Int(n) => Some(n.to_string()),
        Constant::Float(f) => {
            let mut s = f.to_string();
            // JSON requires a float-looking literal; Rust prints 42.0 as
            // "42", which is a valid JSON int but the WRONG type.
            if !s.contains('.') && !s.contains('e') && !s.contains('E') {
                s.push_str(".0");
            }
            Some(s)
        }
        Constant::Bool(true) => Some("true".into()),
        Constant::Bool(false) => Some("false".into()),
        Constant::String(s) => Some(json_quote(s)),
        Constant::Nil | Constant::Unit => Some("null".into()),
        _ => None, // TypeDescriptor, FunctionRef, BehaviorRef
    }
}

/// JSON-quote a string (escape `"`, `\`, and control characters).
fn json_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl WasmBackend {
    pub fn new() -> Self {
        let mut types = TypeSection::new();
        types.ty().function([], [ValType::I64]); // 0
        types.ty().function([ValType::I64], [ValType::I64]); // 1
        types
            .ty()
            .function([ValType::I64, ValType::I64], [ValType::I64]); // 2
        types
            .ty()
            .function([ValType::I64, ValType::I64, ValType::I64], [ValType::I64]); // 3
        types
            .ty()
            .function([ValType::I32, ValType::I32], [ValType::I64]); // 4

        let mut imports = ImportSection::new();
        imports.import(
            "env",
            "memory",
            MemoryType {
                minimum: 1,
                maximum: None,
                memory64: false,
                shared: false,
                page_size_log2: None,
            },
        );
        imports.import("env", "nulang_alloc", EntityType::Function(TY_VOID_TO_I64)); // placeholder type — rebuilt in rebuild_imports()
        imports.import(
            "env",
            "nulang_dispatch",
            EntityType::Function(TY_VOID_TO_I64),
        ); // placeholder type — rebuilt in rebuild_imports()
        imports.import("env", "log", EntityType::Function(TY_I32I32_TO_I64));
        imports.import("env", "io_print", EntityType::Function(TY_I32I32_TO_I64));
        imports.import("env", "io_read", EntityType::Function(TY_VOID_TO_I64));

        // Placeholder import type indices are fixed up in `rebuild_imports()`
        // after the type section is finalized, so the constructor uses
        // provisional types here.

        WasmBackend {
            types,
            imports,
            functions: FunctionSection::new(),
            exports: ExportSection::new(),
            codes: CodeSection::new(),
            data: DataSection::new(),
            string_data: Vec::new(),
            interned: HashMap::new(),
            func_index_map: HashMap::new(),
            next_func_idx: FUNC_IMPORT_COUNT,
            func_types: HashMap::new(),
            next_type_idx: TY_FIXED_COUNT,
            field_map: HashMap::new(),
            foreign_functions: Vec::new(),
        }
    }

    /// Intern a string into the data segment. Returns (offset, len) in
    /// the data section. The WASM module's memory must be initialized
    /// with this data at the given offset.
    fn intern_string(&mut self, s: &str) -> (u32, u32) {
        if let Some(&entry) = self.interned.get(s) {
            return entry;
        }
        let offset = self.string_data.len() as u32;
        let len = s.len() as u32;
        self.string_data.extend_from_slice(s.as_bytes());
        // Null-terminate so the host `str_concat` helper can recover each
        // string's length with a `strlen` scan. The value's offset and the
        // explicit `len` reported to `io_print` are unchanged by the byte.
        self.string_data.push(0);
        self.interned.insert(s.to_string(), (offset, len));
        (offset, len)
    }

    // ── Compile ───────────────────────────────────────────────────

    pub fn compile(&mut self, mir: &mir::Module, _module_name: &str) -> NuResult<Vec<u8>> {
        self.foreign_functions = mir.foreign_functions.clone();
        // Pre-scan: build the module-wide record field name → slot index map
        // (mirrors the AOT backend) so Record literals and LoadFieldNamed agree.
        let mut field_map: std::collections::HashMap<String, u8> = std::collections::HashMap::new();
        let mut next_field_id: u8 = 0;
        for func in mir.functions.iter().chain(mir.behaviors.iter()) {
            for block in &func.blocks {
                for stmt in &block.stmts {
                    collect_wasm_fields(stmt, &mut field_map, &mut next_field_id);
                }
            }
        }
        self.field_map = field_map;

        // Pre-scan: the WASM backend does not currently support closures (first-class functions).
        // Return a compilation error so callers (like the differential fuzzer) know it's unsupported.
        for func in mir.functions.iter().chain(mir.behaviors.iter()) {
            for block in &func.blocks {
                for stmt in &block.stmts {
                    if let Stmt::Assign { op, .. } = stmt {
                        if let RValue::Call {
                            func: FuncRef::Local(_),
                            ..
                        } = op
                        {
                            return Err(crate::types::NuError::VMError {
                                msg: "WASM backend does not support closures (FuncRef::Local)"
                                    .into(),
                                span: crate::types::Span::default(),
                            });
                        }
                        // Reject RValues the standalone WASM runtime has no
                        // actor/effect machinery for — they previously silently
                        // compiled to nil. Fail loudly at compile time instead.
                        let unsupported = match op {
                            RValue::Send { .. }
                            | RValue::Spawn { .. }
                            | RValue::Ask { .. }
                            | RValue::Receive
                            | RValue::ReceiveMatch { .. }
                            | RValue::ReceiveWait { .. }
                            | RValue::ReceiveCommit
                            | RValue::Migrate { .. }
                            | RValue::PerformAsync { .. }
                            | RValue::SignalWait { .. }
                            | RValue::Closure { .. } => true,
                            // The borrow (`&`) and dereference operators touch
                            // reference capabilities which are compile-time
                            // only (no runtime representation in the WASM VM).
                            RValue::Unary(crate::ast::UnOp::Ref(_), _)
                            | RValue::Unary(crate::ast::UnOp::Deref, _) => true,
                            // IO.print/println/read and Array.length keep
                            // dedicated imports; every OTHER effect dispatches
                            // through nulang_dispatch (handled below — not
                            // unsupported).
                            RValue::Perform { .. } => false,
                            _ => false,
                        };
                        if unsupported {
                            return Err(crate::types::NuError::VMError {
                                msg: "WASM backend does not support this actor/effect operation"
                                    .into(),
                                span: crate::types::Span::default(),
                            });
                        }
                        // Effect dispatch pre-scan: `perform Effect.op(args)`
                        // lowers to `nulang_dispatch(tag, payload)` where the
                        // payload is a compile-time JSON array of the args.
                        // Constant args are interned here (intern_string needs
                        // `&mut self`, compile_rvalue has only `&self`);
                        // dynamic args are a loud compile error.
                        if let RValue::Perform {
                            effect, op, args, ..
                        } = op
                        {
                            let dispatchable = !matches!(
                                (effect.as_str(), op.as_str()),
                                ("IO", "print")
                                    | ("IO", "println")
                                    | ("IO", "read")
                                    | ("Array", "length")
                            );
                            if dispatchable {
                                let consts = args
                                    .iter()
                                    .map(|l| resolve_const(func, *l))
                                    .collect::<Option<Vec<_>>>();
                                let Some(consts) = consts else {
                                    return Err(crate::types::NuError::VMError {
                                        msg: format!(
                                            "WASM backend: effect {effect}.{op} requires constant \
                                             args (dynamic effect args are not yet supported in \
                                             the WASM backend)"
                                        ),
                                        span: crate::types::Span::default(),
                                    });
                                };
                                let encoded =
                                    consts.iter().map(json_arg).collect::<Option<Vec<_>>>();
                                let Some(encoded) = encoded else {
                                    return Err(crate::types::NuError::VMError {
                                        msg: format!(
                                            "WASM backend: effect {effect}.{op} has an arg that \
                                             is not JSON-encodable (int/float/bool/string/nil \
                                             only)"
                                        ),
                                        span: crate::types::Span::default(),
                                    });
                                };
                                let tag = format!("{effect}.{op}");
                                self.intern_string(&tag);
                                self.intern_string(&format!("[{}]", encoded.join(",")));
                            }
                        }
                        self.intern_const_strings(op);
                    }
                }
            }
        }
        // Pre-intern FFI library/symbol strings so compile_rvalue (which has
        // only `&self`) can look them up by content.
        for ff in &mir.foreign_functions {
            self.intern_string(&ff.library);
            self.intern_string(&ff.symbol);
        }

        // Register function types.
        for func in &mir.functions {
            self.register_function_type(func);
        }
        for func in &mir.behaviors {
            self.register_function_type(func);
        }

        // Rebuild imports with correct type indices now that types are
        // finalized.
        self.rebuild_imports();

        // Compile functions.
        for (idx, func) in mir.functions.iter().enumerate() {
            self.compile_function(func, idx);
        }
        for (idx, func) in mir.behaviors.iter().enumerate() {
            self.compile_function(func, mir.functions.len() + idx);
        }

        if !mir.functions.is_empty() {
            // Export the actual entry function as `nulang_init`. Lifted closure
            // functions are appended after `__main`, so `len()-1` can point at
            // a closure carrying parameters, which the host rejects when it
            // looks for a `() -> i64` export.
            if let Some(main_in_module) = mir
                .functions
                .iter()
                .position(|f| f.name == "__main" || f.name == "main")
            {
                let main_idx = FUNC_IMPORT_COUNT + main_in_module as u32;
                self.exports
                    .export("nulang_init", ExportKind::Func, main_idx);
            } else {
                // Library module (no entry expression): export a synthetic
                // `() -> i64` function returning nil, matching the interpreter
                // (a program with only function definitions evaluates to nil).
                // Falling back to the last module function could be a
                // parameterized one, which the host can't call as `() -> i64`.
                self.emit_nil_entry();
            }
        }

        // Emit data segment.
        if !self.string_data.is_empty() {
            self.data
                .active(0, &ConstExpr::i32_const(0), self.string_data.clone());
        }

        // Build module.
        let mut module = Module::new();
        module.section(&self.types);
        module.section(&self.imports);
        module.section(&self.functions);
        module.section(&self.exports);
        module.section(&self.codes);
        module.section(&self.data);
        Ok(module.finish())
    }

    fn intern_const_strings(&mut self, rvalue: &RValue) {
        if let RValue::Const(crate::bytecode::Constant::String(s)) = rvalue {
            self.intern_string(s);
        }
    }

    fn rebuild_imports(&mut self) {
        use wasm_encoder::ValType;
        // Alloc: (i32) -> i32
        let ty_alloc = self.ensure_type(vec![ValType::I32], vec![ValType::I32]);
        // Dispatch: (i32, i32, i32, i32) -> i64 (bytes of effect result
        // written to the ring buffer; 0 = no result/no handler). Mirrors
        // io_read's length-return contract so the compiler lowering can
        // read the result back from linear memory.
        let ty_dispatch = self.ensure_type(vec![ValType::I32; 4], vec![ValType::I64]);

        let mut imports = ImportSection::new();
        imports.import(
            "env",
            "memory",
            MemoryType {
                minimum: 1,
                maximum: None,
                memory64: false,
                shared: false,
                page_size_log2: None,
            },
        );
        imports.import("env", "nulang_alloc", EntityType::Function(ty_alloc));
        imports.import("env", "nulang_dispatch", EntityType::Function(ty_dispatch));
        imports.import("env", "log", EntityType::Function(TY_I32I32_TO_I64));
        imports.import("env", "io_print", EntityType::Function(TY_I32I32_TO_I64));
        imports.import("env", "io_read", EntityType::Function(TY_VOID_TO_I64));
        imports.import("env", "str_concat", EntityType::Function(TY_I64I64_TO_I64));
        imports.import("env", "str_eq", EntityType::Function(TY_I64I64_TO_I64));
        imports.import("env", "pow", EntityType::Function(TY_I64I64_TO_I64));
        imports.import("env", "arith_add", EntityType::Function(TY_I64I64_TO_I64));
        imports.import("env", "arith_sub", EntityType::Function(TY_I64I64_TO_I64));
        imports.import("env", "arith_mul", EntityType::Function(TY_I64I64_TO_I64));
        imports.import("env", "arith_div", EntityType::Function(TY_I64I64_TO_I64));
        imports.import("env", "arith_mod", EntityType::Function(TY_I64I64_TO_I64));
        imports.import(
            "env",
            "arith_cmp",
            EntityType::Function(TY_I64I64I64_TO_I64),
        );
        imports.import("env", "arith_neg", EntityType::Function(TY_I64_TO_I64));
        imports.import("env", "arr_load", EntityType::Function(TY_I64I64_TO_I64));
        // ffi_call_N(lib, sym, sig, arg0..argN-1) -> i64.
        let ffi0 = self.ensure_type(vec![ValType::I64; 3], vec![ValType::I64]);
        let ffi1 = self.ensure_type(vec![ValType::I64; 4], vec![ValType::I64]);
        let ffi2 = self.ensure_type(vec![ValType::I64; 5], vec![ValType::I64]);
        let ffi3 = self.ensure_type(vec![ValType::I64; 6], vec![ValType::I64]);
        let ffi4 = self.ensure_type(vec![ValType::I64; 7], vec![ValType::I64]);
        imports.import("env", "ffi_call_0", EntityType::Function(ffi0));
        imports.import("env", "ffi_call_1", EntityType::Function(ffi1));
        imports.import("env", "ffi_call_2", EntityType::Function(ffi2));
        imports.import("env", "ffi_call_3", EntityType::Function(ffi3));
        imports.import("env", "ffi_call_4", EntityType::Function(ffi4));
        // Keep this new import at the end so existing function indices stay stable.
        imports.import("env", "arith_fneg", EntityType::Function(TY_I64_TO_I64));
        self.imports = imports;
    }

    fn ensure_type(&mut self, params: Vec<ValType>, results: Vec<ValType>) -> u32 {
        // Always add a new type — simple, correct, minimal overhead.
        let idx = self.next_type_idx;
        self.next_type_idx += 1;
        if results.is_empty() {
            self.types.ty().function(params, []);
        } else {
            self.types.ty().function(params, results);
        }
        idx
    }

    // ── Function type registration ─────────────────────────────────

    fn register_function_type(&mut self, func: &mir::Function) {
        let count = func.params.len() + func.captures.len();
        let param_types: Vec<ValType> = vec![ValType::I64; count];
        if self.func_types.contains_key(&param_types) {
            return;
        }
        let type_idx = self.next_type_idx;
        self.next_type_idx += 1;
        self.func_types.insert(param_types.clone(), type_idx);
        if param_types.is_empty() {
            self.types.ty().function([], [ValType::I64]);
        } else {
            self.types.ty().function(param_types, [ValType::I64]);
        }
    }

    fn func_type_idx(&self, func: &mir::Function) -> u32 {
        let count = func.params.len() + func.captures.len();
        let param_types: Vec<ValType> = vec![ValType::I64; count];
        self.func_types.get(&param_types).copied().unwrap_or(0)
    }

    // ── Function compilation ───────────────────────────────────────

    /// Emit a synthetic `() -> i64` function returning nil and export it as
    /// `nulang_init` — the entry for a module with no `__main`/`main`.
    fn emit_nil_entry(&mut self) {
        let wasm_idx = self.next_func_idx;
        self.next_func_idx += 1;
        self.functions.function(TY_VOID_TO_I64);
        let mut body = Function::new(vec![]);
        body.instruction(&Instruction::I64Const(value_layout::TAG_NIL as i64));
        body.instruction(&Instruction::End); // function end
        self.codes.function(&body);
        self.exports
            .export("nulang_init", ExportKind::Func, wasm_idx);
    }

    fn compile_function(&mut self, func: &mir::Function, mir_idx: usize) {
        let wasm_idx = self.next_func_idx;
        self.next_func_idx += 1;
        self.func_index_map.insert(mir_idx, wasm_idx);
        self.functions.function(self.func_type_idx(func));

        let _local_count = func.locals.len() + func.params.len() + func.captures.len();
        let wasm_locals: Vec<_> = vec![(256u32, ValType::I64)];
        let mut body = Function::new(wasm_locals);

        let block_order: Vec<BlockId> = (0..func.blocks.len() as u32).map(BlockId).collect();
        let mut labels: HashMap<BlockId, u32> = HashMap::new();
        for (li, &bid) in block_order.iter().enumerate() {
            labels.insert(bid, li as u32);
        }

        let vec_loops = crate::mir_wasm_simd::find_vectorizable_loops(func);
        let vec_body_to_loop: HashMap<BlockId, &crate::mir_wasm_simd::VecLoop> =
            vec_loops.iter().map(|l| (l.body, l)).collect();

        let state_local = 251u32;
        body.instruction(&Instruction::I64Const(0));
        body.instruction(&Instruction::LocalSet(state_local));

        body.instruction(&Instruction::Loop(BlockType::Empty));

        for _ in &block_order {
            body.instruction(&Instruction::Block(BlockType::Empty));
        }

        body.instruction(&Instruction::LocalGet(state_local));
        body.instruction(&Instruction::I32WrapI64);
        let targets: Vec<u32> = (0..block_order.len() as u32).collect();
        body.instruction(&Instruction::BrTable(
            std::borrow::Cow::Owned(targets.clone()),
            targets.last().copied().unwrap_or(0),
        ));

        for (li, &bid) in block_order.iter().enumerate() {
            let li = li as u32;
            body.instruction(&Instruction::End); // end block

            let block = &func.blocks[bid.0 as usize];
            if let Some(vloop) = vec_body_to_loop.get(&bid) {
                self.compile_simd_body(&mut body, vloop, func);
            } else {
                for stmt in &block.stmts {
                    self.compile_stmt(&mut body, stmt, func);
                }
            }

            match &block.terminator {
                Terminator::Return(Some(l)) => {
                    body.instruction(&Instruction::LocalGet(self.mir_local(l, func)));
                    body.instruction(&Instruction::Return);
                }
                Terminator::Return(None) => {
                    body.instruction(&Instruction::I64Const(crate::value_layout::TAG_UNIT as i64));
                    body.instruction(&Instruction::Return);
                }
                Terminator::Jump(t) => {
                    let tl = labels.get(t).copied().unwrap_or(0);
                    if tl > li {
                        body.instruction(&Instruction::Br(tl - li - 1));
                    } else {
                        body.instruction(&Instruction::I64Const(tl as i64));
                        body.instruction(&Instruction::LocalSet(state_local));
                        body.instruction(&Instruction::Br(
                            (block_order.len() - 1 - li as usize) as u32,
                        ));
                    }
                }
                Terminator::Branch { cond, then_, else_ } => {
                    body.instruction(&Instruction::LocalGet(self.mir_local(cond, func)));
                    body.instruction(&Instruction::I64Const(
                        crate::value_layout::tag_bool(false) as i64
                    ));
                    body.instruction(&Instruction::I64Ne);
                    body.instruction(&Instruction::If(BlockType::Empty));

                    let tl = labels.get(then_).copied().unwrap_or(0);
                    if tl > li {
                        body.instruction(&Instruction::Br(tl - li));
                    } else {
                        body.instruction(&Instruction::I64Const(tl as i64));
                        body.instruction(&Instruction::LocalSet(state_local));
                        body.instruction(&Instruction::Br(
                            (block_order.len() - li as usize) as u32,
                        ));
                    }

                    body.instruction(&Instruction::Else);

                    let el = labels.get(else_).copied().unwrap_or(0);
                    if el > li {
                        body.instruction(&Instruction::Br(el - li));
                    } else {
                        body.instruction(&Instruction::I64Const(el as i64));
                        body.instruction(&Instruction::LocalSet(state_local));
                        body.instruction(&Instruction::Br(
                            (block_order.len() - li as usize) as u32,
                        ));
                    }

                    body.instruction(&Instruction::End); // end If
                }
                Terminator::Resume(_) | Terminator::Unterminated => {
                    body.instruction(&Instruction::I64Const(crate::value_layout::TAG_NIL as i64));
                    body.instruction(&Instruction::Return);
                }
            }
        }

        body.instruction(&Instruction::End); // end Loop
        body.instruction(&Instruction::I64Const(crate::value_layout::TAG_NIL as i64)); // default return
        body.instruction(&Instruction::End); // function end

        self.codes.function(&body);
    }

    fn compile_simd_body(
        &self,
        body: &mut Function,
        vloop: &crate::mir_wasm_simd::VecLoop,
        func: &mir::Function,
    ) {
        let pm = value_layout::PAYLOAD_MASK as i64;
        let i_loc = self.mir_local(&vloop.induction, func);
        let a_loc = self.mir_local(&vloop.array_a, func);
        let b_loc = self.mir_local(&vloop.array_b, func);
        let c_loc = self.mir_local(&vloop.array_c, func);

        // We use Wasm locals 255, 254, 253, 252 for scratch.
        // It's safe since MIR limits locals, and Wasm allocation is up to 256.
        let scratch_len = 255;
        let scratch_base_a = 254;
        let scratch_base_b = 253;
        let scratch_base_c = 252;

        // len = ArrayLen(c)
        body.instruction(&Instruction::LocalGet(c_loc));
        body.instruction(&Instruction::I64Const(pm));
        body.instruction(&Instruction::I64And);
        body.instruction(&Instruction::I32WrapI64);
        body.instruction(&Instruction::I64Load(MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        }));
        body.instruction(&Instruction::LocalSet(scratch_len)); // untagged i64 len

        // base_a = a & pm
        body.instruction(&Instruction::LocalGet(a_loc));
        body.instruction(&Instruction::I64Const(pm));
        body.instruction(&Instruction::I64And);
        body.instruction(&Instruction::LocalSet(scratch_base_a));

        // base_b = b & pm
        body.instruction(&Instruction::LocalGet(b_loc));
        body.instruction(&Instruction::I64Const(pm));
        body.instruction(&Instruction::I64And);
        body.instruction(&Instruction::LocalSet(scratch_base_b));

        // base_c = c & pm
        body.instruction(&Instruction::LocalGet(c_loc));
        body.instruction(&Instruction::I64Const(pm));
        body.instruction(&Instruction::I64And);
        body.instruction(&Instruction::LocalSet(scratch_base_c));

        // 1. SIMD strided loop (processes 2 elements per iteration)
        body.instruction(&Instruction::Block(BlockType::Empty)); // block for SIMD exit
        body.instruction(&Instruction::Loop(BlockType::Empty)); // SIMD loop header

        // check i + 1 < len
        body.instruction(&Instruction::LocalGet(i_loc));
        body.instruction(&Instruction::I64Const(pm));
        body.instruction(&Instruction::I64And);
        body.instruction(&Instruction::I64Const(1));
        body.instruction(&Instruction::I64Add);
        body.instruction(&Instruction::LocalGet(scratch_len));
        body.instruction(&Instruction::I64GeS);
        body.instruction(&Instruction::BrIf(1)); // exit SIMD loop

        // compute offset for i: (i + 1) * 8
        let compute_offset = |b: &mut Function| {
            b.instruction(&Instruction::LocalGet(i_loc));
            b.instruction(&Instruction::I64Const(pm));
            b.instruction(&Instruction::I64And);
            b.instruction(&Instruction::I64Const(1));
            b.instruction(&Instruction::I64Add);
            b.instruction(&Instruction::I64Const(8));
            b.instruction(&Instruction::I64Mul);
        };

        // c_addr (push first so it's ready for store later)
        body.instruction(&Instruction::LocalGet(scratch_base_c));
        compute_offset(body);
        body.instruction(&Instruction::I64Add);
        body.instruction(&Instruction::I32WrapI64);

        // a_addr
        body.instruction(&Instruction::LocalGet(scratch_base_a));
        compute_offset(body);
        body.instruction(&Instruction::I64Add);
        body.instruction(&Instruction::I32WrapI64);
        self.emit_simd_load(body, 0);

        // b_addr
        body.instruction(&Instruction::LocalGet(scratch_base_b));
        compute_offset(body);
        body.instruction(&Instruction::I64Add);
        body.instruction(&Instruction::I32WrapI64);
        self.emit_simd_load(body, 0);

        // Untag: mask both loaded v128s with PAYLOAD_MASK
        // PM as two i64 lanes: PAYLOAD_MASK = 0x0000_FFFF_FFFF_FFFF
        // little-endian bytes: [0xFF; 6][0x00; 2][0xFF; 6][0x00; 2]
        static PM128: [u8; 16] = [
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
            0x00, 0x00,
        ];
        self.emit_v128_const(body, &PM128);
        self.emit_v128_and(body); // a_v128 & PM128 → untagged a
        self.emit_v128_const(body, &PM128);
        self.emit_v128_and(body); // b_v128 & PM128 → untagged b

        // binop on untagged values
        if vloop.lane_type.is_float() {
            self.emit_simd_f64x2_binop(body, vloop.op);
        } else {
            self.emit_simd_i64x2_binop(body, vloop.op);
        }

        // Re-tag: OR result with TAG_INT
        // TAG_INT = 0x7FFB_0000_0000_0000
        // LE bytes: [0x00; 6][0xFB, 0x7F][0x00; 6][0xFB, 0x7F]
        static TAG128: [u8; 16] = [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFB, 0x7F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0xFB, 0x7F,
        ];
        self.emit_v128_const(body, &TAG128);
        self.emit_v128_or(body);

        // store
        self.emit_simd_store(body, 0);

        // i += 2 (tagged)
        body.instruction(&Instruction::LocalGet(i_loc));
        body.instruction(&Instruction::I64Const(pm));
        body.instruction(&Instruction::I64And);
        body.instruction(&Instruction::I64Const(2));
        body.instruction(&Instruction::I64Add);
        body.instruction(&Instruction::I64Const(value_layout::TAG_INT as i64));
        body.instruction(&Instruction::I64Or);
        body.instruction(&Instruction::LocalSet(i_loc));

        body.instruction(&Instruction::Br(0)); // loop again
        body.instruction(&Instruction::End); // end Loop
        body.instruction(&Instruction::End); // end Block

        // 2. Scalar epilogue loop (for remaining elements)
        body.instruction(&Instruction::Block(BlockType::Empty)); // block for scalar exit
        body.instruction(&Instruction::Loop(BlockType::Empty)); // scalar loop header

        // check i < len
        body.instruction(&Instruction::LocalGet(i_loc));
        body.instruction(&Instruction::I64Const(pm));
        body.instruction(&Instruction::I64And);
        body.instruction(&Instruction::LocalGet(scratch_len));
        body.instruction(&Instruction::I64GeS);
        body.instruction(&Instruction::BrIf(1)); // exit scalar loop

        // compute offset for i: (i + 1) * 8
        let compute_offset_scalar = |b: &mut Function| {
            b.instruction(&Instruction::LocalGet(i_loc));
            b.instruction(&Instruction::I64Const(pm));
            b.instruction(&Instruction::I64And);
            b.instruction(&Instruction::I64Const(1));
            b.instruction(&Instruction::I64Add);
            b.instruction(&Instruction::I64Const(8));
            b.instruction(&Instruction::I64Mul);
        };

        // c_addr
        body.instruction(&Instruction::LocalGet(scratch_base_c));
        compute_offset_scalar(body);
        body.instruction(&Instruction::I64Add);
        body.instruction(&Instruction::I32WrapI64);

        // a_addr
        body.instruction(&Instruction::LocalGet(scratch_base_a));
        compute_offset_scalar(body);
        body.instruction(&Instruction::I64Add);
        body.instruction(&Instruction::I32WrapI64);
        body.instruction(&Instruction::I64Load(MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        }));

        // b_addr
        body.instruction(&Instruction::LocalGet(scratch_base_b));
        compute_offset_scalar(body);
        body.instruction(&Instruction::I64Add);
        body.instruction(&Instruction::I32WrapI64);
        body.instruction(&Instruction::I64Load(MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        }));

        // scalar binop
        self.emit_binop(body, vloop.op);

        // store scalar
        body.instruction(&Instruction::I64Store(MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        }));

        // i += 1 (tagged)
        body.instruction(&Instruction::LocalGet(i_loc));
        body.instruction(&Instruction::I64Const(pm));
        body.instruction(&Instruction::I64And);
        body.instruction(&Instruction::I64Const(1));
        body.instruction(&Instruction::I64Add);
        body.instruction(&Instruction::I64Const(value_layout::TAG_INT as i64));
        body.instruction(&Instruction::I64Or);
        body.instruction(&Instruction::LocalSet(i_loc));

        body.instruction(&Instruction::Br(0)); // loop again
        body.instruction(&Instruction::End); // end Loop
        body.instruction(&Instruction::End); // end Block
    }

    // ── Statement compilation ──────────────────────────────────────

    fn compile_stmt(&mut self, body: &mut Function, stmt: &Stmt, func: &mir::Function) {
        match stmt {
            Stmt::Assign { dst, op } => {
                self.compile_rvalue(body, op, func);
                body.instruction(&Instruction::LocalSet(self.mir_local(dst, func)));
            }
            Stmt::EnterHandle { .. } | Stmt::PopHandler => {
                // User-defined effect handlers not yet supported.
                // Effect dispatch goes through host imports for built-ins.
                body.instruction(&Instruction::I64Const(value_layout::TAG_NIL as i64));
                body.instruction(&Instruction::Drop);
            }
            Stmt::ArrayStore { arr, idx, src } => {
                self.compile_array_store(body, arr, idx, src, func);
            }
            Stmt::StoreFieldNamed { obj, field, src } => {
                self.compile_field_store(body, obj, field, src, func);
            }
            Stmt::Emit { .. } | Stmt::StateSet { .. } => {
                // Effects and actor state are unsupported in the standalone
                // WASM backend. These statements remain no-ops until the
                // corresponding runtime machinery is implemented.
                body.instruction(&Instruction::I64Const(value_layout::TAG_NIL as i64));
                body.instruction(&Instruction::Drop);
            }
        }
    }

    // ── RValue compilation ─────────────────────────────────────────

    fn compile_rvalue(&self, body: &mut Function, rvalue: &RValue, func: &mir::Function) {
        match rvalue {
            RValue::Const(c) => {
                self.compile_const(body, c);
            }
            RValue::Load(l) => {
                body.instruction(&Instruction::LocalGet(self.mir_local(l, func)));
            }
            RValue::Binary(op, a, b) => {
                body.instruction(&Instruction::LocalGet(self.mir_local(a, func)));
                body.instruction(&Instruction::LocalGet(self.mir_local(b, func)));
                use crate::ast::BinOp;
                // Numeric ops route through host helpers so float operands
                // (raw bit patterns the inline integer path would corrupt) get
                // f64 arithmetic, matching the interpreter. Comparisons too.
                let import = match op {
                    BinOp::Add => Some(IMPORT_ARITH_ADD),
                    BinOp::Sub => Some(IMPORT_ARITH_SUB),
                    BinOp::Mul => Some(IMPORT_ARITH_MUL),
                    BinOp::Div => Some(IMPORT_ARITH_DIV),
                    BinOp::Mod => Some(IMPORT_ARITH_MOD),
                    BinOp::Pow => Some(IMPORT_POW),
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                        let code = match op {
                            BinOp::Eq => 0,
                            BinOp::Ne => 1,
                            BinOp::Lt => 2,
                            BinOp::Gt => 3,
                            BinOp::Le => 4,
                            _ => 5, // Ge
                        };
                        body.instruction(&Instruction::I64Const(code));
                        Some(IMPORT_ARITH_CMP)
                    }
                    _ => None,
                };
                match import {
                    Some(imp) => {
                        body.instruction(&Instruction::Call(imp));
                    }
                    None => {
                        self.emit_binop(body, *op);
                    }
                }
            }
            RValue::Unary(op, a) => {
                self.compile_unary(body, *op, a, func);
            }
            RValue::StrConcat(a, b) => {
                // String concatenation (`s1 + s2`): pass both tagged string
                // values to the host helper, which reads the (null-terminated)
                // bytes from memory, concatenates them into a fresh buffer, and
                // returns the new tagged string value.
                body.instruction(&Instruction::LocalGet(self.mir_local(a, func)));
                body.instruction(&Instruction::LocalGet(self.mir_local(b, func)));
                body.instruction(&Instruction::Call(IMPORT_STR_CONCAT));
            }
            RValue::StringEq(a, b) => {
                // String content equality (`s1 == s2`): compare by text, not by
                // pool/data offset — an interned constant and a runtime concat
                // result with the same text must compare equal. The host helper
                // returns a tagged bool (false when either operand is not a
                // string), mirroring the interpreter's SCmpEq.
                body.instruction(&Instruction::LocalGet(self.mir_local(a, func)));
                body.instruction(&Instruction::LocalGet(self.mir_local(b, func)));
                body.instruction(&Instruction::Call(IMPORT_STR_EQ));
            }
            RValue::Call { func: fr, args } => {
                self.compile_call(body, fr, args, func);
            }
            RValue::Perform {
                effect, op, args, ..
            } => {
                self.compile_perform(body, effect, op, args, func);
            }
            RValue::ArrayLit(elems) => {
                self.compile_array_lit(body, elems, func);
            }
            RValue::ArrayLoad { arr, idx } => {
                self.compile_array_load(body, arr, idx, func);
            }
            RValue::ArrayLen(arr) => {
                self.compile_array_len(body, arr, func);
            }
            RValue::Tuple(elems) => {
                // Tuple: a heap object `[count][elem0]..[elemN-1]` (i64 words),
                // tagged TAG_PTR — mirroring the array literal layout.
                let scratch = 255u32;
                let size = ((elems.len() + 1) * 8) as i32;
                body.instruction(&Instruction::I32Const(size));
                body.instruction(&Instruction::Call(IMPORT_ALLOC_IDX));
                body.instruction(&Instruction::I64ExtendI32U);
                body.instruction(&Instruction::LocalSet(scratch));
                // count at offset 0
                body.instruction(&Instruction::LocalGet(scratch));
                body.instruction(&Instruction::I32WrapI64);
                body.instruction(&Instruction::I64Const(elems.len() as i64));
                body.instruction(&Instruction::I64Store(MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                }));
                for (i, elem) in elems.iter().enumerate() {
                    let off = ((i + 1) * 8) as i64;
                    body.instruction(&Instruction::LocalGet(scratch));
                    body.instruction(&Instruction::I64Const(off));
                    body.instruction(&Instruction::I64Add);
                    body.instruction(&Instruction::I32WrapI64);
                    body.instruction(&Instruction::LocalGet(self.mir_local(elem, func)));
                    body.instruction(&Instruction::I64Store(MemArg {
                        offset: 0,
                        align: 3,
                        memory_index: 0,
                    }));
                }
                body.instruction(&Instruction::LocalGet(scratch));
                body.instruction(&Instruction::I64Const(value_layout::TAG_PTR as i64));
                body.instruction(&Instruction::I64Or);
            }
            RValue::Record(fields) => {
                // Record: `[count][slot0]..[slotN-1]` where slots are the
                // module-wide field_map positions (sparse fields are nil-padded).
                let max_slot = fields
                    .iter()
                    .filter_map(|(name, _)| self.field_map.get(name).copied())
                    .max()
                    .unwrap_or(0);
                let count = max_slot as i64 + 1;
                let scratch = 255u32;
                let size = ((count as usize + 1) * 8) as i32;
                body.instruction(&Instruction::I32Const(size));
                body.instruction(&Instruction::Call(IMPORT_ALLOC_IDX));
                body.instruction(&Instruction::I64ExtendI32U);
                body.instruction(&Instruction::LocalSet(scratch));
                body.instruction(&Instruction::LocalGet(scratch));
                body.instruction(&Instruction::I32WrapI64);
                body.instruction(&Instruction::I64Const(count));
                body.instruction(&Instruction::I64Store(MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                }));
                for (name, val) in fields {
                    let slot = self.field_map.get(name).copied().unwrap_or(0) as i64;
                    let off = ((slot + 1) * 8) as i64;
                    body.instruction(&Instruction::LocalGet(scratch));
                    body.instruction(&Instruction::I64Const(off));
                    body.instruction(&Instruction::I64Add);
                    body.instruction(&Instruction::I32WrapI64);
                    body.instruction(&Instruction::LocalGet(self.mir_local(val, func)));
                    body.instruction(&Instruction::I64Store(MemArg {
                        offset: 0,
                        align: 3,
                        memory_index: 0,
                    }));
                }
                body.instruction(&Instruction::LocalGet(scratch));
                body.instruction(&Instruction::I64Const(value_layout::TAG_PTR as i64));
                body.instruction(&Instruction::I64Or);
            }
            RValue::LoadFieldNamed { obj, field } => {
                let slot = self.field_map.get(field).copied().unwrap_or(0);
                self.emit_obj_load(body, obj, slot, func);
            }
            RValue::LoadFieldPos { obj, index } => {
                self.emit_obj_load(body, obj, *index as u8, func);
            }
            RValue::RecordUpdate { base, overrides } => {
                // Copy `base` (a record) into a fresh object, then apply the
                // field overrides. Scratch locals: 255 = new ptr, 254 = base
                // ptr, 253 = count, 252 = i.
                let pm = value_layout::PAYLOAD_MASK as i64;
                // base_ptr = base & PAYLOAD_MASK
                body.instruction(&Instruction::LocalGet(self.mir_local(base, func)));
                body.instruction(&Instruction::I64Const(pm));
                body.instruction(&Instruction::I64And);
                body.instruction(&Instruction::LocalSet(254));
                // count = load(base_ptr + 0)
                body.instruction(&Instruction::LocalGet(254));
                body.instruction(&Instruction::I32WrapI64);
                body.instruction(&Instruction::I64Load(MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                }));
                body.instruction(&Instruction::LocalSet(253));
                // new = alloc((count+1)*8); count lives at offset 0.
                body.instruction(&Instruction::LocalGet(253));
                body.instruction(&Instruction::I64Const(1));
                body.instruction(&Instruction::I64Add);
                body.instruction(&Instruction::I64Const(8));
                body.instruction(&Instruction::I64Mul);
                body.instruction(&Instruction::I32WrapI64);
                body.instruction(&Instruction::Call(IMPORT_ALLOC_IDX));
                body.instruction(&Instruction::I64ExtendI32U);
                body.instruction(&Instruction::LocalSet(255));
                // store count at new + 0
                body.instruction(&Instruction::LocalGet(255));
                body.instruction(&Instruction::I32WrapI64);
                body.instruction(&Instruction::LocalGet(253));
                body.instruction(&Instruction::I64Store(MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                }));
                // i = 0
                body.instruction(&Instruction::I64Const(0));
                body.instruction(&Instruction::LocalSet(252));
                // loop: copy slot i from base to new.
                body.instruction(&Instruction::Block(BlockType::Empty));
                body.instruction(&Instruction::Loop(BlockType::Empty));
                body.instruction(&Instruction::LocalGet(252));
                body.instruction(&Instruction::LocalGet(253));
                body.instruction(&Instruction::I64GeU);
                // depth 1 = the enclosing Block (exit the Loop when i >= count);
                // `br 0` would target the Loop itself and spin forever.
                body.instruction(&Instruction::BrIf(1));
                // dst addr = new + (i+1)*8
                body.instruction(&Instruction::LocalGet(255));
                body.instruction(&Instruction::LocalGet(252));
                body.instruction(&Instruction::I64Const(1));
                body.instruction(&Instruction::I64Add);
                body.instruction(&Instruction::I64Const(8));
                body.instruction(&Instruction::I64Mul);
                body.instruction(&Instruction::I64Add);
                body.instruction(&Instruction::I32WrapI64);
                // src addr = base_ptr + (i+1)*8
                body.instruction(&Instruction::LocalGet(254));
                body.instruction(&Instruction::LocalGet(252));
                body.instruction(&Instruction::I64Const(1));
                body.instruction(&Instruction::I64Add);
                body.instruction(&Instruction::I64Const(8));
                body.instruction(&Instruction::I64Mul);
                body.instruction(&Instruction::I64Add);
                body.instruction(&Instruction::I32WrapI64);
                // load src → push value
                body.instruction(&Instruction::I64Load(MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                }));
                // store value at dst
                body.instruction(&Instruction::I64Store(MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                }));
                // i++
                body.instruction(&Instruction::LocalGet(252));
                body.instruction(&Instruction::I64Const(1));
                body.instruction(&Instruction::I64Add);
                body.instruction(&Instruction::LocalSet(252));
                body.instruction(&Instruction::Br(0));
                body.instruction(&Instruction::End);
                body.instruction(&Instruction::End);
                // apply overrides
                for (name, val) in overrides {
                    let slot = self.field_map.get(name).copied().unwrap_or(0) as i64;
                    let off = ((slot + 1) * 8) as i64;
                    body.instruction(&Instruction::LocalGet(255));
                    body.instruction(&Instruction::I64Const(off));
                    body.instruction(&Instruction::I64Add);
                    body.instruction(&Instruction::I32WrapI64);
                    body.instruction(&Instruction::LocalGet(self.mir_local(val, func)));
                    body.instruction(&Instruction::I64Store(MemArg {
                        offset: 0,
                        align: 3,
                        memory_index: 0,
                    }));
                }
                // result = new | TAG_PTR
                body.instruction(&Instruction::LocalGet(255));
                body.instruction(&Instruction::I64Const(value_layout::TAG_PTR as i64));
                body.instruction(&Instruction::I64Or);
            }
            RValue::CapabilityCheck { .. } => {
                // Reference capabilities are compile-time-only and erased at
                // runtime, so the check always succeeds — mirroring the
                // interpreter (`CapabilityCheck` compiles to `Const1`) and the
                // AOT backend. Previously this silently fell through to nil.
                body.instruction(&Instruction::I64Const(value_layout::tag_bool(true) as i64));
            }
            RValue::FFICall { idx, args } => {
                // Foreign function call: intern the library/symbol as string
                // constants, bit-pack the CType signature, push the args, and
                // call the arity-matched host `ffi_call_N`.
                let def = self
                    .foreign_functions
                    .get(*idx)
                    .cloned()
                    .unwrap_or_else(|| mir::ForeignFunction {
                        library: String::new(),
                        symbol: String::new(),
                        params: vec![],
                        ret: crate::types::Type::unit(),
                    });
                let ctype_tag = |c: crate::ffi::marshal::CType| -> u64 {
                    match c {
                        crate::ffi::marshal::CType::I64 => 0,
                        crate::ffi::marshal::CType::F64 => 1,
                        crate::ffi::marshal::CType::Bool => 2,
                        crate::ffi::marshal::CType::CStr => 3,
                        crate::ffi::marshal::CType::VoidPtr => 4,
                        crate::ffi::marshal::CType::Unit => 5,
                    }
                };
                let mut params: Vec<crate::ffi::marshal::CType> =
                    Vec::with_capacity(def.params.len());
                for p in &def.params {
                    let ffi_ty = crate::ffi::marshal::nulang_type_to_ffi_type(p)
                        .unwrap_or(crate::bytecode::FfiType::Int);
                    let ctype = crate::ffi::marshal::ffi_type_to_ctype(&ffi_ty)
                        .unwrap_or(crate::ffi::marshal::CType::I64);
                    params.push(ctype);
                }
                let ret = crate::ffi::marshal::ffi_type_to_ctype(
                    &crate::ffi::marshal::nulang_type_to_ffi_type(&def.ret)
                        .unwrap_or(crate::bytecode::FfiType::Int),
                )
                .unwrap_or(crate::ffi::marshal::CType::I64);
                let mut sig: u64 = ctype_tag(ret);
                for (i, c) in params.iter().enumerate() {
                    sig |= ctype_tag(*c) << (3 + 3 * i);
                }
                let import = match args.len() {
                    0 => IMPORT_FFI_CALL_0,
                    1 => IMPORT_FFI_CALL_1,
                    2 => IMPORT_FFI_CALL_2,
                    3 => IMPORT_FFI_CALL_3,
                    _ => IMPORT_FFI_CALL_4,
                };
                // Library + symbol were pre-interned into the data segment.
                let (lib_off, _) = self.interned.get(&def.library).copied().unwrap_or((0, 0));
                let (sym_off, _) = self.interned.get(&def.symbol).copied().unwrap_or((0, 0));
                body.instruction(&Instruction::I64Const(
                    value_layout::TAG_STRING as i64 | lib_off as i64,
                ));
                body.instruction(&Instruction::I64Const(
                    value_layout::TAG_STRING as i64 | sym_off as i64,
                ));
                body.instruction(&Instruction::I64Const(sig as i64));
                for a in args {
                    body.instruction(&Instruction::LocalGet(self.mir_local(a, func)));
                }
                body.instruction(&Instruction::Call(import));
            }
            _ => {
                body.instruction(&Instruction::I64Const(value_layout::TAG_NIL as i64));
            }
        }
    }

    /// Emit a field load: `obj[slot]` where `obj` is a TAG_PTR heap object
    /// with layout `[count][slot0]..`. Leaves the loaded i64 on the stack.
    fn emit_obj_load(&self, body: &mut Function, obj: &LocalId, slot: u8, func: &mir::Function) {
        let pm = value_layout::PAYLOAD_MASK as i64;
        // base = obj & PAYLOAD_MASK
        body.instruction(&Instruction::LocalGet(self.mir_local(obj, func)));
        body.instruction(&Instruction::I64Const(pm));
        body.instruction(&Instruction::I64And);
        // base + (slot+1)*8
        body.instruction(&Instruction::I64Const(((slot as i64) + 1) * 8));
        body.instruction(&Instruction::I64Add);
        body.instruction(&Instruction::I32WrapI64);
        body.instruction(&Instruction::I64Load(MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        }));
    }

    fn compile_unary(
        &self,
        body: &mut Function,
        op: crate::ast::UnOp,
        a: &mir::LocalId,
        func: &mir::Function,
    ) {
        use crate::ast::UnOp;

        match op {
            UnOp::Neg => {
                // Route through `env.arith_neg` so float operands negate their
                // sign bit instead of being int-corrupted.
                body.instruction(&Instruction::LocalGet(self.mir_local(a, func)));
                // Mirror mir_codegen's type-directed opcode choice. MIR locals
                // carry Float for comparisons in this pipeline, even though the
                // runtime comparison result is a tagged Bool; that path is still
                // FNeg in the VM and must produce -0.0 for the Bool fallback.
                let is_float = func
                    .locals
                    .get(a.0 as usize)
                    .map(|local| {
                        local.ty
                            == crate::types::Type::Primitive(crate::types::PrimitiveType::Float)
                    })
                    .unwrap_or(false);
                body.instruction(&Instruction::Call(if is_float {
                    IMPORT_ARITH_FNEG
                } else {
                    IMPORT_ARITH_NEG
                }));
            }
            UnOp::Not => {
                let tf = value_layout::tag_bool(false) as i64;
                let tt = value_layout::tag_bool(true) as i64;
                body.instruction(&Instruction::LocalGet(self.mir_local(a, func)));
                body.instruction(&Instruction::I64Const(tt));
                body.instruction(&Instruction::I64Eq);
                body.instruction(&Instruction::I64ExtendI32S);
                body.instruction(&Instruction::I64Const(tf - tt));
                body.instruction(&Instruction::I64Mul);
                body.instruction(&Instruction::I64Const(tt));
                body.instruction(&Instruction::I64Add);
            }
            _ => {
                body.instruction(&Instruction::I64Const(value_layout::TAG_NIL as i64));
            }
        }
    }

    fn compile_const(&self, body: &mut Function, c: &crate::bytecode::Constant) {
        use crate::bytecode::Constant;
        let bits: i64 = match c {
            Constant::Int(n) => value_layout::tag_int(*n) as i64,
            Constant::Float(f) => f.to_bits() as i64,
            Constant::Bool(b) => value_layout::tag_bool(*b) as i64,
            Constant::Nil => value_layout::TAG_NIL as i64,
            Constant::Unit => value_layout::TAG_UNIT as i64,
            Constant::String(s) => {
                // Tag as string with the interned offset in payload.
                // Actually, strings in Nulang are interned: Value::string(idx).
                // For WASM, we store the data-segment offset.
                let (offset, _len) = self.interned.get(s).copied().unwrap_or((0, 0));
                value_layout::TAG_STRING as i64 | (offset as i64)
            }
            _ => value_layout::TAG_NIL as i64,
        };
        body.instruction(&Instruction::I64Const(bits));
    }

    fn compile_call(
        &self,
        body: &mut Function,
        fr: &FuncRef,
        args: &[LocalId],
        func: &mir::Function,
    ) {
        match fr {
            FuncRef::Index(idx) => {
                for a in args {
                    body.instruction(&Instruction::LocalGet(self.mir_local(a, func)));
                }
                let wi = self.func_index_map.get(idx).copied().unwrap_or(0);
                body.instruction(&Instruction::Call(wi));
            }
            FuncRef::Local(_) => {
                body.instruction(&Instruction::I64Const(value_layout::TAG_NIL as i64));
            }
        }
    }

    fn compile_perform(
        &self,
        body: &mut Function,
        effect: &str,
        op: &str,
        args: &[LocalId],
        func: &mir::Function,
    ) {
        match (effect, op) {
            ("IO", "print") | ("IO", "println") => {
                // Push string pointer and length from first arg.
                // args[0] should be a string constant.
                if let Some(arg) = args.first() {
                    // Load the string value; its payload is the data offset.
                    body.instruction(&Instruction::LocalGet(self.mir_local(arg, func)));
                    // Extract payload as i32 offset.
                    body.instruction(&Instruction::I64Const(value_layout::PAYLOAD_MASK as i64));
                    body.instruction(&Instruction::I64And);
                    body.instruction(&Instruction::I32WrapI64);
                    // Length: hardcoded to 0 for now (host reads until null).
                    body.instruction(&Instruction::I32Const(0));
                } else {
                    body.instruction(&Instruction::I32Const(0));
                    body.instruction(&Instruction::I32Const(0));
                }
                body.instruction(&Instruction::Call(IMPORT_IO_PRINT));
            }
            ("IO", "read") => {
                body.instruction(&Instruction::Call(IMPORT_IO_READ));
            }
            ("Array", "length") => {
                if let Some(arg) = args.first() {
                    self.compile_array_len(body, arg, func);
                } else {
                    body.instruction(&Instruction::I64Const(value_layout::TAG_NIL as i64));
                }
            }
            _ => {
                // Generic effect dispatch: `perform Effect.op(args)` →
                // `nulang_dispatch(tag_ptr, tag_len, payload_ptr, payload_len)`
                // where the tag is the dotted "Effect.op" path and the payload
                // is a compile-time JSON array of the args (both interned in
                // the pre-scan). The host writes the result to the ring buffer
                // and returns its length; the read-back parses a JSON
                // int/string/bool/null into a tagged Nulang value.
                let tag = format!("{effect}.{op}");
                let (tag_off, tag_len) = self.interned.get(&tag).copied().unwrap_or((0, 0));
                let consts = args
                    .iter()
                    .map(|l| resolve_const(func, *l))
                    .collect::<Option<Vec<_>>>()
                    .expect("pre-scan guaranteed constant effect args");
                let encoded = consts
                    .iter()
                    .map(json_arg)
                    .collect::<Option<Vec<_>>>()
                    .expect("pre-scan guaranteed JSON-encodable effect args");
                let payload = format!("[{}]", encoded.join(","));
                let (pay_off, pay_len) = self.interned.get(&payload).copied().unwrap_or((0, 0));

                body.instruction(&Instruction::I32Const(tag_off as i32));
                body.instruction(&Instruction::I32Const(tag_len as i32));
                body.instruction(&Instruction::I32Const(pay_off as i32));
                body.instruction(&Instruction::I32Const(pay_len as i32));
                body.instruction(&Instruction::Call(IMPORT_NULANG_DISPATCH));
                self.compile_dispatch_readback(body);
            }
        }
    }

    /// Emit the dispatch-result read-back: the `nulang_dispatch` call left
    /// the result LENGTH (i64) on the stack; 0 means no result (nil). For a
    /// non-zero length the result is a JSON value at [`RING_BUFFER_BASE`]
    /// in linear memory. Parses:
    /// - `"..."`   → string (content copied to a bump-allocated buffer)
    /// - `true`    → bool true
    /// - `false`   → bool false
    /// - `null`    → nil
    /// - integer   → int (decimal parse; non-integer JSON falls back to nil)
    /// - anything else → nil (defensive)
    ///
    /// Scratch locals (all transient within this emission): 252 = result
    /// length / int accumulator, 253 = first byte / content length / sign,
    /// 254 = string dest ptr / int accumulator, 255 = loop index.
    fn compile_dispatch_readback(&self, body: &mut Function) {
        use wasm_encoder::{BlockType, MemArg, ValType};
        let mem0 = MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        };
        let base = RING_BUFFER_BASE as i32;
        // All WASM locals in this backend are i64 (the function declares
        // 256 i64 locals), so every i32 value stored to a local is
        // sign-extended on the way in and wrapped on the way out.

        // 252 = result length L (i64). If L == 0 → nil.
        body.instruction(&Instruction::LocalSet(252));
        body.instruction(&Instruction::LocalGet(252));
        body.instruction(&Instruction::I64Eqz);
        body.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
        body.instruction(&Instruction::I64Const(value_layout::TAG_NIL as i64));
        body.instruction(&Instruction::Else);
        // 253 = first byte at [base] (i64).
        body.instruction(&Instruction::I32Const(base));
        body.instruction(&Instruction::I32Load8U(mem0));
        body.instruction(&Instruction::I64ExtendI32U);
        body.instruction(&Instruction::LocalSet(253));

        // String: first byte '"' (0x22).
        body.instruction(&Instruction::LocalGet(253));
        body.instruction(&Instruction::I64Const(0x22));
        body.instruction(&Instruction::I64Eq);
        body.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
        {
            // content_len = L - 2 (drop the surrounding quotes).
            body.instruction(&Instruction::LocalGet(252));
            body.instruction(&Instruction::I64Const(2));
            body.instruction(&Instruction::I64Sub);
            body.instruction(&Instruction::LocalSet(253)); // 253 = content_len
                                                           // dest = nulang_alloc(content_len + 1) — +1 for the NUL.
            body.instruction(&Instruction::LocalGet(253));
            body.instruction(&Instruction::I32WrapI64);
            body.instruction(&Instruction::I32Const(1));
            body.instruction(&Instruction::I32Add);
            body.instruction(&Instruction::Call(IMPORT_ALLOC_IDX));
            body.instruction(&Instruction::I64ExtendI32U);
            body.instruction(&Instruction::LocalSet(254)); // 254 = dest (i64)

            // Copy loop: dest[i] = mem[base + 1 + i] for i in 0..content_len.
            body.instruction(&Instruction::I64Const(0));
            body.instruction(&Instruction::LocalSet(255)); // 255 = i (i64)
            body.instruction(&Instruction::Block(BlockType::Empty)); // exit (depth 1)
            body.instruction(&Instruction::Loop(BlockType::Empty)); // depth 0
            body.instruction(&Instruction::LocalGet(255));
            body.instruction(&Instruction::LocalGet(253));
            body.instruction(&Instruction::I64GeU);
            body.instruction(&Instruction::BrIf(1)); // i >= content_len → exit
                                                     // addr = dest + i (i32)
            body.instruction(&Instruction::LocalGet(254));
            body.instruction(&Instruction::I32WrapI64);
            body.instruction(&Instruction::LocalGet(255));
            body.instruction(&Instruction::I32WrapI64);
            body.instruction(&Instruction::I32Add);
            // value = mem[base + 1 + i] (i32)
            body.instruction(&Instruction::I32Const(base + 1));
            body.instruction(&Instruction::LocalGet(255));
            body.instruction(&Instruction::I32WrapI64);
            body.instruction(&Instruction::I32Add);
            body.instruction(&Instruction::I32Load8U(mem0));
            // store (pops value, then addr — value must be on top).
            body.instruction(&Instruction::I32Store8(mem0));
            // i += 1
            body.instruction(&Instruction::LocalGet(255));
            body.instruction(&Instruction::I64Const(1));
            body.instruction(&Instruction::I64Add);
            body.instruction(&Instruction::LocalSet(255));
            body.instruction(&Instruction::Br(0));
            body.instruction(&Instruction::End); // end Loop
            body.instruction(&Instruction::End); // end Block
                                                 // dest[content_len] = 0 (null terminator).
            body.instruction(&Instruction::LocalGet(254));
            body.instruction(&Instruction::I32WrapI64);
            body.instruction(&Instruction::LocalGet(253));
            body.instruction(&Instruction::I32WrapI64);
            body.instruction(&Instruction::I32Add);
            body.instruction(&Instruction::I32Const(0));
            body.instruction(&Instruction::I32Store8(mem0));
            // TAG_STRING | dest
            body.instruction(&Instruction::I64Const(value_layout::TAG_STRING as i64));
            body.instruction(&Instruction::LocalGet(254));
            body.instruction(&Instruction::I64Or);
        }
        body.instruction(&Instruction::Else);
        {
            // true?
            body.instruction(&Instruction::LocalGet(253));
            body.instruction(&Instruction::I64Const(b't' as i64));
            body.instruction(&Instruction::I64Eq);
            body.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
            body.instruction(&Instruction::I64Const(value_layout::tag_bool(true) as i64));
            body.instruction(&Instruction::Else);
            // false?
            body.instruction(&Instruction::LocalGet(253));
            body.instruction(&Instruction::I64Const(b'f' as i64));
            body.instruction(&Instruction::I64Eq);
            body.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
            body.instruction(&Instruction::I64Const(value_layout::tag_bool(false) as i64));
            body.instruction(&Instruction::Else);
            // null?
            body.instruction(&Instruction::LocalGet(253));
            body.instruction(&Instruction::I64Const(b'n' as i64));
            body.instruction(&Instruction::I64Eq);
            body.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
            body.instruction(&Instruction::I64Const(value_layout::TAG_NIL as i64));
            body.instruction(&Instruction::Else);
            {
                // Integer parse: optional '-', then digits. 253 = sign,
                // 254 = accumulator, 255 = index (all i64).
                body.instruction(&Instruction::I64Const(1));
                body.instruction(&Instruction::LocalSet(253));
                body.instruction(&Instruction::I64Const(0));
                body.instruction(&Instruction::LocalSet(254));
                body.instruction(&Instruction::I64Const(0));
                body.instruction(&Instruction::LocalSet(255));
                // If mem[base] == '-' (0x2D): sign = -1, i = 1.
                body.instruction(&Instruction::I32Const(base));
                body.instruction(&Instruction::I32Load8U(mem0));
                body.instruction(&Instruction::I32Const(0x2D));
                body.instruction(&Instruction::I32Eq);
                body.instruction(&Instruction::If(BlockType::Empty));
                body.instruction(&Instruction::I64Const(-1));
                body.instruction(&Instruction::LocalSet(253));
                body.instruction(&Instruction::I64Const(1));
                body.instruction(&Instruction::LocalSet(255));
                body.instruction(&Instruction::End);
                // Digit loop: while i < L && mem[base+i] is a digit.
                body.instruction(&Instruction::Block(BlockType::Empty)); // exit (depth 1)
                body.instruction(&Instruction::Loop(BlockType::Empty)); // depth 0
                                                                        // i >= L → exit
                body.instruction(&Instruction::LocalGet(255));
                body.instruction(&Instruction::LocalGet(252));
                body.instruction(&Instruction::I64GeU);
                body.instruction(&Instruction::BrIf(1));
                // (mem[base+i] - 0x30) < 10 (unsigned ⇒ digit check)?
                body.instruction(&Instruction::I32Const(base));
                body.instruction(&Instruction::LocalGet(255));
                body.instruction(&Instruction::I32WrapI64);
                body.instruction(&Instruction::I32Add);
                body.instruction(&Instruction::I32Load8U(mem0));
                body.instruction(&Instruction::I32Const(0x30));
                body.instruction(&Instruction::I32Sub);
                body.instruction(&Instruction::I32Const(10));
                body.instruction(&Instruction::I32LtU);
                body.instruction(&Instruction::I32Eqz);
                body.instruction(&Instruction::BrIf(1)); // not a digit → exit
                                                         // acc = acc * 10 + (mem[base+i] - 0x30)
                body.instruction(&Instruction::LocalGet(254));
                body.instruction(&Instruction::I64Const(10));
                body.instruction(&Instruction::I64Mul);
                body.instruction(&Instruction::I32Const(base));
                body.instruction(&Instruction::LocalGet(255));
                body.instruction(&Instruction::I32WrapI64);
                body.instruction(&Instruction::I32Add);
                body.instruction(&Instruction::I32Load8U(mem0));
                body.instruction(&Instruction::I32Const(0x30));
                body.instruction(&Instruction::I32Sub);
                body.instruction(&Instruction::I64ExtendI32U);
                body.instruction(&Instruction::I64Add);
                body.instruction(&Instruction::LocalSet(254));
                // i += 1
                body.instruction(&Instruction::LocalGet(255));
                body.instruction(&Instruction::I64Const(1));
                body.instruction(&Instruction::I64Add);
                body.instruction(&Instruction::LocalSet(255));
                body.instruction(&Instruction::Br(0));
                body.instruction(&Instruction::End); // end Loop
                body.instruction(&Instruction::End); // end Block
                                                     // result = sign * acc, tagged int.
                body.instruction(&Instruction::LocalGet(253));
                body.instruction(&Instruction::LocalGet(254));
                body.instruction(&Instruction::I64Mul);
                body.instruction(&Instruction::I64Const(value_layout::PAYLOAD_MASK as i64));
                body.instruction(&Instruction::I64And);
                body.instruction(&Instruction::I64Const(value_layout::TAG_INT as i64));
                body.instruction(&Instruction::I64Or);
            }
            body.instruction(&Instruction::End); // end null-check If
            body.instruction(&Instruction::End); // end false-check If
            body.instruction(&Instruction::End); // end true-check If
        }
        body.instruction(&Instruction::End); // end string-check If
        body.instruction(&Instruction::End); // end L==0 If
    }

    // ── Binary ops ─────────────────────────────────────────────────

    fn emit_binop(&self, body: &mut Function, op: crate::ast::BinOp) {
        use crate::ast::BinOp;
        let pm = value_layout::PAYLOAD_MASK as i64;
        let ti = value_layout::TAG_INT as i64;

        // Extract payloads: both operands are on the stack as tagged i64.
        // Mask b (top of stack).
        body.instruction(&Instruction::I64Const(pm));
        body.instruction(&Instruction::I64And);
        body.instruction(&Instruction::LocalSet(254));
        // Mask a.
        body.instruction(&Instruction::I64Const(pm));
        body.instruction(&Instruction::I64And);
        // Now stack: a_payload (top), b_payload (in local 254) — reversed.
        // Swap into correct order: a, b.
        body.instruction(&Instruction::LocalGet(254));

        let sign_extend_both = |b: &mut Function| {
            b.instruction(&Instruction::LocalSet(254));
            b.instruction(&Instruction::I64Const(16));
            b.instruction(&Instruction::I64Shl);
            b.instruction(&Instruction::I64Const(16));
            b.instruction(&Instruction::I64ShrS);
            b.instruction(&Instruction::LocalGet(254));
            b.instruction(&Instruction::I64Const(16));
            b.instruction(&Instruction::I64Shl);
            b.instruction(&Instruction::I64Const(16));
            b.instruction(&Instruction::I64ShrS);
        };

        match op {
            BinOp::And => {
                body.instruction(&Instruction::I64And);
                body.instruction(&Instruction::I64Const(value_layout::TAG_BOOL as i64));
                body.instruction(&Instruction::I64Or);
                return;
            }
            BinOp::Or => {
                body.instruction(&Instruction::I64Or);
                body.instruction(&Instruction::I64Const(value_layout::TAG_BOOL as i64));
                body.instruction(&Instruction::I64Or);
                return;
            }
            BinOp::Add => {
                body.instruction(&Instruction::I64Add);
            }
            BinOp::Sub => {
                body.instruction(&Instruction::I64Sub);
            }
            BinOp::Mul => {
                body.instruction(&Instruction::I64Mul);
            }
            BinOp::Div => {
                sign_extend_both(body);
                body.instruction(&Instruction::I64DivS);
            }
            BinOp::Mod => {
                sign_extend_both(body);
                body.instruction(&Instruction::I64RemS);
            }
            BinOp::BitAnd => {
                body.instruction(&Instruction::I64And);
            }
            BinOp::BitOr => {
                body.instruction(&Instruction::I64Or);
            }
            BinOp::BitXor => {
                body.instruction(&Instruction::I64Xor);
            }
            BinOp::Shl => {
                sign_extend_both(body);
                body.instruction(&Instruction::I64Shl);
            }
            BinOp::Shr => {
                sign_extend_both(body);
                body.instruction(&Instruction::I64ShrS);
            }
            cmp @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge) => {
                sign_extend_both(body);
                match cmp {
                    BinOp::Eq => body.instruction(&Instruction::I64Eq),
                    BinOp::Ne => body.instruction(&Instruction::I64Ne),
                    BinOp::Lt => body.instruction(&Instruction::I64LtS),
                    BinOp::Gt => body.instruction(&Instruction::I64GtS),
                    BinOp::Le => body.instruction(&Instruction::I64LeS),
                    BinOp::Ge => body.instruction(&Instruction::I64GeS),
                    _ => unreachable!(),
                };
                body.instruction(&Instruction::I64ExtendI32S);
                let tf = value_layout::tag_bool(false) as i64;
                let tt = value_layout::tag_bool(true) as i64;
                body.instruction(&Instruction::I64Const(tt - tf));
                body.instruction(&Instruction::I64Mul);
                body.instruction(&Instruction::I64Const(tf));
                body.instruction(&Instruction::I64Add);
                return;
            }
            _ => {
                body.instruction(&Instruction::Drop);
                body.instruction(&Instruction::Drop);
                body.instruction(&Instruction::I64Const(value_layout::TAG_NIL as i64));
                return;
            }
        }
        body.instruction(&Instruction::I64Const(pm));
        body.instruction(&Instruction::I64And);
        body.instruction(&Instruction::I64Const(ti));
        body.instruction(&Instruction::I64Or);
    }

    // ── SIMD lowering ──────────────────────────────────────────────
    //
    // WASM SIMD (0xFD prefix) opcodes for vectorized array operations.
    // The runtime must enable wasm_simd in its Wasmtime config.
    // Values are currently tagged i64; full SIMD benefit requires the
    // compiler to emit untagged array element IR. This module provides
    // the lowering infrastructure that such compiler changes can target.

    /// Emit raw WASM SIMD opcode bytes. `opcode` is the LEB128-encoded
    /// SIMD opcode (without the 0xFD prefix), followed by optional
    fn emit_simd(&self, body: &mut Function, opcode: u32, immediates: &[u8]) {
        // WASM SIMD prefix byte.
        body.raw([0xFDu8].into_iter());
        // Encode the SIMD opcode as unsigned LEB128.
        let mut buf = [0u8; 5];
        let len = leb128_u32(opcode, &mut buf);
        body.raw(buf[..len].iter().copied());
        if !immediates.is_empty() {
            body.raw(immediates.iter().copied());
        }
    }
    /// Emit a SIMD memory load: `v128.load align=4 offset=<offset>`.
    /// Returns the v128 value on the stack.

    fn emit_simd_load(&self, body: &mut Function, offset: u32) {
        // v128.load opcode = 0x00; align=4 (natural for v128), offset as LEB128.
        let mut buf = [0u8; 5];
        let olen = leb128_u32(offset, &mut buf);
        // MemArg: align (u32 LEB) + offset (u32 LEB).
        let mut align_buf = [0u8; 5];
        let alen = leb128_u32(4, &mut align_buf); // natural alignment for v128
        let mut imms = Vec::with_capacity(alen + olen);
        imms.extend_from_slice(&align_buf[..alen]);
        imms.extend_from_slice(&buf[..olen]);
        self.emit_simd(body, 0x00, &imms);
    }

    /// Emit a SIMD memory store: `v128.store align=4 offset=<offset>`.
    /// Consumes the v128 value from the stack.

    fn emit_simd_store(&self, body: &mut Function, offset: u32) {
        let mut buf = [0u8; 5];
        let olen = leb128_u32(offset, &mut buf);
        let mut align_buf = [0u8; 5];
        let alen = leb128_u32(4, &mut align_buf);
        let mut imms = Vec::with_capacity(alen + olen);
        imms.extend_from_slice(&align_buf[..alen]);
        imms.extend_from_slice(&buf[..olen]);
        self.emit_simd(body, 0x0B, &imms);
    }

    /// Emit a SIMD binary operation on i64x2 lanes.

    fn emit_simd_i64x2_binop(&self, body: &mut Function, op: crate::ast::BinOp) {
        use crate::ast::BinOp;
        let simd_op: u32 = match op {
            BinOp::Add => 0xCE, // i64x2.add
            BinOp::Sub => 0xD1, // i64x2.sub
            BinOp::Mul => 0xD5, // i64x2.mul
            _ => return,        // unsupported op — fall through to scalar
        };
        self.emit_simd(body, simd_op, &[]);
    }

    /// Emit a SIMD binary operation on f64x2 lanes.

    fn emit_simd_f64x2_binop(&self, body: &mut Function, op: crate::ast::BinOp) {
        use crate::ast::BinOp;
        let simd_op: u32 = match op {
            BinOp::Add => 0xEE, // f64x2.add
            BinOp::Sub => 0xF4, // f64x2.sub
            BinOp::Mul => 0xF3, // f64x2.mul
            BinOp::Div => 0xFA, // f64x2.div
            _ => return,
        };
        self.emit_simd(body, simd_op, &[]);
    }

    /// Emit v128.const with 16 raw bytes.
    fn emit_v128_const(&self, body: &mut Function, bytes: &[u8; 16]) {
        let mut imms = Vec::with_capacity(16);
        imms.extend_from_slice(bytes);
        self.emit_simd(body, 0x0C, &imms);
    }

    /// Emit v128.and (bitwise AND of two v128 values).
    fn emit_v128_and(&self, body: &mut Function) {
        self.emit_simd(body, 0x4E, &[]);
    }

    /// Emit v128.or (bitwise OR of two v128 values).
    fn emit_v128_or(&self, body: &mut Function) {
        self.emit_simd(body, 0x50, &[]);
    }

    // ── Array helpers ──────────────────────────────────────────────

    fn compile_array_lit(&self, body: &mut Function, elems: &[LocalId], func: &mir::Function) {
        let scratch = 255u32;
        let size = ((elems.len() + 1) * 8) as i32;
        let len = elems.len() as i64;

        // Allocate: nulang_alloc(size) → i32 base
        body.instruction(&Instruction::I32Const(size));
        body.instruction(&Instruction::Call(IMPORT_ALLOC_IDX));
        body.instruction(&Instruction::I64ExtendI32U);
        body.instruction(&Instruction::LocalSet(scratch));

        // Store length at offset 0
        body.instruction(&Instruction::LocalGet(scratch));
        body.instruction(&Instruction::I32WrapI64);
        body.instruction(&Instruction::I64Const(len));
        body.instruction(&Instruction::I64Store(MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        }));

        // Store each element
        for (i, elem) in elems.iter().enumerate() {
            let offset = ((i + 1) * 8) as i64;
            body.instruction(&Instruction::LocalGet(scratch));
            body.instruction(&Instruction::I64Const(offset));
            body.instruction(&Instruction::I64Add);
            body.instruction(&Instruction::I32WrapI64);
            body.instruction(&Instruction::LocalGet(self.mir_local(elem, func)));
            body.instruction(&Instruction::I64Store(MemArg {
                offset: 0,
                align: 3,
                memory_index: 0,
            }));
        }

        // Tag: TAG_PTR | base
        body.instruction(&Instruction::LocalGet(scratch));
        body.instruction(&Instruction::I64Const(value_layout::TAG_PTR as i64));
        body.instruction(&Instruction::I64Or);
    }

    fn compile_array_load(
        &self,
        body: &mut Function,
        arr: &LocalId,
        idx: &LocalId,
        func: &mir::Function,
    ) {
        // Route through `env.arr_load` for a BOUNDS CHECK: out-of-range (and
        // negative) indices must yield nil, matching the interpreter. The old
        // inline path had no check and read garbage for OOB indices.
        body.instruction(&Instruction::LocalGet(self.mir_local(arr, func)));
        body.instruction(&Instruction::LocalGet(self.mir_local(idx, func)));
        body.instruction(&Instruction::Call(IMPORT_ARR_LOAD));
    }

    fn compile_array_len(&self, body: &mut Function, arr: &LocalId, func: &mir::Function) {
        let pm = value_layout::PAYLOAD_MASK as i64;
        // base = arr & PAYLOAD_MASK
        body.instruction(&Instruction::LocalGet(self.mir_local(arr, func)));
        body.instruction(&Instruction::I64Const(pm));
        body.instruction(&Instruction::I64And);
        // i32 address
        body.instruction(&Instruction::I32WrapI64);
        // load len
        body.instruction(&Instruction::I64Load(MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        }));
        // tag as TAG_INT
        body.instruction(&Instruction::I64Const(pm));
        body.instruction(&Instruction::I64And);
        body.instruction(&Instruction::I64Const(value_layout::TAG_INT as i64));
        body.instruction(&Instruction::I64Or);
    }

    fn compile_array_store(
        &mut self,
        body: &mut Function,
        arr: &LocalId,
        idx: &LocalId,
        src: &LocalId,
        func: &mir::Function,
    ) {
        // Bounds: no bounds check. We rely on the guard-page trap model
        // (OOB access SIGSEGVs into a Wasmtime trap).
        let pm = value_layout::PAYLOAD_MASK as i64;
        // base = arr & PAYLOAD_MASK
        body.instruction(&Instruction::LocalGet(self.mir_local(arr, func)));
        body.instruction(&Instruction::I64Const(pm));
        body.instruction(&Instruction::I64And);
        // idx = idx & PAYLOAD_MASK
        body.instruction(&Instruction::LocalGet(self.mir_local(idx, func)));
        body.instruction(&Instruction::I64Const(pm));
        body.instruction(&Instruction::I64And);
        // (idx + 1) * 8
        body.instruction(&Instruction::I64Const(8));
        body.instruction(&Instruction::I64Mul);
        body.instruction(&Instruction::I64Const(8));
        body.instruction(&Instruction::I64Add);
        // base + (idx + 1) * 8
        body.instruction(&Instruction::I64Add);
        // i32 address
        body.instruction(&Instruction::I32WrapI64);
        // value
        body.instruction(&Instruction::LocalGet(self.mir_local(src, func)));
        // store
        body.instruction(&Instruction::I64Store(MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        }));
    }

    /// Store a named record field. Records use the same flat heap layout as
    /// tuples and arrays, with the field slot following the count word.
    fn compile_field_store(
        &self,
        body: &mut Function,
        obj: &LocalId,
        field: &str,
        src: &LocalId,
        func: &mir::Function,
    ) {
        let pm = value_layout::PAYLOAD_MASK as i64;
        let slot = self.field_map.get(field).copied().unwrap_or(0) as i64;

        // base = obj & PAYLOAD_MASK
        body.instruction(&Instruction::LocalGet(self.mir_local(obj, func)));
        body.instruction(&Instruction::I64Const(pm));
        body.instruction(&Instruction::I64And);
        // base + (slot + 1) * 8
        body.instruction(&Instruction::I64Const((slot + 1) * 8));
        body.instruction(&Instruction::I64Add);
        body.instruction(&Instruction::I32WrapI64);
        // value
        body.instruction(&Instruction::LocalGet(self.mir_local(src, func)));
        body.instruction(&Instruction::I64Store(MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        }));
    }

    /// Detect and compile element-wise loops as SIMD operations.
    ///
    /// Scans the function's MIR blocks for sequential `ArrayStore` +
    /// adjacent-element patterns. When two adjacent iterations of an
    /// element-wise binary operation on array elements are detected,
    /// replaces the scalar pair with `v128.load` + vector op + `v128.store`.
    ///
    /// Returns `true` if any SIMD lowering was applied.

    /// Attempt SIMD lowering for a binary operation whose operands are
    /// array-element loads. When both `a` and `b` are adjacent array element
    /// loads (from the same base pointer), emit a vectorized operation.

    // ── Helpers ────────────────────────────────────────────────────

    fn mir_local(&self, local: &LocalId, func: &mir::Function) -> u32 {
        let pc = func.params.len() as u32;
        for (i, p) in func.params.iter().enumerate() {
            if p == local {
                return i as u32;
            }
        }
        for (i, c) in func.captures.iter().enumerate() {
            if c == local {
                return pc + i as u32;
            }
        }
        pc + func.captures.len() as u32 + local.0
    }
}

/// Collect record field names from a statement into the module-wide
/// field-name → slot-index map (mirrors the AOT backend's collection).
fn collect_wasm_fields(
    stmt: &Stmt,
    field_map: &mut std::collections::HashMap<String, u8>,
    next_field_id: &mut u8,
) {
    let mut insert = |name: &str| {
        field_map.entry(name.to_string()).or_insert_with(|| {
            let id = *next_field_id;
            *next_field_id = next_field_id.saturating_add(1);
            id
        });
    };
    match stmt {
        Stmt::Assign { op, .. } => match op {
            RValue::Record(fields)
            | RValue::RecordUpdate {
                overrides: fields, ..
            } => {
                for (name, _) in fields {
                    insert(name);
                }
            }
            RValue::LoadFieldNamed { field, .. } => insert(field),
            _ => {}
        },
        Stmt::StoreFieldNamed { field, .. } => insert(field),
        _ => {}
    }
}

/// Encode a u32 as unsigned LEB128 into `buf`. Returns the number of
/// bytes written (1–5).
fn leb128_u32(mut value: u32, buf: &mut [u8; 5]) -> usize {
    let mut i = 0;
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf[i] = byte;
        i += 1;
        if value == 0 {
            break;
        }
    }
    i
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn compile_source(source: &str) -> NuResult<Vec<u8>> {
        let tokens = crate::lexer::Lexer::new(source).lex()?;
        let ast = crate::parser::Parser::new(tokens).parse_module()?;
        let mut tc = crate::typechecker::TypeChecker::new();
        tc.check_module(&ast)?;
        let hir = crate::hir_lower::lower_module(&ast, &tc.inferred_decl_types);
        let mir = crate::mir_lower::lower_module(&hir)?;
        let mut backend = WasmBackend::new();
        backend.compile(&mir, "test")
    }

    #[test]
    fn test_compile_literal_int() {
        let wasm = compile_source("42").expect("compile");
        assert_eq!(&wasm[0..4], b"\0asm");
    }

    #[test]
    fn test_compile_addition() {
        let wasm = compile_source("1 + 2").expect("compile");
        assert_eq!(&wasm[0..4], b"\0asm");
    }

    #[test]
    fn test_compile_bool() {
        let wasm = compile_source("true").expect("compile");
        assert_eq!(&wasm[0..4], b"\0asm");
    }

    #[test]
    fn test_compile_io_print() {
        let wasm = compile_source(r#"perform IO.print("hello")"#).expect("compile");
        assert_eq!(&wasm[0..4], b"\0asm");
    }

    #[test]
    fn test_compile_float() {
        let wasm = compile_source("3.14").expect("compile float");
        assert_eq!(&wasm[0..4], b"\0asm");
    }

    #[test]
    fn test_compile_comparison_eq() {
        let wasm = compile_source("1 == 1").expect("compile comparison");
        assert_eq!(&wasm[0..4], b"\0asm");
    }

    #[test]
    fn test_compile_if_expr() {
        let wasm = compile_source("if true then 1 else 2").expect("compile if");
        assert_eq!(&wasm[0..4], b"\0asm");
    }

    #[test]
    fn test_compile_let_binding() {
        let wasm = compile_source("let x = 42; x").expect("compile let");
        assert_eq!(&wasm[0..4], b"\0asm");
    }

    #[test]
    fn test_compile_block() {
        let wasm = compile_source("{ 1; 2; 3 }").expect("compile block");
        assert_eq!(&wasm[0..4], b"\0asm");
    }

    #[test]
    fn test_compile_string() {
        let wasm = compile_source(r#""hello world""#).expect("compile string");
        assert_eq!(&wasm[0..4], b"\0asm");
    }

    #[test]
    fn test_compile_arithmetic_sub() {
        let wasm = compile_source("10 - 3").expect("compile sub");
        assert_eq!(&wasm[0..4], b"\0asm");
    }

    #[test]
    fn test_compile_arithmetic_mul() {
        let wasm = compile_source("4 * 5").expect("compile mul");
        assert_eq!(&wasm[0..4], b"\0asm");
    }

    #[cfg(all(test, feature = "wasm-backend"))]
    fn run_source(source: &str) -> NuResult<crate::vm::Value> {
        let wasm = compile_source(source)?;
        let mut runtime = crate::wasm_runtime::WasmRuntime::new(&wasm, None)?;
        runtime.run()
    }

    #[cfg(all(test, feature = "wasm-backend"))]
    fn run_source_with_dispatch(
        source: &str,
        dispatch_result: Option<Vec<u8>>,
    ) -> NuResult<(crate::vm::Value, Option<(Vec<u8>, Vec<u8>)>)> {
        let wasm = compile_source(source)?;
        let mut runtime = crate::wasm_runtime::WasmRuntime::new(&wasm, None)?;
        runtime.set_dispatch_result(dispatch_result);
        let value = runtime.run()?;
        let last = runtime.take_last_dispatch();
        Ok((value, last))
    }

    #[test]
    #[cfg(all(test, feature = "wasm-backend"))]
    fn test_wasm_array_index() {
        let value = run_source("let a = [10, 20, 30]; a[1]").expect("run");
        assert_eq!(value.as_int(), Some(20), "a[1] should be 20");
    }

    #[test]
    #[cfg(all(test, feature = "wasm-backend"))]
    fn test_wasm_array_store() {
        let value = run_source("let a = [1, 2, 3]; a[0] = 99; a[0]").expect("run");
        assert_eq!(value.as_int(), Some(99), "a[0] after store should be 99");
    }

    #[test]
    #[cfg(all(test, feature = "wasm-backend"))]
    fn test_wasm_array_length() {
        let value = run_source("let a = [5, 6]; perform Array.length(a)").expect("run");
        assert_eq!(value.as_int(), Some(2), "Array.length should be 2");
    }

    #[test]
    #[cfg(all(test, feature = "wasm-backend"))]
    fn test_effect_dispatch_marshals_tag_and_json_payload() {
        // A generic effect with constant args must lower to
        // nulang_dispatch(tag="Test.echo", payload="[42,\"x\",true]").
        // The host records the (tag, payload) pair and injects a string
        // result, which the read-back must parse back into a Nulang string.
        let (value, last) = run_source_with_dispatch(
            r#"perform Test.echo(42, "x", true)"#,
            Some(br#""ok""#.to_vec()),
        )
        .expect("run");
        let (tag, payload) = last.expect("dispatch must have been called");
        assert_eq!(tag, b"Test.echo", "tag is the dotted effect path");
        assert_eq!(payload, br#"[42,"x",true]"#, "payload is JSON-encoded args");
        // Read-back: the JSON string result must become a Nulang string.
        assert_eq!(
            value.as_raw() as u64 & crate::value_layout::TAG_MASK,
            crate::value_layout::TAG_STRING,
            "JSON string result must parse to a string value"
        );
    }

    #[test]
    #[cfg(all(test, feature = "wasm-backend"))]
    fn test_effect_dispatch_int_result() {
        let (value, last) =
            run_source_with_dispatch(r#"perform Test.echo(1, 2, 3)"#, Some(b"42".to_vec()))
                .expect("run");
        let (tag, payload) = last.expect("dispatch must have been called");
        assert_eq!(tag, b"Test.echo");
        assert_eq!(payload, b"[1,2,3]", "ints marshal as JSON numbers");
        assert_eq!(
            value.as_int(),
            Some(42),
            "JSON int result must parse to a Nulang int"
        );
    }

    #[test]
    #[cfg(all(test, feature = "wasm-backend"))]
    fn test_effect_dispatch_string_result() {
        let mut runtime = {
            let wasm = compile_source(r#"perform Greet.say("hi")"#).expect("compile");
            let mut runtime = crate::wasm_runtime::WasmRuntime::new(&wasm, None).unwrap();
            runtime.set_dispatch_result(Some(br#""hello world""#.to_vec()));
            runtime
        };
        let value = runtime.run().expect("run");
        assert_eq!(
            runtime.string_value(&value).as_deref(),
            Some("hello world"),
            "JSON string result must parse to a Nulang string"
        );
    }

    #[test]
    #[cfg(all(test, feature = "wasm-backend"))]
    fn test_effect_dispatch_bool_result() {
        let (value, _) = run_source_with_dispatch(r#"perform Test.flag()"#, Some(b"true".to_vec()))
            .expect("run");
        assert_eq!(
            value.as_bool(),
            Some(true),
            "JSON true must parse to a Nulang bool"
        );
    }

    #[test]
    #[cfg(all(test, feature = "wasm-backend"))]
    fn test_effect_dispatch_nil_when_no_result() {
        // No handler / no result: dispatch returns length 0 → nil.
        let (value, last) =
            run_source_with_dispatch(r#"perform Test.nothing()"#, None).expect("run");
        assert!(last.is_some(), "dispatch must still have been called");
        assert_eq!(
            value.as_raw() as u64,
            crate::value_layout::TAG_NIL as u64,
            "length-0 dispatch must yield nil"
        );
        // JSON null also parses to nil.
        let (value, _) =
            run_source_with_dispatch(r#"perform Test.nothing()"#, Some(b"null".to_vec()))
                .expect("run");
        assert_eq!(value.as_raw() as u64, crate::value_layout::TAG_NIL as u64);
    }

    #[test]
    #[cfg(all(test, feature = "wasm-backend"))]
    fn test_effect_dispatch_dynamic_args_rejected() {
        // Dynamic (non-constant) effect args are a loud compile error, not
        // a silent nil.
        let err = compile_source(r#"let x = 1 + 2; perform Test.echo(x)"#).unwrap_err();
        assert!(
            err.to_string().contains("constant args"),
            "compile error must explain the constant-args requirement: {err}"
        );
    }

    #[test]
    #[cfg(all(test, feature = "wasm-backend"))]
    fn test_wasm_simd_even_length() {
        let code = r#"
            let a = [10, 20, 30, 40] in
            let b = [1, 2, 3, 4] in
            let c = [0, 0, 0, 0] in
            var i = 0 in {
                while i < 4 {
                    c[i] = a[i] + b[i];
                    i = i + 1;
                };
                c[3]
            }
        "#;
        let value = run_source(code).expect("run");
        assert_eq!(value.as_int(), Some(44), "c[3] should be 40 + 4");
    }

    #[test]
    #[cfg(all(test, feature = "wasm-backend"))]
    fn test_wasm_simd_odd_length() {
        let code = r#"
            let a = [10, 20, 30, 40, 50] in
            let b = [1, 2, 3, 4, 5] in
            let c = [0, 0, 0, 0, 0] in
            var i = 0 in {
                while i < 5 {
                    c[i] = a[i] + b[i];
                    i = i + 1;
                };
                c[4]
            }
        "#;
        let value = run_source(code).expect("run");
        assert_eq!(value.as_int(), Some(55), "c[4] should be 50 + 5");
    }
}

// ---------------------------------------------------------------------------
// WasmBackend trait impl — adapts the WASM compiler to the backend trait
// ---------------------------------------------------------------------------

#[cfg(feature = "wasm-backend")]
impl crate::backends::WasmBackend for WasmBackend {
    fn compile(
        &mut self,
        module: &crate::mir::Module,
        name: &str,
    ) -> crate::types::NuResult<Vec<u8>> {
        self.compile(module, name)
    }

    fn run(&mut self, wasm: &[u8]) -> crate::types::NuResult<crate::vm::Value> {
        let mut runtime = crate::wasm_runtime::WasmRuntime::new(wasm, None)?;
        runtime.run()
    }
}
