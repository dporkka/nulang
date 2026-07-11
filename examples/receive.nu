// Receive — demonstrates selective receive for actor message handling.
//
// `receive` scans the actor's mailbox in FIFO order for the first message
// whose behavior matches any arm, binds that message's payload values to
// the arm's params, and evaluates the arm body. Non-matching messages are
// skipped and stay queued. When nothing matches, the legacy non-blocking
// fallback runs: pop the next message and yield its first payload value
// (nil when the mailbox is empty).
//
// Run with: nulang examples/receive.nu

actor Echo {
    behavior respond() {
        // Selective receive: wait-free scan for a `msg` message; binds the
        // payload to `x`. Other queued messages are left in the mailbox.
        receive {
            | msg(x) => x
        }
    }
    behavior msg(x: Int) { x }
}

actor MainActor {
    behavior start() {
        // Spawn the echo actor and send it a message.
        let echo = spawn Echo {} in
        send echo respond();
        unit
    }
}

fn main() {
    let main = spawn MainActor {} in
    send main start();
    unit
}
