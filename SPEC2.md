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

Nulang source files use the `.nula` extension. A source file is a sequence of Unicode code points encoded in UTF-8. The UTF-8 Byte Order Mark (U+FEFF) at the beginning of a file is recognized and ignored, though its use is discouraged.

A source file consists of a sequence of declarations: functions, type definitions, actor definitions, effect definitions, and module-level expressions. Declarations are separated by whitespace; there is no statement terminator. The parser uses an indentation-sensitive grammar where indentation determines block structure (see Section 2.8).

A minimal Nulang program is a single module file that need not contain a `main` function. Module-level expressions are evaluated in order when the program starts, and any spawned actors continue running:

```nulang
-- hello.nula: a minimal Nulang program
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

---

# Chapter 4: Effects

## 4.1 Effect System Overview

Nulang uses algebraic effects and handlers as the primary mechanism for defining, composing, and handling computational effects. Effects include IO, file system access, network communication, random number generation, time access, exceptions, state, and—uniquely—LLM inference. Every effectful operation in Nulang is expressed through this uniform mechanism.

The effect system has four key properties:

**Explicit effect tracking.** Every function's type includes an effect row that enumerates the effects it may perform. This makes dependencies on external systems visible in the type signature.

**Compositional handlers.** Effects are handled locally, not globally. A handler intercepts effect operations and defines their meaning in a specific context. Different handlers can provide different interpretations of the same effect.

**Resume-based semantics.** When an effect is performed, the current computation is suspended and a continuation (represented by a `resume` function) is passed to the handler. The handler may resume the computation zero, one, or multiple times.

**Type-safe effect polymorphism.** Higher-order functions can be polymorphic in their effect rows, enabling generic code that works with both pure and effectful functions.

## 4.2 Effect Declarations

Effects are declared with the `effect` keyword, followed by a name and a set of operations:

```nulang
effect Console {
  println(message: String): Unit
  read_line(): String
}

effect FileSystem {
  read(path: String): Result[String, String]
  write(path: String, content: String): Result[Unit, String]
  exists(path: String): Bool
}

effect Random {
  int(): Int
  float(): Float
}
```

Each operation has a name, parameter types, and a return type. Operations may themselves have effect rows, enabling effects that depend on other effects.

## 4.3 Performing Effects

The `perform` keyword invokes an effect operation:

```nulang
let greet_user = () -> [Console] Unit {
  perform Console.println("What is your name?")
  let name = perform Console.read_line()
  perform Console.println("Hello, " ++ name ++ "!")
}
```

The effect row `[Console]` in the function type indicates that `greet_user` may perform the `Console` effect. Without this annotation (or an equivalent inferred type), the `perform` expressions would be compile-time errors.

## 4.4 Effect Handlers

The `handle` expression installs an effect handler for a block of code:

```nulang
let result = handle {
  perform Console.println("Hello from handled code!")
  42
} with {
  | Console.println(msg) => {
      perform IO.println("[LOG] " ++ msg)
      resume(())
    }
  | Console.read_line() => {
      resume("test user")
    }
}
```

The handler pattern matches on effect operations. Each branch receives the operation's arguments and a `resume` function that continues the suspended computation. Calling `resume(v)` resumes with value `v` as the result of the `perform` expression.

### 4.4.1 Handler Return Value

The handler's return value becomes the value of the `handle` expression. When the handled block completes without performing any unhandled effects, its final value is returned.

```nulang
let sum = handle {
  let x = perform Random.int()
  let y = perform Random.int()
  x + y
} with {
  | Random.int() => resume(5)
}
-- sum == 10
```

### 4.4.2 Resume Semantics

The `resume` function captures the continuation of the performed effect. It can be called:

- **Once:** Standard linear resumption
- **Zero times:** Abort the computation (e.g., for exceptions)
- **Multiple times:** Non-deterministic or backtracking semantics

```nulang
-- Exception-like handler (resume called zero times)
let safe_divide = (a: Float, b: Float) -> [Console] Float {
  handle {
    if b == 0.0 then
      perform Error.raise("Division by zero")
    else
      a / b
  } with {
    | Error.raise(msg) => {
        perform Console.println("Error: " ++ msg)
        0.0  -- Return default value instead of resuming
      }
  }
}
```

## 4.5 Effect Rows

Effect rows describe the set of effects a function may perform. They support polymorphism and subtyping.

### 4.5.1 Closed Effect Rows

A closed effect row enumerates exactly the effects a function performs:

```nulang
let pure_function = (x: Int) -> [] Int { x + 1 }

let console_function = (msg: String) -> [Console] Unit {
  perform Console.println(msg)
}

let multi_effect = () -> [Console, FileSystem, Random] Unit {
  perform Console.println("Starting...")
  let content = perform FileSystem.read("data.txt")
  perform Console.println(content)
}
```

### 4.5.2 Open Effect Rows

An open effect row (ending with `...`) indicates polymorphism over effects:

```nulang
let map_option = [E, A, B] (opt: Option[A], f: (A) -> [E] B) -> [E] Option[B] {
  match opt {
    | None => None
    | Some(a) => Some(f(a))
  }
}
```

The function `map_option` preserves the effect row of its callback function `f`. If `f` is pure, `map_option` is pure. If `f` performs `Console` effects, so does `map_option`.

### 4.5.3 Effect Row Subtyping

A function with a smaller effect row can be used where a larger effect row is expected:

```nulang
let use_callback = [E] (f: () -> [E] Int) -> [Console, E] Int {
  perform Console.println("About to call callback...")
  let result = f()
  perform Console.println("Callback returned: " ++ Int.to_string(result))
  result
}

-- Pure callback works
let r1 = use_callback(() -> 42)

-- Effectful callback also works
let r2 = use_callback(() -> [Random] perform Random.int())
```

## 4.6 Built-in Effects

Nulang provides several built-in effects that are available without explicit declaration:

| Effect | Operations | Description |
|--------|-----------|-------------|
| `IO` | `println`, `print`, `read_line` | Console input/output |
| `FileSystem` | `read`, `write`, `exists`, `delete` | File system access |
| `Network` | `get`, `post`, `put`, `delete`, `request` | HTTP requests |
| `Random` | `int`, `float`, `bool` | Random number generation |
| `Time` | `now`, `sleep` | Time access and delays |
| `Error` | `raise` | Exception handling |
| `LLM` | `complete`, `embed` | Language model inference |
| `Metrics` | `counter`, `histogram`, `gauge` | Observability metrics |
| `Trace` | `span`, `annotate` | Distributed tracing |

## 4.7 Effect Inference

The compiler infers effect rows automatically. A function's effect row is the union of all effects performed in its body, plus the effects of any functions it calls.

```nulang
-- Effect row inferred as [Console]
let inferred = () -> {
  perform IO.println("Hello")
}

-- Effect row inferred as [FileSystem, Console]
let read_and_print = (path: String) -> {
  let content = perform FileSystem.read(path)
  match content {
    | Ok(data) => perform IO.println(data)
    | Error(e) => perform IO.println("Error: " ++ e)
  }
}
```

Effect annotations are required only for top-level function parameters and explicit type declarations.

## 4.8 Effect Elaboration

At compile time, the compiler transforms effectful code into efficient low-level code. This process, called *effect elaboration*, replaces `perform` and `handle` with direct function calls and continuation-passing style transformations. The result is zero-cost abstraction: effectful code runs as fast as equivalent callback-based code.

## 4.9 Effect Safety

The effect system guarantees several safety properties:

**Effect containment.** An effect performed inside a handler cannot escape the handler unless explicitly re-performed.

**Handler exhaustiveness.** Every effect performed in the handled block must be handled by a matching pattern. Unhandled effects result in a compile-time error.

**No implicit effects.** Pure functions (those with an empty effect row `[]`) cannot perform IO, access mutable state, or call LLMs. This is verified at compile time.

---

# Chapter 5: Capabilities

## 5.1 Capability System Overview

Nulang employs two complementary capability systems that together provide comprehensive security and safety guarantees:

1. **Reference capabilities** control how data can be read, written, and shared across actor boundaries. They are part of the type system and are checked at compile time.

2. **Authority capabilities** control what effects an actor can perform. They are declared on actors and checked both at compile time and runtime.

These systems work together: reference capabilities prevent data races, while authority capabilities prevent unauthorized access to external resources.

## 5.2 Reference Capabilities

Reference capabilities are described in detail in Section 3.8. They form a lattice of permissions:

```
iso (read+write, sendable)
  |
