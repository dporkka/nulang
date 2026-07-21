# RFC 0008: Migration Contracts

- **Status:** Draft
- **Tier:** Stable
- **Author:** AI assistant review
- **Created:** 2026-07-21
- **Resolved:** (pending)
- **Language-version at effect:** 2.0 (planned)
- **Supersedes:** none
- **Superseded by:** none

## Summary

Define Stable language and runtime support for evolving durable entity schemas over time. A `migration` contract attached to an entity declares how to upgrade historical snapshot state and event-journal entries from an older schema version to the current one. Migrations are pure functions, applied lazily during replay, and recorded in the entity's metadata so that the same entity code can load journals written years ago.

## Motivation

Durable entities are designed to live for years or decades. During that lifetime, their state schema and event definitions will change: fields will be added, removed, renamed, or retyped; new event types will appear; old event types will become obsolete. Without a migration mechanism, every schema change would break the recovery of existing entities and force manual data migration.

A migration contract solves this by:

1. **Keeping the source of truth intact.** The event journal is never rewritten in place.
2. **Applying upgrades lazily.** On recovery, the runtime replays the journal through the chain of migrations until the current schema version is reached.
3. **Making evolution explicit.** Migrations are code, reviewed and versioned like any other behavior.
4. **Preserving determinism.** Migrations are pure functions, so replay remains deterministic.

## Design

### 1. Entity versions

Every `entity` has an implicit or explicit version:

```nulang
entity BankAccount {
    version: 3

    state balance: Int = 0
    state currency: String = "USD"   // added in v2
    state overdraft_limit: Int = 0   // added in v3

    events
        | Deposited(amount: Int)
        | Withdrawn(amount: Int)
        | CurrencyChanged(currency: String)
}
```

Rules:

- `version` is a positive integer. It defaults to 1 if omitted.
- The version is recorded in every snapshot and journal segment header.
- A journal segment without a version field is treated as version 1.
- The version applies to both the state schema and the event schema.

### 2. `migration` block

A `migration` block declares a pure function that upgrades state and events from one version to the next:

```nulang
entity BankAccount {
    version: 2

    state balance: Int = 0
    state currency: String = "USD"

    events
        | Deposited(amount: Int)
        | Withdrawn(amount: Int)
        | CurrencyChanged(currency: String)

    migration from 1 to 2 {
        state => {
            // Add the new field with a default value.
            self.currency = "USD"
        }
        events {
            | Deposited(amount) => {
                emit Deposited(amount)
                emit CurrencyChanged("USD")
            }
            | Withdrawn(amount) => emit Withdrawn(amount)
            | other => other
        }
    }
}
```

Syntax:

- `migration from V to W` where `W == V + 1`.
- Optional `state => { ... }` clause for upgrading snapshot state.
- Optional `events { ... }` clause with event handlers.
- Multiple `migration` blocks can be declared to form a chain: 1→2, 2→3, 3→4.

Rules:

- Migrations are pure: they may not perform effects, send messages, access the clock, or read external state.
- Inside a migration, `self` refers to the entity state at the old version. Assignments to `self` upgrade the state in place.
- `emit` inside a migration appends zero or more upgraded events to the replay stream; it does not mutate the persistent journal.
- Event handlers may use a wildcard catch-all (`| other => other`) to pass through unchanged events.
- It is a compile-time error if a migration is declared for a version that does not match `W == V + 1` or if there is a gap in the chain.

### 3. Migration semantics during replay

When an entity recovers from a snapshot and journal:

1. Read the snapshot version `S`.
2. Apply `migration from S to S+1`, then `S+1 to S+2`, ..., up to the current entity version `C`.
3. Replay journal events starting from the snapshot offset.
4. For each journal event at version `J`, apply migrations from `J` to `C` before running the entity's `apply` handlers.
5. If the snapshot version equals the current version, no state migrations are needed; only journal events may need migration.

