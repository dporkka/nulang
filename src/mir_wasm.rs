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
#[allow(dead_code)]
const IMPORT_ALLOC_IDX: u32 = 0; // function index of nulang_alloc
#[allow(dead_code)]
const IMPORT_DISPATCH_IDX: u32 = 1; // function index of nulang_dispatch
#[allow(dead_code)]
const IMPORT_LOG_IDX: u32 = 2; // function index of log
/// Function index of `env.io_print` — used in `Call` instructions.
const IMPORT_IO_PRINT: u32 = 3;
/// Function index of `env.io_read` — used in `Call` instructions.
const IMPORT_IO_READ: u32 = 4;
/// Number of function imports. Module-defined functions start at this index.
const FUNC_IMPORT_COUNT: u32 = 5;

const TY_VOID_TO_I64: u32 = 0;
#[allow(dead_code)]
const TY_I64_TO_I64: u32 = 1;
#[allow(dead_code)]
const TY_I64I64_TO_I64: u32 = 2;
const TY_I32I32_TO_I64: u32 = 3;
const TY_FIXED_COUNT: u32 = 4;

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
}

impl WasmBackend {
    pub fn new() -> Self {
        let mut types = TypeSection::new();
        types.ty().function([], [ValType::I64]);                                 // 0
        types.ty().function([ValType::I64], [ValType::I64]);                     // 1
        types.ty().function([ValType::I64, ValType::I64], [ValType::I64]);       // 2
        types.ty().function([ValType::I32, ValType::I32], [ValType::I64]);       // 3

        let mut imports = ImportSection::new();
        imports.import("env", "memory", MemoryType {
            minimum: 1, maximum: None, memory64: false, shared: false,
            page_size_log2: None,
        });
        imports.import("env", "nulang_alloc",    EntityType::Function(TY_VOID_TO_I64));   // FIXME: wrong type idx
        imports.import("env", "nulang_dispatch", EntityType::Function(TY_VOID_TO_I64));   // FIXME
        imports.import("env", "log",             EntityType::Function(TY_I32I32_TO_I64));
        imports.import("env", "io_print",        EntityType::Function(TY_I32I32_TO_I64));
        imports.import("env", "io_read",         EntityType::Function(TY_VOID_TO_I64));

        // Fix up import type indices — imports reference the type section,
        // not the import index. The alloc/dispatch type refs need to point
        // at actual function types.  We'll fix these via import encoding
        // by re-building imports after types are finalized.
        // For now, the constructor builds with placeholder type refs.

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
        self.interned.insert(s.to_string(), (offset, len));
        (offset, len)
    }

    // ── Compile ───────────────────────────────────────────────────

