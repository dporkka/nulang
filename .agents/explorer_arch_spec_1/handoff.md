# Architectural and Specification Alignment Analysis

## 1. Observation

### Compiler Pipeline & CLI Execution
* In `src/main.rs:172-250`, the compiler pipeline is defined inside `run_source`. It executes the following steps:
  1. Lexer: `Lexer::new(source).lex()`
  2. Parser: `Parser::new(tokens).parse_module()`
  3. Type Checker: `TypeChecker::new().check_module(&ast)`
  4. Effect Checker: `EffectChecker::new().infer_effects(&effect_ctx, body)` on function bodies.
  5. Capability Analyzer: `CapabilityAnalyzer::new().infer_cap(&cap_ctx, body)` on function bodies.
  6. Compiler: `Compiler::new("main").compile_module(&ast)`
  7. VM: `VM::new()`, `vm.load_module(code_module)`, and `vm.run()`
* **Observation**: There is no invocation of Escape Analysis in the pipeline.
* **Observation**: The JIT compiler is never initialized or called during `run_source`.

### Typechecker & Unification
* In `src/typechecker.rs:883`, when inferring function application in `infer_app`, the expected function type is constructed as follows:
  ```rust
  let expected = Type::Function {
      param: Box::new(param_ty),
      ret: Box::new(result_ty.clone()),
      effect: EffectRow::empty(),
      cap: Capability::Ref,
  };
  ```
  Unifying this expected type with a function type containing non-empty effects (e.g. `Effect::IO`) results in a type unification error because `effect_row_compatible` (at line 145) requires identical effect rows for closed rows.
* In `src/typechecker.rs:1479-1506`, the `infer_agent_decl` function stubs out agent types with fresh type variables and performs no real validation:
  ```rust
  fn infer_agent_decl(...) -> NuResult<(Substitution, Type)> {
      // Check each behavior (similar to actors)
      for behavior in behaviors {
          ...
      }
      let agent_ty = Type::Agent {
          state: Box::new(Type::Var(TypeVar::fresh())),
          policy: Box::new(Type::Var(TypeVar::fresh())),
          memory: Box::new(Type::Var(TypeVar::fresh())),
          tools: Box::new(Type::Var(TypeVar::fresh())),
      };
      Ok((vec![], agent_ty))
  }
  ```

### Effect Checker
* In `src/typechecker.rs:1599-1609`, the `infer_handle` function simply infers the body type and returns it, ignoring actual handler bodies:
  ```rust
  fn infer_handle(
      &mut self,
      ctx: &TypeContext,
      body: &Expr,
      _span: Span,
  ) -> NuResult<(Substitution, Type)> {
      let (s, body_ty) = self.infer_expr(ctx, body)?;
      Ok((s, body_ty))
  }
  ```

### Escape Analysis
* In `src/escape_analysis.rs:173`, the main entry point `analyze_function` performs escape analysis over bytecode instructions.
* **Observation**: `EscapeAnalyzer` is never imported, initialized, or used in `src/compiler.rs`, `src/vm.rs`, or `src/jit/`. It is only referenced and tested within its own file `src/escape_analysis.rs` (lines 830-1500).

### JIT Compiler Tiered Execution
* In `src/jit/mod.rs:265-301`, the function `tiered_execute_step` is defined to manage recording hot counters, compiling a bytecode region to native code using Cranelift, and running it.
* **Observation**: Ripgrep search for `tiered_execute_step` across the codebase returns exactly one match (its definition). It is never called in the interpreter loop in `src/vm.rs` or any other compiler module. The VM interpreter (at `src/vm.rs:253-1120`) only interprets bytecode.

### Memory & GC Runtime
* In `src/vm.rs:694-702`, the VM handles `OpCode::Alloc` directly via standard Rust system allocation:
  ```rust
  OpCode::Alloc => {
      let size = frame.regs[instr.op1 as usize].as_int().unwrap_or(0) as usize;
      let dst = instr.op3;
      if size > 0 && size <= 256 {
          let layout = std::alloc::Layout::from_size_align(size * std::mem::size_of::<Value>(), 8).unwrap();
          let ptr = unsafe { std::alloc::alloc(layout) };
          if !ptr.is_null() { frame.regs[dst as usize] = Value::ptr(ptr); }
      }
  }
  ```
  In `src/vm.rs:780`, `OpCode::Drop` is stubbed to clear the register:
  ```rust
  OpCode::Drop => { frame.regs[instr.op1 as usize] = Value::nil(); }
  ```
  This leaks memory globally and does not call any ORCA GC routines.
* **Observation**: Neither the `DualHeap` (from `src/runtime/dual_heap.rs`) nor `OrcaGc` (from `src/runtime/gc.rs`) are imported or integrated into the VM.

### Actor Scheduler & Mailbox
* In `src/runtime/actor.rs:17-50`, the actor mailbox is defined inline as a basic wrapper around a standard vector:
  ```rust
  pub struct Mailbox {
      messages: Vec<Message>,
      capacity: usize,
  }
  ```
  It does not use the high-performance atomic lock-free MPSC ring buffer mailbox implemented in `src/runtime/mailbox.rs:43`.