trn (read+write, local)
  |
ref (read+write, local)
  |
box (read-only, local)
  |
val (read-only, sendable)
  |
tag (no access, sendable)
```

The compiler uses these capabilities to guarantee:

**Memory safety.** No dangling references or use-after-free errors.

**Data-race freedom.** Mutable references cannot be shared between actors concurrently.

**Safe concurrency.** Only `iso` and `val` references can be sent between actors.

## 5.3 Authority Capabilities

Authority capabilities are declared on actors using the `capability` keyword:

```nulang
actor FileProcessor {
  capability file
  capability io

  behavior process(path: String) {
    let content = perform FileSystem.read(path)
    match content {
      | Ok(data) => perform IO.println("Read: " ++ data)
      | Error(e) => perform IO.println("Error: " ++ e)
    }
  }
}
```

Without the `capability file` declaration, the `perform FileSystem.read(...)` would be a compile-time error. This is authority-based security: the actor must explicitly declare what external resources it needs.

## 5.4 Capability Delegation

Authority capabilities can be delegated from one actor to another:

```nulang
actor Supervisor {
  capability llm
  capability http

  behavior spawn_worker(): Worker {
    -- Delegate capabilities to child actor
    spawn Worker with capabilities [llm, http]
  }
}

actor Worker {
  -- Worker inherits llm and http from parent
  capability llm
  capability http

  behavior do_work(query: String): String {
    perform llm.complete(query)
  }
}
```

Delegation creates a capability chain that can be audited. The runtime tracks which actor granted which capability to which other actor.

## 5.5 Capability Revocation

Capabilities can be revoked at any time:

```nulang
actor ResourceManager {
  capability network

  behavior revoke_access(worker: Worker) {
    revoke worker.network
  }
}
```

After revocation, any attempt by the worker to perform network operations results in a runtime error.

## 5.6 Capability Auditing

The runtime maintains a capability graph that records all capability grants and revocations. This graph can be queried for security auditing:

```nulang
-- Query which actors hold the llm capability
let llm_holders = perform audit.actors_with_capability(LLM)

-- Query the delegation chain for an actor
let chain = perform audit.capability_chain(worker_id)
```

## 5.7 Sendable Types

A type is *sendable* if it can be safely passed between actors. The sendable types are:

- Primitive types (`Bool`, `Int`, `Float`, `Decimal`, `Char`, `Unit`)
- `iso` reference types
- `val` reference types
- `tag` reference types
- Immutable collections of sendable types
- Actor references (as `tag`)

The compiler checks that all values sent between actors are sendable.

## 5.8 Capability Defaults

In the absence of explicit annotations, the compiler applies the following defaults:

- **Function parameters:** `box` (read-only view)
- **Local variables:** `ref` (mutable, local)
- **Actor behavior parameters:** `val` (immutable, sendable)
- **Return values:** Inferred from the expression

These defaults ensure that data sent between actors is safe by default.

---

# Chapter 6: Expressions

## 6.1 Expression Overview

Nulang is an expression-oriented language: every construct produces a value. There are no statements, only expressions that may or may not be used. The value of the last expression in a block is the value of the block.

## 6.2 Literals

Literal expressions produce constant values:

```nulang
42          -- Int literal
3.14        -- Float literal
"hello"     -- String literal
'a'         -- Char literal
true        -- Bool literal
()          -- Unit literal
```

## 6.3 Variables

Variable references look up the value bound to a name:

```nulang
let x = 42
x  -- evaluates to 42
```

## 6.4 Function Application

Function application applies a function to its arguments:

```nulang
add(1, 2)           -- Named function
String.length("hi") -- Method-style call on module
list |> map(f)      -- Pipe operator
```

## 6.5 Let Bindings

The `let` expression binds a name to a value:

```nulang
let x = 42
let y = x + 1
y  -- evaluates to 43
```

Let bindings are immutable by default. Mutable bindings use `var` (within a single actor):

```nulang
var counter = 0
counter = counter + 1
counter  -- evaluates to 1
```

## 6.6 Conditionals

The `if` expression chooses between two branches:

```nulang
if x > 0 then
  "positive"
else
  "non-positive"
```

Both branches must have the same type. The `else` branch is required.

## 6.7 Pattern Matching

The `match` expression performs pattern matching:

```nulang
match option {
  | Some(x) => x
  | None => 0
}
```

Patterns can be nested and include guards:

```nulang
match list {
  | [] => "empty"
  | [x] if x > 0 => "single positive"
  | [x, ..] => "starts with " ++ Int.to_string(x)
}
```

## 6.8 Lambda Expressions

Lambda expressions create anonymous functions:

```nulang
(x) -> x + 1
(x, y) -> x + y
() -> perform IO.println("hello")
```

The parameter types and effect row can be inferred or annotated explicitly.

## 6.9 Pipe Operator

The pipe operator `|>` passes the left-hand side as the last argument to the right-hand side:

```nulang
list |> map(f) |> filter(g) |> fold(h, 0)
-- Equivalent to: fold(h, 0, filter(g, map(f, list)))
```

## 6.10 Blocks

A block is a sequence of expressions enclosed in braces. The value of a block is the value of its last expression:

```nulang
let result = {
  let x = 1
  let y = 2
  x + y
}  -- result == 3
```

## 6.11 Error Handling

Nulang uses the `Result` and `Option` types for error handling:

```nulang
let safe_divide = (a: Float, b: Float) -> Result[Float, String] {
  if b == 0.0 then
    Error("Division by zero")
  else
    Ok(a / b)
}

-- Using match
match safe_divide(10.0, 2.0) {
  | Ok(result) => perform IO.println("Result: " ++ Float.to_string(result))
  | Error(msg) => perform IO.println("Error: " ++ msg)
}
```

## 6.12 Effect Handling

The `handle` expression installs an effect handler (see Chapter 4):

```nulang
let result = handle {
  perform Console.println("Hello!")
  42
} with {
  | Console.println(msg) => {
      perform IO.println("[LOG] " ++ msg)
      resume(())
    }
}
```

## 6.13 Recover Expressions

The `recover` expression creates an `iso` or `val` reference:

```nulang
let immutable = recover {
  { x = 1, y = 2 }
}
```

## 6.14 Actor Operations

Actor expressions create and interact with actors:

```nulang
let actor = spawn Counter  -- Create an actor
actor <- increment(1)      -- Send a message (fire-and-forget)
let result = ask(actor, get())  -- Request-response
```

---

# Chapter 7: Declarations

## 7.1 Declaration Overview

Declarations introduce names into the module scope. Nulang supports several kinds of declarations: value bindings, type definitions, actor definitions, effect definitions, and imports.

## 7.2 Value Bindings

Value bindings associate names with values or functions:

```nulang
let x = 42

let add = (a: Int, b: Int) -> Int {
  a + b
}

let factorial = (n: Int) -> Int {
  if n == 0 then 1 else n * factorial(n - 1)
}
```

Value bindings are immutable. They cannot be reassigned.

## 7.3 Type Definitions

Type definitions create new types:

```nulang
type Point = { x: Float, y: Float }

type Color = Red | Green | Blue

type Option[T] =
  | None
  | Some(T)

type Result[T, E] =
  | Ok(T)
  | Error(E)
