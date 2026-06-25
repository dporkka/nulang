# v0.7 Push Instructions

The full v0.7 implementation is in local commit `1c2cde9` at `/mnt/agents/output/nulang-impl/`.

To push to GitHub, run:
```bash
cd /mnt/agents/output/nulang-impl
git push origin main
```

## Files Changed (15 files, +7,091 lines)

### New Files
- `src/runtime/process_groups.rs` — Process groups (Erlang pg), 323 lines
- `src/runtime/registry.rs` — Actor name registry, 453 lines
- `src/runtime/timer.rs` — Hierarchical timer wheel, 438 lines

### Modified Files
- `src/lib.rs` — Fixed module declarations
- `src/types.rs` — Added ExitReason enum
- `src/vm.rs` — Implemented 7 BEAM opcodes (Receive, Monitor, Demon, Link, Unlink, Exit, Yield)
- `src/runtime/actor.rs` — Added exit_reason field
- `src/runtime/mod.rs` — Integrated timer, registry, process_groups into Runtime
- `src/runtime/supervisor.rs` — Updated for typed ExitReason
- `src/runtime/tests.rs` — Added 24 integration tests for BEAM primitives

### BEAM_PRIMITIVES.md
- Full adoption analysis of BEAM/OTP primitives
