# Nulang Language Specification v2.0

## DRAFT — December 2025

---

# Forward

This document defines the Nulang programming language, version 2.0. It is intended as the authoritative reference for both language implementers and users, providing a complete and precise account of Nulang's syntax, semantics, type system, runtime model, and standard library.

Nulang 2.0 represents a significant architectural evolution from the 1.x series. Where the earlier specification treated AI agents, distributed computing, and persistence as separate subsystems accessed through domain-specific keywords (`agent`, `cluster`, `store`), version 2.0 unifies these concerns under a single, coherent abstraction: the actor. In Nulang 2.0, all concurrent and distributed computation is expressed through actors. AI capabilities are granted to actors through the capability system, not through a separate agent DSL. Durability is a property of actors, not a separate storage layer. Distribution is an emergent property of the actor runtime, not a bolt-on framework.

This unification yields a language with fewer primitives and greater compositional power. A programmer learns one abstraction—the actor with behaviors, state, and effects—and applies it uniformly from a single-threaded script to a globally distributed, AI-augmented workflow.

The specification is organized into five conceptual layers:

1. **The Language Layer** (Chapters 1–7) defines the core language: syntax, types, algebraic effects, capability-based security, expressions, and declarations. This layer is self-contained and can be implemented independently of any runtime.

2. **The Actor Runtime Layer** (Chapter 8) defines the actor model: how actors are declared, how they communicate via asynchronous message passing, how they manage state, and how they are supervised. This layer is the foundation upon which all higher layers are built.

3. **The Durable Execution Layer** (Chapter 9) extends the actor runtime with persistence. Persistent actors survive process restarts through automatic checkpointing, event journaling, deterministic replay, and snapshotting.

4. **The Distributed Platform Layer** (Chapter 12) extends the durable actor runtime across machine boundaries. Virtual actors are transparently activated on any cluster node. Messages are routed across the network. CRDT state converges automatically. Faults are contained and recovered.

5. **The AI Runtime Layer** (Chapter 11) provides language-integrated access to large language models, tool use, memory systems, and planning. AI capabilities are expressed through the same algebraic effect system used for IO and network effects, and are gated by the same capability-based security model.

Each chapter contains a detailed outline of all sections and subsections, followed by the prose and examples for those sections. Chapters 1 through 3 are fully written. Chapters 4 through 15 contain detailed outlines with section headings, descriptive bullet points, and at least one complete code example per chapter to illustrate the key concepts.

Unless otherwise noted, all examples in this specification are complete, syntactically valid Nulang programs or program fragments.

---

# Table of Contents

- Chapter 1: Introduction
- Chapter 2: Lexical Structure
- Chapter 3: Types
- Chapter 4: Effects
- Chapter 5: Capabilities
- Chapter 6: Expressions
- Chapter 7: Declarations
- Chapter 8: Actors
- Chapter 9: Persistent Actors
- Chapter 10: Workflows
- Chapter 11: AI Runtime
- Chapter 12: Distributed Runtime
- Chapter 13: WebAssembly Integration
- Chapter 14: Standard Library
- Chapter 15: Operational Model
- Appendix A: Grammar Reference
- Appendix B: Built-in Types Reference
- Appendix C: Effect Reference
- Appendix D: Migration Guide from v1 to v2

---

# Chapter 1: Introduction

## 1.1 What is Nulang?

Nulang is a general-purpose, statically typed programming language designed for building distributed, concurrent, durable, and AI-augmented applications. It compiles to WebAssembly and runs on a purpose-built actor runtime that provides persistence, clustering, and AI integration as first-class language features.

Nulang occupies a distinctive position in the language design space. Like Erlang and Elixir, it is built on the actor model of concurrency, where independent computational entities communicate exclusively through asynchronous message passing. Like Rust, it employs a sophisticated type system with affine reference capabilities to guarantee memory safety and data-race freedom at compile time. Like Koka and Eff, it uses algebraic effects as the primary mechanism for defining, composing, and handling computational effects such as IO, exceptions, state mutation, and—uniquely—AI model inference. Like modern workflow orchestration systems, it provides durable execution semantics where long-running processes survive crashes and restarts automatically.

The synthesis of these features produces a language with four defining characteristics:

**Concurrency without locks.** Nulang actors do not share mutable memory. All communication is asynchronous and message-based, eliminating data races by construction. The type system enforces that mutable references cannot escape an actor's boundary.

**Effects as a unifying abstraction.** All side effects in Nulang—reading a file, making an HTTP request, querying a database, calling an LLM—are expressed through a single mechanism: algebraic effects. An effect is declared, performed, and handled within a well-typed framework that makes effectful dependencies explicit in function signatures.

**Capability-based security.** Every reference in Nulang carries a capability that governs how it may be read, written, and shared. These capabilities form a lattice of authority that propagates through the program automatically. Combined with effect declarations, they provide a comprehensive security model: a function's type signature reveals exactly what it can do and what data it can access.