```

## 7.4 Actor Definitions

Actor definitions declare actor types (see Chapter 8 for full details):

```nulang
actor Counter {
  state local count: Int = 0

  behavior increment() {
    count = count + 1
  }

  behavior get(): Int {
    count
  }
}
```

## 7.5 Effect Definitions

Effect definitions declare new effect types (see Chapter 4 for full details):

```nulang
effect Console {
  println(message: String): Unit
  read_line(): String
}
```

## 7.6 Imports

The `import` declaration brings names from other modules into scope:

```nulang
import List
import Map
import Http

import List exposing [map, filter, fold]
import Math as M
```

Imports are resolved at compile time and have no runtime cost.

## 7.7 Module Structure

A Nulang module is a file with the `.nula` extension. The module name is derived from the file name. A module exports all top-level declarations by default.

```nulang
-- math.nula
let pi = 3.14159

let circumference = (radius: Float) -> Float {
  2.0 * pi * radius
}

let area = (radius: Float) -> Float {
  pi * radius * radius
}
```

## 7.8 Generics in Declarations

Type parameters can be declared on functions and types:

```nulang
let map = [A, B] (list: List[A], f: (A) -> B) -> List[B] {
  match list {
    | [] => []
    | [x, ..xs] => [f(x), ..map(xs, f)]
  }
}

type Tree[T] =
  | Leaf
  | Node { left: Tree[T], value: T, right: Tree[T] }
```

## 7.9 Visibility

By default, all declarations are public. The `private` keyword restricts visibility to the current module:

```nulang
private let helper = (x: Int) -> Int { x * 2 }

public let exported = (x: Int) -> Int { helper(x) + 1 }
```

## 7.10 Documentation

Documentation comments use the `{-|` and `-}` delimiters:

```nulang
{-| Calculate the factorial of a non-negative integer.
    Returns 1 for n = 0, and n * factorial(n - 1) otherwise.
-}
let factorial = (n: Int) -> Int {
  if n == 0 then 1 else n * factorial(n - 1)
}
```

---

# Chapter 8: Actors

## 8.1 Actor Model Overview

Actors are the fundamental unit of concurrency and state in Nulang. An actor is an isolated computational entity with:

- A unique identity (actor ID)
- A mailbox for receiving messages
- Private state that cannot be accessed from outside
- A set of behaviors that process messages
- Optional supervision and lifecycle management

Actors communicate exclusively through asynchronous message passing. There is no shared mutable state between actors.

## 8.2 Actor Declaration

Actors are declared with the `actor` keyword:

```nulang
actor Counter {
  state local count: Int = 0

  behavior increment() {
    count = count + 1
  }

  behavior get(): Int {
    count
  }

  behavior reset() {
    count = 0
  }
}
```

## 8.3 State Declarations

State is declared with the `state` keyword:

```nulang
state local name: Type = initial_value
```

The state model (`local`, `durable`, `event_sourced`, or `crdt`) determines how the state is stored and recovered.

## 8.4 Behavior Declarations

Behaviors are declared with the `behavior` keyword:

```nulang
behavior name(parameters): ReturnType {
  -- body
}
```

Behaviors execute sequentially within an actor. Each behavior processes one message at a time.

## 8.5 Message Passing

Messages are sent using the `<-` operator:

```nulang
let counter = spawn Counter
counter <- increment()
counter <- increment()
```

Message sending is asynchronous and non-blocking.

## 8.6 Request-Response

The `ask` pattern sends a message and waits for a response:

```nulang
let counter = spawn Counter
counter <- increment()
let count = ask(counter, get())
```

## 8.7 Actor Lifecycle

Actors are created with `spawn`:

```nulang
let counter = spawn Counter
```

Actors can be stopped with `stop`:

```nulang
stop counter
```

## 8.8 Supervision

Actors can supervise other actors:

```nulang
actor Supervisor {
  behavior start_worker(): Worker {
    spawn Worker
  }

  behavior on_child_failed(child: ActorRef, error: String) {
    -- Restart the failed child
    let new_child = spawn Worker
    -- Log the failure
    perform IO.println("Worker failed: " ++ error ++ ", restarted.")
  }
}
```

## 8.9 Actor Types

Actor types can be parameterized:

```nulang
actor Buffer[T] {
  state local items: List[T] = []

  behavior push(item: T) {
    items = items ++ [item]
  }

  behavior pop(): Option[T] {
    match items {
      | [] => None
      | [x, ..xs] => {
          items = xs
          Some(x)
        }
    }
  }
}
```

## 8.10 Actor References

Actor references (`ActorRef`) are opaque identifiers that can be passed between actors:

```nulang
let counter = spawn Counter
let ref: ActorRef = counter
another_actor <- use_counter(ref)
```

---

# Chapter 9: Persistent Actors

## 9.1 Overview

Persistent actors survive process restarts through automatic checkpointing, event journaling, and deterministic replay. They are the foundation for durable execution in Nulang.

## 9.2 Declaring Persistent Actors

A persistent actor is declared with the `persistent` keyword:

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

## 9.3 State Models

Persistent actors support four state models:

| Model | Persistence | Replication | Recovery |
|-------|------------|-------------|----------|
| `local` | None | None | Reset to initial value |
| `durable` | Snapshot + journal | None | Replay from journal |
| `event_sourced` | Event journal | Event stream | Full event replay |
| `crdt` | Delta log | Automatic merge | CRDT merge |

## 9.4 Automatic Checkpointing

The runtime automatically checkpoints persistent actor state after each behavior invocation:

1. Behavior completes successfully
2. Runtime captures state snapshot
3. Snapshot is written to persistent storage
4. Journal entry is recorded for replay

## 9.5 Event Journaling

All state mutations are recorded in an event journal:

```nulang
persistent actor ShoppingCart {
  state durable items: List[Item] = []
  state event_sourced events: List[CartEvent] = []

  behavior add_item(item: Item) {
    items = items ++ [item]
    emit ItemAdded(item)
  }

  behavior remove_item(item_id: String) {
    items = List.filter(items, (i) -> i.id != item_id)
    emit ItemRemoved(item_id)
  }
}
```

## 9.6 Crash Recovery

Runtime recovery process:
1. Read last snapshot for each persistent actor
2. Replay events from journal from snapshot point forward
3. Restore `durable` state from snapshot
4. Reconstruct `event_sourced` state by applying replayed events
5. Merge `crdt` state from all available replicas

## 9.7 Deterministic Replay

- Same input sequence + same initial state = same output
- Enables testing and debugging of persistent actor execution
- Testing framework provides `snapshot` and `replay_from_start` helpers

## 9.8 Snapshotting and Compaction

- Snapshots capture state at a point in time
- Old snapshots and journal entries compacted periodically
- Configurable snapshot frequency and retention

## 9.9 Event Sourcing

- All state changes captured as immutable events
- Automatic event journal maintenance
- State derived by applying events

## 9.10 CRDT State

- Automatic replication across cluster nodes
- Built-in CRDT types with merge semantics
- Delta synchronization for efficient updates

---

# Chapter 10: Workflows

## 10.1 Workflows as Actor Graphs

- Workflows are long-running, durable compositions of actor interactions
- Special kind of persistent actor that orchestrates other actors
- Built from steps: individual units of durable work
- Survive crashes transparently; compensation for failures

## 10.2 Workflow Declaration

- Declared with `workflow` keyword
- Body contains steps, state declarations, event declarations
- Durable state automatically checkpointed

```nulang
workflow OrderFulfillment {
  state durable order_id: String = ""
  state durable status: OrderStatus = Received

  step receive_order(order: Order) {
    order_id = order.id
    status = Processing
    perform io.println("Processing order: " ++ order.id)
    Ok(order)
  }

  step validate_inventory(order: Order) {
    let available = perform inventory.check(order.items)
    if available then Ok(order) else Error("Out of stock")
  }

  step charge_payment(order: Order) {
    perform payment.charge(order.total, order.customer)
  }

  step ship_order(order: Order) {
    let tracking = perform shipping.create_label(order)
    status = Shipped
    Ok(tracking)
  }
}
```

## 10.3 Steps

- Named, durably executed blocks of code
- Checkpointed before and after execution
- Can declare compensation logic
- Idempotent by design

## 10.4 Sequential Execution

- Default execution mode for workflow steps
- Output of one step becomes input of the next
- Data flows through the workflow pipeline

## 10.5 Conditional Execution

- `if` within steps or `when` guards
- Conditional routing of workflow based on step output

## 10.6 Parallel Execution

- `parallel` keyword for concurrent step execution
- Waits for all branches before continuing
- Entire block fails if any branch fails

```nulang
workflow ParallelProcessing {
  step gather_data(requests: List[Request]): List[Response] {
    parallel for request in requests {
      perform http.get(request.url)
    }
  }

  step aggregate_results(responses: List[Response]): Report {
    perform Report.generate(responses)
  }
}
```

## 10.7 Loops and Iteration

- `for` loops over collections within workflow steps
- State accumulated across iterations
- Each iteration checkpointed for durability

## 10.8 Compensation and Sagas

- `compensate` keyword declares undo logic per step
- Saga pattern: automatically compensates on failure
- Compensation runs in reverse order of step execution

```nulang
workflow SagaTransaction {
  step reserve_inventory(order: Order): Reservation {
    let r = perform inventory.reserve(order.items)
    compensate { perform inventory.release(r) }
    r
  }

  step charge_payment(order: Order): PaymentId {
    let p = perform payment.charge(order.total)
    compensate { perform payment.refund(p) }
    p
  }

  step ship_goods(order: Order): TrackingId {
    perform shipping.dispatch(order)
  }
}
```

## 10.9 Human-in-the-Loop

- `await_human` construct pauses workflow for human input
- Configurable assignee, timeout, and default action
- Workflow state durably preserved during wait

```nulang
workflow ApprovalWorkflow {
  step submit_request(req: ApprovalRequest): ApprovedRequest {
    let approval = await_human approve(
      assignee = req.manager,
      timeout = Duration.hours(48),
      on_timeout = Reject
    )
    match approval {
      | Approved => { req .. approved = true }
      | Rejected => throw WorkflowError("Request rejected")
    }
  }
}
```

## 10.10 Time-Based Operations

- `sleep_until` for delayed action scheduling
- Time-based triggers and timeouts
- Workflow state preserved during sleep

## 10.11 Subworkflows

- Workflows can invoke other workflows
- Inherit parent's durability guarantees
- Participate in same compensation scope

## 10.12 Error Handling and Retry

- Automatic retry with configurable policies
- Exponential backoff, max attempts, transient error detection
- Integration with saga compensation for non-retryable failures

```nulang
workflow ResilientWorkflow {
  step fetch_with_retry(url: String): Response {
    retry {
      max_attempts: 3
      backoff: exponential(100, 2.0)
      retry_if: (error) -> is_transient(error)
    } {
      perform http.get(url)
    }
  }
}
```

---

# Chapter 11: AI Runtime

## 11.1 Overview

- Language-integrated AI, not external SDKs
- First-class constructs through algebraic effect system
- Governed by capability-based security model
- Handles model selection, prompt construction, response parsing, error recovery

## 11.2 Model Providers

- Unified interface for multiple providers
- OpenAI, Anthropic, Google, local models (Ollama/vLLM)
- Per-call provider selection

```nulang
config llm {
  provider = "openai"
  model = "gpt-4"
  temperature = 0.7
  max_tokens = 2048
}

