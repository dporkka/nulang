---
title: Actor Model
description: Erlang-style actors with location transparency — spawn, send, receive, and supervise.
---

## The Actor Model

Nulang actors are isolated, concurrent units of computation that communicate exclusively through asynchronous message passing. Each actor has:

- **Private state** — mutable fields only accessible from within the actor
- **Behaviors** — message handlers that transform state and send responses
- **A mailbox** — FIFO-ordered message queue
- **An identity** — a unique `Actor` reference (64-bit id)

## Defining Actors

```nulang
actor Counter {
    state count: Int = 0

    behavior inc() {
        self.count = self.count + 1
    }

    behavior inc_by(amount: Int) {
        self.count = self.count + amount
    }

    behavior get(sender: Actor) {
        send sender reply(self.count)
    }
}
```

## Spawning Actors

`spawn` creates a new actor instance and returns its reference:

```nulang
let counter = spawn Counter {} in {
    // counter is an Actor reference
    send counter inc()
    counter
}
```

The expression after `in` runs in the spawner's context and can reference the new actor.

## Sending Messages

`send` delivers a message to an actor's mailbox asynchronously:

```nulang
send counter inc()
send counter inc_by(5)
send counter get(self)  // self refers to the current actor
```

Messages are always delivered and never dropped. The sender does not block.

## Receiving Messages

Inside a behavior, `receive` blocks until a matching message arrives:

```nulang
behavior wait_for_reply() {
    receive {
        | reply(value: Int) => {
            perform IO.print("Got: " + Int.to_string(value))
        }
    }
}
```

The `receive` expression supports selective matching — only matching messages are consumed; non-matching messages are requeued:

```nulang
receive {
    | reply(value: Int) => {
        // Handle integer reply
        perform IO.print("Int: " + Int.to_string(value))
    }
    | reply(msg: String) => {
        // Handle string reply
        perform IO.print("String: " + msg)
    }
}
```

### Timed Receive

Wait for a message with a timeout:

```nulang
receive {
    | reply(value: Int) => perform IO.print("Got it")
} after 5000 => {
    perform IO.print("Timed out after 5 seconds")
}
```

## Actor Lifecycle

1. **Spawned** — Actor is created with initial state
2. **Running** — Processing messages from the mailbox
3. **Waiting** — Blocked in a `receive` with no matching messages
4. **Exited** — Actor terminated (normal, error, or killed)

Actors can self-exit:

```nulang
perform Actor.exit(0)   // Normal exit
perform Actor.exit(1)   // Error exit
perform Actor.exit(2)   // Kill (cannot be trapped)
```

## Scheduling

Actors are scheduled cooperatively with reduction-bounded fairness. Each actor gets a budget of 1000 message reductions per turn. When exhausted, it yields and requeues at the back of its priority queue.

Priorities: High → Normal (default) → Low. All High-priority actors run before any Normal; Normal before Low.

## Next

- [Distribution & Clustering](/actors/distribution/) — run actors across multiple nodes
- [Supervision Trees](/actors/supervision/) — build fault-tolerant systems with OTP supervisors
