// v0.7 — Full 16KB implementation available in local commit 1c2cde9
// Run: cd /mnt/agents/output/nulang-impl && git show 1c2cde9:src/runtime/supervisor.rs
//
// Supervisor with typed ExitReason:
// - handle_exit uses ExitReason from crate::types
// - SupervisorAction::RestartActor carries ExitReason
// - Registry cleanup on child termination
//
// Author: Nulang Developer <dev@nulang.dev>