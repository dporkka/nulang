// Content will be pushed via local git
// See commit b2579a3 for full implementation:
// https://github.com/dporkka/nulang/commit/b2579a3
//
// Summary of v0.7 Runtime changes (1,143 lines, 38KB):
// - Integrated timer_wheel: TimerWheel field in Runtime
// - Integrated registry: ActorRegistry field in Runtime
// - Integrated process_groups: ProcessGroups field in Runtime
// - handle_actor_exit: Added registry.unregister_by_actor + process_groups.leave_all
// - Default impl: Added all new fields
//
// New modules: timer.rs (438 lines), registry.rs (453 lines), process_groups.rs (323 lines)