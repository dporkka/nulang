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

    # 6. Check main.rs or compiler.rs for JIT/Escape Analysis integration
    # We want to verify that escape_analysis and JIT are actually used.
    integrated_escape = False
    integrated_jit = False
    for filename in ["src/main.rs", "src/compiler.rs", "src/vm.rs"]:
        if os.path.exists(filename):
            with open(filename, "r", encoding="utf-8") as f:
                content = f.read()
                if "EscapeAnalyzer" in content or "escape_analysis" in content:
                    integrated_escape = True
                if "tiered_execute_step" in content or "jit" in content:
                    # check if JIT is wired in VM loop/main
                    integrated_jit = True
                    
    # Wait, we need the team to integrate it, so let's check for these integration keywords.
    # Note: We'll make sure the agent integrates them.
    # If not integrated, print warning or error
    if not integrated_escape:
        print("Error: EscapeAnalyzer is not integrated into compiler/runtime pipeline.")
        return False
    if not integrated_jit:
        print("Error: JIT/tiered_execute_step is not integrated into compiler/runtime pipeline.")
        return False

    print("Success: All files passed implementation checks!")
    return True

if __name__ == "__main__":
    if verify_files() and run_tests():
        sys.exit(0)
    else:
        sys.exit(1)
