---
title: "IO Effect"
description: "Built-in IO effect operations (auto-generated from src/stdlib.rs)"
sidebar:
  label: "IO"
editUrl: false
---

> **This page is auto-generated from `src/stdlib.rs`.**
> Do not edit it by hand — your changes will be overwritten on the next CI run.
> To add or update a built-in operation, edit the `StdLib::new()` registry in `src/stdlib.rs`.

# IO Effect

The `IO` effect provides the following built-in operations, wired into the VM and runtime.

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `IO.print` | `print(msg: String) -> Unit` | Write the argument to stdout, followed by a newline. |
| `IO.println` | `println(msg: String) -> Unit` | Alias of `IO.print`; writes the argument to stdout with a newline. |
| `IO.read` | `read() -> String` | Read one line from stdin; returns the line without the trailing newline. |

_Implementation site: Standalone VM_