* In `src/runtime/scheduler.rs:29-35`, the work-stealing algorithm is stubbed out to perform a basic vector pop:
  ```rust
  pub fn steal(&mut self) -> Option<u64> {
      self.global_queue.pop()
  }
  ```

### Distributed Opcodes & Web Framework
* In `src/vm.rs:1086-1091`, distributed opcodes are stubbed out as no-ops or default values:
  ```rust
  // -- Distribution (MVP) --
  OpCode::NodeId => { frame.regs[instr.op1 as usize] = Value::int(0); }
  OpCode::Migrate => {}
  OpCode::RAsk => { frame.regs[instr.op3 as usize] = Value::nil(); }
  OpCode::Gossip => {}
  ```
  The Gossip state machine (`src/runtime/cluster.rs`) and anti-entropy CRDT replication are not integrated with the VM runtime.
* **Observation**: No files implementing the web framework endpoints, routing, templates, database integration, or LiveView (as specified in `DESIGN_WEB_FRAMEWORK.md`) exist in the `src/` directory.

---

## 2. Logic Chain

1. **JIT Compilation Gap**: Since `tiered_execute_step` in `src/jit/mod.rs` is never invoked by the interpreter loop in `src/vm.rs`, and the VM contains no references to Cranelift or the JIT module, the JIT compilation engine (including the typed compiler and SIMD vectorizer) operates purely as an isolated module and is never executed in standard compilation or runtime execution paths.
2. **Memory Leak and Allocation Mismatch**: `OpCode::Alloc` uses `std::alloc::alloc` directly, and `OpCode::Drop` is a no-op that simply clears registers. This bypasses the entire thread-safe ORCA garbage collector (`src/runtime/gc.rs`) and generational bump allocator (`src/runtime/dual_heap.rs`), leading to memory leaks for all dynamically allocated objects during VM execution.
3. **Actor Model Stubs**: The runtime `Actor` structure uses a standard vector-based mailbox instead of the atomic ring-buffer mailbox in `src/runtime/mailbox.rs`. Similarly, the scheduler does not implement work-stealing, merely acting as a single FIFO/LIFO queue. This departs from the high-concurrency design decisions specified in `ARCHITECTURE.md` Layer 3.
4. **Escape Analysis Inaction**: Because the compiler and JIT do not import or run the `EscapeAnalyzer`, any heap allocations that could have been optimized to stack allocations are left as-is, meaning the escape analysis remains an unused feature.
5. **AI SDK & Web Framework Incompleteness**:
   - The parser syntax for agents does not support the `=` declaration syntax specified in `DESIGN_AI_SDK.md` (e.g. `agent Greeter = { ... }`).
   - The AI SDK Agent features (LLM Provider, Memory Subsystems, Tool Binding) and the Web Framework are completely absent from the actual VM interpreter and compiler, only existing as types/effects stubs.

---

## 3. Caveats

* The build and test execution (e.g. `cargo test`) could not be run locally during this investigation due to permission prompt timeouts. However, the analysis is verified strictly via static source inspection.
* It is assumed that the modules in `src/runtime/` (like CRDTs, SWIM Gossip, and ORCA GC) and `src/jit/` are fully functional and pass their own unit tests in isolation, despite not being integrated into the main execution pipeline.

---

## 4. Conclusion

The nulang codebase contains highly sophisticated runtime components (generational dual heaps, ORCA cycle detectors, SWIM gossip membership, Cranelift JIT, SIMD loop vectorization, lock-free mailboxes) that are successfully implemented and tested in isolation. However, there is a **significant gap between the architectural specifications and the actual execution pipeline**. 

The main VM interpreter operates as a simple, single-threaded register-based VM that allocates on the global heap, leaks memory, stubs out all distributed operations, and does not invoke the JIT compilation engine. Furthermore, the Web Framework and AI SDK are not implemented in the compiler/runtime, remaining as stubs in the parser and typechecker.

---

## 5. Verification Method

1. **JIT Unused Verification**: Inspect `src/vm.rs` and search for imports of `nulang::jit` or `tiered_execute_step`. Verify that no JIT compiler or execution hooks are present in `VM::run()` or `VM::step()`.
2. **Memory Allocator Verification**: Inspect `src/vm.rs` at line 694 for `OpCode::Alloc` and confirm that it uses `std::alloc::alloc`. Inspect line 780 for `OpCode::Drop` and verify it contains no deallocation logic.
3. **Mailbox & Scheduler Verification**: Inspect `src/runtime/actor.rs` at line 17 and verify `Mailbox` uses a `Vec<Message>`. Inspect `src/runtime/scheduler.rs` at line 33 to confirm the `steal` function is a dummy vector pop.
4. **Typechecker Function Effect Verification**: Run the nulang binary or REPL with a function that performs a side effect (e.g., `perform IO.print("test")`) and apply it. Verify if it fails typechecking due to unification mismatch with `EffectRow::empty()`.
5. **Running Unit Tests**: Run `cargo test` to execute the isolated unit tests for `escape_analysis`, `jit::tests`, `runtime::tests`, and `typechecker::tests`.
