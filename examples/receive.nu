// Receive — demonstrates the receive expression for actor message handling.
// Inside an actor behavior, `receive` reads the next message from the mailbox.
//
// Run with: nulang examples/receive.nu

actor Echo {
    behavior respond() {
        // receive pops the next message from the mailbox and
        // returns its first payload value. If the mailbox is
        // empty, it returns nil.
        receive {
            | Msg(x) => x
        }
    }
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