let result = perform llm.complete(
  prompt = "Explain quantum computing",
  provider = Provider.Anthropic("claude-3-opus")
)
```

## 11.3 LLM Capability

- `llm` authority capability required on actor
- `perform llm.complete()`, `perform llm.embed()`, etc.
- Compile-time error without capability declaration

```nulang
actor ResearchAssistant {
  capability llm
  state local conversation: List[Message] = []

  behavior research(topic: String): Report {
    let prompt = build_research_prompt(topic, conversation)
    let response = perform llm.complete(
      system = "You are a research assistant. Provide structured reports.",
      user = prompt
    )
    conversation = conversation ++ [
      Message { role = User, content = prompt },
      Message { role = Assistant, content = response.text }
    ]
    parse_report(response.text)
  }
}
```

## 11.4 Tool System

- Nulang functions exposed as tools to LLMs
- Automatic description from type signatures and doc comments
- Structured tool call and result handling

```nulang
actor CalculatorAgent {
  capability llm

  tool calculate(expression: String): Float {
    perform math.evaluate(expression)
  }

  tool convert_currency(amount: Float, from: String, to: String): Float {
    perform forex.convert(amount, from, to)
  }

  behavior answer_query(query: String): String {
    let response = perform llm.tool_call(
      system = "Use the calculator and currency converter to help answer questions.",
      user = query,
      tools = [calculate, convert_currency]
    )
    response.text
  }
}
```

## 11.5 Memory (Short-term, Long-term, Event)

### 11.5.1 Short-term Memory
- Actor-local state for conversation context
- Persists for duration of interaction

### 11.5.2 Long-term Memory
- Vector embeddings for semantic retrieval
- Store and recall facts based on semantic similarity

### 11.5.3 Event Memory
- Immutable audit trail via event sourcing
- Complete history of agent actions

## 11.6 Planning and Delegation

- Structured planning via LLM
- Delegation to specialist sub-agents
- Tool selection and sequential execution

## 11.7 Observability

- Automatic logging and tracing of all LLM interactions
- Token usage tracking
- OpenTelemetry export support

```nulang
config llm.observability {
  log_prompts = true
  log_responses = true
  trace_tokens = true
  export_to = "opentelemetry"
}
```

---

# Chapter 12: Distributed Runtime

## 12.1 Overview

- Actor model extended across machine boundaries
- Location-transparent: same code on single node or cluster
- Virtual actors activated on any node
- Message routing handled by runtime
- CRDT convergence, fault containment and recovery

## 12.2 Clustering

- Nodes connected through mesh network
- Discovery via gossip protocol or seed list
- Configurable heartbeat and gossip parameters

```nulang
config cluster {
  node_id = "node-1"
  seed_nodes = ["node-1:8080", "node-2:8080", "node-3:8080"]
  heartbeat_interval = 1000
  gossip_fanout = 3
}
```

## 12.3 Node Lifecycle

- **Joining**: Connect to seeds, announce presence
- **Active**: Participate in routing and hosting
- **Leaving**: Drain actors, disconnect gracefully
- **Failed**: Detected via missed heartbeats

## 12.4 Message Routing

- Consistent hashing over actor IDs for routing
- Small fraction of actors relocated on membership change
- Programmer sends messages; runtime handles routing

```nulang
let cart = virtual ShoppingCart("user-123")
cart <- add_item(CartItem { product = "Book", quantity = 1 })
-- Runtime routes to whichever node hosts the actor
```

## 12.5 CRDT Replication

- CRDT state automatically replicated across hosting nodes
- Periodic delta synchronization
- Automatic conflict resolution via CRDT merge

```nulang
persistent actor GlobalCounter {
  state crdt count: GCounter = GCounter.empty()

  behavior increment() {
    count = count.increment(cluster.node_id())
  }

  behavior get(): Int {
    count.total()
  }
}
```

## 12.6 Fault Tolerance

- Actor migration on node failure
- Message buffering for failed-node actors
- CRDT healing on node rejoin
- Supervision-based recovery

```nulang
actor FaultTolerantService {
  behavior resilient_operation() {
    perform cluster.with_failover {
      let result = perform database.query("SELECT * FROM orders")
      Ok(result)
    } on_failure {
      | NetworkPartition => {
          perform time.sleep(Duration.seconds(5))
          resilient_operation()
        }
      | NodeFailure => {
          perform io.println("Node failed, retried on another node")
          Error("Redirected")
        }
    }
  }
}
```

## 12.7 Network Transport

- Binary protocol over TCP
- Compact binary serialization preserving type info
- Small messages sent inline; large messages streamed with backpressure
- Actor references sent as virtual IDs

---

# Chapter 13: WebAssembly Integration

## 13.1 Compilation Target

Nulang compiles to WebAssembly (Wasm), a portable, sandboxed bytecode format supported by all modern browsers and server-side runtimes. The compilation pipeline consists of five phases:

1. **Parsing**: Source code is parsed into an abstract syntax tree (AST).
2. **Type checking**: Hindley-Milner inference, reference capability analysis, and effect row verification.
3. **Effect elaboration**: Effect handler positions are determined and effect dispatch code is generated.
4. **Code generation**: The AST is lowered to WebAssembly instructions with the Nulang runtime embedded as a Wasm component.
5. **Linking**: External functions are linked through WIT (Wasm Interface Types) interfaces.

The WebAssembly target provides several advantages for Nulang: near-native execution speed, sandboxed security through capability-based WASI integration, cross-platform deployment without recompilation, and seamless composition with other Wasm modules written in any language.

The Nulang runtime is embedded within each compiled Wasm module. This runtime includes the actor scheduler, garbage collector, effect dispatch mechanism, and (for persistent actors) the durability layer. When a Nulang program starts, the `__nulang_start` export initializes the runtime before executing module-level code.

## 13.2 WIT Interface Generation

Nulang can both generate and consume WIT (Wasm Interface Types) interfaces for inter-language composition. The `@export` annotation exposes a Nulang function to other Wasm modules, while the `@import` annotation imports functions from other modules.

```nulang
-- Nulang function exposed to other Wasm modules
@export
let compute_hash: (String) -> String = (input) -> {
  perform crypto.sha256(input)
}

