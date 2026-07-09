# Nulang 50-Year Architecture Review — Diagrams

> **Status:** Central diagram reference for the architecture review.  
> **Date:** 2026-07-06

---

## 1. Compiler Architecture

This diagram shows the proposed pipeline from multiple frontends through Intent IR, AST, HIR, MIR, optimizer, and backends.

```mermaid
flowchart TB
    subgraph Inputs
        I1[.nula source]
        I2[Natural language]
        I3[Visual blocks]
        I4[JSON API]
        I5[Voice / IDE]
    end

    IR[Intent IR]
    AST[AST<br/>src/ast.rs]
    HIR[HIR<br/>typed + resolved]
    MIR[MIR<br/>closures / refs / linearity]
    OPT[Optimizer]
    BYTE[Bytecode VM<br/>src/vm.rs]
    JIT[Cranelift JIT<br/>src/jit]
    LLVM[LLVM AOT]

    V1[Parser]
    V2[Typecheck]
    V3[Effect check]
    V4[Capability check]
    V5[Linearity check]
    V6[Tests]

    I1 --> AST
    I2 --> IR
    I3 --> IR
    I4 --> IR
    I5 --> IR
    IR --> AST
    AST --> V1 --> HIR
    HIR --> V2 --> V3 --> V4 --> V5 --> MIR
    MIR --> OPT
    OPT --> BYTE
    OPT --> JIT
    OPT --> LLVM
    BYTE --> V6
    JIT --> V6
    LLVM --> V6
```

---

## 2. Runtime Architecture

This diagram shows the actor runtime, memory management, distributed runtime, and cloud/workflow targets.

```mermaid
flowchart TB
    subgraph Frontend ["Compiler Pipeline (not runtime)"]
        L[Lexer] --> P[Parser]
        P --> TC[Type/Effect/Cap Checker]
        TC --> C[Compiler]
        C --> VM[Register VM + JIT]
    end

    subgraph Runtime ["Actor Runtime (src/runtime/mod.rs)"]
        RT[Runtime god-object]
        RT --> A[Actor<br/>mailbox + heap + OrcaGc]
        RT --> S[Scheduler<br/>Chase-Lev deque]
        RT --> SUP[Supervisor tree]
        RT --> CD[CycleDetector<br/>intra-node only]
        RT --> TW[TimerWheel]
        RT --> REG[ActorRegistry]
        RT --> PG[ProcessGroups]
        RT --> PER[PersistenceStore]
    end

    subgraph Memory ["Per-Actor Memory (src/runtime/heap.rs, gc.rs)"]
        AH[ActorHeap<br/>bump + size-class free lists]
        OH[OrcaHeader<br/>ref_count / foreign_count / sticky]
        OG[OrcaGc<br/>local_ref / send_ref_to / drop_local_ref]
        OC[OrcaCoordinator<br/>ForeignRefOp routing]
    end

    subgraph Distribution ["Distributed Runtime (src/runtime/network.rs, cluster.rs, distributed.rs)"]
        NT[NetworkTransport<br/>TCP + NUL0 framing]
        CS[ClusterState<br/>gossip + heartbeat]
        AR[AddressResolver<br/>local vs remote]
        RAC[RemoteActorCache<br/>LRU 10k]
        CM[CrdtManager<br/>8 CRDT types]
    end

    subgraph Cloud ["Cloud / Workflow Targets (DESIGN_*.md)"]
        CP[Control Plane<br/>scheduler / autoscaler / router]
        ES[Event Store / Journal]
        WF[Workflow Engine<br/>sagas / timers / signals]
        OBS[Observability<br/>traces / metrics / logs]
    end

    VM -->|ActorVmCallbacks| RT
    RT -->|step_actor| VM
    A --> AH
    A --> OG
    OG --> OC
    OC --> OG
    OC --> CD
    RT -->|send_message_by_id| A
    S -->|dequeue / enqueue| RT
    SUP -->|restart / escalate| RT
    RT -->|heartbeat / gossip| CS
    CS --> NT
    AR -->|local| RT
    AR -->|remote| NT
    CM -->|CrdtSync packets| NT
    RT --> PER
    RT --> WF
    CP --> RT
    OBS --> RT
```

---

## 3. Natural-Language Compilation Pipeline

This diagram shows how multiple frontends converge on Intent IR, which is validated, clarified, planned, and lowered to AST before entering the deterministic compiler pipeline.