**Durable execution by default.** Any actor can be declared `persistent`, which enables automatic checkpointing, event journaling, and deterministic replay. Persistent actors form the building blocks of workflows—long-running compositions that survive crashes, support compensation, and orchestrate human-in-the-loop interactions.

## 1.2 Design Philosophy

Nulang's design is guided by five principles that influence every aspect of the language, from syntax to runtime architecture.

**Composition over configuration.** Nulang prefers composable language primitives over framework-specific configuration. Distributed systems are built by composing actors, not by configuring cluster topologies. AI integration is achieved by performing effects, not by wiring model providers. Persistence is a keyword, not a deployment concern. This principle ensures that the full power of the language—types, effects, pattern matching, higher-order functions—is available at every layer of the system.

**Explicit is better than implicit.** Every effect a function can perform is visible in its type signature through effect rows. Every reference's sharing properties are visible through capability annotations. Every state model is declared explicitly. This explicitness makes programs easier to reason about, test, and audit. It also enables powerful program analyses: the compiler can verify that a function performs no network effects, that an actor's state is serializable, or that a workflow step is deterministic.

**Failure as a first-class concern.** Nulang treats failure as a normal condition to be handled, not an exceptional circumstance to be ignored. Actors are supervised by parent actors that define restart strategies. Messages that cannot be delivered are reported through links and monitors. Workflow steps that fail trigger compensation handlers. This supervision-oriented design, inherited from Erlang's "let it crash" philosophy, enables the construction of resilient systems that recover automatically from hardware failures, network partitions, and software bugs.

**Uniformity across scales.** The same actor abstraction works for a single-process application and a globally distributed cluster. A `persistent` actor on one node uses the same syntax and semantics as a `persistent` actor replicated across a hundred nodes. An effect performed in a unit test can be handled by a pure mock, while the same effect in production is handled by a system call. This uniformity reduces the conceptual surface area programmers must learn and enables code reuse across deployment scenarios.

**Language-integrated AI.** Access to large language models is not provided through external SDKs or API wrappers. It is a first-class language feature, expressed through the algebraic effect system and governed by the capability security model. A function that calls an LLM declares this in its effect row. An actor that uses AI must hold the `llm` capability. This integration enables static reasoning about AI usage, structured output through types, and seamless composition of LLM calls with other effects.

## 1.3 The Actor as Universal Abstraction

The actor is the fundamental unit of computation, concurrency, state, and distribution in Nulang. Every running program consists of a tree of actors, each with its own mailbox, behaviors, and optionally persistent state.

An actor is declared with the `actor` keyword, followed by a name, optional type parameters, optional capability parameters, and a body containing behaviors, state declarations, and helper functions:

```nulang
actor Counter[T: Numeric] {
  state local count: T = 0

  behavior increment(by: T) {
    count = count + by
  }

  behavior get(): T {
    count
  }

  behavior reset() {
    count = 0
  }
}
```

Actors communicate exclusively through asynchronous messages. Sending a message is a non-blocking operation that places the message in the recipient's mailbox. The recipient processes messages sequentially, one at a time, guaranteeing that an actor's behavior handlers execute atomically with respect to each other. This single-threaded illusion within each actor eliminates the need for locks or other synchronization primitives.

Actors can be made persistent by adding the `persistent` keyword:

```nulang
persistent actor BankAccount {
  state durable balance: Decimal = 0.00

  behavior deposit(amount: Decimal) {
    balance = balance + amount
  }

  behavior withdraw(amount: Decimal): Result[Unit, String] {
    if amount > balance then
      Error("Insufficient funds")
    else
      balance = balance - amount
      Ok(())
  }

  behavior get_balance(): Decimal {
    balance
  }
}
```

The `persistent` keyword enables automatic checkpointing after each behavior invocation, ensuring that the actor's state survives process restarts. The `durable` state model (one of four available) guarantees that `balance` is written to persistent storage before the behavior returns.

Actors can hold capabilities that grant them authority to perform effects:

```nulang
actor ChatBot {
  capability llm
  capability http

  state local history: List[Message] = []

  behavior ask(question: String): String {
    let context = build_context(history)
    let answer = perform llm.complete(prompt: context, user_message: question)
    history = history ++ [User(question), Assistant(answer)]
    answer
  }
}
```

The `capability llm` declaration indicates that this actor is authorized to perform LLM effects. Without this declaration, the `perform llm.complete(...)` expression would be a compile-time error. This is capability-based security in action: authority is explicit, granular, and auditable.

## 1.4 State Models Overview

Every state variable in an actor has an associated *state model* that determines how it is stored, replicated, and recovered. Nulang provides four state models:

| Model | Persistence | Replication | Recovery | Use Case |
|-------|------------|-------------|----------|----------|
| `local` | None | None | Reset to initial value | Ephemeral caches, temporary buffers |
| `durable` | Snapshot + journal | None | Replay from journal | Single-node persistent state |
| `event_sourced` | Event journal | Event stream | Full event replay | Audit trails, temporal queries |
| `crdt` | Delta log | Automatic merge | CRDT merge | Shared distributed state |

