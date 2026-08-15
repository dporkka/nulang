# The Killer Demo — Supervised Durable Chat Server

**Validation status: validated against `spec/grammar.ebnf`, SPEC2, and the
conformance suite patterns; not yet executed.** Every construct used below is
exercised by an existing conformance test or verified example
(`conformance/behavior/org_03_*`, `actor_09_supervisor_one_for_one_restart`,
`persist_01_*`, `examples/chat_room.nula`, `examples/supervisor_tree.nula`).
See "Execution status" at the bottom before recording.

## What the demo shows

1. An event-sourced `entity` (durable actor) used as a chat room.
2. Clients posting messages through it — state accumulates in the journal.
3. The room actor is deliberately crashed (`Actor.exit(1)`).
4. The OTP supervisor (one-for-one, permanent) restarts it — and its state
   is rebuilt from the event journal, not reset. `count` after the crash
   includes everything posted before it.
5. That this took zero error-handling code.

## `chat_server.nula` (~100 lines)

```nulang
// chat_server.nula — a supervised, durable chat room.
//
// The room is an `entity`: an actor that is event-sourced by default.
// Every posted message is emitted as an event into the room's journal.
// When the supervisor restarts the room after a crash, the runtime
// replays the journal and rebuilds the state — no recovery code required.
//
// Run with: nulang chat_server.nula

entity ChatRoom {
    // Event-sourced by default (no `local` marker): recovered from the
    // event journal on restart.
    state messages: Int = 0
    state last_sender: String = "nobody"

    events
        | MessagePosted(sender: String, text: String)
        | SenderSeen(sender: String)

    apply
        | MessagePosted(sender, text) => self.messages = self.messages + 1
        | SenderSeen(sender) => self.last_sender = sender

    behavior post(sender: String, text: String) {
        emit MessagePosted(sender, text)
        emit SenderSeen(sender)
        perform IO.print("[" + sender + "]: " + text)
        self.messages
    }

    behavior count() {
        self.messages
    }

    behavior last() {
        self.last_sender
    }

    // The controlled crash: an abnormal exit, exactly like a real failure.
    behavior crash() {
        perform IO.print("[room] crashing...")
        perform Actor.exit(1)
    }
}

actor ChatClient {
    state local name: String = ""
    state local room_messages: Int = 0

    behavior join(client_name: String) {
        self.name = client_name
        perform IO.print(client_name + " joined")
    }

    behavior leave() {
        perform IO.print(self.name + " left")
    }
}

fn main() {
    // A one-for-one supervisor (strategy 0), permanent restart policy (0):
    // any child that exits abnormally is restarted, by itself.
    let sup = perform Otp.create_supervisor("chat_sup", 0)

    // Spawn the room under a stable name, then supervise it.
    let room = spawn ChatRoom {} as "room:main"
    perform Otp.supervise_child(sup, room, 0)

    let alice = spawn ChatClient {}
    let bob = spawn ChatClient {}

    alice ! join("alice")
    bob ! join("bob")

    // A normal chat round.
    let n1 = ask room post("alice", "hello everyone!")
    let n2 = ask room post("bob", "hi alice!")
    perform IO.print(
        "messages so far: " + perform Int.to_string(ask room count())
    )

    // Kill the room. No try/catch, no checkpoint code — the crash is real.
    room ! crash()

    // The supervisor restarts the room; the runtime replays the journal.
    // Ask the restarted room for its state:
    perform IO.print(
        "messages after crash: " + perform Int.to_string(ask room count())
    )
    perform IO.print("last sender: " + ask room last())

    // Chat resumes as if nothing happened.
    let n3 = ask room post("alice", "did we just survive that?")
    perform IO.print(
        "final count: " + perform Int.to_string(ask room count())
    )

    alice ! leave()
    bob ! leave()
    0
}
```

### Expected output (projected from conformance-test semantics)

```
alice joined
bob joined
[alice]: hello everyone!
[bob]: hi alice!
messages so far: 2
[room] crashing...
messages after crash: 2
last sender: bob
[alice]: did we just survive that?
final count: 3
alice left
bob left
```

