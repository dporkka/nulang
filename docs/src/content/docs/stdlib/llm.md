---
title: "LLM Effect"
description: "Built-in LLM effect operations"
sidebar:
  label: "LLM"
---

# LLM Effect

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `LLM.ask` | `ask(prompt: String) -> String` | Send the prompt to the configured LLM client and return the reply; suspends non-blockingly when the runtime supports it. |

_Implementation site: Runtime Host_
