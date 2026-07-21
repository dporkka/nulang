# RFC 0006: Temporal Effects

- **Status:** Draft
- **Tier:** Stable
- **Author:** AI assistant review
- **Created:** 2026-07-21
- **Resolved:** (pending)
- **Language-version at effect:** 2.0 (planned)
- **Supersedes:** none
- **Superseded by:** none

## Summary

Make time a first-class Stable language effect. Add `Timer.sleep` and `Timer.sleep_until` as Stable effects available in any actor context, and provide syntactic sugar `after duration_ms => expr` and `until condition => expr` for common temporal patterns. Timers are durable by default: a sleeping actor that crashes or is hibernated resumes when the timer fires.

## Motivation

Nulang is repositioning as a durable computation language for long-lived software entities. Time is a fundamental primitive for such entities: delays, deadlines, timeouts, scheduled work, and polling loops appear in almost every durable program. Currently, `Timer.sleep` exists but is only available inside workflow actors and is not treated as a Stable language primitive. This RFC elevates durable time to a Stable language effect and gives it readable syntax.

Time also appears in the user's proposed strategic updates (e.g., `after`, `until`, temporal syntax). By making these constructs sugar over a small set of Stable `Timer` effects, we keep the language kernel small while giving programmers a natural way to express temporal behavior.

## Design

### 1. Stable `Timer` effects

Two operations become Stable language effects:

```nulang
effect Timer {
    sleep(duration_ms: Int) -> Unit
    sleep_until(timestamp_ms: Int) -> Unit
}
```

Semantics:

- `perform Timer.sleep(duration_ms)` suspends the current actor behavior for at least `duration_ms` milliseconds, then resumes at the next available scheduler turn.
- `perform Timer.sleep_until(timestamp_ms)` suspends until the absolute timestamp (milliseconds since the Unix epoch) is reached.
- Both are **durable**: the runtime records the timer in persistent storage. If the actor crashes, is restarted, or is hibernated, it resumes when the timer fires.
- The return value is `Unit`.
- Calling either outside an actor context is a runtime error (the standalone VM has no scheduler to arm timers).

### 2. `after` contextual keyword

`after ms => expr` is syntactic sugar for `receive {} after ms => expr`. This is
already a fully working Nulang expression that uses the durable
`ReceiveWait` suspension mechanism. No additional AST nodes, bytecode
opcodes, or runtime changes are required.

```nulang
after 5000 => perform IO.print("done")
```

desugars to:

```nulang
receive {} after 5000 => perform IO.print("done")
```

The expression's type is the type of the body.

### 3. `until` syntactic sugar (Planned)

```nulang
until self.ready => perform IO.print("ready!")
```

desugars to a polling loop that yields the actor between checks:

```nulang
let poll_interval_ms = 100 in
rec loop() {
    if self.ready then
        perform IO.print("ready!")
    else {
        perform Timer.sleep(poll_interval_ms)
        loop()
    }
}()
```

Rules:

- The condition is re-evaluated after each `Timer.sleep`.
- The default poll interval is 100 ms; it can be overridden with an explicit interval: `until self.ready poll 50 => expr`.
- `until` consumes one actor turn per evaluation of the condition plus one per sleep. It is not a busy-wait.
- Because the loop is explicit, the actor can process other messages between iterations. (In the current single-threaded-per-actor model, this means the behavior must complete; a future `await` extension could allow preemption inside `until`.)

### 4. Interaction with the effect system

`after` and `until` require the `Timer` effect in the enclosing function/behavior's effect row:

```nulang
behavior wait_then_greet() ! {Timer, IO} {
    after 1000 => perform IO.print("hello")
}
```

The typechecker desugars first, then infers the `Timer` effect from the `perform Timer.sleep` calls.

### 5. Deterministic testing

For tests and simulation, the runtime provides a deterministic clock effect `Timer.now_ms` (or a separate `Clock` effect):

```nulang
effect Clock {
    now_ms() -> Int
}
```

This is **Planned** and not part of this RFC. Tests for `after`/`until` should mock `Timer.sleep` with a zero-duration handler so the test completes synchronously.

### 6. Relationship to `Signal.wait`

`Signal.wait(name)` remains a workflow-specific runtime effect for durable workflow synchronization. It is not elevated to Stable language status. Temporal effects (`Timer.sleep`, `after`, `until`) are the Stable time primitives; signals remain a workflow/library concern.

### 7. Implementation targets

- `src/lexer.rs`: reserve `until` as a keyword (already done as `TokenKind::Until`).
  `after` remains a contextual keyword — recognized only in `receive` position
  and in `parse_after` expression parsing.
- `src/parser.rs`: `after` is handled contextually; when seen as an expression
  prefix, it desugars to `receive {} after ... => ...` internally.
- No new AST nodes, typechecker cases, effect checker cases, HIR/bytecode
  lowering, or runtime changes are needed — the existing `ReceiveWait`
  infrastructure handles the suspension, timer, and effect row.
- `until` is reserved as `TokenKind::Until` but its parsing and lowering
  are **Planned** (see RFC 0007 for event-sourcing infrastructure that
  `until` may use).

### 8. Example: durable timeout

```nulang
behavior fetch_with_timeout(url: String) ! {Timer, Net, IO} {
    let fetch_task = spawn Fetcher { url = url }
    after 5000 => {
        send fetch_task cancel()
        Error("timeout")
    }
}
```

If the actor crashes during the 5-second wait, the timer is restored on recovery and the timeout still fires.

### 9. Example: polling with `until`

```nulang
behavior wait_for_payment(order_id: Int) ! {Timer, Storage, IO} poll 200 {
    until self.paid => {
        let status = perform Storage.read(order_id)
        if status == "paid" then
            self.paid = true
    }
}
```

Note: the explicit `poll 200` overrides the default 100 ms interval.

## Tier Classification

- **Tier:** Stable.
- **Frozen Core impact:** None. Time effects are outside Core.
- **Breaking change:** No. Existing `Timer.sleep` behavior is preserved; it is generalized from workflow-only to any actor.
- **Deprecation interaction:** None.

## Backwards Compatibility

This RFC is additive. Existing programs using `Timer.sleep` in workflow actors continue to work. The runtime change to allow `Timer.sleep` in any actor is a relaxation, not a breaking change.

## Alternatives Considered

1. **Make `after`/`until` Frozen Core primitives.** Rejected because the Frozen Core excludes actors, effects, and time.
2. **Implement `after`/`until` as runtime opcodes instead of sugar.** Rejected because it duplicates the effect system; sugar over `Timer.sleep` keeps the language smaller and easier to formalize.
3. **Make `until` a language-level blocking wait on arbitrary conditions without polling.** Rejected because it requires either a runtime subscription mechanism (too specific to today's platforms) or implicit preemption (future work). The polling-loop desugaring is explicit and portable.
4. **Use `Timer.now` for absolute deadlines instead of `sleep_until`.** Rejected because `sleep_until` is a single durable timer; `now` is useful but belongs to a separate `Clock` effect.

## Open Questions

1. Should `Timer` also include `now_ms` in this RFC, or should clock reading be a separate `Clock` effect?
2. What is the default poll interval for `until`? 100 ms is proposed as a reasonable starting point.
3. Should `after`/`until` be allowed outside actor behaviors (e.g., in top-level `__main`)? The standalone VM has no scheduler, so this would be a runtime error; should it be a compile-time error instead?
4. Should `until` support a maximum wait time to avoid infinite loops?

## Resolution

(To be filled on accept/reject.)