The load-bearing line is `messages after crash: 2` — the restarted room's
state came from the journal, not from the initializers.

> **If a live run shows `messages after crash: 0` or a dispatch anomaly, do
> not record around it.** Known dispatch caveats exist for behavior-name
> resolution (SPEC2 §8.5). If the run misbehaves, fall back to showing
> `conformance/behavior/actor_09_supervisor_one_for_one_restart.nula` (verified
> output `3\n30\n90`) and the persistence integration tests, and file the
> discrepancy as an issue. Never ship a demo output that wasn't observed.

## Part B — the kill -9 segment (be careful, be honest)

The dream demo is: post messages, `kill -9` the OS process, restart
`nulang chat_server.nula`, state intact. **Do not present this as working
today.** Current repo reality (verified in source):

- The `PersistenceStore` trait has three backends — in-memory, JSON file,
  SQLite (`src/runtime/persistence.rs`) — with file/SQLite recovery covered by
  Rust-level tests.
- The CLI runtime constructs `MemoryStore` (`src/runtime/mod.rs:516`); there
  is currently no CLI flag or env var selecting a file-backed store, so
  cross-*process* recovery is not user-reachable from `nulang file.nula`.

So for the recording, Part B is a **roadmap beat**, not a live demo:

- Say: "Crash recovery within a process you just saw. Cross-process recovery —
  surviving kill -9 — is implemented at the storage layer (JSON-file and
  SQLite backends with recovery tests) but isn't wired to a CLI flag yet;
  that's the next milestone."
- Optionally show `src/runtime/persistence.rs` tests
  (`JsonFileStore::new` round-trip tests around line 1300) as evidence the
  mechanism exists.
- If the CLI wiring lands before launch, replace this section with the real
  kill -9 recording and update this file.

## Recording instructions

**Tool:** asciinema (preferred — text is copy-pasteable, embeds cleanly on
GitHub) with agg or a GIF conversion for social. Terminalizer is an
acceptable alternative.

```bash
# 80x24, large font, clean PS1 like '$ ', dark theme.
asciinema rec --cols 100 --rows 30 -i 1.5 nulang-demo.cast
# In the recording:
$ cat chat_server.nula            # briefly — scroll with bat/nl if long
$ nulang --check chat_server.nula # type-check only: shows HM inference
$ nulang chat_server.nula         # the run: chat, crash, recovery
asciinema upload nulang-demo.cast
```

Pacing: pause 1–2 s after the crash line and after `messages after crash: 2`
— those are the moments. Total runtime target: 60–90 seconds. Record a second
take with `NULANG_SHARDS=4` set only if someone asks about parallelism; don't
clutter take one.

GIF for README/social: `agg nulang-demo.cast nulang-demo.gif` (asciinema's
agg) at 1000px width, or terminalizer's `render`. Keep the GIF under ~5 MB.

## Talking points (while the demo runs)

1. "This is one file. No framework imports — supervision and durability are
   runtime and language features."
2. "`entity` means event-sourced by default. The `emit` is the only
   persistence code in the program."
3. "The type system is checking all of this — no annotations anywhere, full
   inference. `--check` runs the HM typechecker without executing."
4. "The crash is a real abnormal exit. The supervisor's restart policy is the
   only fault-tolerance code, and it's one line."
5. "The restarted room got its state back by replaying its journal. On the
   BEAM, a restarted process starts fresh; here, recovery is the runtime's
   job."
6. Honesty beat: "What you saw is within-process recovery. kill -9 recovery
   needs the file-backed store wired to the CLI — implemented, tested, not
   yet exposed. Alpha means alpha."

## Execution status

- [ ] Build compiler (`cargo build --no-default-features`, Rust 1.95.0)
- [ ] Run `nulang --check chat_server.nula` and `nulang chat_server.nula`
- [ ] Diff observed output against the expected output above; update this
      file with the observed output verbatim
- [ ] Only then record

(Updated by whoever executes it; delete this checklist once verified.)
