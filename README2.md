# Nulang

> **Erlang's fault tolerance + Temporal's durability + LangGraph's agent orchestration — in one language with one mental model.**

[![Rust](https://img.shields.io/badge/built%20with-Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![Docs](https://img.shields.io/badge/docs-nulang.dev-green)](https://nulang.dev/docs)
[![Discord](https://img.shields.io/discord/1234567890?label=discord&logo=discord&color=7289da)](https://discord.gg/nulang)

---

## What is Nulang?

**Today, building a distributed AI agent requires stitching together 5+ tools.** Temporal for durability, LangGraph for orchestration, Akka for actors, a vector DB for memory, Kubernetes for deployment. Each has a different mental model, different failure modes, and different operational requirements. You spend more time plumbing than building.

**Nulang replaces all of them with one unified actor model.**

Actors in Nulang are simultaneously services, AI agents, workflows, and durable objects. A single `persistent actor` declaration gives you state persistence, crash recovery, message ordering, and location transparency — no frameworks, no YAML, no distributed systems expertise required. The runtime handles replication, checkpointing, and failover automatically.

Nulang compiles to WebAssembly components, so your actors interoperate with any language (Rust, Go, Python, JavaScript) and run anywhere — edge, cloud, or embedded. The effect system makes side effects typed, traced, and mockable, giving you deterministic replay for debugging and testing. Capability security provides fine-grained authority control at the language level.

**The vision**: We believe the future of software is billions of cooperating, autonomous agents — some AI-driven, some purely logical, all durable and secure by default. Nulang is the language for that future.

---

## Why Nulang?

| What you need today | What you need with Nulang |
|---|---|
| Temporal for durable workflows | `persistent actor` — durability is a keyword |
| LangGraph for agent orchestration | `behavior` + `perform llm.complete()` — agents are just actors |
| Akka / Orleans for distributed actors | Built-in actor runtime with location transparency |
| Vector DB for agent memory | `state` is queryable, persistent, and versioned |
| Kubernetes for deployment | Compile to WASM, run on any host (no containers needed) |
| Custom auth + mTLS | `capability` declarations enforced by the runtime |
| Separate testing frameworks | Effect system gives you deterministic replay and mocks |

**The mental model is simple**: Everything is an actor. Some actors talk to LLMs, some talk to databases, some talk to each other. All of them are durable, all of them are secure, all of them compose the same way.

---

## Quick Example

A durable AI agent workflow that survives crashes and resumes exactly where it left off:

```nulang
// A durable AI agent workflow — survives crashes, resumes automatically
workflow CustomerSupport {
  step ReceiveTicket
  step ClassifyPriority
  step ResolveOrEscalate
  step NotifyCustomer
}

persistent actor SupportAgent {
  capability llm
  capability database

  behavior handle_ticket(ticket: Ticket) =
    let context = perform database.lookup(ticket.customer_id)
    let analysis = perform llm.complete(
      "Analyze this support ticket: " <> ticket.body
      <> "\nCustomer context: " <> context
    )
    
    match analysis.priority with
    | "high" =>
        self ! escalate(ticket, analysis)
    | _ =>
        let response = perform llm.complete(
          "Draft a response to: " <> ticket.body
        )
        perform database.save_response(ticket.id, response)
        perform email.send(ticket.customer_email, response)
```

And a simple durable counter — state is persisted automatically, survives restarts:

```nulang
// A counter that survives restarts — state is persisted automatically
persistent actor VisitCounter {
  state count: Int = 0

  behavior increment() =
    count = count + 1
    count
}
```

Spawn it, crash it, restart it — `count` resumes from its last value. No database code. No ORM. No state management framework.

---

## Features

| Feature | Description |
|---|---|
| **Durable Actors** | State persists, actors survive crashes, resume exactly where they left off |
| **Agent-Native** | LLM calls are typed effects — no separate framework or DSL needed |
| **WASM Components** | Compiles to WebAssembly for universal interop with Rust, Go, Python, JS |
| **4 State Models** | Choose `local`, `durable`, `event_sourced`, or `crdt` per actor |
| **Effect System** | Side effects are typed, traced, and mockable — deterministic replay built in |
| **Capability Security** | Fine-grained authority control enforced by the runtime |
| **Workflow Engine** | Multi-step workflows with retries, timeouts, and sagas — declarative |
| **Location Transparent** | Send messages to actors anywhere — local, remote, or edge |
| **Pattern Matching** | Expressive destructuring for messages, enums, and structured data |
| **Type Inference** | Static types with minimal annotation — safety without verbosity |

---

## Installation

```bash
# Install the Nulang compiler and runtime
cargo install nulang

# Verify installation
nulang --version
```

Requires Rust 1.80+ and [Wasmtime](https://wasmtime.dev/) for local execution.

---

## Getting Started

### 1. Create a project

```bash
nulang new hello-world
cd hello-world
```

### 2. Write your first actor

```nulang
// src/hello.nu
persistent actor Greeter {
  state greetings: Int = 0

  behavior greet(name: String) =
    greetings = greetings + 1
    "Hello, " <> name <> "! (greeting #" <> to_string(greetings) <> ")"
}
```

### 3. Run it

```bash
nulang run
# => Hello, World! (greeting #1)
```

Kill the process, run again — the count continues. Read the [Getting Started Guide](https://nulang.dev/docs/getting-started) to build your first agent.

---

## Architecture Overview

Nulang compiles to **WebAssembly components** and runs on a **distributed actor runtime** built in Rust. The runtime handles message routing, state persistence via an embedded event store, crash recovery through deterministic replay, and capability-based sandboxing for security.

Actors communicate asynchronously via typed mailboxes. The scheduler provides per-actor concurrency — millions of actors run on a single node, transparently distributed across a cluster. State models (`local`, `durable`, `event_sourced`, `crud`) are selected at declaration time and enforced by the runtime.

For the full architecture deep-dive, see [ARCHITECTURE.md](https://nulang.dev/docs/architecture).

---

## Documentation

| Resource | Link | Description |
|---|---|---|
| **Language Guide** | [nulang.dev/docs/guide](https://nulang.dev/docs/guide) | Syntax, types, actors, and effects |
| **Agent Building** | [nulang.dev/docs/agents](https://nulang.dev/docs/agents) | Building AI agents with LLM effects |
| **API Reference** | [nulang.dev/docs/api](https://nulang.dev/docs/api) | Standard library and runtime API |
| **Examples** | [github.com/nulang/examples](https://github.com/nulang/examples) | Complete example projects |
| **Contributing** | [CONTRIBUTING.md](./CONTRIBUTING.md) | How to contribute to Nulang |
| **Architecture** | [ARCHITECTURE.md](https://nulang.dev/docs/architecture) | Runtime and compiler design |

---

## Status & Roadmap

**Status**: Alpha — core language and runtime are functional. APIs may change.

- ✅ Actor runtime with durable state
- ✅ Effect system with typed LLM calls
- ✅ WASM component compilation
- ✅ Pattern matching and type inference
- ✅ Capability-based security
- 🔄 Event sourcing and CRDT state models
- 🔄 Distributed clustering
- 📋 Web IDE and REPL
- 📋 Visual workflow designer

See the full [roadmap](https://nulang.dev/roadmap) for details.

---

## License

Copyright 2026 © David Porkka

Nulang is licensed under the [Apache License, Version 2.0](https://opensource.org/licenses/Apache-2.0).
