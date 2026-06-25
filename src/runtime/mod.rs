// v0.7 Runtime with BEAM Primitive Integrations
// See local commit 1c2cde9 for full 38KB implementation
// Integrated: timer_wheel, registry, process_groups
// handle_actor_exit: registry.unregister_by_actor + process_groups.leave_all