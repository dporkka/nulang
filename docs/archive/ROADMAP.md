# Nulang 5-Year and 10-Year Roadmap

**Document Version:** 1.0
**Date:** January 2025
**Audience:** Core team, contributors, investors, and early adopters
**Status:** Active planning document

---

## Table of Contents

1. [Vision Statement](#1-vision-statement)
2. [Development Phases Overview](#2-development-phases-overview)
3. [Year 1: Foundation (v0.7-v1.0-alpha)](#3-year-1-foundation-v07-v10-alpha)
4. [Year 2: Production (v1.0-v1.3)](#4-year-2-production-v10-v13)
5. [Year 3: Scale (v1.4-v1.6)](#5-year-3-scale-v14-v16)
6. [Year 4: Platform (v1.7-v1.9)](#6-year-4-platform-v17-v19)
7. [Year 5: Ecosystem (v2.0)](#7-year-5-ecosystem-v20)
8. [10-Year Vision (Years 6-10)](#8-10-year-vision-years-6-10)
9. [Risk Analysis](#9-risk-analysis)
10. [Success Metrics](#10-success-metrics)

---

## 1. Vision Statement

In ten years, Nulang will be the default programming language for building distributed, durable, AI-powered systems. It will replace the modern backend stack -- microservices, job queues, workflow engines, state machines, and AI agent frameworks -- with a single, coherent actor-based language and runtime. A developer will define actors and their protocols, and the runtime will handle durability, distribution, scaling, security, and recovery automatically. AI agents will be first-class citizens of the same runtime as databases and HTTP services, not bolted-on frameworks. Nulang will power everything from edge devices to multi-region cloud deployments, with the same source code compiling to WASM components that run anywhere. The combination of virtual actors, built-in durability, capability-based security, and the WASM component model will make Nulang the Erlang of the AI era -- the language you reach for when failure is not an option, distribution is not a choice, and AI integration is not a novelty.

---

## 2. Development Phases Overview

| Phase | Name | Version | Duration | Key Deliverables | Status |
|-------|------|---------|----------|-------------------|--------|
| 1 | Foundation | v0.7 | Q1 Year 1 | Durable execution core, `persistent` keyword, local + durable state models, checkpointing, event journal | Planned |
| 2 | Workflows | v0.8 | Q2 Year 1 | Event sourcing, `workflow` keyword, workflow compilation to actor graphs, sagas | Planned |
| 3 | AI Runtime | v0.9 | Q3 Year 1 | LLM capability, typed tool system, actor memory, planning/delegation | Planned |
| 4 | Developer Tooling | v1.0-alpha | Q4 Year 1 | LSP, formatter, package manager with WIT, VS Code extension | Planned |
| 5 | Stable Release | v1.0 | Q1-Q2 Year 2 | WASM component compilation (wasm32-wasip2), complete stdlib, test framework with replay testing, documentation generator | Planned |
| 6 | Advanced Distribution | v1.1 | Q3 Year 2 | Cluster sharding, virtual actor placement strategies, cross-region replication | Planned |
| 7 | Cloud Platform | v1.2 | Q4 Year 2 | Cloud deployment CLI, Kubernetes operator, auto-scaling, blue-green deployment | Planned |
| 8 | Ecosystem | v1.3 | Q1 Year 3 | OpenAPI/gRPC bindings, PostgreSQL adapter, Kafka/NATS connector, S3-compatible storage | Planned |
| 9 | Scale | v1.4-v1.6 | Q2-Q4 Year 3 | Hot code reloading, advanced workflow features (sagas, human-in-the-loop), workflow visualizer, distributed debugger, actor inspector | Planned |
| 10 | Platform | v1.7-v1.9 | Year 4 | Managed cloud offering (Nulang Cloud), multi-tenant hosting, usage-based billing, marketplace, enterprise features | Planned |
| 11 | Maturity | v2.0 | Year 5 | Complete feature set, 1000+ packages, multi-language SDKs, industry recognition | Planned |
| 12 | Universal Network | v3.0+ | Years 6-10 | AI-native development, universal actor interoperability, self-healing systems, industry standard | Vision |

---

## 3. Year 1: Foundation (v0.7-v1.0-alpha)

**Theme:** Make actors durable, make workflows possible, make AI native, make developers productive.

### Q1 (v0.7): Durable Execution Core

**Remove AI Agent DSL**

The current v0.6 implementation has a separate AI agent DSL with `agent`, `tool`, `prompt`, and `memory` keywords. This quarter removes all of them. Agents become regular actors that hold the `LLM` capability.

- Remove `agent`, `tool`, `prompt`, `memory` keywords from lexer, parser, and AST
- Remove agent-specific AST nodes (`AgentDecl`, `ToolBinding`, `PromptDef`, `MemoryDef`)
- Remove agent compilation pipeline in the compiler
- Convert the existing agent runtime to a generic capability-based runtime
- Provide a migration script that converts `.agent` files to `.nul` actor definitions

**Implement the `persistent` Keyword**

- Add `persistent` as a modifier keyword for actor declarations
- Add `local`, `durable`, `event_sourced`, and `crdt` as state model specifiers
- Update the parser to accept: `persistent actor Foo { ... }`, `persistent durable actor Foo { ... }`, `persistent event_sourced actor Foo { ... }`, `persistent crdt actor Foo { ... }`
- Update the type checker to treat state model as part of the actor type
- Update the compiler to emit state model metadata in the bytecode module header

**Implement Local + Durable State Models**

- **Local state model**: Actors with no persistence. State lives in linear memory and is lost on crash. This is the default for non-`persistent` actors. Zero overhead.
- **Durable state model**: Automatic checkpointing. Implementation:
  - State persistence engine with pluggable backends: SQLite (development), PostgreSQL (production single-node)
  - On each message boundary, capture the actor's entire linear memory and serialize it to the configured backend
  - Configurable checkpoint policy: every N messages (default: 1), every T seconds, or when memory exceeds M MB
  - Batched checkpoint writes across actors to amortize storage costs
  - Target: <5ms p99 checkpoint latency

**Milestone**: A `Counter` actor created, sent 1,000 `increment` messages, node killed with `kill -9`, restarts, actor resumes with `count == 1000`.

### Q2 (v0.8): Event Sourcing + Workflows

**Implement Event-Sourced State Model**

- Full event sourcing: actor state is computed by folding a pure projection function over the event journal
- Add `emit` keyword for emitting events within behavior handlers
- Add `projection` keyword for defining state projection functions from events
- Deterministic replay: all effect results captured in events
- Compile-time enforcement through the effect system

**Implement `workflow` Keyword + Basic Syntax**

- Add `workflow`, `step`, `parallel`, `compensate`, `await`, `subworkflow` keywords
- Workflow parser, type checker, and compiler (transforms to actor graph)
- Sequential + conditional workflow steps, parallel execution, error handling with retry

**Milestone**: A `PurchaseOrder` workflow executes end-to-end with persistence, survives node restart mid-workflow, and resumes exactly where it left off.

### Q3 (v0.9): AI Runtime

**LLM Capability + Model Provider Abstraction**

- Define the `LLM` capability type
- Provider backends: OpenAI, Anthropic, Azure OpenAI, Ollama, vLLM
- Cost tracking: per-actor, per-workflow token usage and cost aggregation

**Typed Tool System**

- Any actor behavior can be exposed as a tool to LLMs
- Automatic tool schema generation from behavior type signatures
- Tool calls are effects: traced, mockable in tests

**Actor Memory**

- Short-term: conversation buffer with auto-truncation
- Long-term: Vector store integration (Qdrant, pgvector)
- Event memory: entire message history for event_sourced actors

**Milestone**: AI agent workflow that researches a topic, uses tools, stores facts in long-term memory, synthesizes a report, and persists all state across restarts.

### Q4 (v1.0-alpha): Developer Tooling

- LSP Server (type checking on every keystroke, auto-completion, go-to-definition, rename)
- Formatter (deterministic, zero config, gofmt-style)
- Package manager with WIT support (`nulang.toml` manifest)
- VS Code extension

**Milestone**: Developer can write, format, check types, and run Nulang in VS Code with full IDE support.

---

## 4. Year 2: Production (v1.0-v1.3)

### Q1-Q2 (v1.0): Stable Release

- WASM component compilation (wasm32-wasip2) — compile actors to WASM core modules
- Complete standard library (core, io, net, time, json, crypto, uuid, decimal, regex)
- Full test framework with replay testing
- Documentation generator

**Milestone**: First production deployment by external team.

### Q3 (v1.1): Advanced Distribution

- Cluster sharding (consistent hashing on actor ID)
- Virtual actor placement strategies (local, least_loaded, affinity, geo)
- Cross-region replication (active-passive and active-active for CRDTs)

**Milestone**: 10-node cluster running 100,000+ actors.

### Q4 (v1.2): Cloud Platform

- Cloud deployment CLI (`nulang deploy`, `nulang scale`, `nulang rollback`)
- Kubernetes operator with CRD for Nulang realms
- Auto-scaling (mailbox depth, CPU, memory)
- Blue-green deployment at actor level

**Milestone**: `nulang deploy` deploys to Kubernetes cluster in <2 minutes.

### Q1 Year 3 (v1.3): Ecosystem

- OpenAPI/gRPC bindings from actor protocols
- PostgreSQL adapter with connection pooling
- Kafka/NATS connector
- S3-compatible storage

**Milestone**: Can build a complete backend service in Nulang.

---

## 5. Year 3: Scale (v1.4-v1.6)

### v1.4: Hot Code Reloading

- Deploy new code without stopping the system
- Actor finishes current message, checkpoints, deactivates
- New WASM module swapped in, actor reactivates with migrated state
- Inspired by Erlang's `code_change` mechanism

### v1.5: Advanced Workflow Features

- Saga compensation with reverse-order execution
- Human-in-the-loop web UI with approval delegation
- Workflow templates (approval chains, ETL pipelines, onboarding)
- Workflow visualizer UI (drag-and-drop designer)

### v1.6: Distributed Debugger + Actor Inspector

- Attach to any actor anywhere in the cluster
- Step through behavior handlers, inspect state, set breakpoints
- Time-travel debugging for event-sourced actors
- Actor topology map, message flow visualization, health dashboard

**Milestone**: 100+ production deployments.

---

## 6. Year 4: Platform (v1.7-v1.9)

### v1.7: Nulang Cloud (Managed Offering)

- `cloud.nulang.io` — sign up, create a realm, deploy
- Zero-infrastructure deployment
- Global regions (us-east, us-west, eu-west, eu-central, ap-south, ap-northeast)
- Managed backends (PostgreSQL, Redis, Kafka, S3)
- Free / Pro / Enterprise tiers

### v1.8: Multi-Tenant Hosting + Marketplace

- Multi-tenant hosting with strong isolation per realm
- Usage-based billing (per message, per GB stored, per LLM token)
- Package marketplace (`marketplace.nulang.io`)

### v1.9: Enterprise Features

- RBAC with granular permissions per realm
- Audit logs for all actor lifecycle events
- SOC 2 Type II, GDPR compliance tools
- SAML and OIDC SSO integration

**Milestone**: Nulang Cloud processes 1B+ actor messages/month.

---

## 7. Year 5: Ecosystem (v2.0)

### v2.0 Release

All planned features complete: virtual actors, four state models, workflows, AI runtime, WASM compilation, capability networking, cloud deployment, complete developer tooling, hot code reloading, distributed debugger, multi-region replication, enterprise features.

### Mature Ecosystem

- 1,000+ packages in the registry
- Multi-language SDKs (Python, JavaScript/TypeScript, Go)
- Industry recognition: case studies, conference presentations, published book
- Community: 5,000+ Discord members, 50+ regular contributors
- GitHub: 10,000+ stars, 100+ external contributors

**Milestone**: Used by 100+ companies in production.

---

## 8. 10-Year Vision (Years 6-10)

### AI-Native Development (Years 6-7)

- AI agents write and deploy Nulang code autonomously
- Self-improving systems: actors monitor performance and generate optimized versions
- Natural language to workflow: describe a business process in English, get a running workflow
- Automated testing: AI generates property tests from production traffic patterns

### Universal Actor Network (Years 7-8)

- Cross-platform actors: Nulang actors communicate with Rust, Go, Python via WIT
- Edge-to-cloud continuum: same code runs on Raspberry Pi, smartphone, CDN, cluster, cloud
- Federated clusters: multiple independent Nulang clusters form a federation

### Self-Healing Systems (Years 8-9)

- Predictive scaling via ML models analyzing traffic patterns
- Automatic placement optimization without human intervention
- Built-in chaos engineering that randomly injects failures
- Supervisor trees that auto-adjust restart strategies

### Industry Standard (Years 9-10)

- Taught in 50+ university programs
- Nulang Foundation with $10M+ annual budget
- 10,000+ packages, 50,000+ developers, 1,000+ production companies
- Nulang Cloud: $100M+ ARR

---

## 9. Risk Analysis

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Checkpoint performance unacceptable | Medium | High | Count-based checkpointing; memory-mapped files; benchmark early |
| WASM compilation overhead too high | Medium | High | Keep native path; target <20% overhead; AOT compilation |
| Distributed state consistency bugs | Medium | Critical | Property-based testing; Jepsen-style testing; formal CRDT verification |
| LLM integration API churn | High | Medium | Abstract behind WIT; support multiple providers |
| Low adoption vs established languages | Medium | Critical | Ship concrete value early; target AI agent niche; build in public |
| Competition from cloud providers | High | Medium | Open-source core; WASM portability; avoid lock-in |

---

## 10. Success Metrics

| Metric | Year 1 | Year 2 | Year 3 | Year 4 | Year 5 | Year 10 |
|--------|--------|--------|--------|--------|--------|---------|
| GitHub stars | 1,000 | 3,000 | 5,000 | 7,000 | 10,000 | 30,000+ |
| Production deployments | 0 | 5 | 100 | 300 | 100+ | 1,000+ |
| Registry packages | 0 | 50 | 300 | 600 | 1,000+ | 10,000+ |
| Contributors | 10 | 30 | 75 | 100 | 150+ | 500+ |
| Discord members | 500 | 2,000 | 3,500 | 5,000 | 10,000 | 30,000+ |
| Cloud ARR | N/A | N/A | N/A | $500K | $5M | $100M+ |
