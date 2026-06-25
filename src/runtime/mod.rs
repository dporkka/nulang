// v0.7 — Full 38KB implementation available in local commit 1c2cde9
// Run: cd /mnt/agents/output/nulang-impl && git show 1c2cde9:src/runtime/mod.rs
//
// Runtime with BEAM primitive integrations:
// - timer_wheel: TimerWheel for send_after/exit_after/kill_after
// - registry: ActorRegistry for register/whereis/registered
// - process_groups: ProcessGroups for join/leave/members
// - handle_actor_exit: cleanup via unregister_by_actor + leave_all
//
// Author: Nulang Developer <dev@nulang.dev>