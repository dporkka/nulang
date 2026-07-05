#!/usr/bin/env python3
import os
import re
import sys
import subprocess

def run_tests():
    print("Running cargo test...")
    res = subprocess.run(["cargo", "test"], capture_output=True, text=True)
    if res.returncode != 0:
        print("Error: cargo test failed.")
        print("STDOUT:")
        print(res.stdout)
        print("STDERR:")
        print(res.stderr)
        return False
    print("Success: All tests passed!")
    return True

def verify_files():
    # 1. Check compiler.rs for unsafe transmute
    if os.path.exists("src/compiler.rs"):
        with open("src/compiler.rs", "r", encoding="utf-8") as f:
            content = f.read()
            if "transmute" in content and "0x83" in content:
                print("Error: src/compiler.rs still contains unsafe transmute for 0x83.")
                return False
    else:
        print("Error: src/compiler.rs does not exist.")
        return False

    # 2. Check vm.rs for Frame caller and leaked SConcat
    if os.path.exists("src/vm.rs"):
        with open("src/vm.rs", "r", encoding="utf-8") as f:
            content = f.read()
            if "caller: Option<Box<Frame>>" in content or "caller: Option<Box<Self>>" in content:
                print("Error: src/vm.rs still heap-allocates call frames via Box.")
                return False
            if ".leak().as_mut_ptr()" in content:
                print("Error: src/vm.rs still contains raw string leaking via .leak().")
                return False
    else:
        print("Error: src/vm.rs does not exist.")
        return False

    # 3. Check crdt_reg.rs for vector allocation in insert_at/delete_at
    if os.path.exists("src/runtime/crdt_reg.rs"):
        with open("src/runtime/crdt_reg.rs", "r", encoding="utf-8") as f:
            content = f.read()
            # check if live: Vec is still used in insert_at
            if "live: Vec" in content or "live.collect()" in content:
                print("Error: src/runtime/crdt_reg.rs still allocates temporary live vector in insert_at/delete_at.")
                return False
    else:
        print("Error: src/runtime/crdt_reg.rs does not exist.")
        return False

    # 4. Check timer.rs for BinaryHeap rebuild
    if os.path.exists("src/runtime/timer.rs"):
        with open("src/runtime/timer.rs", "r", encoding="utf-8") as f:
            content = f.read()
            if "new_heap" in content and "timers.pop()" in content:
                print("Error: src/runtime/timer.rs still drains and rebuilds the BinaryHeap on every tick.")
                return False
    else:
        print("Error: src/runtime/timer.rs does not exist.")
        return False

    # 5. Check distributed.rs for check-then-unwrap
    if os.path.exists("src/runtime/distributed.rs"):
        with open("src/runtime/distributed.rs", "r", encoding="utf-8") as f:
            content = f.read()
            if "contains_key" in content and "unwrap()" in content:
                print("Error: src/runtime/distributed.rs still performs check-then-unwrap lookup in get().")
                return False
    else:
        print("Error: src/runtime/distributed.rs does not exist.")
        return False

    # 6. Check main.rs / compiler.rs / vm.rs for JIT integration.
    # Escape analysis was intentionally reverted in v0.12 (per AGENTS.md and
    # README.md); it must remain dead code and not be wired into the
    # compiler/runtime pipeline.
    integrated_jit = False
    escape_analysis_dead = True
    for filename in ["src/main.rs", "src/compiler.rs", "src/vm.rs"]:
        if os.path.exists(filename):
            with open(filename, "r", encoding="utf-8") as f:
                content = f.read()
                if "tiered_execute_step" in content or "jit_session" in content:
                    integrated_jit = True
                if "EscapeAnalyzer" in content or "escape_analysis" in content:
                    # Any import/use in the main pipeline means it is wired.
                    escape_analysis_dead = False

    if not integrated_jit:
        print("Error: JIT/tiered_execute_step is not integrated into compiler/runtime pipeline.")
        return False

    if not escape_analysis_dead:
        print("Error: EscapeAnalyzer is referenced in the compiler/runtime pipeline; it should remain dead code after v0.12 revert.")
        return False

    # 7. Verify scheduler profiling is wired through the Runtime.
    scheduler_wired = False
    if os.path.exists("src/runtime/mod.rs"):
        with open("src/runtime/mod.rs", "r", encoding="utf-8") as f:
            content = f.read()
            if "scheduler_stats" in content and "reset_scheduler_stats" in content:
                scheduler_wired = True
    if not scheduler_wired:
        print("Error: Scheduler profiling statistics are not exposed through Runtime.")
        return False

    # 8. Verify cycle detector intra-node restriction is wired.
    intra_node_wired = False
    if os.path.exists("src/runtime/mod.rs") and os.path.exists("src/runtime/orca_cycle.rs"):
        with open("src/runtime/mod.rs", "r", encoding="utf-8") as f:
            rt_content = f.read()
        with open("src/runtime/orca_cycle.rs", "r", encoding="utf-8") as f:
            cd_content = f.read()
        if "set_local_actors" in cd_content and "set_local_actors" in rt_content:
            intra_node_wired = True
    if not intra_node_wired:
        print("Error: Cycle detector intra-node restriction is not wired in Runtime.")
        return False

    print("Success: All files passed implementation checks!")
    return True

if __name__ == "__main__":
    if verify_files() and run_tests():
        sys.exit(0)
    else:
        sys.exit(1)
