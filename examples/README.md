# Nulang Example Programs

A curated suite of self-contained Nulang programs demonstrating the language
from basic to advanced. Each example is runnable and verified against the
current compiler.

**Verified against commit:** `0e8fd58`

## Running an example

```bash
nulang examples/NN_name.nula
```

## Examples

| # | File | Description | Command |
|---|------|-------------|---------|
| 01 | `01_hello.nula` | IO.print, string literals, basic expressions | `nulang examples/01_hello.nula` |
| 02 | `02_arithmetic.nula` | Let bindings, arithmetic, comparisons, booleans, type annotations | `nulang examples/02_arithmetic.nula` |
| 03 | `03_functions.nula` | Closures, multi-arg functions, recursion (factorial, fibonacci), blocks | `nulang examples/03_functions.nula` |
| 04 | `04_pattern_match.nula` | Match on literals, variants, tuples, records, guards, alias patterns | `nulang examples/04_pattern_match.nula` |
| 05 | `05_records.nula` | Record construction, field access, mutation, nested records | `nulang examples/05_records.nula` |
| 06 | `06_higher_order.nula` | Higher-order functions, composition, closure factories, recursion | `nulang examples/06_higher_order.nula` |
| 07 | `07_effects.nula` | Algebraic effects: perform, handle, String.length, Int.to_string | `nulang examples/07_effects.nula` |
| 08 | `08_actors.nula` | Actor declaration, spawn, message passing with `!`, behaviors with IO | `nulang examples/08_actors.nula` |
| 09 | `09_loops.nula` | While loops, for-in loops, break with/without values, nested loops | `nulang examples/09_loops.nula` |
| 10 | `10_pipe.nula` | Pipe operator `|>`, chaining transformations, closures in pipelines | `nulang examples/10_pipe.nula` |
| 11 | `11_arrays.nula` | Array literals, indexing, element mutation, array-based algorithms | `nulang examples/11_arrays.nula` |

## Notes

- Use `//` for line comments. `--` is NOT a standalone line comment (only valid
  after expressions on the same line, e.g. `let x = 1 -- inline`).
- The em dash character `—` (U+2014) is NOT accepted by the lexer.
- `let` bindings are immutable; use array slots or record fields for mutable
  state in loops and algorithms.
- Actors must appear before `let` bindings at the top level.
- `spawn Actor { ... }` field initializers are currently ignored; state always
  uses declared defaults.
- Top-level `fn` declarations with `match` on variant types may fail
  exhaustiveness checking when an `actor` block is present in the same file.
