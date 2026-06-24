# Nulang Implementation Specification

## Architecture

Monolithic Rust crate with modules. All shared types defined in a types module.

## Module Dependencies

```
lexer → token types
parser → lexer, ast, types
types → (shared: Type, EffectRow, Capability, etc.)
ast → types
hir → types
bytecode → (shared: OpCode, Instruction, Module, etc.)
compiler → ast, hir, bytecode, types
effects → types
capabilities → types
runtime::actor → bytecode, types
runtime::mailbox → types
runtime::scheduler → actor, mailbox
runtime::heap → types
runtime::gc → heap
runtime::supervisor → actor
vm → bytecode, runtime
repl → vm, compiler, parser, lexer
```

## Core Design Decisions

1. **Value representation**: NaN tagging on 64-bit values. Immediate integers/floats, heap pointers use NaN payload.
2. **Actor model**: Green threads (not OS threads). M:N scheduling. Work-stealing queue per worker.
3. **Memory**: Per-actor bump allocator. Shared immutable heap for `val` objects. No full GC in MVP - reference counting.
4. **Type system**: Basic types + generics + effects + capabilities. No dependent types in MVP.
5. **Bytecode**: Register-based, 256 registers per frame, 32-bit fixed-width instructions.

## MVP Scope

### In Scope
- Lexer: Full token set (literals, keywords, operators, delimiters)
- Parser: All expression types, declarations, actor definitions, agent definitions, effect handlers
- AST: Complete node types
- Type checker: Basic types, generics, function types, actor types, effect rows, capabilities
- Bytecode: 40 core opcodes (arithmetic, control flow, memory, actor ops)
- VM: Register-based execution, direct-threaded dispatch
- Runtime: Actor spawn, message send/receive, scheduler with work-stealing, bounded mailbox, supervision trees
- Compiler: AST -> bytecode for all core constructs
- REPL: Parse -> compile -> execute cycle

### Out of Scope (future phases)
- MIR optimization passes
- Full ORCA GC (reference counting in MVP)
- Multi-node distribution
- CRDT integration
- AI agent LLM integration (agent framework present, LLM effect stubbed)
- Package manager
- LSP server

## Shared Type Definitions

Defined in `src/types.rs`. All modules import from here.

### Core Types
- `Type`: Primitive, Tuple, Record, Variant, Function(with effect), Actor, Generic, Var
- `EffectRow`: Closed set or open row with row variable
- `Capability`: Iso, Trn, Ref, Val, Box, Tag + lattice operations
- `TypeVar`: u64 unique ID for type variables
- `Region`: u64 for region variables

### AST Types (in `src/ast.rs`)
- `Expr`: All expression variants (Literal, Var, Lambda, App, Let, Match, ActorNew, Send, Receive, Handle, Perform, If, Block, etc.)
- `Decl`: Function, Actor, Agent, TypeAlias, Module, Import
- `Pattern`: Wild, Var, Lit, Tuple, Record, Variant
- `Behavior`: Name, params, body, effect annotation

### Bytecode Types (in `src/bytecode.rs`)
- `OpCode`: u8 enum
- `Instruction`: OpCode + 3 u8 operands (or extended)
- `Module`: Constant pool + bytecode + behavior table + debug info
- `Constant`: String, Float, Int, TypeDescriptor, FunctionRef

### Runtime Types (in `src/runtime/`)
- `Value`: 64-bit NaN-tagged (immediate int/float, heap ptr, actor ref, special)
- `ActorRef`: 64-bit (node_id: u16, local_id: u32, generation: u16)
- `ActorContext`: self_addr, mailbox, heap pointer, behavior table, state
- `Message`: behavior_id + payload bytes + sender Addr
- `Mailbox`: MPSC bounded ring buffer
- `ActorHeap`: bump allocator with size classes
- `Scheduler`: worker threads + global queue

## Interfaces Between Modules

### Parser -> Compiler
- Parser produces `ast::Module` (list of declarations)
- Compiler consumes `ast::Module`, produces `bytecode::Module`

### Compiler -> VM
- VM loads `bytecode::Module` via `vm.load_module()`
- VM executes via `vm.run()` or `vm.call_function()`

### VM -> Runtime
- VM calls runtime functions for: spawn, send, receive, monitor, link, exit
- Runtime callbacks to VM for: behavior dispatch, message delivery

## Key Algorithms

### Lexer: Single-pass, hand-written state machine
### Parser: Recursive descent, Pratt for expressions (precedence climbing)
### Type Check: Unification-based (Algorithm W), constraint collection then solving
### VM Dispatch: Token-threaded (array of function pointers indexed by opcode)
### Scheduler: Chase-Lev deque per worker, FIFO local, LIFO stolen
### Mailbox: Atomic ring buffer (power of 2 capacity, head/tail atomics)
