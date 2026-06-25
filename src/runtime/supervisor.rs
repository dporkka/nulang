// Content will be pushed via local git
// See commit b2579a3 for full implementation:
// https://github.com/dporkka/nulang/commit/b2579a3
//
// Summary of v0.7 Supervisor changes (536 lines, 16KB):
// - Updated to use typed ExitReason from crate::types
// - handle_exit method uses ExitReason instead of String
// - SupervisorAction::RestartActor carries ExitReason
// - Integration with registry cleanup on child termination