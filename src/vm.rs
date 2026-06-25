// v0.7 — Full 60KB implementation available in local commit 1c2cde9
// Run: cd /mnt/agents/output/nulang-impl && git show 1c2cde9:src/vm.rs
//
// Implements 7 BEAM opcodes with full rustdoc:
// - Receive: Mailbox receive with pattern matching + after timeout
// - Monitor/Demon: Actor observation with DOWN notifications
// - Link/Unlink: Bidirectional fault propagation
// - Exit: Typed actor exit with ExitReason
// - Yield: Cooperative scheduler yield
//
// Author: Nulang Developer <dev@nulang.dev>