-- Import a function from another Wasm module
@import(module = "env", name = "external_api")
let external_api: (Int) -> String

-- Import an entire interface
@import_interface(module = "logging")
let logger: LoggerInterface
```

The compiler automatically generates WIT files from `@export` annotations and validates `@import` annotations against available WIT interfaces. Type mappings between Nulang types and WIT types are handled automatically: records map to WIT records, variants map to WIT variants, strings map to WIT strings, and arrays map to WIT lists.

## 13.3 WASI Worlds

Nulang programs run within WASI (WebAssembly System Interface) worlds that define the sandboxed system resources available to the program. The WASI world determines which system calls the program may perform, and Nulang's authority capabilities map directly to WASI capability grants.

```nulang
-- Configure the WASI world for this program
config wasi {
  world = "command"
  allowed_dirs = ["/tmp", "/data"]
  allowed_env = ["API_KEY", "DATABASE_URL"]
  allowed_network = ["api.example.com:443"]
}
```

| Nulang Capability | WASI Rights |
|-------------------|-------------|
| `file` | `path_open`, `fd_read`, `fd_write`, `fd_seek` |
| `network` | `sock_open`, `sock_connect`, `sock_send`, `sock_recv` |
| `random` | `random_get` |
| `time` | `clock_time_get`, `clock_time_set` |

When a Nulang program attempts to perform an effect that the WASI world does not permit, the effect handler raises an appropriate error rather than causing a runtime trap. This provides graceful degradation: a program that cannot access the filesystem receives a `FileSystem` error that can be handled by the programmer, rather than an opaque sandbox violation.

## 13.4 Cross-Language Composition

Nulang actors can spawn and communicate with actors implemented in other languages that compile to WebAssembly. This enables polyglot systems where each component is written in the most appropriate language while maintaining the uniform actor interface.

```nulang
-- Spawn an actor implemented in Rust
let rust_service = spawn @wasm("rust_service.wasm") DataProcessor

-- Send messages as if it were a Nulang actor
rust_service <- process(data)

-- Receive responses normally
let result = ask(rust_service, query(q))
```

The runtime marshals messages between Nulang's internal representation and the WIT interface types used by the foreign module. Actor references, promises, and exceptions are all translated correctly across language boundaries.

Nulang modules can also be used as libraries from other languages. A Nulang module compiled with `@export` annotations on its public functions can be imported and called from Rust, Go, Python, or JavaScript via standard Wasm component bindings.

## 13.5 Building Blocks

The Nulang runtime is organized into composable Wasm components that can be selectively included in the compiled output:

| Component | Description | Optional? |
|-----------|-------------|-----------|
| **Actor runtime** | Core scheduler, message passing, actor lifecycle | No |
| **GC** | Reference-counting + cycle-collecting garbage collector | No |
| **Effect handlers** | Default handlers for IO, FileSystem, Network, Time, Random | Yes (custom handlers can replace) |
| **Persistence layer** | Snapshotting, event journaling, recovery | Yes (only for `persistent` actors) |
| **AI connector** | LLM provider integration, embedding client | Yes (only for actors with `llm` capability) |
| **Cluster transport** | Inter-node message routing, gossip, CRDT sync | Yes (only for distributed deployments) |

Each component adds to the final Wasm module size. A simple single-threaded program with no actors compiles to a module of approximately 50KB. A full distributed persistent AI-augmented service compiles to approximately 2MB.

---

# Chapter 14: Standard Library

## 14.1 Core Module

The `Core` module provides the fundamental types, functions, and typeclasses that all Nulang programs depend on. It is automatically imported in every module and does not require an explicit `import` statement.

Key components of the Core module:

- **Identity and constant functions**: `identity`, `constant`, `flip`, `compose`
- **Comparison utilities**: `compare`, `min`, `max`, `clamp`
- **Result and Option utilities**: `Result.map`, `Result.flat_map`, `Option.get_or`, `Option.map`
- **Typeclasses**: `Eq`, `Ordered`, `Numeric`, `Show`, `Semigroup`, `Monoid`, `Functor`, `Applicative`, `Monad`

```nulang
-- Core functions are always available
let id = identity(42)                       -- 42
let c = constant(7)("hello")               -- 7
let cmp = compare(3, 5)                     -- LessThan
let bounded = clamp(0, 100, 150)            -- 100

-- Result/Option utilities
let mapped = Result.map(Ok(42), (x) -> x * 2)   -- Ok(84)
let fallback = Option.get_or(None, "default")    -- "default"
```

## 14.2 IO Module

The `IO` module provides the default handler for the `IO` effect, implementing console input and output operations.

```nulang
import IO

perform io.println("Hello, World!")
let name = perform io.read_line()
perform io.print("You entered: ")
perform io.println(name)

-- Formatted output
perform io.println("Value: {value}, Count: {count}")

-- File descriptors (via WASI)
perform io.write_to(1, "stdout message\n")   -- fd 1 = stdout
perform io.write_to(2, "error message\n")    -- fd 2 = stderr
```

The `IO` module also provides convenience functions for common patterns:

```nulang
perform io.print_lines(["Line 1", "Line 2", "Line 3"])
let lines = perform io.read_all_lines()
```

## 14.3 Collections

The standard library provides several persistent collection types with rich APIs:

### List

`List[T]` is a singly-linked list optimized for prepend and sequential access.

```nulang
import List

let numbers = [1, 2, 3, 4, 5]
let doubled = List.map(numbers, (x) -> x * 2)           -- [2, 4, 6, 8, 10]
let evens = List.filter(numbers, (x) -> x % 2 == 0)     -- [2, 4]
let total = List.fold(numbers, 0, (a, b) -> a + b)      -- 15
let has_three = List.contains(numbers, 3)               -- true
let length = List.length(numbers)                        -- 5
let reversed = List.reverse(numbers)                     -- [5, 4, 3, 2, 1]
let sorted = List.sort([3, 1, 4, 1, 5])                  -- [1, 1, 3, 4, 5]
let zipped = List.zip([1, 2, 3], ["a", "b", "c"])       -- [(1, "a"), (2, "b"), (3, "c")]
```

### Map

`Map[K, V]` is an immutable hash map with O(1) average-case lookup.

```nulang
import Map

