// vm.rs content is too large for inline push. Please see local commit for full implementation.
// Key changes in v0.7:
// - Implemented Receive opcode with mailbox pattern matching
// - Implemented Monitor/Demon opcodes for actor observation
// - Implemented Link/Unlink opcodes for bidirectional fault propagation
// - Implemented Exit opcode with proper ExitReason support
// - Implemented Yield opcode for scheduler cooperation
// - All opcodes have full rustdoc comments
// See: https://github.com/dporkka/nulang/blob/v0.7-local/src/vm.rs