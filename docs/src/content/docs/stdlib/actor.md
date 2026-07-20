---
title: "Actor Effect"
description: "Built-in Actor effect operations"
sidebar:
  label: "Actor"
---

# Actor Effect

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `Actor.link` | `link(target: Actor) -> Nil` | Link the current actor to `target`; abnormal exits propagate to linked peers. Nil no-op outside an actor. |
| `Actor.unlink` | `unlink(target: Actor) -> Nil` | Remove the link between the current actor and `target`. Nil no-op outside an actor. |
| `Actor.monitor` | `monitor(target: Actor) -> Nil` | Monitor `target` from the current actor; a DOWN system message is delivered when it exits. Nil no-op outside an actor. |
| `Actor.demonitor` | `demonitor(target: Actor) -> Nil` | Stop the current actor's monitor on `target`, so no DOWN message is delivered. Nil no-op outside an actor. |
| `Actor.trap_exit` | `trap_exit(flag: Bool) -> Nil` | Set the current actor's trap_exits flag; when true, linked-peer exit signals arrive as system messages instead of killing it. Nil no-op outside an actor. |
| `Actor.exit` | `exit(reason: Int \| String) -> Nil` | Self-exit the current actor; 0/"normal", 1/"error", 2/"kill" select the reason, anything else is custom. Nil no-op outside an actor. |
| `Actor.register` | `register(name: String) -> Nil` | Register the current actor under `name` in the local actor registry. Nil no-op outside an actor. |
| `Actor.unregister` | `unregister(name: String) -> Nil` | Remove `name` from the local actor registry. |
| `Actor.whereis` | `whereis(name: String) -> Actor \| Nil` | Look up `name` in the local actor registry; returns the actor ref, or nil when the name is not registered. |
| `Actor.set_priority` | `set_priority(level: Int) -> Nil` | Set the current actor's scheduling priority: 0=High, 1=Normal, 2=Low (any other value selects Normal). Ready High-priority actors are scheduled before Normal, Normal before Low; affects scheduling only, not message order. Nil no-op outside an actor. |

_Implementation site: Runtime Host_
