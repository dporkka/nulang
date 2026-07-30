# Nulang Standard Library Reference

Covers the working stdlib modules as of commit `8f13bea`.
More modules (`string`, `set`, `map`, `core`) are being repaired.

## Using the Standard Library

Import a module with `import stdlib::<module>` (double-colon path separator):

```nula
import stdlib::math
import stdlib::list
import stdlib::test
import stdlib::fs
```

Functions are called **unqualified** — `abs(-5)`, not `math.abs`. With imports from
multiple modules, avoid name collisions (e.g. `list::max_of` vs `math::max`).

### Locating the stdlib

From outside the project root, set the environment variable pointing at the
stdlib tree:

```sh
export NULANG_STDLIB=/path/to/nulang/src/stdlib
```

### Built-in Effects

Built-in effects (`FS`, `Test`, `IO`, `Array`, `String`, `Int`) are invoked with
`perform` and need **no** import:

```nula
perform Array.push(arr, elem)
perform FS.read("data.txt")
perform Test.assert(cond, msg)
```

---

## Module: math

Integer and floating-point numeric functions. No effect constraints — all
functions are pure.

| Function | Signature | Description |
|---|---|---|
| `abs` | `fn abs(x: Int) -> Int` | Absolute value. |
| `min` | `fn min(a: Int, b: Int) -> Int` | Minimum of two integers. |
| `max` | `fn max(a: Int, b: Int) -> Int` | Maximum of two integers. |
| `clamp` | `fn clamp(x: Int, lo: Int, hi: Int) -> Int` | Clamp `x` to the range `[lo, hi]`. |
| `sign` | `fn sign(x: Int) -> Int` | Sign: `-1` for negative, `0` for zero, `1` for positive. |
| `pow` | `fn pow(base: Int, exp: Int) -> Int` | Integer exponentiation (base^exp) for non-negative exp. |
| `factorial` | `fn factorial(n: Int) -> Int` | Factorial. Returns 1 for `n <= 1`. |
| `gcd` | `fn gcd(a: Int, b: Int) -> Int` | Greatest common divisor (Euclidean algorithm). |
| `lcm` | `fn lcm(a: Int, b: Int) -> Int` | Least common multiple. |
| `is_even` | `fn is_even(n: Int) -> Bool` | True when `n` is even. |
| `is_odd` | `fn is_odd(n: Int) -> Bool` | True when `n` is odd. |
| `sqrt` | `fn sqrt(x: Float) -> Float` | Square root via Newton's method. Returns `-1.0` for negative input. |

### Usage

```nula
import stdlib::math

let a = abs(-5)          // 5
let m = clamp(12, 0, 10) // 10
let p = pow(2, 8)        // 256
let g = gcd(48, 18)      // 6
let r = sqrt(25.0)       // ~5.0
```

---

## Module: list

List/array operations over native arrays (`[Int]`). Traversal functions
(`length`, `sum`, `contains`, `index_of`, `any`, `all`, `fold`, `max_of`,
`min_of`) accept `[Int]` arrays.

Array-producing functions (`map`, `filter`, `reverse`, `take`, `drop`, `range`)
are backed by the `Array` builtin effect (`perform Array.push`, `perform
Array.new`, `perform Array.length`). These functions are polymorphic — they
accept and return arrays of any element type.

### Traversal functions

| Function | Signature | Description |
|---|---|---|
| `length` | `fn length(xs: [Int]) -> Int` | Number of elements in the array. |
| `sum` | `fn sum(xs: [Int]) -> Int` | Sum of all elements. |
| `contains` | `fn contains(x: Int, xs: [Int]) -> Bool` | True if `x` appears in `xs`. |
| `index_of` | `fn index_of(x: Int, xs: [Int]) -> Int` | First index of `x` in `xs`, or `-1` if not found. |
| `any` | `fn any(pred, xs: [Int]) -> Bool` | True if any element satisfies the predicate. |
| `all` | `fn all(pred, xs: [Int]) -> Bool` | True if all elements satisfy the predicate. |
| `fold` | `fn fold(f, init: Int, xs: [Int]) -> Int` | Left fold: `fold(f, init, [a,b,c])` = `f(f(f(init, a), b), c)`. |
| `max_of` | `fn max_of(xs: [Int]) -> Int` | Maximum element. Crashes for empty array. |
| `min_of` | `fn min_of(xs: [Int]) -> Int` | Minimum element. Crashes for empty array. |