The state model is declared alongside the variable:

```nulang
persistent actor ShoppingCart {
  state durable items: List[CartItem] = []
  state crdt   viewers: GSet[NodeId] = GSet.empty()
  state local  temp_discount: Option[Decimal] = None

  behavior add_item(item: CartItem) {
    items = items ++ [item]
  }

  behavior apply_discount(code: String) {
    -- Temporary, not persisted
    temp_discount = lookup_discount(code)
  }

  behavior track_viewer(node: NodeId) {
    -- Automatically replicated across cluster
    viewers = viewers.add(node)
  }
}
```

The compiler verifies that each state model is used correctly. For example, `event_sourced` state can only be mutated by appending events through a designated `emit` operation, and `crdt` state must use a CRDT data type that supports automatic merging.

## 1.5 Capability Security Overview

Nulang employs two complementary capability systems: reference capabilities (which control how data can be aliased and shared) and authority capabilities (which control what effects an actor can perform).

Reference capabilities are part of the type system. Every reference has one of six capabilities:

- `iso` — unique, sendable (no other references exist)
- `trn` — unique but locally writable (transitioning to `val`)
- `ref` — uniquely writable but not sendable
- `val` — immutable and sendable
- `box` — read-only view of `ref` or `val`
- `tag` — opaque identifier, not readable

These capabilities form a lattice under a subtyping relation. The compiler uses them to guarantee that no data race can occur: an `iso` or `val` reference can be sent to another actor because the sender cannot retain the ability to mutate the data.

Authority capabilities are declared on actors and govern which effects they can perform:

```nulang
capability llm      -- Can perform LLM effects
capability http     -- Can make HTTP requests
capability file     -- Can access the file system
capability network  -- Can open network connections
capability random   -- Can access random number generation
capability time     -- Can access the system clock
```

Authority capabilities can be delegated from one actor to another, and they can be revoked at any time. This enables fine-grained security policies: an AI agent can be given `llm` and `http` capabilities, but not `file` or `network`.

## 1.6 Relationship to Other Languages

Nulang's design synthesizes ideas from several language families:

**From the actor languages (Erlang, Elixir, Pony, Akka):** The actor model as the fundamental concurrency primitive, supervision trees for fault tolerance, and the philosophy of isolated mutable state. Nulang differs in its static type system, algebraic effects, and unified treatment of persistence and distribution.

**From the effect languages (Koka, Eff, Flix):** Algebraic effects and handlers as the primary abstraction for computational effects, and effect rows in function types. Nulang extends this to include LLM calls as just another effect, and integrates effects with the actor model so that effect handlers can be actor-local.

**From the capability languages (Pony, Rust, Wyvern):** Reference capabilities for memory safety and data-race freedom. Nulang's capability system is most closely related to Pony's, but extends it with authority capabilities and distributed capability delegation.

**From the workflow languages (Temporal, Durable Functions):** Durable execution, deterministic replay, and saga compensation. Nulang embeds these concepts into the actor model rather than providing them as a separate framework.

