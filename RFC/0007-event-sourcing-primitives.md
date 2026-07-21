# RFC 0007: Event Sourcing Primitives

- **Status:** Draft
- **Tier:** Stable
- **Author:** AI assistant review
- **Created:** 2026-07-21
- **Resolved:** (pending)
- **Language-version at effect:** 2.0 (planned)
- **Supersedes:** none
- **Superseded by:** none

## Summary

Define the Stable language primitives for event sourcing in Nulang: the `emit` keyword for appending domain events, the `events` block for declaring event types, event-journal semantics, deterministic replay, and read-only projections. These primitives build on the durable entity model introduced in RFC 0005 and make the event journal the source of truth for entity state.

## Motivation

Durable computation requires an audit trail. For software entities that must survive years or decades, mutable state alone is insufficient: you need to know *why* the state changed. Event sourcing answers this by storing every state-changing operation as an event in an append-only log and reconstructing state by replaying that log.

The current Nulang runtime already supports four state models (`local`, `durable`, `event_sourced`, `crdt`) and persistence backends, but event sourcing is not yet a first-class language primitive. Programmers must manually wire state mutations to a journal. This RFC gives them `emit`, `events`, and projections.

## Design

### 1. `events` block

Inside an `entity` (RFC 0005), an `events` block declares the domain events the entity can emit:

```nulang
entity BankAccount {
    state balance: Int = 0          // event_sourced by default

    events
        | Deposited(amount: Int)
        | Withdrawn(amount: Int)
        | Closed(reason: String)
}
```

Rules:

- The `events` block is allowed only inside `entity` declarations. (It may later be permitted inside `persistent actor` with explicit `event_sourced` state.)
- Each event is a variant with an optional payload tuple.
- Event payloads must be serializable: primitives, strings, records, variants, arrays, and maps of serializable types. Closures, actor references, raw pointers, and non-sendable capabilities are prohibited by the type system.
- Events are scoped to the entity. Two entities can define events with the same name without conflict.

### 2. `emit` keyword

`emit EventName(args)` appends an event to the entity's event journal:

```nulang
behavior deposit(amount: Int) {
    emit Deposited(amount)
    self.balance = self.balance + amount
}
```

Rules:

- `emit` may appear only inside an `entity` behavior.
- The emitted event must match one of the entity's declared event types.
- `emit` returns `Unit`.
- The runtime guarantees the event is persisted before the behavior returns (unless the behavior itself crashes, in which case the event may or may not be persisted; this is the same atomicity boundary as the behavior).
- `emit` is an effect (`Entity.emit` or `Event.emit`) and appears in the behavior's effect row.

### 3. Event journal semantics

The event journal is an append-only, ordered log of events for a single entity instance.

- **Ordering:** Events are appended in the order the behaviors that emitted them complete.
- **Durability:** The journal is stored in the configured persistence backend. It survives crashes, restarts, and migration.
- **Replay:** On recovery, the runtime reconstructs the entity's event-sourced state by replaying the journal from the beginning or from the latest snapshot.
- **Snapshots:** Snapshots are a performance optimization, not part of the semantics. Any two conforming runtimes that replay the same journal from the same starting point must arrive at the same state.
- **Immutability:** Events are immutable. There is no operation to delete or modify a past event; errors are corrected by emitting compensating events.

### 4. Deterministic replay

Replay is deterministic if:

1. The entity code is unchanged, or migration contracts (RFC 0008) are applied.
2. External effects performed during replay are handled by the runtime's replay handlers (e.g., `Timer.sleep` during replay returns immediately or uses a recorded timestamp).
3. Effect handlers are pure functions of the entity state and the event being processed.

The runtime must provide a replay mode for the entity runtime that:

- Re-arms timers from recorded timestamps rather than wall-clock delays.
- Skips or records side effects such as message sends and storage writes, depending on replay purpose (recovery vs. audit vs. testing).

### 5. Event handlers and projections

Entities can declare explicit event handlers that update state from events. This is optional: direct mutation in the behavior plus `emit` is sufficient for simple cases. For complex cases, handlers separate state evolution from command handling:

```nulang
entity BankAccount {
    state balance: Int = 0

    events
        | Deposited(amount: Int)
        | Withdrawn(amount: Int)

    apply
        | Deposited(amount) => self.balance = self.balance + amount
        | Withdrawn(amount) => self.balance = self.balance - amount

    behavior deposit(amount: Int) {
        emit Deposited(amount)
    }
}
```

Rules:

- `apply` defines handlers for each event type.
- During normal execution, `emit` appends the event and immediately runs the corresponding `apply` handler to update state.
- During replay, only `apply` handlers run; command behaviors do not.
- If no `apply` handler is provided for an event, the behavior must update state directly. It is an error to emit an event with no corresponding `apply` handler if `apply` is present.

### 6. Projections

A projection is a read-only view over an entity's event journal. Projections are defined outside the entity and are useful for queries, indexes, and reporting:

```nulang
projection TotalDeposits from BankAccount.Deposited {
    state total: Int = 0
    apply | Deposited(amount) => self.total = self.total + amount
}
```

Rules:

- Projections are **Planned** for the initial implementation of this RFC; the language syntax above is aspirational.
- The Stable part of this RFC is the `emit`/`events`/`apply` model inside entities.
- Projections may be implemented first as a Cloud SDK library (`nlc.projections`) and later elevated to Stable if proven valuable.

### 7. Interaction with CRDT state

An entity may mix `event_sourced` and `crdt` state fields. Event-sourced fields are recovered by replaying the local journal; CRDT fields are recovered by merging remote deltas. The two models are orthogonal:

```nulang
entity ChatRoom {
    state durable messages: List[String] = []      // event_sourced
    state crdt viewers: Int = 0                       // merged across nodes

    events
        | MessageSent(text: String)
        | ViewerJoined

    apply
        | MessageSent(text) => self.messages = List.append(self.messages, text)
        | ViewerJoined => self.viewers = self.viewers + 1
}
```

### 8. Implementation targets

- `src/ast.rs`: Add `Decl::Entity`, `EventDecl`, `EventVariant`, `Expr::Emit`, and optional `ApplyBlock`.
- `src/parser.rs`: Parse `entity`, `events`, `emit`, and `apply`.
- `src/typechecker.rs`: Check that emitted events match declared variants and payloads are serializable.
- `src/effect_checker.rs`: Treat `emit` as an effect (`Entity.emit` or `Event.emit`) in the entity's effect row.
- `src/hir_lower.rs`: Desugar `entity` and `apply` to `persistent actor` with event-sourced state.
- `src/mir_lower.rs` / `src/mir_codegen.rs`: Generate event-journal append instructions for `emit`.
- `src/runtime/actor.rs`: Extend actor state to hold an event journal and snapshot metadata.
- `src/runtime/persistence.rs`: Store and load event journals and snapshots per entity.
- `src/stdlib.rs`: Register the `Entity.emit` / `Event.emit` effect.

### 9. Example: event-sourced counter

```nulang
entity Counter {
    state count: Int = 0

    events
        | Incremented(by: Int)
        | Reset

    apply
        | Incremented(by) => self.count = self.count + by
        | Reset => self.count = 0

    behavior increment(by: Int) {
        emit Incremented(by)
    }

    behavior reset() {
        emit Reset
    }

    behavior get() {
        self.count
    }
}
```

## Tier Classification

- **Tier:** Stable.
- **Frozen Core impact:** None.
- **Breaking change:** No. `emit`, `events`, and `apply` are additive.
- **Relationship to RFC 0005:** This RFC specifies the event-sourcing half of durable entities. RFC 0005 defines the `entity` keyword; this RFC defines `emit`, `events`, and replay semantics.

## Backwards Compatibility

This RFC is additive. Existing `persistent actor` programs continue to work. Entities without an `events` block behave like `persistent actor` with `event_sourced` state.

## Alternatives Considered

1. **Keep event sourcing as a runtime state model only (`state event_sourced x`).** Rejected because it forces programmers to manually emit events and maintain journals, making the model error-prone and opaque to tooling.
2. **Use a single global event stream instead of per-entity journals.** Rejected because it couples unrelated entities and complicates recovery and migration.
3. **Make events immutable values returned from behaviors.** Rejected because behaviors already return values; `emit` makes the side effect explicit and durable.
4. **Include projections in Stable immediately.** Rejected because projection syntax and runtime behavior need real-world validation; they start as a Cloud SDK concern and can be promoted later.

## Open Questions

1. Should `emit` be a keyword or an effect `perform Event.emit(...)`? A keyword is more readable; an effect is more consistent with the rest of the language.
2. Should event payloads be limited to a serializable subset at the type level, or should the runtime reject non-serializable payloads?
3. Should snapshots be taken automatically by the runtime, or should entities request them explicitly (`perform Entity.snapshot()`)?
4. How should event journals be garbage-collected or archived for entities with very long lifetimes?

## Resolution

(To be filled on accept/reject.)