let phonebook = Map.from_list([("Alice", "555-1234"), ("Bob", "555-5678")])
let alice_phone = Map.get(phonebook, "Alice")                  -- Some("555-1234")
let updated = Map.insert(phonebook, "Carol", "555-9012")
let without_bob = Map.delete(phonebook, "Bob")
let keys = Map.keys(phonebook)                                   -- ["Alice", "Bob"]
let merged = Map.merge(phonebook, additional_entries)
```

### Set

`Set[T]` is an immutable hash set.

```nulang
import Set

let tags = Set.from_list(["nulang", "programming", "actors"])
let has_tag = Set.contains(tags, "actors")                     -- true
let combined = Set.union(tags, Set.from_list(["wasm", "ai"]))
let common = Set.intersection(tags, Set.from_list(["actors", "rust"]))
let added = Set.insert(tags, "distributed")
```

## 14.4 String Processing

The `String` module provides comprehensive string manipulation functions.

```nulang
import String

let trimmed = String.trim("  hello  ")                          -- "hello"
let parts = String.split("a,b,c", ",")                         -- ["a", "b", "c"]
let replaced = String.replace("hello world", "world", "Nulang") -- "hello Nulang"
let prefixed = String.starts_with("nulang", "nu")              -- true
let suffixed = String.ends_with("nulang", "lang")              -- true
let lower = String.to_lowercase("HELLO")                       -- "hello"
let upper = String.to_uppercase("hello")                       -- "HELLO"
let substring = String.slice("nulang", 0, 2)                   -- "nu"
let joined = String.join(["a", "b", "c"], ",")                -- "a,b,c"
let parsed_int = String.parse_int("42")                        -- Some(42)
let parsed_float = String.parse_float("3.14")                  -- Some(3.14)
```

## 14.5 Concurrency Primitives

The `Concurrent` module provides actor-level concurrency utilities beyond basic message passing.

```nulang
import Concurrent

-- Spawn an actor
let worker = spawn Worker

-- Fire-and-forget message sending
worker <- do_work(data)

-- Request-response pattern
let promise = ask(worker, compute(data))
let result = await promise

-- Timeout on await
let result = await promise within Duration.seconds(5)

-- Select from multiple promises
let first = select {
  | promise_a => handle_a
  | promise_b => handle_b
  | timeout(Duration.seconds(10)) => handle_timeout
}

-- Parallel map using actors
let results = parallel_map(items, (item) -> {
  let worker = spawn Processor
  ask(worker, process(item))
})
```

## 14.6 HTTP Client/Server

The `Http` module provides both client and server functionality through the `Network` effect.

### HTTP Client

```nulang
import Http

-- Simple GET request
let response = perform http.get("https://api.example.com/users")

-- Request with headers
let response = perform http.request(
  method = Get,
  url = "https://api.example.com/users",
  headers = [("Authorization", "Bearer " ++ token)],
  body = None
)

-- POST with JSON body
let response = perform http.post(
  url = "https://api.example.com/users",
  headers = [("Content-Type", "application/json")],
  body = Some(Json.stringify(new_user))
)

-- Response handling
match response {
  | Ok(resp) => {
      perform io.println("Status: " ++ Int.to_string(resp.status))
      perform io.println("Body: " ++ resp.body)
    }
  | Error(e) => perform io.println("Request failed: " ++ e)
}
```

### HTTP Server

```nulang
actor HttpServer {
  capability http
  capability network

  behavior start(port: Int) {
    perform http.serve(port, (request) -> {
      match (request.method, request.path) {
        | (Get, "/health") =>
            Http.Response { status = 200, body = "OK", headers = [] }
        | (Get, "/users") =>
            let users = perform database.query("SELECT * FROM users")
            Http.Response {
              status = 200,
              body = Json.stringify(users),
              headers = [("Content-Type", "application/json")]
            }
        | _ =>
            Http.Response { status = 404, body = "Not Found", headers = [] }
      }
    })
  }
}
```

## 14.7 Serialization

The `Json` and `Binary` modules provide data serialization and deserialization.

### JSON

```nulang
import Json

-- Serialization
let user = { name = "Alice", age = 30, active = true }
let json = Json.stringify(user)
-- Result: "{\"name\":\"Alice\",\"age\":30,\"active\":true}"

-- Pretty printing
let pretty = Json.stringify_pretty(user)

-- Deserialization with type inference
let parsed: Result[{ name: String, age: Int, active: Bool }, String] =
  Json.parse(json)

-- Safe field access
let result = Json.parse(json)
match result {
  | Ok(user) => perform io.println("Hello, " ++ user.name)
  | Error(e) => perform io.println("Parse error: " ++ e)
}

-- Partial parsing
let name = Json.get_field(json, "name")           -- Some(Json.String("Alice"))
let age = Json.get_field_as(json, "age", Int)     -- Some(30)
```

### Binary Serialization

```nulang
import Binary

-- Serialize to binary format
let bytes = Binary.serialize(user)

-- Deserialize from binary
let restored: Result[User, String] = Binary.deserialize(bytes)

-- Schema evolution support
let compatible = Binary.deserialize_with_schema(bytes, UserSchema.v2)
```

---

# Chapter 15: Operational Model

## 15.1 Deployment

Nulang applications are deployed as WebAssembly modules with an embedded manifest that describes the application's requirements and configuration.

The deployment manifest (`nulang.toml`) specifies:

```nulang
-- nulang.toml: deployment manifest
[package]
name = "my-service"
version = "1.0.0"
description = "Example Nulang service"

[deployment]
type = "actor-service"
min_instances = 2
max_instances = 10
auto_scale = true

[capabilities]
llm = true
http = true
database = true
filesystem = false
network = true

[persistence]
enabled = true
storage_backend = "s3"       -- or "local", "postgres"
snapshot_interval = 1000
journal_retention = "30d"

[cluster]
enabled = true
replication_factor = 3

[ai]
provider = "openai"
model = "gpt-4"
fallback_model = "gpt-3.5-turbo"
```

Deployment targets include:

| Target | Description |
|--------|-------------|
| **Edge runtime** | Single-node execution on edge devices |
| **Server runtime** | Multi-process execution on servers |
| **Cluster runtime** | Distributed execution across a cluster |
| **Serverless** | Function-as-a-service with cold-start optimization |
| **Browser** | Client-side execution via WebAssembly |

## 15.2 Configuration

Configuration is provided through a hierarchy of sources, with later sources overriding earlier ones:

1. Default values from the configuration schema
2. `nulang.toml` deployment manifest
3. Environment variables (prefixed with `NULANG_`)
4. Command-line flags
5. Runtime API calls

```nulang
-- config.nula
config app {
  port = env("PORT", 8080)
  database_url = env("DATABASE_URL")
  log_level = env("LOG_LEVEL", "info")
  max_connections = env("MAX_CONNECTIONS", 100)
}

-- Accessing configuration values
let port = config.app.port
let db_url = config.app.database_url
```

Configuration values are typed and validated at startup. Invalid values produce clear error messages indicating the expected type and the actual value received.

## 15.3 Observability

Nulang provides built-in observability through three pillars: logging, metrics, and tracing.

### Structured Logging

```nulang
-- Log levels: debug, info, warn, error, fatal
perform io.log_debug("Processing item", { item_id = item.id, index = i })
perform io.log_info("Order completed", { order_id = order.id, total = order.total })
perform io.log_warn("High latency detected", { endpoint = "/api/users", latency_ms = 500 })
perform io.log_error("Payment failed", { order_id = order.id, reason = error_msg })
```

Logs are emitted in structured JSON format by default, with configurable output formats (JSON, pretty, syslog).

### Metrics

```nulang
-- Counter (monotonically increasing)
perform metrics.counter("orders.processed", 1)
perform metrics.counter("requests.total", 1, { method = "GET", path = "/users" })

-- Histogram (distribution of values)
perform metrics.histogram("request.duration_ms", elapsed)
perform metrics.histogram("response.size_bytes", body_length)