Migrations are applied in-memory. The persistent journal is not rewritten.

### 4. Determinism and purity

Migrations must be deterministic functions of their input. The compiler and runtime enforce this by:

- Forbidding effectful operations inside migration blocks.
- Forbidding `perform`, `send`, `spawn`, `ask`, `after`, `until`, and external function calls.
- Allowing only pure Nulang expressions, record/variant construction, and `emit` to the replay stream.

This restriction ensures that replay produces the same state on any conforming runtime.

### 5. Default migrations

For common additive changes, the compiler can generate a default migration:

- Adding a new state field with a literal default value.
- Adding a new event type that does not affect existing state.

Explicit migrations are required for:

- Renaming or removing fields.
- Changing field types.
- Splitting or merging event types.
- Deriving new state from historical events.

The compiler emits a warning when an entity's version increases without a corresponding explicit or default migration.

### 6. Migration registry and tooling

- Each entity carries a list of its migrations in the compiled artifact metadata.
- The LSP can show which migrations apply to an entity and flag missing migration clauses.
- A future CLI tool (`nulang migrate --check`) can validate that all entities with a version > 1 have complete migration chains.

### 7. Example: changing an event type

Version 1 event:

```nulang
events | Deposited(amount: Int)
```

Version 2 splits amount into whole and fractional units:

```nulang
events | Deposited(whole: Int, fractional: Int)

migration from 1 to 2 {
    events {
        | Deposited(amount) => emit Deposited(amount / 100, amount % 100)
        | other => other
    }
}
```

### 8. Implementation targets

- `src/ast.rs`: Add `MigrationBlock`, `MigrationClause`, and version fields to `Decl::Entity`.
- `src/parser.rs`: Parse `version: N` and `migration from V to W { ... }`.
- `src/typechecker.rs`: Verify migration purity and event/state compatibility.
- `src/effect_checker.rs`: Reject effectful operations inside migrations.
- `src/hir_lower.rs`: Desugar migrations into pure upgrade functions.
- `src/mir_lower.rs` / `src/mir_codegen.rs`: Generate migration application during replay.
- `src/runtime/actor.rs`: Store entity version in actor state and invoke migrations on recovery.
- `src/runtime/persistence.rs`: Read and write versioned snapshots and journal segments.
- `src/stdlib.rs`: No new effects needed; migrations are pure code.

## Tier Classification

- **Tier:** Stable.
- **Frozen Core impact:** None.
- **Breaking change:** No. Migrations are additive.
- **Relationship to other RFCs:** Depends on RFC 0005 (Durable Entities) and RFC 0007 (Event Sourcing Primitives). Migrations upgrade both the state schema and event schema defined in those RFCs.

## Backwards Compatibility

This RFC is additive. Existing `persistent actor` and `actor` programs are unaffected. Entities without explicit versions default to version 1 and have no migrations.

## Alternatives Considered

1. **Offline migration tools that rewrite the journal.** Rejected because rewriting the source of truth loses historical data and is risky for long-lived entities.
2. **Automatic schema inference and coercion.** Rejected because silently coercing old events can hide semantic changes (e.g., units changing from cents to dollars).
3. **Migration as a separate top-level declaration.** Rejected because attaching migrations to the entity keeps the evolution story local and makes it clear which entity a migration belongs to.
4. **Allow effectful migrations.** Rejected because it would break deterministic replay and make auditing impossible.

## Open Questions

1. Should migrations be allowed to access the old entity code, or only the old state/events? Accessing old code complicates versioning but may be needed for complex transformations.
2. Should migrations support downgrades (e.g., `migration from 2 to 1`)? Downgrades are useful for rollback but significantly complicate the model.
3. Should the runtime cache migrated journal segments to avoid re-applying migrations on every recovery?
4. Should migrations be unit-testable in isolation by feeding them old events/state?

## Resolution

(To be filled on accept/reject.)
