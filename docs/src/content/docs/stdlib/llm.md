---
title: "LLM Effect"
description: "Built-in LLM effect operations (auto-generated from src/stdlib.rs)"
sidebar:
  label: "LLM"
editUrl: false
---

> **This page is auto-generated from `src/stdlib.rs`.**
> Do not edit it by hand — your changes will be overwritten on the next CI run.
> To add or update a built-in operation, edit the `StdLib::new()` registry in `src/stdlib.rs`.

# LLM Effect

The `LLM` effect provides the following built-in operations, wired into the VM and runtime.

> **Deprecated since language version 1.0.0-frozen.** `LLM.ask` is deprecated in favor of `perform Provider.ask("llm", prompt)`, which references a general provider abstraction. The `LLM`/`LlmAsk` surface remains functional for the deprecation cycle (≥2 major versions). New code should use `Provider.ask`.

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `LLM.ask` | `ask(prompt: String) -> String` | Send the prompt to the configured LLM client and return the reply; suspends non-blockingly when the runtime supports it. |

_Implementation site: Runtime Host_
