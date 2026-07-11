// Receive — demonstrates the receive expression for actor message handling.
//
// STATUS (MVP): `receive` lexes, parses, and type-checks, but MIR lowering
// currently yields `nil` — the arms are not yet dispatched, so this program
// compiles and runs but the receive expression always evaluates to nil.
// See README.md "Known limitations".
//
// Run with: nulang examples/receive.nu

actor Echo {
    behavior respond() {
        // Planned semantics: pop the next message from the mailbox and
        // return its first payload value (nil if the mailbox is empty).
        // Today this evaluates to nil regardless of mailbox contents.
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