### Array-producing functions

| Function | Signature | Description |
|---|---|---|
| `map` | `fn map(f, xs)` | Apply `f` to each element, return new array. |
| `filter` | `fn filter(pred, xs)` | Keep elements where `pred` returns true. |
| `reverse` | `fn reverse(xs)` | Return new array with elements in reverse order. |
| `take` | `fn take(n, xs)` | Take first `n` elements. |
| `drop` | `fn drop(n, xs)` | Drop first `n` elements. |
| `range` | `fn range(n)` | Array of `[0, 1, ..., n-1]`. |

### Usage

```nula
import stdlib::list

let xs = [1, 2, 3, 4, 5]

// Traversals
let s = sum(xs)                // 15
let c = contains(3, xs)        // true
let i = index_of(4, xs)        // 3
let y = any(fn(x) { x > 3 }, xs)   // true
let z = fold(fn(a, b) { a + b }, 0, xs)  // 15

// Array-producing
let doubled = map(fn(x) { x * 2 }, xs)  // [2, 4, 6, 8, 10]
let evens   = filter(fn(x) { is_even(x) }, xs)  // [2, 4]
let rev     = reverse(xs)        // [5, 4, 3, 2, 1]
let first3  = take(3, xs)        // [1, 2, 3]
let rest    = drop(2, xs)        // [3, 4, 5]
let r       = range(4)           // [0, 1, 2, 3]
```

---

## Module: test

Test assertions via the built-in `Test` effect. Assertion failures produce
runtime errors that abort execution and are reported by the `nula test` runner.

All functions are annotated `! {Test}`, meaning they can only be called from
contexts that handle the `Test` effect.

| Function | Signature | Description |
|---|---|---|
| `assert` | `fn assert(cond: Bool, msg: String) -> Unit ! {Test}` | Assert a boolean condition. On failure: `assertion failed: {msg}`. |
| `assert_eq` | `fn assert_eq(a: Int, b: Int) -> Unit ! {Test}` | Assert two integers are equal. On failure: `assertion failed: expected {b}, got {a}`. `a` = actual, `b` = expected. |
| `assert_true` | `fn assert_true(cond: Bool) -> Unit ! {Test}` | Assert a boolean condition is true. On failure: `assertion failed`. |
| `fail_with` | `fn fail_with(message: String) -> Unit ! {Test}` | Fail unconditionally with a message. Produces: `assertion failed: {message}`. |

### Usage

```nula
import stdlib::test

fn my_tests() {
    assert(1 + 1 == 2, "basic arithmetic failed")
    assert_eq(add(1, 2), 3)
    assert_true(meaning_of_life() == 42)
    // fail_with("not implemented yet")
}
```

---

## Module: fs

Filesystem I/O via the built-in `FS` effect. Operations are `perform FS.read`,
`perform FS.write`, `perform FS.append`, and `perform FS.exists`.

Effect annotations (`! {FS}`) mean callers must handle or propagate the `FS`
effect.

| Function | Signature | Description |
|---|---|---|
| `read` | `fn read(path: String) -> String ! {FS}` | Read entire file contents into a string. Returns nil if the file cannot be read. |
| `write` | `fn write(path: String, content: String) -> Unit ! {FS}` | Write a string to a file, overwriting existing content. Creates the file if needed. Returns nil on failure. |
| `append` | `fn append(path: String, content: String) -> Unit ! {FS}` | Append a string to the end of a file. Creates the file if needed. Returns nil on failure. |
| `exists` | `fn exists(path: String) -> Bool ! {FS}` | Check whether a file or directory exists at the given path. |

### Usage

```nula
import stdlib::fs

let content = read("data.txt")
write("output.txt", "Hello, world!")
append("log.txt", "another line\n")
let ok = exists("config.json")  // true or false
```