-- Gauge (point-in-time value)
perform metrics.gauge("active_connections", current_connections)

-- Timer (convenience for timing blocks)
let result = perform metrics.timer("database.query", {
  perform database.query(sql)
})
```

Metrics are exported via OpenTelemetry by default, with adapters for Prometheus, StatsD, and cloud monitoring services.

### Tracing

```nulang
-- Manual span creation
perform trace.span("process_order", {
  perform trace.annotate("order_id", order.id)
  let validated = perform trace.span("validate", { validate(order) })
  let charged = perform trace.span("charge", { charge_payment(order) })
  let shipped = perform trace.span("ship", { ship_order(order) })
  { validated = validated, charged = charged, shipped = shipped }
})

-- Automatic actor message tracing
config trace {
  auto_trace_actor_messages = true
  sample_rate = 0.1    -- Trace 10% of messages
}
```

## 15.4 Debugging

Nulang provides several debugging capabilities for development and production environments.

### Actor Inspection

```nulang
-- Query actor state and mailbox at runtime
perform debug.inspect(actor_id)
-- Returns: { state, mailbox_size, behaviors, supervisor }

-- Trace message flow
perform debug.trace_messages(actor_id, duration = Duration.minutes(1))

-- Snapshot actor state for offline analysis
let snapshot = perform debug.snapshot(actor_id)
```

### Deterministic Replay

Persistent actors support deterministic replay for debugging:

```nulang
-- Replay a persistent actor from the beginning
debug replay OrderProcessor {
  from = "start"
  breakpoints = ["create_order"]
}

-- Replay from a specific snapshot
debug replay OrderProcessor {
  from = Snapshot.at(10_000)
  speed = Realtime
}
```

### Interactive Shell

The Nulang interactive shell connects to a running system for exploration:

```nulang
$ nulang shell --target production-cluster
Connected to cluster (12 nodes, 4,832 actors)

> list actors --type ShoppingCart
actor://ShoppingCart/user-123   (node-3, 12 messages/sec)
actor://ShoppingCart/user-456   (node-7, 3 messages/sec)
actor://ShoppingCart/user-789   (node-1, 0 messages/sec)

> inspect actor://ShoppingCart/user-123
State: { items = [CartItem {...}, CartItem {...}], last_updated = 2025-01-15T10:30:00Z }
Mailbox: 2 pending messages
Behaviors: add_to_cart, remove_from_cart, get_cart, checkout

> send actor://ShoppingCart/user-123 get_cart()
Response: [CartItem { product = "Book", qty = 2 }, CartItem { product = "Pen", qty = 5 }]
```

## 15.5 Performance Tuning

Performance tuning options for Nulang applications:

### Scheduler Configuration

```nulang
config runtime.scheduler {
  thread_count = 8                    -- Number of scheduler threads
  work_stealing = true                -- Enable work stealing between threads
  affinity = true                     -- Pin threads to CPU cores
  spin_before_park = 100              -- Spin iterations before parking
}
```

### GC Tuning

```nulang
config runtime.gc {
  collection_interval = 1000          -- GC cycle interval in ms
  heap_size_hint = "1gb"             -- Target heap size
  nursery_size = "256mb"             -- Young generation size
  full_gc_threshold = 0.75           -- Heap usage threshold for full GC
}
```

### Network Tuning

```nulang
config runtime.network {
  tcp_nodelay = true                  -- Disable Nagle's algorithm
  buffer_size = 65536                 -- Network buffer size
  max_message_size = "16mb"          -- Maximum message size
  connection_pool_size = 100          -- Connection pool per node
}
```

## 15.6 Security

### Capability Audit

```nulang
-- List all actors and their capabilities
let audit = perform security.audit_capabilities()

-- Verify no actor has unexpected capabilities
let violations = List.filter(audit, (a) -> not is_authorized(a))
```

### Network Security

- All inter-node communication is encrypted via TLS
- Certificate pinning for cluster nodes
- Mutual TLS authentication

### Sandboxing

- WASI capabilities restrict system access
- Effect handlers can implement additional sandboxing
- Resource limits (CPU, memory, file descriptors) enforced by runtime

---

# Appendix A: Grammar Reference

## A.1 Grammar Notation

This appendix uses Extended Backus-Naur Form (EBNF) notation:

- `|` separates alternatives
- `"..."` surrounds literal strings
- `( ... )` groups elements
- `[ ... ]` denotes optional elements
- `{ ... }` denotes zero or more repetitions
- `{ ... }-` denotes one or more repetitions

## A.2 Top-Level Grammar

```
module        ::= { declaration }

declaration   ::= value_binding
                | type_definition
                | actor_definition
                | effect_definition
                | import_declaration
```

## A.3 Expression Grammar

```
expression    ::= literal
                | identifier
                | application
                | lambda
                | let_binding
                | conditional
                | match_expr
                | handle_expr
                | perform_expr
                | recover_expr
                | block

application   ::= expression "(" [ arguments ] ")"
                | expression "|>" expression

arguments     ::= expression { "," expression }

lambda        ::= "(" [ parameters ] ")" "->" [ effect_row ] expression

let_binding   ::= "let" pattern "=" expression
                | "var" identifier "=" expression

conditional   ::= "if" expression "then" expression "else" expression

match_expr    ::= "match" expression "{" { "|" pattern "=>" expression } "}"

handle_expr   ::= "handle" expression "with" "{" { handler_clause } "}"

perform_expr  ::= "perform" effect_operation

recover_expr  ::= "recover" expression

block         ::= "{" { expression } "}"
```

## A.4 Pattern Grammar

```
pattern       ::= identifier
                | "_"
                | literal
                | constructor_pattern
                | record_pattern
                | tuple_pattern

constructor_pattern ::= identifier [ "(" [ patterns ] ")" ]

record_pattern ::= "{" { identifier "=" pattern } "}"

tuple_pattern  ::= "(" [ patterns ] ")"

patterns      ::= pattern { "," pattern }
```

## A.5 Type Grammar

```
type          ::= primitive_type
                | type_constructor
                | function_type
                | tuple_type
                | record_type
                | capability_type

type_constructor ::= identifier [ "[" types "]" ]

function_type ::= "(" [ types ] ")" "->" [ effect_row ] type

tuple_type    ::= "(" [ types ] ")"

record_type   ::= "{" { identifier ":" type } "}"

capability_type ::= capability type

capability    ::= "iso" | "trn" | "ref" | "val" | "box" | "tag"

effect_row    ::= "[" [ effect_refs ] [ "..." ] "]"

effect_refs   ::= identifier { "," identifier }

types         ::= type { "," type }
```

## A.6 Actor Grammar

```
actor_definition ::= [ "persistent" ] "actor" identifier [ type_params ] "{" { actor_member } "}"

actor_member  ::= state_declaration
                | behavior_declaration
                | capability_declaration
                | value_binding

state_declaration ::= "state" state_model identifier ":" type "=" expression

state_model   ::= "local" | "durable" | "event_sourced" | "crdt"

behavior_declaration ::= "behavior" identifier "(" [ parameters ] ")" [ ":" type ] expression

capability_declaration ::= "capability" identifier
```

## A.7 Workflow Grammar

```
workflow_definition ::= "workflow" identifier "{" { workflow_member } "}"

workflow_member ::= step_declaration
                  | state_declaration
                  | event_declaration

step_declaration ::= "step" identifier "(" [ parameters ] ")" [ ":" type ] expression

