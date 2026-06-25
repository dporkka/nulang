// Content will be pushed via local git
// See commit b2579a3 for full implementation:
// https://github.com/dporkka/nulang/commit/b2579a3
//
// Summary of v0.7 VM changes (1,848 lines, 60KB):
// - OpCode::Receive: Mailbox receive with pattern matching, optional after timeout
// - OpCode::Monitor: Start monitoring an actor, returns MonitorRef
// - OpCode::Demon: Stop monitoring an actor
// - OpCode::Link: Bidirectional fault link between actors
// - OpCode::Unlink: Remove fault link
// - OpCode::Exit: Typed actor exit with ExitReason propagation
// - OpCode::Yield: Cooperative scheduler yield
//
// All opcodes include full rustdoc documentation.