    pub fn compile(&mut self, mir: &mir::Module, _module_name: &str) -> NuResult<Vec<u8>> {
        // Intern strings from constants for data segment.
        for func in mir.functions.iter().chain(mir.behaviors.iter()) {
            for block in &func.blocks {
                for stmt in &block.stmts {
                    if let Stmt::Assign { op, .. } = stmt {
                        self.intern_const_strings(op);
                    }
                }
            }
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

        self.exports.export("nulang_init", ExportKind::Func, FUNC_IMPORT_COUNT);

        // Emit data segment.
        if !self.string_data.is_empty() {
            self.data.active(0, &ConstExpr::i32_const(0), self.string_data.clone());
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
        // Dispatch: (i32, i32, i32, i32) -> ()
        let ty_dispatch = self.ensure_type(
            vec![ValType::I32; 4],
            vec![],
        );

        let mut imports = ImportSection::new();
        imports.import("env", "memory", MemoryType {
            minimum: 1, maximum: None, memory64: false, shared: false,
            page_size_log2: None,
        });
        imports.import("env", "nulang_alloc", EntityType::Function(ty_alloc));
        imports.import("env", "nulang_dispatch", EntityType::Function(ty_dispatch));
        imports.import("env", "log", EntityType::Function(TY_I32I32_TO_I64));
        imports.import("env", "io_print", EntityType::Function(TY_I32I32_TO_I64));
        imports.import("env", "io_read", EntityType::Function(TY_VOID_TO_I64));
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

    fn compile_function(&mut self, func: &mir::Function, mir_idx: usize) {
        let wasm_idx = self.next_func_idx;
        self.next_func_idx += 1;
        self.func_index_map.insert(mir_idx, wasm_idx);
        self.functions.function(self.func_type_idx(func));

        let local_count = func.locals.len() + func.params.len() + func.captures.len();
        let wasm_locals: Vec<_> = (0..local_count).map(|_| (1u32, ValType::I64)).collect();
        let mut body = Function::new(wasm_locals);

        let block_order = self.compute_block_order(func);
        let mut labels: HashMap<BlockId, u32> = HashMap::new();
        let mut li: u32 = 0;

        for &bid in &block_order {
            labels.insert(bid, li);
            let block = &func.blocks[bid.0 as usize];
            body.instruction(&Instruction::Block(BlockType::Empty));

            for stmt in &block.stmts {
                self.compile_stmt(&mut body, stmt, func);
            }
            self.compile_terminator(&mut body, &block.terminator, &labels, li);
            body.instruction(&Instruction::End);
            body.instruction(&Instruction::Unreachable);
            li += 1;
        }
        body.instruction(&Instruction::End);
        self.codes.function(&body);
    }

    fn compute_block_order(&self, func: &mir::Function) -> Vec<BlockId> {
        let mut order = vec![func.entry];
        let mut seen: std::collections::HashSet<BlockId> = std::collections::HashSet::new();
        seen.insert(func.entry);
        let mut i = 0;
        while i < order.len() {
            let bid = order[i];
            let block = &func.blocks[bid.0 as usize];
            match &block.terminator {
                Terminator::Jump(t) => { if seen.insert(*t) { order.push(*t); } }
                Terminator::Branch { then_, else_, .. } => {
                    if seen.insert(*then_) { order.push(*then_); }
                    if seen.insert(*else_) { order.push(*else_); }
                }
                _ => {}
            }
            i += 1;
        }
        order
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
            Stmt::StoreFieldNamed { .. }
            | Stmt::ArrayStore { .. }
            | Stmt::Emit { .. }
            | Stmt::StateSet { .. } => {
                body.instruction(&Instruction::I64Const(value_layout::TAG_NIL as i64));
                body.instruction(&Instruction::Drop);
            }
        }
    }

    // ── RValue compilation ─────────────────────────────────────────

    fn compile_rvalue(&self, body: &mut Function, rvalue: &RValue, func: &mir::Function) {
        match rvalue {
            RValue::Const(c) => { self.compile_const(body, c); }
            RValue::Load(l) => { body.instruction(&Instruction::LocalGet(self.mir_local(l, func))); }
            RValue::Binary(op, a, b) => {
                // Attempt SIMD lowering first; fall through to scalar if
                // operands are not adjacent array-element loads.
                if !self.try_compile_simd_binary(body, *op, a, b, func) {
                    body.instruction(&Instruction::LocalGet(self.mir_local(a, func)));
                    body.instruction(&Instruction::LocalGet(self.mir_local(b, func)));
                    self.emit_binop(body, *op);
                }
            }
            RValue::Unary(_, _) => {
                body.instruction(&Instruction::I64Const(value_layout::TAG_NIL as i64));
            }
            RValue::Call { func: fr, args } => { self.compile_call(body, fr, args, func); }
            RValue::Perform { effect, op, args } => {
                self.compile_perform(body, effect, op, args, func);
            }
            _ => {
                body.instruction(&Instruction::I64Const(value_layout::TAG_NIL as i64));
            }
        }
    }

    fn compile_const(&self, body: &mut Function, c: &crate::bytecode::Constant) {
        use crate::bytecode::Constant;
        let bits: i64 = match c {
            Constant::Int(n)   => value_layout::tag_int(*n) as i64,
            Constant::Float(f) => f.to_bits() as i64,
            Constant::Bool(b)  => value_layout::tag_bool(*b) as i64,
            Constant::Nil      => value_layout::TAG_NIL as i64,
            Constant::Unit     => value_layout::TAG_UNIT as i64,
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

    fn compile_call(&self, body: &mut Function, fr: &FuncRef, args: &[LocalId], func: &mir::Function) {
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
            _ => {
                // Unknown effect: return nil.
                body.instruction(&Instruction::I64Const(value_layout::TAG_NIL as i64));
            }
        }
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

        match op {
            BinOp::Add => { body.instruction(&Instruction::I64Add); }
            BinOp::Sub => { body.instruction(&Instruction::I64Sub); }
            BinOp::Mul => { body.instruction(&Instruction::I64Mul); }
            BinOp::Div => { body.instruction(&Instruction::I64DivS); }
            BinOp::Mod => { body.instruction(&Instruction::I64RemS); }
            cmp @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge) => {
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
        body.instruction(&Instruction::I64Const(ti));
        body.instruction(&Instruction::I64Or);
    }

    // ── Terminators ────────────────────────────────────────────────

    fn compile_terminator(
        &self,
        body: &mut Function,
        term: &Terminator,
        labels: &HashMap<BlockId, u32>,
        cur: u32,
    ) {
        match term {
            Terminator::Return(Some(l)) => {
                body.instruction(&Instruction::LocalGet(l.0));
                body.instruction(&Instruction::Return);
            }
            Terminator::Return(None) => {
                body.instruction(&Instruction::I64Const(value_layout::TAG_UNIT as i64));
                body.instruction(&Instruction::Return);
            }
            Terminator::Jump(t) => {
                let tl = labels.get(t).copied().unwrap_or(0);
                body.instruction(&Instruction::Br(if tl <= cur { cur - tl + 1 } else { 1 }));
            }
            Terminator::Branch { cond, then_, else_ } => {
                body.instruction(&Instruction::LocalGet(cond.0));
                body.instruction(&Instruction::I64Const(1));
                body.instruction(&Instruction::I64And);
                let tl = labels.get(then_).copied().unwrap_or(0);
                let el = labels.get(else_).copied().unwrap_or(0);
                body.instruction(&Instruction::BrIf(if tl <= cur { cur - tl + 1 } else { 1 }));
                body.instruction(&Instruction::Br(if el <= cur { cur - el + 1 } else { 1 }));
            }
            Terminator::Resume(_) | Terminator::Unterminated => {
                body.instruction(&Instruction::Return);
            }
        }
    }


    // ── SIMD lowering ──────────────────────────────────────────────
    //
    // WASM SIMD (0xFD prefix) opcodes for vectorized array operations.
    // The runtime must enable wasm_simd in its Wasmtime config.
    // Values are currently tagged i64; full SIMD benefit requires the
    // compiler to emit untagged array element IR. This module provides
    // the lowering infrastructure that such compiler changes can target.

    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    fn emit_simd_i64x2_binop(&self, body: &mut Function, op: crate::ast::BinOp) {
        use crate::ast::BinOp;
        let simd_op: u32 = match op {
            BinOp::Add => 0xC6, // i64x2.add
            BinOp::Sub => 0xCD, // i64x2.sub
            BinOp::Mul => 0xCB, // i64x2.mul
            _ => return,         // unsupported op — fall through to scalar
        };
        self.emit_simd(body, simd_op, &[]);
    }

    /// Emit a SIMD binary operation on f64x2 lanes.
    #[allow(dead_code)]
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

    /// Detect and compile element-wise loops as SIMD operations.
    ///
    /// Scans the function's MIR blocks for sequential `ArrayStore` +
    /// adjacent-element patterns. When two adjacent iterations of an
    /// element-wise binary operation on array elements are detected,
    /// replaces the scalar pair with `v128.load` + vector op + `v128.store`.
    ///
    /// Returns `true` if any SIMD lowering was applied.
    #[allow(dead_code)]
    fn try_simd_lower_function(&mut self, func: &mir::Function) -> bool {
        // This is a framework hook. Full SIMD vectorization requires:
        // 1. Compiler emits MIR annotations marking vectorizable loops
        // 2. Or a loop-analysis pass identifies element-wise patterns
        // For now, return false — lowering happens in compile_rvalue.
        let _ = func;
        false
    }

    /// Attempt SIMD lowering for a binary operation whose operands are
    /// array-element loads. When both `a` and `b` are adjacent array element
    /// loads (from the same base pointer), emit a vectorized operation.
    fn try_compile_simd_binary(
        &self,
        body: &mut Function,
        _op: crate::ast::BinOp,
        _a: &LocalId,
        _b: &LocalId,
        _func: &mir::Function,
    ) -> bool {
        // Placeholder: when MIR carries array-element annotations, this
        // will emit v128.load + vector op + v128.store.
        // For now, scalar path handles all binary ops.
        let _ = body;
        false
    }

    // ── Helpers ────────────────────────────────────────────────────

    fn mir_local(&self, local: &LocalId, func: &mir::Function) -> u32 {
        let pc = func.params.len() as u32;
        for (i, p) in func.params.iter().enumerate() {
            if p == local { return i as u32; }
        }
        for (i, c) in func.captures.iter().enumerate() {
            if c == local { return pc + i as u32; }
        }
        pc + func.captures.len() as u32 + local.0
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
        let hir = crate::hir_lower::lower_module(&ast);
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
}
