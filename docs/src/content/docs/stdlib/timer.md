---
title: "Timer Effect"
description: "Built-in Timer effect operations (auto-generated from src/stdlib.rs)"
sidebar:
  label: "Timer"
editUrl: false
---

> **This page is auto-generated from `src/stdlib.rs`.**
> Do not edit it by hand — your changes will be overwritten on the next CI run.
> To add or update a built-in operation, edit the `StdLib::new()` registry in `src/stdlib.rs`.

# Timer Effect

The `Timer` effect provides the following built-in operations, wired into the VM and runtime.

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `Timer.sleep` | `sleep(name: String, duration_ms: Int) -> Unit` | Schedule a durable workflow timer; only available inside workflow actors. |

_Implementation site: Runtime Host_
