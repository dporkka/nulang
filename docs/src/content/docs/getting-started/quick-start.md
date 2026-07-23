---
title: Quick Start
description: Write and run your first Nulang programs — from Hello World to distributed actors.
---

## Hello World

Start the REPL:

```bash
cargo run -- --repl
```

```nulang
nulang> perform IO.print("Hello, Nulang!")
Hello, Nulang!
```

Or evaluate from the command line:

```bash
cargo run -- --eval 'perform IO.print("Hello, Nulang!")'
```

## Variables and Functions

```nulang
let x = 42
let y = x + 8

fn greet(name: String) -> String {
    "Hello, " + name
}

perform IO.print(greet("World"))
```

## Records and Pattern Matching

```nulang
type Person = { name: String, age: Int }

fn describe(p: Person) -> String {
    match p {
        { name: n, age: a } if a < 18 => n + " is young",
        { name: n, age: a } => n + " is " + perform Int.to_string(a)
    }
}

let alice = { name: "Alice", age: 30 }
perform IO.print(describe(alice))
```

## Actors — The Heart of Nulang

```nulang
actor Counter {
    state count: Int = 0

    behavior inc() {
        self.count = self.count + 1
    }

    behavior get(sender: Actor) {
        send sender reply(self.count)
    }
}

actor Main {
    behavior run() {
        let counter = spawn Counter {} in {
            send counter inc()
            send counter inc()
            send counter get(self)
        }
    }

    behavior reply(value: Int) {
        perform IO.print("Count is: " + Int.to_string(value))
    }
}

spawn Main {} in {}
```

## Algebraic Effects

```nulang
effect Logger {
    log: (String) -> Unit
}

fn greet_with_log(name: String) {
    perform Logger.log("Greeting " + name)
    perform IO.print("Hello, " + name)
}

handle greet_with_log("World") {
    | Logger.log(msg) resume => {
        perform IO.print("[LOG] " + msg)
    }
}
```

## Next

- Dive into [Syntax Basics](/language/syntax/)
- Explore the [Type System](/language/types/)
- Learn about [Algebraic Effects](/language/effects/)
- Build [Distributed Actors](/actors/overview/)
