# Common Pitfalls & Idioms

The Nulang syntax is clean and consistent once you know the rules, but
newcomers reliably trip on the same few things.  Each section below is
a one-line rule, a ❌ wrong snippet, and a ✅ correct one — all verified
against `examples/`, `docs/GETTING_STARTED.md`, and the integration-test
suite.

---

### 1. Imports use `::`, not `.`

Module paths use `::` as separator.  Stdlib modules live under
`stdlib::*` and are resolved via `NULANG_STDLIB` (or `src/stdlib/` in
development).

```nula
// ❌
import stdlib.list

// ✅
import stdlib::list
```

Available stdlib modules: `list`, `string`, `test`, `fs`, `map`,
`math`, `json`, `set`, `http`, `option`, `result`, `datetime`, `core`.

---

### 2. Imported names are unqualified

`import stdlib::test` brings every `pub` declaration into scope
*without* a module prefix.  Call them directly — no `test.` qualifier.

```nula
import stdlib::test

// ❌
test.assert_eq(40 + 2, 42)

// ✅
assert_eq(40 + 2, 42)
```

> **Built-in effects don't need an import at all.**  You can write
> `perform Test.assert_eq(a, b)` or `perform FS.read(path)` directly,
> without any `import`.  The `Test`, `FS`, `IO`, `String`, and `Int`
> effects are wired into the VM.

---

### 3. Built-in effects are called with `perform`

Every built-in effect operation requires the `perform` keyword.  These
are VM-wired — they work without an `import` statement.

```nula
// ❌
IO.print("hello")
FS.read("data.txt")
Test.assert_eq(40 + 2, 42)

// ✅
perform IO.print("hello")
perform FS.read("data.txt")
perform Test.assert_eq(40 + 2, 42)
perform Int.to_string(42)
perform String.length("hello")
```

---

### 4. `let` is immutable; `var` is mutable

`let` bindings cannot be reassigned.  Use `var` when you need mutation.

```nula
// ❌
let x = 0
x = 1                // error: x is immutable

// ✅
let x = 0            // immutable — fine if never reassigned
var x = 0            // mutable
x = x + 1            // ok
```

---

### 5. Comments are `//`, not `--`

Em dashes (`—`) are rejected by the lexer.  Only `//` to end-of-line
and `/* ... */` block comments are accepted.

```nula
// ❌
-- this is not a comment
// ✅
// this is a comment
```

---

### 6. Record literals use `:` — record updates use `=`

Field definitions in a literal are `field: value`.  When updating an
existing record (`{ base .. overrides }`), overrides use `=` — the colons
come from the *base* record.

```nula
// ❌
let p = { x = 1, y = 2 }          // literals need `:`
let q = { p .. y: 9 }            // overrides need `=`

// ✅
let p = { x: 1, y: 2 }           // literal
let q = { p .. y = 9 }           // override y, keep x from p
let r = { p .. x = 10, y = 20 }  // multiple overrides
```

---

### 7. `spawn` field overrides use `=`

When spawning an actor, state-field overrides use `FieldName = value`
(not `:`).  This overrides the default declared in the actor body.

```nula
actor Counter {
    state count: Int = 0
    behavior add(n: Int) { self.count = self.count + n }
}

// ❌
let c = spawn Counter { count: 42 }

// ✅
let c = spawn Counter { count = 42 }
```

---

### 8. Message sends use `!`, not `.`

Sending a message to an actor requires the `!` operator.
The dotted form (`actor.field`) is field *access*, not a send.

```nula
// ❌
counter.increment(5)       // field access, not a message send

// ✅
counter ! increment(5)     // send the `increment` message
```

---

### 9. `let … in` scopes to the body; a block-`let` scopes to the rest of the block

**Expression form:** `let x = V in BODY` — `x` is visible *only* in
`BODY`, not after the `in`.

**Statement form:** `let x = V` (without `in`, inside a block) —
`x` is visible for the remainder of the block.

```nula
// ✅ expression form: x scoped to x + 5 only
let result = let x = 10 in x + 5
// x is NOT visible here

// ✅ statement form: x visible for rest of block
{
    let x = 10
    let y = x + 5     // x is in scope
    y
}

// ❌ expression form without `in`
{
    let x = 10         // statement-form, scopes to end of block
    x + 5
}
// ^ this is actually fine — it's using the statement form.
// The real gotcha is mixing them up:
let x = 1 in {
    let x = 10 in 0    // inner x shadows outer, scoped to `0` only
    x                   // → 1  (the outer x!)
}
```

---

### 10. `if` uses `then` / `else`

Conditions don't need parentheses, and the branches are separated by
`then` and `else`.

```nula
// ❌
if n <= 1 { 1 } else { n * factorial(n - 1) }

// ✅
if n <= 1 then 1 else n * factorial(n - 1)
```

---

### 11. `match` arms are `|`-prefixed; guards use `if`

Every arm starts with `|`.  Guards attach to a bound variable —
a guard variable must be *bound* by its pattern.

```nula
// ❌
match n with {
    x if x < 0 => "negative"       // missing leading `|`
    _ if x < 0 => "negative"       // `_` does NOT bind `x`
}

// ✅
match n with {
    | x if x < 0  => "negative"
    | x if x == 0 => "zero"
    | _           => "non-negative"
}
```

---

### 12. `**` is right-associative and binds tighter than `*`

Exponentiation groups right-to-left and has higher precedence than
multiplication.

```nula
2 ** 3 ** 2    // → 2 ** (3 ** 2) = 2 ** 9 = 512  (right-associative)
2 * 3 ** 2     // → 2 * (3 ** 2)  = 2 * 9  = 18   (tighter than *)
```

---

### 13. Multi-line strings use `"""`; unicode via `\u{…}`

Triple-quoted strings span multiple lines.  Unicode escapes use the
`\u{hex}` form (not `\uXXXX`).

```nula
// ❌
let s = "line1\nline2"
let smile = "\u1F600"

// ✅
let s = """line1
line2"""
let smile = "\u{1F600}"          // 😀
```

---

### 14. `catch` (postfix or prefix) and `fail`

Both forms of `catch` desugar to a `match` on `Ok`/`Error` variants.
`fail` produces an early-exit `Error` value.

```nula
type Result[Ok, Err] = Ok(Ok) | Error(Err)

// Postfix catch
ok_val() catch 0                  // Ok(42)  → 42
err_val() catch 0                 // Error(_) → 0

// Prefix catch (same semantics)
catch ok_val() 0                  // → 42
catch err_val() 0                 // → 0

// fail: early exit from a function
fn div(a: Int, b: Int) -> Int ! String {
    if b == 0 then fail Error("div by zero") else Ok(a / b)
}
```

> `?` unwraps: `div(10, 2)?` → `5`.

---

## Quick Idioms

| Idiom | Snippet |
|-------|---------|
| Pipe (`\|>`) | `41 \|> fn(n) { n + 1 }` |
| Recursive closures | `let rec fib = fn(n) { … }` |
| Block expression | `{ let a = 10; let b = 20; a + b }` |
| Alias pattern | `\| s @ Some(x) => …` |
| Type annotation | `let x: Int = 42` |
| Function type param | `fn map[T, U](arr: [T], f: fn(T) -> U) -> [U]` |
| Effect annotation | `fn div(a: Int, b: Int) -> Int ! String` |
| For loop | `for i in [1, 2, 3] { perform IO.print(i) }` |
| While loop | `while i < n { i = i + 1 }` |
| Array literal | `[1, 2, 3]` — `.len()`, `[i]`, `.push(v)` |
| Tuple | `(1, "hello")` |
