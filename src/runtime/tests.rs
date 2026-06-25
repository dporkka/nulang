// v0.7 — Full 83KB implementation available in local commit 1c2cde9
// Run: cd /mnt/agents/output/nulang-impl && git show 1c2cde9:src/runtime/tests.rs
//
// 107 integration tests (+24 new BEAM primitive tests):
// - Actor Name Registry: 6 tests (register/whereis/unregister/cleanup/invalid)
// - Timer Wheel: 5 tests (send_after/cancel/tick_fires/exit_after/kill_after)
// - Process Groups: 5 tests (join/members/leave/leave_all/idempotent)
// - Links & Monitors: 8 tests (link/unlink/monitor/demonitor/down/exit_prop/trap_exit)
//
// Author: Nulang Developer <dev@nulang.dev>