**From the ML family (OCaml, Haskell, F#, Elm):** Hindley-Milner type inference, algebraic data types, pattern matching, and higher-order functions. Nulang's type system is closest to Elm's in its simplicity and inferability, extended with reference capabilities and effect rows.

---

# Chapter 2: Lexical Structure

## 2.1 Source Files and Encoding

Nulang source files use the `.nu` extension. A source file is a sequence of Unicode code points encoded in UTF-8. The UTF-8 Byte Order Mark (U+FEFF) at the beginning of a file is recognized and ignored, though its use is discouraged.

A source file consists of a sequence of declarations: functions, type definitions, actor definitions, effect definitions, and module-level expressions. Declarations are separated by whitespace; there is no statement terminator. The parser uses an indentation-sensitive grammar where indentation determines block structure (see Section 2.8).

A minimal Nulang program is a single module file that need not contain a `main` function. Module-level expressions are evaluated in order when the program starts, and any spawned actors continue running:

```nulang
-- hello.nu: a minimal Nulang program
let greeting = "Hello, World!"
perform io.println(greeting)
```

Programs are compiled to WebAssembly modules. The entry point of a Nulang program is the `__nulang_start` function generated by the compiler, which initializes the runtime, evaluates module-level bindings, and starts the actor scheduler.

## 2.2 Comments

Nulang supports two comment styles:

**Line comments** begin with `--` and extend to the end of the line:

```nulang
-- This is a line comment
let x = 42  -- Comments can also follow code on the same line
```

**Block comments** are delimited by `{-` and `-}`. Block comments may be nested, which allows commenting out code that itself contains block comments:

```nulang
{- This is a block comment.
   It can span multiple lines.
   {- And they can be nested. -}
-}
```

Comments are treated as whitespace by the parser and have no semantic significance. They may appear between any two tokens.

**Documentation comments** are a special form of block comment delimited by `{-|` and `-}`. These comments are extracted by the documentation generator and associated with the declaration that follows them:

```nulang
{-| Calculate the factorial of a non-negative integer.
    Returns 1 for n = 0, and n * factorial(n - 1) otherwise.
    
    Example:
    factorial(5) == 120
-}
let factorial = (n: Int) -> Int {
  if n == 0 then 1 else n * factorial(n - 1)
}
```

## 2.3 Keywords

The following identifiers are reserved as keywords in Nulang and may not be used as ordinary identifiers:

```
actor        behavior     capability   compensate
crdt         durable      effect       else
emit         enum         event        event_sourced
false        handle       if           iso
let          local        match        module
parallel     perform      persistent   ref
resume       state        step         tag
then         trn          true         type
val          workflow     box          import
from         as           step         parallel
```

Keywords are case-sensitive and must be written in lowercase. Keywords that introduce declarations (`actor`, `behavior`, `effect`, `let`, `type`, `state`, `capability`, `persistent`, `workflow`, `event`) are only recognized in declaration position and may be used as field names or variables in expression position, though this is discouraged.

## 2.4 Identifiers

An identifier is a sequence of Unicode characters that begins with a letter (Unicode categories `Lu`, `Ll`, `Lt`, `Lm`, or `Lo`) or an underscore (`_`), followed by any number of letters, decimal digits (Unicode category `Nd`), or underscores.

Nulang uses the following naming conventions, which are enforced by the compiler's style checker:

- **Types and modules**: PascalCase (`String`, `Option`, `HttpClient`, `BankAccount`)
- **Functions and variables**: snake_case or camelCase (`map`, `get_balance`, `process_request`)
- **Type variables in generics**: PascalCase with a single uppercase letter, or PascalCase starting with one (`T`, `U`, `A`, `Elem`, `Key`)
- **Effect names**: PascalCase (`IO`, `FileSystem`, `Network`, `LLM`)
- **Constants**: UPPER_SNAKE_CASE (`MAX_RETRIES`, `PI`)

Examples of valid identifiers:

```nulang
name         _private     http2        x_y_z
Counter      Option       T            Elem
```

## 2.5 Literals

Nulang provides literals for the following types:

### 2.5.1 Integer Literals

Integer literals are sequences of decimal digits, optionally prefixed with a base indicator:

```nulang
42        -- Decimal
0x2A      -- Hexadecimal
0o52      -- Octal
0b101010  -- Binary
```

Integer literals without a type annotation are polymorphic and can be instantiated to any type that implements the `Integral` typeclass, subject to range constraints. The default type for an unannotated integer literal is `Int`.

Underscores may be inserted between digits for readability and are ignored by the parser:

```nulang
1_000_000     -- One million
0xFF_FF       -- Max 16-bit value
0b1010_0101   -- Grouped binary digits
```

### 2.5.2 Floating-Point Literals

Floating-point literals consist of an integer part, a decimal point, a fractional part, and optionally an exponent:

```nulang
3.14159
2.99792458e8    -- Scientific notation
1.0e-9          -- Small numbers
```

Floating-point literals default to type `Float`. An `f64` suffix may be used to specify 64-bit precision explicitly; `f32` for 32-bit.

### 2.5.3 String Literals

String literals are delimited by double quotes (`"`). They may contain any Unicode character except an unescaped double quote, backslash, or newline. The following escape sequences are recognized:

```nulang
"Hello, World!"
"Line 1\nLine 2"     -- Newline
"Tab\tseparated"      -- Tab
"Quote: \"hello\""    -- Escaped quotes
"Backslash: \\"       -- Escaped backslash
"Unicode: \u{1F600}"  -- Unicode escape (emoji)
```

Multi-line string literals use triple quotes (`"""`) and preserve all characters between the delimiters, including newlines and indentation:

```nulang
let poem = """
    Roses are red,
    Violets are blue,
    Nulang is typed,
    And effects are too.
    """
```

The common leading whitespace of all non-empty lines is stripped from multi-line strings. In the example above, each line's leading 4 spaces are removed.

### 2.5.4 Character Literals

Character literals are single Unicode characters enclosed in single quotes (`'`). They have type `Char`:

```nulang
'a'     '\n'    '\u{03BB}'   -- Greek letter lambda
```

### 2.5.5 Boolean Literals

The boolean literals are `true` and `false`, with type `Bool`.
n
### 2.5.6 Unit Literal

The unit literal is `()`, with type `Unit`. It represents the absence of a meaningful value and is the return type of effectful operations that produce no data.

## 2.6 Operators

Nulang operators are classified by precedence level, from highest (tightest binding) to lowest. Operators at the same precedence level are left-associative unless noted.

### 2.6.1 Arithmetic Operators

| Operator | Description | Example |
|----------|-------------|---------|
| `+` | Addition | `a + b` |
| `-` | Subtraction | `a - b` |
| `*` | Multiplication | `a * b` |
| `/` | Division | `a / b` |
| `%` | Remainder | `a % b` |
| `**` | Exponentiation (right-associative) | `2 ** 10` |
| `-` | Unary negation | `-x` |

All arithmetic operators work on any numeric type (`Int`, `Float`, `Decimal`). Mixed-type arithmetic requires explicit conversion.

### 2.6.2 Comparison Operators

| Operator | Description |
|----------|-------------|
| `==` | Structural equality |
| `!=` | Structural inequality |
| `<` | Less than |
| `<=` | Less than or equal |
| `>` | Greater than |
| `>=` | Greater than or equal |

Comparison operators are non-associative. Chained comparisons like `a < b < c` are not permitted; use `(a < b) && (b < c)` instead.

### 2.6.3 Boolean Operators

| Operator | Description |
|----------|-------------|
| `&&` | Logical AND (short-circuiting) |
| `\|\|` | Logical OR (short-circuiting) |
| `!` | Logical NOT (unary) |

The `&&` and `||` operators use short-circuit evaluation: the right operand is only evaluated if necessary.

### 2.6.4 Reference Capability Operators

| Operator | Description |
|----------|-------------|
| `consume` | Consume a reference, producing an `iso` |
| `recover` | Recover a reference to `iso` or `val` |

These operators are discussed in detail in Chapter 5.

### 2.6.5 The Pipe Operator

The pipe operator `|>` passes the left operand as the last argument to the function on the right:

```nulang
list |> map(f) |> filter(g) |> fold(h, 0)
-- Equivalent to: fold(h, 0, filter(g, map(f, list)))
```

The pipe operator has low precedence and is left-associative. It is described fully in Section 6.9.

## 2.7 Delimiters

Nulang uses the following delimiters:

| Delimiter | Usage |
|-----------|-------|
| `()` | Parentheses for grouping and function arguments |
| `{}` | Braces for actor bodies and explicit blocks |
| `[]` | Square brackets for array literals and type parameters |
| `,` | Comma for separating list elements |
| `:` | Colon for type annotations and field definitions |
| `;` | Semicolon for separating expressions on the same line (rare) |
| `->` | Arrow for function types and match branches |
| `=>` | Fat arrow for lambda expressions |
| `=` | Equals for assignments and definitions |
| `..` | Double dot for record update and range operations |
| `\|` | Vertical bar for pattern match branches |

## 2.8 Indentation-Based Grouping

Nulang uses indentation to determine block structure, following a rule similar to Haskell's *offside rule*. A block begins when a construct expects a sub-expression, and the indentation of the first token in that sub-expression determines the indentation level for the entire block. All subsequent lines in the block must be indented at least to this level.

Specifically:

1. After `then` in an `if` expression, the expression body must be indented.
2. After `else`, the alternative expression must be indented.
3. After `let` with an `=`, the binding expression must be indented.
4. After `behavior` name and parameters, the behavior body must be indented.
5. After `actor` name, the actor body must be indented.
6. After `workflow` name, the workflow body must be indented.
7. After a `match` expression, each `\|` branch must be indented consistently.
8. After `handle` and `perform`, the handler body must be indented.

The parser allows the use of explicit braces `{` and `}` to override indentation-based grouping. When braces are used, indentation is not significant within the braces:

```nulang
-- Indentation-based
let max = (a: Int, b: Int) -> Int {
  if a > b then
    a
  else
    b
}

-- Braces-based (equivalent)
let max = (a: Int, b: Int) -> Int {
  if a > b then { a } else { b }
}
```

When mixing indentation and braces, the outermost construct determines the mode. If the opening token of a block uses braces, all nested blocks within it may use braces freely. If the outer block uses indentation, braces may still be used for individual inner blocks.

A tab character is treated as equivalent to 2 spaces for indentation purposes. Consistent use of spaces is strongly recommended.

```nulang
-- Example: indentation in an actor definition
actor WeatherService {
  state local cache: Map[String, Weather] = Map.empty()

  behavior get_forecast(city: String): Weather {
    match Map.get(cache, city) {
      | Some(weather) => weather
      | None => {
          let weather = fetch_from_api(city)
          cache = Map.insert(cache, city, weather)
          weather
        }
    }
  }
}
```

---

# Chapter 3: Types

## 3.1 Type System Overview

Nulang employs a static type system based on Hindley-Milner type inference with extensions for reference capabilities, effect rows, and generic programming. The type system has the following properties:

**Soundness.** Well-typed programs do not go wrong at runtime due to type errors. The type system prevents null pointer dereferences (through `Option` types), out-of-bounds array access (through safe indexing), and data races (through reference capabilities).

**Complete inference.** The types of all expressions can be inferred automatically by the compiler. Type annotations are optional except on top-level function parameters and explicit type declarations. Annotations may be provided for documentation purposes or to constrain inference.

**Parametric polymorphism.** Functions and types may be parameterized by type variables, enabling generic programming without the overhead of boxing or runtime type checks.

**Effect tracking.** Function types include an effect row that describes which computational effects the function may perform. This makes effectful dependencies explicit and enables local reasoning about code.

**Capability safety.** Reference types are qualified with capabilities that control how data can be read, written, and shared across actor boundaries. The capability system guarantees memory safety and data-race freedom.

Types in Nulang are organized into a hierarchy of *kinds*. The base kind `*` represents ordinary types (types of values). Higher kinds represent type constructors: `* -> *` is the kind of a type constructor taking one type argument (like `List`), and `* -> * -> *` takes two (like `Map`).

## 3.2 Primitive Types

Nulang provides the following primitive types:

### 3.2.1 Bool

The type `Bool` has two values: `true` and `false`. It supports the logical operators `&&`, `||`, and `!`.

```nulang
let is_valid = (x: Int) -> Bool {
  x > 0 && x < 100
}
```

### 3.2.2 Int

The type `Int` represents signed integers with platform-defined precision (at least 32 bits, typically 64 bits on modern platforms). It supports all arithmetic operators and bitwise operations (`&`, `|`, `^`, `<<`, `>>`).

```nulang
let double = (x: Int) -> Int { x * 2 }
let is_even = (x: Int) -> Bool { x % 2 == 0 }
```

### 3.2.3 Float

The type `Float` represents IEEE 754 double-precision floating-point numbers. Single-precision `Float32` is also available for memory-constrained scenarios.

```nulang
let area = (radius: Float) -> Float {
  3.14159 * radius * radius
}
```

### 3.2.4 Decimal

The type `Decimal` represents arbitrary-precision decimal numbers, suitable for financial calculations where floating-point rounding is unacceptable. `Decimal` values are created from string literals or integer literals:

```nulang
let price: Decimal = 19.99
let tax_rate: Decimal = 0.08
let total = price * (1 + tax_rate)  -- 21.5892
```

### 3.2.5 Char

The type `Char` represents a single Unicode scalar value. Character literals use single quotes.

### 3.2.6 Unit

The type `Unit` has a single value `()` and is used for functions and effects that return no meaningful value. It is analogous to `void` in C or `()` in Haskell.

## 3.3 Product Types

Product types combine multiple values into a single value. Nulang provides two product type constructors: tuples and records.

### 3.3.1 Tuples

A tuple is an ordered collection of values of possibly different types. Tuple types are written with parentheses and commas; tuple values use the same syntax:

```nulang
let point: (Float, Float) = (3.0, 4.0)
let person: (String, Int, Bool) = ("Alice", 30, true)
```

Tuples are destructured with pattern matching:

```nulang
let (x, y) = point
let distance = (p: (Float, Float)) -> Float {
  let (x, y) = p
  (x * x + y * y) ** 0.5
}
```

The empty tuple `()` is the same as the unit value. Single-element tuples `(a,)` are distinguished from parenthesized expressions by the trailing comma.

Tuple components are accessed by position using zero-based indexing: `point.0`, `point.1`.

### 3.3.2 Records

A record is a labeled product type, where each field has a name and a type. Record types are structural: two record types are equivalent if they have the same fields with the same types, regardless of declaration order.

```nulang
let person = { name = "Alice", age = 30, active = true }

-- Type is inferred as: { name: String, age: Int, active: Bool }
let greet = (p: { name: String, age: Int }) -> String {
  "Hello, " ++ p.name ++ "! You are " ++ Int.to_string(p.age) ++ " years old."
}
```

Record fields are accessed using dot notation: `person.name`, `person.age`.

Records support structural update with the `..` syntax, which creates a new record with modified fields:

```nulang
let older_person = { person .. age = person.age + 1 }
-- Result: { name = "Alice", age = 31, active = true }
```

The update expression `r .. f = v` creates a new record with all fields of `r` except `f`, which is replaced by `v`. The original record is not modified; records are immutable by default.

Record types can be abbreviated using type aliases (Section 3.11):

```nulang
type Person = { name: String, age: Int, active: Bool }

type Point = { x: Float, y: Float }

let translate = (p: Point, dx: Float, dy: Float) -> Point {
  { p .. x = p.x + dx, y = p.y + dy }
}
```

## 3.4 Sum Types

Sum types represent values that can be one of several alternatives. Nulang provides two sum type constructors: variants (tagged unions) and enums.

### 3.4.1 Variant Types

A variant type is defined with the `type` keyword and consists of a set of constructors, each optionally carrying data:

```nulang
type Option[T] =
  | None
  | Some(T)

type Result[T, E] =
  | Ok(T)
  | Error(E)

type Tree[T] =
  | Leaf
  | Node { left: Tree[T], value: T, right: Tree[T] }
```

Variant constructors are used to create values, and pattern matching is used to destructure them:

```nulang
let safe_divide = (a: Float, b: Float) -> Result[Float, String] {
  if b == 0.0 then
    Error("Division by zero")
  else
    Ok(a / b)
}

let handle_result = (r: Result[Float, String]) -> String {
  match r {
    | Ok(value) => "Result: " ++ Float.to_string(value)
    | Error(msg) => "Error: " ++ msg
  }
}
```

Record-style constructors (like `Node` above) carry named fields. Tuple-style constructors (like `Some` and `Ok`) carry anonymous positional fields.

### 3.4.2 Enums

An enum is a variant type where no constructor carries data. Enums are defined with a concise syntax:

```nulang
type Color = Red | Green | Blue

type Status = Pending | Running | Completed | Failed
```

Enum constructors are compared with structural equality and pattern-matched like other variants:

```nulang
let status_message = (s: Status) -> String {
  match s {
    | Pending   => "Waiting to start..."
    | Running   => "In progress..."
    | Completed => "Done!"
    | Failed    => "Something went wrong."
  }
}
```

## 3.5 Function Types

Function types describe the type of a function, including its parameter types, return type, and effect row. The general form is:

```nulang
(A1, A2, ..., An) -> [EffectRow] R
```

Where `A1` through `An` are the parameter types, `R` is the return type, and `EffectRow` is the set of effects the function may perform. The effect row may be omitted if it is the default (pure) effect row.

```nulang
-- Pure function: no effects
let add: (Int, Int) -> Int = (a, b) -> a + b

-- Effectful function: may perform IO
let greet: (String) -> [IO] Unit = (name) -> {
  perform io.println("Hello, " ++ name)
}

-- Function with explicit effect row
let read_config: () -> [FileSystem, IO] Result[Config, String] = () -> {
  perform filesystem.read("config.json")
}
```

Effect rows are enclosed in square brackets after the parameter list. An empty effect row `[]` denotes a pure function. A non-empty row lists the effects the function may perform.

Functions are first-class values: they can be passed as arguments, returned from other functions, and stored in data structures.

```nulang
let compose = (f: (B) -> C, g: (A) -> B) -> (A) -> C {
  (x) -> f(g(x))
}

let twice = (f: (Int) -> Int) -> (Int) -> Int {
  (x) -> f(f(x))
}
```

## 3.6 Generic Types

Type constructors may be parameterized by type variables, enabling generic programming. Type variables are written with a leading uppercase letter and may be constrained by typeclasses.

### 3.6.1 Type Parameters

Type parameters are declared in square brackets after a type or function name:

```nulang
type Pair[A, B] = (A, B)

let first = [T, U] (p: Pair[T, U]) -> T {
  let (a, _) = p
  a
}

type Box[T] =
  | Empty
  | Full(T)

let map_box = [A, B] (b: Box[A], f: (A) -> B) -> Box[B] {
  match b {
    | Empty    => Empty
    | Full(x)  => Full(f(x))
  }
}
```

### 3.6.2 Type Parameter Constraints

Type parameters may be constrained to implement certain typeclasses. A typeclass constraint is written as `T: ClassName` in the type parameter list:

```nulang
let max = [T: Ordered] (a: T, b: T) -> T {
  if a > b then a else b
}

let sum = [T: Numeric] (list: List[T]) -> T {
  List.fold(list, 0, (acc, x) -> acc + x)
}
```

Common typeclasses include:

| Typeclass | Methods | Description |
|-----------|---------|-------------|
| `Eq` | `==`, `!=` | Structural equality |
| `Ordered` | `<`, `<=`, `>`, `>=` | Total ordering |
| `Numeric` | `+`, `-`, `*`, `/` | Arithmetic operations |
| `Show` | `to_string` | String representation |
| `Semigroup` | `++` | Associative combination |
| `Monoid` | `++`, `empty` | Semigroup with identity |

### 3.6.3 Higher-Kinded Types

Type parameters can themselves be type constructors (higher-kinded types):

```nulang
let map = [F: Functor, A, B] (fa: F[A], f: (A) -> B) -> F[B] {
  Functor.map(fa, f)
}

let sequence = [F: Applicative, G: Traversable, A] (fga: G[F[A]]) -> F[G[A]] {
  Traversable.sequence(fga)
}
```

## 3.7 Array and String Types

### 3.7.1 Arrays

Arrays are homogeneous, dynamically-sized sequences with O(1) indexed access. The type `Array[T]` represents an array of elements of type `T`.

```nulang
let numbers: Array[Int] = [1, 2, 3, 4, 5]
let first = numbers.0     -- 1
let length = Array.length(numbers)
```

Array elements are accessed using bracket notation: `arr.i` or `arr.(i)` for dynamic indexing. Arrays are immutable by default; mutable arrays are available through reference types.

Array literals use square bracket syntax. The type can usually be inferred from the elements.

Common array operations:

```nulang
let doubled = Array.map(numbers, (x) -> x * 2)       -- [2, 4, 6, 8, 10]
let evens = Array.filter(numbers, (x) -> x % 2 == 0) -- [2, 4]
let total = Array.fold(numbers, 0, (a, b) -> a + b)  -- 15
let has_three = Array.contains(numbers, 3)           -- true
```

### 3.7.2 Strings

The type `String` represents a sequence of Unicode characters. Strings are immutable and support concatenation, slicing, and various transformation functions.

```nulang
let greeting = "Hello"
let target = "World"
let message = greeting ++ ", " ++ target ++ "!"

let length = String.length(message)           -- 13
let upper = String.to_uppercase(message)        -- "HELLO, WORLD!"
let contains_hello = String.contains(message, "Hello")  -- true
let words = String.split(message, ", ")         -- ["Hello", "World!"]
```

String interpolation provides a convenient syntax for embedding expressions within strings:

```nulang
let name = "Nulang"
let version = 2.0
let desc = "Welcome to {name} version {version}!"
-- Result: "Welcome to Nulang version 2.0!"
```

Interpolated expressions are enclosed in `{` and `}` within a string literal. The expression's `to_string` method is called automatically.

## 3.8 Reference Types

Nulang's reference type system is adapted from Pony's capability system. Every reference to an object carries a *reference capability* that determines how it may be read, written, and shared. These capabilities enable the compiler to guarantee memory safety and data-race freedom.

### 3.8.1 The Six Reference Capabilities

| Capability | Read | Write | Sendable | Description |
|------------|------|-------|----------|-------------|
| `iso` | Yes | Yes | Yes | Unique reference; no aliases exist |
| `trn` | Yes | Yes | No | Transitioning; will become `val` |
| `ref` | Yes | Yes | No | Standard read-write reference |
| `val` | Yes | No | Yes | Immutable, globally readable |
| `box` | Yes | No | No | Read-only view of `ref` or `val` |
| `tag` | No | No | Yes | Opaque reference; only identity |

Reference capabilities are written as a prefix to the type: `iso String`, `val Array[Int]`, `ref Tree[T]`.

```nulang
-- iso: unique, can be sent to another actor
let unique_data: iso Buffer = Buffer.create(1024)

-- val: immutable, can be shared freely
let config: val Config = load_config()

-- ref: local mutable reference
let counter: ref Int = 0

-- box: read-only view
let view: box { name: String } = person_record
```

### 3.8.2 Capability Semantics

An `iso` reference guarantees that no other reference to the same object exists. This makes `iso` references safe to send to other actors, because the sender cannot retain any alias that would allow concurrent mutation. When an `iso` reference is consumed (sent or destructured), the original binding becomes inaccessible.

A `val` reference is an immutable view of an object. Multiple `val` aliases can exist, and `val` references can be sent to other actors because the data cannot change. An object is promoted to `val` when all write-capable references (`iso`, `trn`, `ref`) are given up.

A `ref` reference is the default for local mutable data. It allows reading and writing but cannot be sent to other actors because the sender could retain an alias and modify the data concurrently.

A `box` reference provides read-only access. It can view either a `ref` object (in which case the `ref` owner may still mutate it) or a `val` object (which is immutable). Because `box` does not guarantee immutability of the underlying object, it cannot be sent to other actors.

A `tag` reference is an opaque identifier. It carries no read or write permissions but can be used for identity comparison and can be sent to other actors. `tag` references are useful for maintaining relationships between actors without accessing their data.

### 3.8.3 Capability Defaults

When no capability is explicitly specified, the compiler infers the most restrictive capability that satisfies the usage. In actor behaviors, the default capability for parameters is `val` (immutable, sendable), ensuring that data sent between actors is safe by default.

```nulang
actor Processor {
  -- 'data' defaults to val, meaning it is immutable and sendable
  behavior process(data: String) {
    perform io.println("Processing: " ++ data)
  }
}
```

## 3.9 Capability-Qualified Types

A capability-qualified type combines a reference capability with a structural type. This combination determines both what operations are permitted on the reference and what type-level guarantees hold about the data.

Capability-qualified types are written with the capability as a prefix to the type expression:

```nulang
-- An iso reference to a mutable buffer
let buf: iso Buffer = Buffer.create(4096)

-- A val reference to an immutable tree
let tree: val Tree[Int] = build_tree()

-- A ref reference to a local record
let state: ref { count: Int, name: String } = { count = 0, name = "default" }
```

The compiler checks that all operations on a capability-qualified type are permitted. Attempting to write through a `val` reference or send a `ref` reference to another actor results in a compile-time error.

### 3.9.1 Capability Subtyping

Reference capabilities form a subtyping lattice. The key subtyping relationships are:

- `iso <: tag` — a unique reference can be viewed as an opaque identity
- `val <: box` — an immutable reference can be viewed as read-only
- `ref <: box` — a mutable reference can be viewed as read-only
- `trn <: val` — a transitioning reference can be frozen to immutable
- `trn <: box` — a transitioning reference can be viewed as read-only
- `iso <: trn` — a unique reference can downgrade to transitioning

These relationships enable safe capability transitions. For example, a function that accepts a `box` parameter can be called with either a `ref` or a `val` argument.

### 3.9.2 Recovering Capabilities

The `recover` expression creates an `iso` or `val` reference from an expression that only uses `iso`, `trn`, `val`, or `tag` references internally:

```nulang
let immutable_tree: val Tree[Int] = recover {
  let left = Tree.Leaf
  let right = Tree.Leaf
  Tree.Node { left = left, value = 42, right = right }
}
```