event_declaration ::= "event" identifier "(" [ parameters ] ")"
```

---

# Appendix B: Built-in Types Reference

## B.1 Primitive Types

| Type | Size | Range/Description |
|------|------|-------------------|
| `Bool` | 1 byte | `true` or `false` |
| `Int` | 64 bits | -9,223,372,036,854,775,808 to 9,223,372,036,854,775,807 |
| `Float` | 64 bits | IEEE 754 double-precision |
| `Decimal` | Arbitrary | Arbitrary-precision decimal |
| `Char` | 32 bits | Single Unicode scalar value |
| `Unit` | 0 bytes | The unit value `()` |

## B.2 Collection Types

| Type | Description | Complexity |
|------|-------------|------------|
| `List[T]` | Immutable linked list | Prepend: O(1), Access: O(n) |
| `Array[T]` | Immutable array | Access: O(1), Append: O(n) |
| `Map[K, V]` | Immutable hash map | Lookup: O(1), Insert: O(n) |
| `Set[T]` | Immutable hash set | Contains: O(1), Insert: O(n) |

## B.3 Actor Types

| Type | Description |
|------|-------------|
| `ActorRef` | Opaque reference to an actor |
| `Promise[T]` | A future value from an async operation |
| `Mailbox[T]` | An actor's message queue |

## B.4 CRDT Types

| Type | Description | Merge Strategy |
|------|-------------|----------------|
| `GCounter` | Grow-only counter | Max of replica values |
| `PNCounter` | Positive-negative counter | Component-wise max |
| `GSet[T]` | Grow-only set | Union |
| `ORSet[T]` | Observed-remove set | Add-wins |
| `AWORSet[T]` | Add-wins observed-remove set | Add-wins |
| `LWWRegister[T]` | Last-write-wins register | Timestamp comparison |
| `MVRegister[T]` | Multi-value register | Concurrent values preserved |
| `RGA[T]` | Replicated growable array | Tombstone-based merge |

## B.5 Capability Types

| Capability | Read | Write | Sendable | Use Case |
|------------|------|-------|----------|----------|
| `iso` | Yes | Yes | Yes | Unique ownership |
| `trn` | Yes | Yes | No | Transitioning to val |
| `ref` | Yes | Yes | No | Local mutable reference |
| `val` | Yes | No | Yes | Immutable shared data |
| `box` | Yes | No | No | Read-only view |
| `tag` | No | No | Yes | Opaque identity |

---

# Appendix C: Effect Reference

## C.1 Console Effect

```
effect Console {
  println(message: String): Unit
  read_line(): String
  print(message: String): Unit
}
```

## C.2 FileSystem Effect

```
effect FileSystem {
  read(path: String): Result[String, String]
  write(path: String, content: String): Result[Unit, String]
  exists(path: String): Bool
  delete(path: String): Result[Unit, String]
  list_dir(path: String): Result[List[String], String]
}
```

## C.3 Network Effect

```
effect Network {
  get(url: String): Result[Response, String]
  post(url: String, body: String): Result[Response, String]
  put(url: String, body: String): Result[Response, String]
  delete(url: String): Result[Response, String]
  request(req: Request): Result[Response, String]
}
```

## C.4 Random Effect

```
effect Random {
  int(): Int
  float(): Float
  bool(): Bool
  int_range(min: Int, max: Int): Int
}
```

## C.5 Time Effect

```
effect Time {
  now(): Timestamp
  sleep(duration: Duration): Unit
}
```

## C.6 LLM Effect

```
effect LLM {
  complete(prompt: String, options: LLMOptions): LLMResponse
  embed(text: String): Embedding
  tool_call(prompt: String, tools: List[Tool]): ToolResult
}
```

## C.7 Metrics Effect

```
effect Metrics {
  counter(name: String, value: Int, tags: Map[String, String]): Unit
  histogram(name: String, value: Float, tags: Map[String, String]): Unit
  gauge(name: String, value: Float, tags: Map[String, String]): Unit
}
```

## C.8 Trace Effect

```
effect Trace {
  span(name: String, operation: () -> T): T
  annotate(key: String, value: String): Unit
}
```

---

# Appendix D: Migration Guide from v1 to v2

## D.1 Overview

This guide helps developers migrate Nulang 1.x code to Nulang 2.0. The key changes are:

1. **Actors replace agents.** The `agent` keyword is removed. Use `actor` with `capability llm` instead.
2. **Effects replace special syntax.** `perform llm.complete()` replaces `agent.llm.complete()`.
3. **Persistent actors replace separate storage.** Use `persistent actor` with `state durable` instead of external storage.
4. **Workflows are now actors.** The `workflow` keyword creates a special kind of persistent actor.
5. **State models are explicit.** Declare `local`, `durable`, `event_sourced`, or `crdt` for each state variable.

## D.2 Agent to Actor Migration

### Before (v1.x)

```nulang
agent ChatBot {
  memory short_term
  
  on message {
    let response = llm.complete(user_message)
    reply(response)
  }
}
```

### After (v2.0)

```nulang
actor ChatBot {
  capability llm
  
  state local history: List[Message] = []
  
  behavior chat(user_message: String): String {
    let response = perform llm.complete(user_message)
    history = history ++ [Message { role = User, content = user_message },
                           Message { role = Assistant, content = response }]
    response
  }
}
```

## D.3 Storage to Persistent Actor Migration

### Before (v1.x)

```nulang
actor VisitCounter {
  store visits: Int = 0
  
  behavior increment() {
    visits = visits + 1
  }
}
```

### After (v2.0)

```nulang
persistent actor VisitCounter {
  state durable visits: Int = 0
  
  behavior increment() {
    visits = visits + 1
  }
}
```

## D.4 Tool Registration Migration

### Before (v1.x)

```nulang
agent Calculator {
  tool add(a: Int, b: Int): Int {
    a + b
  }
  
  tool multiply(a: Int, b: Int): Int {
    a * b
  }
}
```

### After (v2.0)

```nulang
actor Calculator {
  capability llm
  
  tool add(a: Int, b: Int): Int {
    a + b
  }
  
  tool multiply(a: Int, b: Int): Int {
    a * b
  }
  
  behavior calculate(query: String): String {
    let response = perform llm.tool_call(
      user = query,
      tools = [add, multiply]
    )
    response.text
  }
}
```

## D.5 Cluster Configuration Migration

### Before (v1.x)

```nulang
config cluster {
  nodes = ["node1:8080", "node2:8080", "node3:8080"]
  replication = 3
}
```

### After (v2.0)

```nulang
config cluster {
  seed_nodes = ["node1:8080", "node2:8080", "node3:8080"]
  replication_factor = 3
}
```

## D.6 Summary of Keyword Changes

| v1.x Keyword | v2.0 Equivalent | Notes |
|-------------|-----------------|-------|
| `agent` | `actor` with `capability llm` | Agents are now actors with LLM capability |
| `memory` | `state local` | Use `state` with appropriate model |
| `store` | `state durable` | Durable state replaces external storage |
| `on message` | `behavior` | Behaviors replace message handlers |
| `reply` | Return value | Behaviors return values directly |
| `tool` | `tool` (unchanged) | Tool declaration syntax unchanged |
| `prompt` | `perform llm.complete()` | Effects replace direct LLM calls |
| `agent.llm` | `perform llm` | Effect syntax replaces agent method calls |

## D.7 Deprecation Timeline

| Version | Action |
|---------|--------|
| v1.6 | Deprecation warnings for v1 keywords |
| v1.7 | Migration tool provided (`nulang migrate`) |
| v1.8 | v1 keywords deprecated, opt-out via flag |
| v1.9 | v1 keywords removed |
| v2.0 | Only v2 syntax supported |

## D.8 Migration Tool

Nulang 2.0 includes a migration tool that automatically converts v1.x code:

```bash
$ nulang migrate --input src_v1 --output src_v2
Migrating 42 files...
- Converted 5 agents to actors
- Converted 8 store declarations to durable state
- Converted 12 message handlers to behaviors
- Generated 0 manual review items

Migration complete. Review src_v2 before committing.
```

The migration tool handles the common cases. Complex migrations may require manual review.

---

This specification is a living document. As Nulang evolves, new features, refinements, and clarifications will be added. Feedback from implementers and users is essential to ensuring that Nulang 2.0 realizes its design goals.

---

*End of Nulang Language Specification v2.0*