# Nulang Codebase Analysis Report

This report presents a comprehensive review of the Nulang compiler pipeline, VM/JIT runtime, performance optimizations, code quality issues, test coverage gaps, and specification alignment. 

---

## 1. Architectural & Design Review

### 1.1 Compiler Pipeline (`src/main.rs`)
The Nulang compiler pipeline is orchestrating execution via the function `run_source` in `src/main.rs`. It sequentially performs the following phases:
1. **Lexer**: tokenizes input source via `Lexer::new(source).lex()`.
2. **Parser**: builds the AST via `Parser::new(tokens).parse_module()`.
3. **Type Checker**: infers types and performs unification via `TypeChecker::new().check_module(&ast)`.
4. **Effect Checker**: performs effect row inference over function bodies via `EffectChecker::new().infer_effects(...)`.
5. **Capability Analyzer**: checks references and function context permissions via `CapabilityAnalyzer::new().infer_cap(...)`.
6. **Compiler**: compiles the AST to register-based bytecode via `Compiler::new("main").compile_module(&ast)`.
7. **VM**: executes the compiled bytecode via `VM::new()`, `vm.load_module(code_module)`, and `vm.run()`.

**Gaps & Disconnections**:
* **Escape Analysis Bypass**: The escape analysis module (`src/escape_analysis.rs`) is completely excluded from the compiler pipeline. It is never imported or executed, meaning that opportunities to optimize heap allocations into stack allocations are bypassed entirely.
* **JIT Tiered Execution Bypass**: The JIT tiered execution helper `tiered_execute_step` defined in `src/jit/mod.rs` is never initialized, registered, or invoked inside `VM::run` or `VM::step`. The interpreter loop remains purely interpretive.

---

### 1.2 Typechecker & Unification (`src/typechecker.rs`)
The typechecker employs Algorithm W style type inference with algebraic effect rows and capabilities.
* **Function Application Limitation**: In `src/typechecker.rs` (lines 883–888), function application (`infer_app`) builds an expected function type assuming `EffectRow::empty()` for its effect row:
  ```rust
  let expected = Type::Function {
      param: Box::new(param_ty),
      ret: Box::new(result_ty.clone()),
      effect: EffectRow::empty(),
      cap: Capability::Ref,
  };
  ```
  This causes unification failures (`mgu`) for any application of functions with non-empty effect rows (e.g. `Effect::IO`), because row unification requires exact compatibility, and open row variables are not supported here.
* **Agent Type Inference Stubs**: The `infer_agent_decl` function in `src/typechecker.rs` (lines 1479–1506) checks the syntax of agent behaviors but stubs out the resulting `Type::Agent` with fresh type variables (`Type::Var(TypeVar::fresh())`), performing no type constraints check or formal verification:
  ```rust
  let agent_ty = Type::Agent {
      state: Box::new(Type::Var(TypeVar::fresh())),
      policy: Box::new(Type::Var(TypeVar::fresh())),
      memory: Box::new(Type::Var(TypeVar::fresh())),
      tools: Box::new(Type::Var(TypeVar::fresh())),
  };
  ```

---

### 1.3 Effect Checker (`src/typechecker.rs` / `src/effect_checker.rs`)
* **Ignored Handler Bodies**: In `src/typechecker.rs` (lines 1599–1609), the handler arms inside `infer_handle` are completely ignored (symbol `handlers: _` in the pattern match in `infer_expr`), and only the main body expression is typed:
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
  This means handler bodies are never checked for correct type signatures or effect discharge, allowing unverified effects to leak into execution.

---

### 1.4 Escape Analysis (`src/escape_analysis.rs`)
* **Isolation**: The `EscapeAnalyzer` implements a flow-sensitive escape analysis over Nulang bytecode to identify objects that do not escape a function context. However, it is never integrated into `src/compiler.rs`, `src/vm.rs`, or the JIT. It remains completely dead code in production.

---

