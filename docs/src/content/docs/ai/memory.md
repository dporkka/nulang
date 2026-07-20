---
title: Agent Memory
description: Three memory subsystems for Nulang agents — episodic conversation history, semantic fact recall, and procedural pattern learning.
---

## Three Memory Subsystems

Nulang agents have three independent memory subsystems, each configured separately and persisted independently. All three survive node restarts when a persistence store is attached.

| Memory | Config field | Purpose |
|--------|-------------|---------|
| Episodic | `memory: { max_turns: N }` | Conversation history window |
| Semantic | `semantic_memory: { dimensions: N }` | Vector embeddings for fact recall |
| Procedural | `procedural_memory: { namespace: "..." }` | Learned patterns and skills |

## Episodic Memory

Episodic memory is the conversation history. `max_turns` controls how many turns of dialogue the agent retains in its context window:

```nulang
agent Assistant = {
    model: "gpt-4o",
    system_prompt: "You are helpful.",
    memory: { max_turns: 10 }
}
```

Older turns beyond `max_turns` are dropped from the context. Episodic memory is in-process and does not persist across restarts — use semantic memory for durable facts.

## Semantic Memory

Semantic memory stores facts as vector embeddings, enabling recall by similarity. The `dimensions` field sets the embedding vector size:

```nulang
agent Researcher = {
    model: "llama3.1",
    system_prompt: "You are a research assistant.",
    semantic_memory: { dimensions: 32 }
}
```

### Persistence across restarts

Semantic memory persists to the attached persistence store. After a node restart, the recovered agent recalls previously stored facts:

```nulang
@tool(description: "Store a research fact tagged with a topic.")
fn store_fact(content: String, topic: String) -> String { content }

agent Researcher = {
    model: "llama3.1",
    system_prompt: "You are a research assistant.",
    pricing: { input: 0.0, output: 0.0 },
    semantic_memory: { dimensions: 32 },
    tools: [store_fact]
}
```

When the agent stores a fact (e.g. "CRDTs are conflict-free replicated data types." tagged with topic "distributed"), a restart with the same persistence store preserves the embedding. The recovered agent can recall the fact when asked a related question.

## Procedural Memory

Procedural memory stores learned patterns and skills, keyed by a namespace. This enables agents to accumulate reusable strategies across sessions:

```nulang
agent Coder = {
    model: "gpt-4o",
    system_prompt: "You are a code reviewer.",
    procedural_memory: { namespace: "code_review" }
}
```

### How procedural memory works

When an agent discovers a useful pattern (e.g. a code review template, a debugging strategy), it stores it in procedural memory under the namespace. On subsequent invocations — even after a restart — the agent retrieves and applies the stored pattern.

The `namespace` field scopes patterns so multiple agents don't collide. Two agents using `namespace: "my_app"` share procedural memory; different namespaces are isolated.

## Combining Memory Subsystems

An agent can use all three memory types simultaneously:

```nulang
agent Researcher = {
    model: "llama3.1",
    system_prompt: "You are a research assistant.",
    pricing: { input: 0.0, output: 0.0 },
    memory: { max_turns: 20 },
    semantic_memory: { dimensions: 32 },
    procedural_memory: { namespace: "research" }
}
```

- **Episodic** keeps the current conversation coherent within the window.
- **Semantic** recalls durable facts learned in prior sessions.
- **Procedural** applies learned research strategies.

## Next

- [Multi-Agent Patterns](/ai/multi-agent/) — pipelines, debates, and supervisor teams
