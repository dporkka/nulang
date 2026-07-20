---
title: "IO Effect"
description: "Built-in IO effect operations"
sidebar:
  label: "IO"
---

# IO Effect

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `IO.print` | `print(msg: String) -> Unit` | Write the argument to stdout, followed by a newline. |
| `IO.println` | `println(msg: String) -> Unit` | Alias of `IO.print`; writes the argument to stdout with a newline. |
| `IO.read` | `read() -> String` | Read one line from stdin; returns the line without the trailing newline. |

_Implementation site: Standalone VM_