### 1.5 VM / JIT Runtime (`src/vm.rs` & `src/jit/`)
* **VM Loop**: The interpreter in `src/vm.rs` uses a huge `match` statement over `OpCode` variants inside `VM::step`. It processes frames dynamically but contains no hooks into the Cranelift JIT engine.
* **JIT Subsystem**: The Cranelift-based JIT compiler compiles hot bytecode sequences into native machine instructions. However, it operates as an isolated subsystem that is only exercised by JIT-specific unit tests (`src/jit/tests.rs`), leaving the production runtime interpreter 100% interpretive.

---

## 2. Performance & Optimization

### 2.1 Area 1: VM Call Frame Allocation Churn
* **File Path**: `src/vm.rs`
* **Locations**: Lines 148–154, 207–209, 290–309
* **Current Code Snippet**:
  ```rust
  pub struct Frame {
      pub regs: [Value; 256],
      pub pc: usize,
      pub closure: Option<Value>,
      pub caller: Option<Box<Frame>>, // Linked list via heap boxing
      pub module_idx: usize,
  }

  // Inside OpCode::Call handling:
  let mut new_frame = Frame::new(None, module_idx);
  new_frame.pc = code_offset;
  ...
  new_frame.caller = Some(frame);
  self.current_frame = Some(Box::new(new_frame));
  ```
* **Explanation**: Since `Frame` contains 256 registers of `Value` (each 8 bytes), each `Frame` is at least 2KB. For every function invocation (`OpCode::Call` or `OpCode::ClosureCall`), a new `Frame` is instantiated and boxed on the heap. In hot execution loops, allocating and deallocating 2KB heap buffers continuously causes severe memory allocator churn and hurts cache locality.
* **Proposed Refactored Version**:
  Change `VM` to use a flat stack vector (`Vec<Frame>`) and track indices to form a call stack, eliminating per-call heap allocations.
  ```rust
  pub struct Frame {
      pub regs: [Value; 256],
      pub pc: usize,
      pub closure: Option<Value>,
      pub caller_idx: Option<usize>, // Track stack parent via index
      pub module_idx: usize,
  }

  // Inside VM struct:
  pub struct VM {
      modules: Vec<CodeModule>,
      frames: Vec<Frame>,            // Stack allocation pool
      current_frame_idx: Option<usize>,
      // ...
  }
  ```
* **Actionable Checklist**:
  - [ ] Remove `Box<Frame>` from the `caller` field in `Frame` and replace with `Option<usize>`.
  - [ ] Refactor `VM` to store `frames: Vec<Frame>` instead of a nested `Option<Box<Frame>>`.
  - [ ] Implement stack-depth validation to prevent stack overflow.
  - [ ] Refactor `OpCode::Call` to push to `VM.frames` and update the active frame index.
  - [ ] Refactor `OpCode::Ret` to pop the frame and restore the caller index.

---

