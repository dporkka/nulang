---
title: Multi-Agent Patterns
description: Compose multiple Nulang agents into pipelines, debate teams, and supervisor hierarchies.
---

## Patterns for Multiple Agents

Nulang's AI runtime provides three multi-agent patterns that compose agents into structured workflows. Each pattern addresses a different coordination need.

| Pattern | Module | Use case |
|---------|--------|----------|
| Pipeline | `src/ai/pipeline.rs` | Sequential processing through stages |
| Debate | `src/ai/debate.rs` | Pro/con argumentation with synthesis |
| Supervisor team | `src/ai/supervisor.rs` | Hierarchical agent management with restart strategies |

## Pipelines

A pipeline chains agents through named stages. Each stage receives the previous stage's output and processes it according to a template.

### Building a pipeline

```nulang
agent Researcher = {
    model: "llama3.1",
    system_prompt: "You are a researcher. Provide factual information.",
    pricing: { input: 0.0, output: 0.0 }
}

agent Writer = {
    model: "llama3.1",
    system_prompt: "You are a writer. Create engaging content.",
    pricing: { input: 0.0, output: 0.0 }
}

fn main() {
    let researcher = spawn Researcher {} in
    let writer = spawn Writer {} in
    let pipeline = Pipeline.new()
        |> Pipeline.stage("research", researcher, "Research: {input}")
        |> Pipeline.stage("write", writer, "Write based on: {input}")
    in
    pipeline.run("CRDTs")
}
```

The `{input}` placeholder in each stage's template is replaced with the previous stage's output. `pipeline.run("CRDTs")` executes the stages in order and returns the final output.

## Debates

Debates coordinate multiple agents arguing different positions, with a moderator synthesizing the final answer.

### Declaring a debate

Define agents for each position plus a moderator:

```nulang
agent ProAgent = {
    model: "llama3.1",
    system_prompt: "Argue in favor.",
    pricing: { input: 0.0, output: 0.0 }
}

agent ConAgent = {
    model: "llama3.1",
    system_prompt: "Argue against.",
    pricing: { input: 0.0, output: 0.0 }
}

agent Moderator = {
    model: "llama3.1",
    system_prompt: "Synthesize the arguments into a balanced conclusion.",
    pricing: { input: 0.0, output: 0.0 }
}
```

The debate runtime (implemented in `src/ai/debate.rs`) runs the pro and con agents in rounds, then feeds their arguments to the moderator for synthesis. Each agent maintains its own memory and position across rounds.

### When to use debates

Debates are effective for questions with genuine tradeoffs — architectural decisions, trade-off analysis, or any problem where considering opposing viewpoints improves the answer. For straightforward tasks, a single agent or a pipeline is simpler and cheaper.

## Supervisor Teams

Supervisor teams apply OTP-style supervision to agents. If an agent fails (LLM timeout, rate limit, malformed response), the supervisor restarts it according to a strategy.

### How supervisor teams work

The supervisor team runtime (implemented in `src/ai/supervisor.rs`) wraps agents in a supervision tree. Each agent is a supervised child. When an agent's LLM call fails, the supervisor applies its restart strategy:

| Strategy | Behavior on failure |
|----------|-------------------|
| Restart the failed agent only | One-for-one |
| Restart all agents in the team | One-for-all |

This mirrors the [actor supervision model](/actors/supervision/) — the same OTP primitives, applied to agents. The difference is that agent supervisors understand LLM-specific failure modes (rate limits, provider outages) and can apply backoff before restarting.

### When to use supervisor teams

Use supervisor teams when running long-lived agents that must survive transient failures — a persistent assistant, a monitoring agent, or any agent that should recover automatically without manual intervention.

## Combining Patterns

The three patterns compose. A common production setup:

1. A **supervisor team** wraps a set of agents for fault tolerance.
2. The supervised agents participate in a **pipeline** for sequential processing.
3. A **debate** stage in the pipeline handles questions requiring multiple perspectives.

## Next

- [Durable Workflows Overview](/workflows/overview/) — workflows orchestrate agents with checkpointed state
- [Supervision Trees](/actors/supervision/) — the OTP supervision model that agent supervisors extend