```mermaid
flowchart LR
    subgraph Frontends
        A[Handwritten Nulang]
        B[Natural Language]
        C[Visual Programming]
        D[JSON API]
        E[Voice]
        F[IDE Interactions]
    end

    G[Intent IR<br/>schema-defined spec graph]
    H[Intent Validator<br/>security / policy / schema]
    I[Clarification Engine<br/>ambiguity scoring]
    J[Architecture Planner<br/>modules / actors / effects]
    K[Architecture Graph]
    L[Semantic Planner<br/>types / algos / effects]
    M[AST Builder<br/>AstModule + spans]
    N[Existing Compiler Pipeline<br/>parse → type → effect → cap → bytecode]
    O[VM / JIT / AOT Backend]
    P[Audit Trail & Approval Log]

    A --> G
    B --> G
    C --> G
    D --> G
    E --> G
    F --> G

    G --> H
    H -->|ambiguous| I
    I -->|clarified| G
    H -->|valid| J
    J --> K
    K --> L
    L --> M
    M --> N
    N --> O

    H -.-> P
    I -.-> P
    J -.-> P
    L -.-> P
    M -.-> P
```

---

## 4. AI Architecture

This diagram shows where AI participates in the Nulang toolchain, how providers are abstracted, and the validation/audit loop that keeps compilation deterministic.

```mermaid
flowchart TB
    subgraph Providers
        P1[OpenAI / Anthropic / Azure]
        P2[Local GGUF via llama.cpp]
        P3[Ollama / vLLM]
        P4[Custom OpenAI-compatible]
    end

    R[Model Registry<br/>capability + cost + latency + privacy]
    S[Provider Scheduler]
    C[Deterministic Cache<br/>intent + params -> AST fragment]
    T[Structured Output / Constrained Decoding]
    U[Tool Router]
    V[Validator<br/>schema + policy]
    W[Compiler + Typechecker + Effect/Cap]
    X[Tests]
    Y[Audit Trail]
    Z[User Approval UI]

    P1 --> R
    P2 --> R
    P3 --> R
    P4 --> R
    R --> S
    S --> C
    C -->|cache miss| T
    T --> U
    U --> V
    V -->|invalid| Z
    V -->|valid| W
    W -->|compile error| U
    W --> X
    X -->|fail| U
    X -->|pass| Y
    X -->|pass| Z
    Z -->|approved| Y
```

---

## 5. Semantic IDE Server Architecture

This diagram shows how the IDE server sits on top of the compiler database and runtime telemetry to power LSP and richer Nulang-native tools.

```mermaid
flowchart LR
    Editor[VS Code / JetBrains / Web IDE]
    LSP[LSP JSON-RPC]
    NIP[Nulang IDE Protocol<br/>graphs / intents / previews]
    CDB[CompileDb<br/>incremental + error-tolerant]
    TC[TypeChecker]
    EC[EffectChecker]
    CA[CapabilityAnalyzer]
    CP[Compiler + source maps]
    RT[Runtime telemetry]
    AI[Intent model / LLM bridge]

    Editor --> LSP
    Editor --> NIP
    LSP --> CDB
    NIP --> CDB
    CDB --> TC
    CDB --> EC
    CDB --> CA
    CDB --> CP
    CDB --> RT
    NIP --> AI
    AI --> CDB
```

---

## 6. Web Framework / LiveView Request Lifecycle

This diagram shows how HTTP requests and WebSocket connections map to supervised actors.

```mermaid
flowchart LR
    HTTP[HTTP request] --> Endpoint[Endpoint actor]
    Endpoint --> Router[Router behavior]
    Router --> Controller[Controller actor]
    Controller --> View[View / Template]
    View --> Response[HTTP response]
```

```mermaid
flowchart LR
    Browser[Browser] -->|HTTP GET| Endpoint
    Endpoint -->|HTML + JS| Browser
    Browser -->|WebSocket upgrade| LiveView[LiveView actor]
    LiveView -->|render diff| Browser
    Browser -->|phx-click| LiveView
    LiveView -->|broadcast| PubSub
    PubSub -->|handle_info| LiveView
```

---

## 7. Text ↔ Intent ↔ AST Loop

This diagram shows bidirectional editing in the IDE: text, intent, and AST stay in sync, with type/effect/capability checks as the safety gate.

```mermaid
flowchart TD
    User[User intent or edit]
    Parser[Parser + error recovery]
    TAST[Typed AST]
    IntentIR[Intent IR<br/>what the code is supposed to do]
    Gen[Code generator / LLM]
    Check[Type / effect / cap check]

    User -->|writes text| Parser
    Parser --> TAST
    TAST -->|summarize| IntentIR
    User -->|writes intent| IntentIR
    IntentIR --> Gen
    Gen --> TAST
    TAST --> Check
    Check -->|valid| PrettyPrint[Pretty print + preserve trivia]
    PrettyPrint --> User
    Check -->|invalid| Gen
```

---

## 8. Supervision / Actor Graph Example

This diagram shows a typical supervision tree with links and monitors.

```mermaid
flowchart TD
    Sup[Supervisor]
    A1[Worker A]
    A2[Worker B]
    A3[Cache Actor]
    M[Monitor Watcher]

    Sup -.OneForAll.-> A1
    Sup -.OneForAll.-> A2
    A1 <--link--> A3
    M -.monitor.-> A2
```