### 2.2 Area 2: Raw System Allocations and Memory Leaks in VM
* **File Path**: `src/vm.rs`
* **Locations**: Lines 694–702 (`OpCode::Alloc`), Lines 1093–1098 (`OpCode::SConcat`), and Lines 1100–1105 (`OpCode::SRead`).
* **Current Code Snippet**:
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

  OpCode::SConcat => {
      let s1 = frame.regs[instr.op1 as usize].to_string_repr();
      let s2 = frame.regs[instr.op2 as usize].to_string_repr();
      let result = format!("{}{}", s1, s2);
      frame.regs[instr.op3 as usize] = Value::ptr(result.into_bytes().leak().as_mut_ptr());
  }
  ```
* **Explanation**: The VM executes allocations using raw system `std::alloc::alloc`, and strings in `SConcat`/`SRead` are permanently leaked via `.leak().as_mut_ptr()`. Because `OpCode::Drop` is a no-op that merely clears registers, all allocated objects are leaked forever. This completely bypasses the custom actor bump allocator (`ActorHeap`) and cycle collector (`OrcaGc`).
* **Proposed Refactored Version**:
  Route all dynamic VM allocations through the current actor's garbage collector.
  ```rust
  OpCode::SConcat => {
      let s1 = frame.regs[instr.op1 as usize].to_string_repr();
      let s2 = frame.regs[instr.op2 as usize].to_string_repr();
      let result = format!("{}{}", s1, s2);
      
      if let Some(actor_id) = self.runtime.current_actor_id() {
          if let Some(actor_gc) = self.runtime.actor_gcs.get_mut(&actor_id) {
              let heap = self.runtime.actor_heaps.get_mut(&actor_id).unwrap();
              let ptr = actor_gc.alloc_object(heap, result.len(), TypeTag::String)
                  .ok_or_else(|| NuError::VMError("GC Allocation failed".to_string()))?;
              unsafe { std::ptr::copy_nonoverlapping(result.as_ptr(), ptr, result.len()); }
              frame.regs[instr.op3 as usize] = Value::ptr(ptr);
          }
      } else {
          // Fallback context allocation
          frame.regs[instr.op3 as usize] = Value::ptr(result.into_bytes().leak().as_mut_ptr());
      }
  }
  ```
* **Actionable Checklist**:
  - [ ] Implement VM-to-Actor GC allocation mapping inside `OpCode::Alloc`.
  - [ ] Refactor `OpCode::SConcat` to allocate the destination string payload inside the `ActorHeap`.
  - [ ] Refactor `OpCode::SRead` to query actor GC space instead of leaking strings.
  - [ ] Update `OpCode::Drop` to register references for reclamation.

---

### 2.3 Area 3: Vector Allocation Churn in RGA CRDT
* **File Path**: `src/runtime/crdt_reg.rs`
* **Locations**: Lines 249–262 (`RGA::insert_at` and `RGA::delete_at`)
* **Current Code Snippet**:
  ```rust
  pub fn insert_at(&mut self, index: usize, value: T) -> ElementId {
      let live: Vec<_> = self.elements.iter().filter(|e| e.value.is_some()).map(|e| e.id).collect();
      let parent = if index == 0 { None } else { Some(live[index - 1]) };
      self.insert_after(parent, value)
  }

  pub fn delete_at(&mut self, index: usize) {
      let live: Vec<_> = self.elements.iter().filter(|e| e.value.is_some()).map(|e| e.id).collect();
      self.delete(live[index]);
  }
  ```
* **Explanation**: Every invocation of `insert_at` or `delete_at` traversally maps and filters the entire internal element buffer to collect a new heap-allocated `Vec<ElementId>`. For long document histories or intensive text editing, this creates $O(N)$ memory allocations on every operation. Using `.nth()` allows finding the element in-place without vector allocation.
* **Proposed Refactored Version**:
  ```rust
  pub fn insert_at(&mut self, index: usize, value: T) -> ElementId {
      let parent = if index == 0 {
          None
      } else {
          self.elements.iter()
              .filter(|e| e.value.is_some())
              .nth(index - 1)
              .map(|e| e.id)
      };
      self.insert_after(parent, value)
  }

  pub fn delete_at(&mut self, index: usize) {
      if let Some(id) = self.elements.iter()
          .filter(|e| e.value.is_some())
          .nth(index)
          .map(|e| e.id) {
          self.delete(id);
      }
  }
  ```
* **Actionable Checklist**:
  - [ ] Eliminate the temporary `live: Vec<_>` allocations in `insert_at`.
  - [ ] Replace collection indexing with lazy iterator evaluation via `.nth()`.
  - [ ] Update `delete_at` similarly to avoid allocations.
  - [ ] Add unit tests verifying index boundaries.

---

### 2.4 Area 4: Redundant Heap Allocation and Value Cloning in MVRegister
* **File Path**: `src/runtime/crdt_reg.rs`
* **Locations**: Lines 140–147, 157–166 (`MVRegister::write` and `MVRegister::merge`)
* **Current Code Snippet**:
  ```rust
  pub fn write(&mut self, value: T) {
      let ts = self.clock.tick();
      let old: Vec<_> = self.values.iter().cloned().collect();
      for (v, t) in &old {
          if *t < ts { self.values.remove(&(v.clone(), *t)); }
      }
      self.values.insert((value, ts));
  }

  pub fn merge(&mut self, other: &Self) {
      let combined: Vec<_> = self.values.iter().cloned().chain(other.values.iter().cloned()).collect();
      if combined.is_empty() {
          self.clock.counter = self.clock.counter.max(other.clock.counter);
          return;
      }
      let max_ts = combined.iter().map(|(_, t)| *t).max().unwrap();
      self.values = combined.into_iter().filter(|(_, t)| *t == max_ts).collect();
      self.clock.counter = self.clock.counter.max(other.clock.counter);
  }
  ```
* **Explanation**: In `write`, a temporary `Vec` is allocated to clone and collect all values, then iterate over them. In `merge`, a flat `Vec` combining self and other elements is created, cloning all inner values. This causes unnecessary heap allocations and redundant clones of complex generic structures `T`. We can update the state in-place using `HashSet::retain`.
* **Proposed Refactored Version**:
  ```rust
  pub fn write(&mut self, value: T) {
      let ts = self.clock.tick();
      self.values.retain(|(_, t)| *t >= ts);
      self.values.insert((value, ts));
  }

  pub fn merge(&mut self, other: &Self) {
      self.clock.counter = self.clock.counter.max(other.clock.counter);
      
      for (val, ts) in &other.values {
          self.values.insert((val.clone(), *ts));
      }
      
      if !self.values.is_empty() {
          let max_ts = self.values.iter().map(|(_, t)| *t).max().copied().unwrap();
          self.values.retain(|(_, t)| *t == max_ts);
      }
  }
  ```
* **Actionable Checklist**:
  - [ ] Refactor `write` to utilize `HashSet::retain` in-place.
  - [ ] Remove the combined vector heap allocation inside `merge`.
  - [ ] Directly insert other register elements and prune outdated timestamps in-place.

---

## 3. Code Quality & Rust Idioms

### 3.1 Issue 1: Redundant `unsafe` Transmute in `src/compiler.rs`
* **File Path**: `src/compiler.rs`
* **Location**: Lines 9–14
* **Current Code Snippet**:
  ```rust
  /// Workaround for the `Self` opcode (0x83) which conflicts with the Rust keyword.
  /// Uses transmute from the known discriminant value.
  fn op_self() -> OpCode {
      // Safety: 0x83 is the guaranteed discriminant for the `Self` variant.
      unsafe { std::mem::transmute::<u8, OpCode>(0x83) }
  }
  ```
* **Explanation**: The author bypassed compiler safety rules under the assumption that `Self` was impossible to type due to name conflicts with the Rust keyword. However, in `src/bytecode.rs`, the variant was successfully defined as `SelfOp` specifically to avoid this issue. The transmute is completely redundant and unsafe.
* **Proposed Refactored Version**:
  ```rust
  fn op_self() -> OpCode {
      OpCode::SelfOp
  }
  ```
* **Actionable Checklist**:
  - [ ] Replace the unsafe transmute block in `src/compiler.rs` with `OpCode::SelfOp`.
  - [ ] Remove the helper comment regarding keyword conflict.

---

### 3.2 Issue 2: Double Lookup and Unsafe `unwrap` in Cache Operations
* **File Path**: `src/runtime/distributed.rs`
* **Location**: Lines 179–192
* **Current Code Snippet**:
  ```rust
  pub fn get(&mut self, node_id: NodeId, actor_id: u64) -> Option<&RemoteActorInfo> {
      let key = (node_id, actor_id);
      if self.entries.contains_key(&key) {
          // Update LRU position: remove and re-insert at back.
          self.access_order.retain(|&k| k != key);
          self.access_order.push_back(key);
          // Update last_accessed timestamp.
          let info = self.entries.get_mut(&key).unwrap();
          info.last_accessed = Instant::now();
          Some(info)
      } else {
          None
      }
  }
  ```
* **Explanation**: Checking `contains_key` followed by `get_mut().unwrap()` causes two hash map lookups and introduces an unsafe panic risk if a race condition modifies the map. Using pattern matching with `if let` accesses the value in a single lookup and is completely safe.
* **Proposed Refactored Version**:
  ```rust
  pub fn get(&mut self, node_id: NodeId, actor_id: u64) -> Option<&RemoteActorInfo> {
      let key = (node_id, actor_id);
      if let Some(info) = self.entries.get_mut(&key) {
          self.access_order.retain(|&k| k != key);
          self.access_order.push_back(key);
          info.last_accessed = Instant::now();
          Some(info)
      } else {
          None
      }
  }
  ```
* **Actionable Checklist**:
  - [ ] Rewrite LRU cache `get` using an `if let Some(...)` match.
  - [ ] Remove the duplicate `.contains_key(...)` lookup.
  - [ ] Delete the unsafe `.unwrap()` call.

---

### 3.3 Issue 3: Check-Then-Unwrap in CRDT Value Matching
* **File Path**: `src/runtime/crdt_reg.rs`
* **Location**: Lines 149–166
* **Current Code Snippet**:
  ```rust
  pub fn read(&self) -> HashSet<T> {
      if self.values.is_empty() { return HashSet::new(); }
      let max_ts = self.values.iter().map(|(_, t)| *t).max().unwrap();
      self.values.iter().filter(|(_, t)| *t == max_ts).map(|(v, _)| v.clone()).collect()
  }
  ```
* **Explanation**: The checking logic (`is_empty()`) separates safety verification from extraction, leading to an unnecessary `.unwrap()` that could panic if state mutations occur concurrently. Utilizing Rust's `Option::map` on `.max()` resolves this idiomatic flaw.
* **Proposed Refactored Version**:
  ```rust
  pub fn read(&self) -> HashSet<T> {
      self.values.iter()
          .map(|(_, t)| *t)
          .max()
          .map(|max_ts| {
              self.values.iter()
                  .filter(|(_, t)| *t == max_ts)
                  .map(|(v, _)| v.clone())
                  .collect()
          })
          .unwrap_or_default()
  }
  ```
* **Actionable Checklist**:
  - [ ] Replace `is_empty` check with `max()` returning an `Option`.
  - [ ] Propagate values safely using `.map` and `.unwrap_or_default()`.
  - [ ] Clean up redundant `unwrap` calls in `merge`.

---

### 3.4 Issue 4: Inefficient `BinaryHeap` Iteration in Timers
* **File Path**: `src/runtime/timer.rs`
* **Location**: Lines 217–253
* **Current Code Snippet**:
  ```rust
  pub fn tick(&self, now: Instant) -> Vec<(u64, TimerMessage)> {
      let mut fired = Vec::new();
      let mut to_remove = Vec::new();
      ...
      for entry in timers.iter() {
          if entry.cancelled.load(Ordering::SeqCst) {
              to_remove.push(entry.id);
              continue;
          }
          if entry.fire_at <= now {
              fired.push((entry.target_actor, entry.message.clone()));
              to_remove.push(entry.id);
          }
      }
      
      if !to_remove.is_empty() {
          if let Ok(mut timers) = self.timers.write() {
              let mut new_heap = BinaryHeap::new();
              while let Some(entry) = timers.pop() {
                  if !to_remove.contains(&entry.id) {
                      new_heap.push(entry);
                  }
              }
              *timers = new_heap;
          }
      }
  ```
* **Explanation**: The method `tick` iterates over the entire `BinaryHeap` of timers, pops all entries, filters out the expired/cancelled elements, and pushes the remainder back into a new heap. This turns a priority queue tick operation into a slow $O(N \log N)$ process. Leveraging the heap property via `peek` allows retrieving expired elements from the top in $O(K \log N)$ complexity.
* **Proposed Refactored Version**:
  ```rust
  pub fn tick(&self, now: Instant) -> Vec<(u64, TimerMessage)> {
      let mut fired = Vec::new();

      if let Ok(mut timers) = self.timers.write() {
          while let Some(entry) = timers.peek() {
              if entry.cancelled.load(Ordering::SeqCst) {
                  timers.pop();
                  continue;
              }
              if entry.fire_at <= now {
                  if let Some(entry) = timers.pop() {
                      fired.push((entry.target_actor, entry.message));
                  }
              } else {
                  break;
              }
          }
      }
      fired
  }
  ```
* **Actionable Checklist**:
  - [ ] Implement `peek` logic inside the write-locked loop.
  - [ ] Discard elements if `cancelled` is true or if `fire_at` has passed.
  - [ ] Break iteration early when encountering an element that has not yet expired.
  - [ ] Remove the temporary `new_heap` instantiation.

---

### 3.5 Issue 5: Weak Type Safety in ORSet Tag Representation
* **File Path**: `src/runtime/crdt.rs`
* **Location**: Lines 320–339
* **Current Code Snippet**:
  ```rust
  pub type Tag = u64;

  // Inside ORSet:
  fn fresh_tag(&mut self) -> Tag {
      assert!(self.tag_counter <= u32::MAX as u64);
      let tag = (self.node_id << 32) | self.tag_counter;
      self.tag_counter += 1;
      tag
  }
  ```
* **Explanation**: Using a packed `u64` as a type alias requires manual bit shifting and dynamic assertions (`assert!(self.node_id <= u32::MAX as u64)`), which can result in runtime panics. Encapsulating this in a structured, compiler-enforced `Tag` type improves safety and expressiveness.
* **Proposed Refactored Version**:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
  pub struct Tag {
      pub node_id: u32,
      pub counter: u32,
  }

  pub struct ORSet<T: Clone + Eq + std::hash::Hash> {
      pub entries: HashMap<T, HashSet<Tag>>,
      pub tag_counter: u32,
      pub node_id: u32,
  }

  impl<T: Clone + Eq + std::hash::Hash> ORSet<T> {
      fn fresh_tag(&mut self) -> Tag {
          let tag = Tag { node_id: self.node_id, counter: self.tag_counter };
          self.tag_counter = self.tag_counter.checked_add(1).expect("Counter overflow");
          tag
      }
  }
  ```
* **Actionable Checklist**:
  - [ ] Create a struct `Tag` wrapping two `u32` fields (`node_id`, `counter`).
  - [ ] Refactor `ORSet` properties to use `Tag` instead of raw `u64`.
  - [ ] Replace unsafe shift logic with structured instantiation.

---

### 3.6 Issue 6: Stringly-Typed Helper Mappings in JIT compiler
* **File Path**: `src/jit/compiler.rs`
* **Location**: Lines 330–337
* **Current Code Snippet**:
  ```rust
  fn emit_binop(builder: &mut FunctionBuilder, helpers: &HashMap<&str, FuncRef>, regs_ptr: Value, op1: usize, op2: usize, dst: usize, helper_name: &str) {
      let a = load_reg(builder, regs_ptr, op1);
      let b = load_reg(builder, regs_ptr, op2);
      let func_ref = *helpers.get(helper_name).unwrap();
      let call = builder.ins().call(func_ref, &[a, b]);
      let result = builder.inst_results(call)[0];
      store_reg(builder, regs_ptr, dst, result);
  }
  ```
* **Explanation**: Looking up functions via string keys (e.g. `"nulang_iadd"`) is prone to typos. Typographic errors in lookup strings result in runtime panics at `.unwrap()`. Defining a strongly-typed enum for helper functions provides compile-time correctness guarantees.
* **Proposed Refactored Version**:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
  pub enum RuntimeHelper {
      IAdd,
      ISub,
      IMul,
      IDiv,
      IMod,
      // ...
  }

  fn emit_binop(
      builder: &mut FunctionBuilder,
      helpers: &HashMap<RuntimeHelper, FuncRef>,
      regs_ptr: Value,
      op1: usize,
      op2: usize,
      dst: usize,
      helper: RuntimeHelper,
  ) {
      let a = load_reg(builder, regs_ptr, op1);
      let b = load_reg(builder, regs_ptr, op2);
      let func_ref = *helpers.get(&helper).expect("Helper missing");
      let call = builder.ins().call(func_ref, &[a, b]);
      let result = builder.inst_results(call)[0];
      store_reg(builder, regs_ptr, dst, result);
  }
  ```
* **Actionable Checklist**:
  - [ ] Define the `RuntimeHelper` enum listing all runtime intrinsics.
  - [ ] Update JIT setup to construct a `HashMap<RuntimeHelper, FuncRef>`.
  - [ ] Replace helper string lookup arguments with enum values.

---

## 4. Verification & Test Coverage

### 4.1 Identified Coverage Gaps
* **Frontend Unit Testing**: Key components like `src/lexer.rs`, `src/parser.rs`, and `src/compiler.rs` have zero dedicated unit tests. Parser logic, tokenization states, and AST generation are only tested transitively via end-to-end integration tests in `src/integration_tests.rs`.
* **JIT Advanced Compiler Variants**: The specialised NaN-tag stripping JIT compiler (`src/jit/typed_compiler.rs`) and loop vectorizer JIT (`src/jit/simd_compiler.rs`) are completely untested. Existing JIT tests only check basic scalar operations.
* **Dual Heap Memory Allocator**: Nursery-to-tenured allocations, pointer alignment arithmetic, and promotion triggers in `src/runtime/dual_heap.rs` are untested at the unit level.
* **CRDT Manager**: The background replication synchronizer, SWIM membership synchronization, and cluster messaging in `src/runtime/crdt_manager.rs` have zero unit test coverage.

### 4.2 Suggestions for Adding Coverage
1. **Frontend Unit Tests**:
   * Add a `mod tests` inside `src/lexer.rs` to verify token boundaries, string literals, and edge-case syntax parsing (e.g. negative numbers, identifiers).
   * Write parser tests checking AST structure directly for complex expressions like `Handle`, nested `Match`, and custom type declarations.
2. **JIT Variants Testing**:
   * Add test scripts that generate loops of float operations and verify that the `simd_compiler.rs` generates correct SIMD vector instructions.
   * Add tests for `typed_compiler.rs` passing known type tagging structures to confirm tags are successfully optimized away.
3. **Dual Heap Testing**:
   * Write tests directly instantiating `DualHeap`. Check that small allocations populate the nursery, and running cycle collection promotes objects into the tenured heap correctly.
4. **CRDT Replication Tests**:
   * Write mock network integration tests for `crdt_manager.rs` that spawn two instances, simulate random packet drops, and assert eventually consistent state convergence.

---

## 5. Specification Alignment

### 5.1 Alignment Evaluation
The Nulang codebase implements the core syntax, typechecker, and interpretation modules outlined in the MVP requirements in `SPEC.md`. However, there are significant deviations and missing features when aligned with the design specifications of `SPEC2.md`, `DESIGN_AI_SDK.md`, and `DESIGN_WEB_FRAMEWORK.md`.

### 5.2 Critical Specification Gaps
1. **AI SDK Implementation Status**:
   * According to `DESIGN_AI_SDK.md`, agents should be declared via a declarative DSL syntax, e.g. `agent Researcher = { ... }`.
   * In the parser, the agent keyword matches block behavior, but in `src/typechecker.rs` the agent's properties (state, policy, memory, tools) are just stubbed with fresh variables.
   * There is no actual implementation in the interpreter VM to execute agent behaviors, LLM requests, memory retrieval, or tool binding.
2. **Missing Web Framework (`phoenix-nl`)**:
   * `DESIGN_WEB_FRAMEWORK.md` describes a highly concurrent Phoenix-inspired web framework with Endpoints, Routers, Controllers, Channels, and HTML templates.
   * No files or directories for the web framework exist in `src/`. The framework is completely missing from the codebase.
3. **Effect Handler Typechecking Disconnect**:
   * In `SPEC2.md` Chapter 4, algebraic effects must check both handler arms and return paths.
   * The typechecker (`src/typechecker.rs` at line 680) discards handler arms entirely. A function containing handlers that returns mismatched types will compile without errors, violating type-safety constraints.
4. **Distributed Actor Runtime Stubs**:
   * `SPEC2.md` Chapter 12 details transparent virtual actor activation and SWIM-based gossip.
   * In `src/vm.rs` (lines 1086–1091), the opcodes `OpCode::NodeId`, `OpCode::Migrate`, `OpCode::RAsk`, and `OpCode::Gossip` are completely stubbed out. No network communication or remote actor proxying is wired to these VM operations.
