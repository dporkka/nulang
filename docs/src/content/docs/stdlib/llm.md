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

The `LLM` effect provides the following built-in operations, wired through the generic `PerformAsync` effect dispatch. The compiler emits a single `PerformAsync` bytecode with the effect-op string `"LLM.ask"`; the VM routes it to the registered LLM client via the runtime's effect handler.

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `LLM.ask` | `ask(prompt: String) -> String` | Send the prompt to the configured LLM client and return the reply; suspends non-blockingly when the runtime supports it. |

_Implementation site: Runtime Host_
