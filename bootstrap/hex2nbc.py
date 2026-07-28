#!/usr/bin/env python3
"""hex2nbc.py — Convert hex bytecode text to .nbc binary.

Reads hex text output from fixup_hex.py (or compile_hex.nula) and writes
a .nbc binary file that the Nulang VM can load and run.

Usage:
  nulang bootstrap/compile_hex.nula < expr | python3 bootstrap/fixup_hex.py | python3 bootstrap/hex2nbc.py > out.nbc
"""

import sys, re, json, struct


def main():
    lines = sys.stdin.readlines()

    # Parse hex instructions and constant pool
    instructions = []
    pool = []
    for line in lines:
        s = line.strip()
        if not s: continue
        if s.startswith("; constant pool:"):
            # Parse "; constant pool: [1, 2, 3]"
            try:
                inside = s.split("[", 1)[1].rsplit("]", 1)[0]
                pool = [int(x.strip()) for x in inside.split(",") if x.strip()]
            except: pass
        elif re.match(r'^[0-9a-fA-F]{8}$', s):
            instructions.append(int(s, 16))

    if not instructions:
        print("Error: no hex instructions found", file=sys.stderr)
        sys.exit(1)

    # Build constant pool JSON
    consts = [{"Int": v} for v in pool]

    # Build .nbc binary
    magic = b"NLBC"
    header = struct.pack(">4sII32sI", magic, 1, 1, b"\x00" * 32, len(instructions))
    sys.stdout.buffer.write(header)

    for w in instructions:
        sys.stdout.buffer.write(struct.pack(">I", w))

    meta = {
        "name": "main", "constants": consts, "instructions": [],
        "behaviors": [], "function_table": [0], "exports": [],
        "entry_point": None, "handler_tables": [], "actor_metadata": [],
        "foreign_functions": [], "tools": [],
    }
    mb = json.dumps(meta).encode()
    sys.stdout.buffer.write(struct.pack(">I", len(mb)))
    sys.stdout.buffer.write(mb)


if __name__ == "__main__":
    main()
