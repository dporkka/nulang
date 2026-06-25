// vm.rs - see local commit for full implementation (60,769 chars)
// v0.7: Implemented 7 VM opcodes for BEAM primitives:
// - Receive: Pattern-matching mailbox receive with optional timeout
// - Monitor/Demon: Actor observation with DOWN message delivery
// - Link/Unlink: Bidirectional fault propagation
// - Exit: Typed actor exit with ExitReason
// - Yield: Cooperative scheduler yielding
// All opcodes have full rustdoc documentation.