#!/usr/bin/env python3
"""fixup_hex.py — Patch placeholder offsets and build constant pool.

Reads the marked hex output from compile_hex.nula (stdin) and produces
corrected hex output (stdout) with proper JmpF/Jmp offsets and ConstU indices.

Markers:
  ; JmpF -> else   — next line is JmpF, target = line after ; else:
  ; Jmp -> end     — next line is Jmp, target = ; end: or end of input
  ; then: / else: / end: — branch boundaries
  ; const N        — next ConstU loads constant value N
"""

import sys
import re


def parse_hex(s: str) -> int:
    return int(s.strip(), 16)


def format_hex(w: int) -> str:
    return f"{w & 0xFFFFFFFF:08x}"


def instr(opcode: int, op1: int, op2: int, op3: int) -> int:
    return (opcode << 24) | (op1 << 16) | (op2 << 8) | op3


def patch_jmpf(word: int, offset: int) -> int:
    cond = (word >> 16) & 0xFF
    return instr(0x52, cond, (offset >> 8) & 0xFF, offset & 0xFF)


def patch_jmp(word: int, offset: int) -> int:
    return instr(0x50, (offset >> 8) & 0xFF, offset & 0xFF, 0)


def patch_constu(word: int, idx: int) -> int:
    dst = word & 0xFF
    return instr(0x07, (idx >> 8) & 0xFF, idx & 0xFF, dst)


def fixup(lines: list[str]) -> list[str]:
    # First pass: collect instructions and markers
    instr_lines = []  # (line_idx, word)
    markers = {}      # line_idx -> marker_text
    
    for i, line in enumerate(lines):
        s = line.strip()
        if not s:
            continue
        if s.startswith(";"):
            markers[i] = s
        elif re.match(r'^[0-9a-fA-F]{8}$', s):
            instr_lines.append((i, parse_hex(s)))
    
    line_to_ic = {li: ic for ic, (li, _) in enumerate(instr_lines)}
    
    # Find jump markers and their instruction positions
    jmpf_info = []  # (line_idx, ic)
    jmp_info = []   # (line_idx, ic)
    
    for li, marker in sorted(markers.items()):
        if "JmpF" in marker:
            for check_li in range(li + 1, len(lines)):
                if check_li in line_to_ic:
                    jmpf_info.append((check_li, line_to_ic[check_li]))
                    break
        elif marker in ("; Jmp -> end", "; Jmp -> or_end", "; Jmp -> and_end", "; Jmp -> fn_end"):
            for check_li in range(li + 1, len(lines)):
                if check_li in line_to_ic:
                    jmp_info.append((check_li, line_to_ic[check_li]))
                    break
    
    else_markers  = [li for li, m in markers.items() if m.startswith("; else:")]
    end_markers   = [li for li, m in markers.items() if m.startswith("; end:")]
    and_end_markers = [li for li, m in markers.items() if m.startswith("; and_end:")]
    or_right_markers = [li for li, m in markers.items() if m.startswith("; or_right:")]
    or_end_markers = [li for li, m in markers.items() if m.startswith("; or_end:")]
    fn_end_markers = [li for li, m in markers.items() if m.startswith("; fn_end:")]
    fn_start_markers = [li for li, m in markers.items() if m.startswith("; FN_START")]
    
    patched = {}  # line_idx -> new word
    
    
    # Patch JmpF offsets (targets: else, and_end, or_right)
    for jf_li, jf_ic in reversed(jmpf_info):
        target_ic = None
        # Check marker after this JmpF to determine target type
        marker_li = jf_li - 1  # marker is just before the instruction
        marker = markers.get(marker_li, "")
        
        if "and_end" in marker:
            for em in and_end_markers:
                if em > jf_li:
                    for check_li in range(em + 1, len(lines)):
                        if check_li in line_to_ic:
                            target_ic = line_to_ic[check_li]
                            break
                    break
        elif "or_right" in marker:
            for em in or_right_markers:
                if em > jf_li:
                    for check_li in range(em + 1, len(lines)):
                        if check_li in line_to_ic:
                            target_ic = line_to_ic[check_li]
                            break
                    break
        else:
            # Default: JmpF -> else
            for em in else_markers:
                if em > jf_li:
                    for check_li in range(em + 1, len(lines)):
                        if check_li in line_to_ic:
                            target_ic = line_to_ic[check_li]
                            break
                    break
        if target_ic is not None:
            offset = target_ic - jf_ic
            old_word = [w for li, w in instr_lines if li == jf_li][0]
            patched[jf_li] = patch_jmpf(old_word, offset)
    
    # Patch Jmp offsets (check ; fn_end: then ; end: then ; or_end: then ; else: then end of list)
    for jp_li, jp_ic in reversed(jmp_info):
        target_ic = None
        marker_text = markers.get(jp_li - 1, "")
        if "fn_end" in marker_text:
            for em in fn_end_markers:
                if em > jp_li:
                    for check_li in range(em + 1, len(lines)):
                        if check_li in line_to_ic:
                            target_ic = line_to_ic[check_li]
                            break
                    break
        if target_ic is None:
            for em in end_markers:
                if em > jp_li:
                    for check_li in range(em + 1, len(lines)):
                        if check_li in line_to_ic:
                            target_ic = line_to_ic[check_li]
                        break
                break
        if target_ic is None:
            for em in or_end_markers:
                if em > jp_li:
                    for check_li in range(em + 1, len(lines)):
                        if check_li in line_to_ic:
                            target_ic = line_to_ic[check_li]
                            break
                    break
        if target_ic is None:
            for em in else_markers:
                if em > jp_li:
                    for check_li in range(em + 1, len(lines)):
                        if check_li in line_to_ic:
                            target_ic = line_to_ic[check_li]
                            break
                    break
        if target_ic is None:
            target_ic = len(instr_lines)
        if target_ic is not None:
            offset = target_ic - jp_ic
            old_word = [w for li, w in instr_lines if li == jp_li][0]
            patched[jp_li] = patch_jmp(old_word, offset)
    
    # Patch Closure function indices from FN_START markers
    fn_indices = {}
    for idx, li in enumerate(sorted(fn_start_markers)):
        fn_indices[li] = idx + 1  # 0 is entry point
    
    for fn_li, fn_idx in sorted(fn_indices.items()):
        for em in fn_end_markers:
            if em > fn_li:
                for check_li in range(em + 1, len(lines)):
                    if check_li in line_to_ic:
                        word = [w for cli, w in instr_lines if cli == check_li][0]
                        if ((word >> 24) & 0xFF) == 0x60:
                            dst = word & 0xFF
                            patched[check_li] = instr(0x60, (fn_idx >> 8) & 0xFF, fn_idx & 0xFF, dst)
                        break
                break

    # Build constant pool from ; const N markers
    const_markers = {}  # line_idx -> value
    for li, marker in markers.items():
        if marker.startswith("; const "):
            try:
                val = int(marker.split("; const ")[1])
                const_markers[li] = val
            except ValueError:
                pass
    
    pool_lines = []
    if const_markers:
        seen = set()
        pool = []
        for li in sorted(const_markers):
            val = const_markers[li]
            if val not in seen:
                seen.add(val)
                pool.append(val)
        
        val_to_idx = {v: i for i, v in enumerate(pool)}
        
        for li, val in sorted(const_markers.items()):
            for check_li in range(li + 1, len(lines)):
                if check_li in line_to_ic:
                    word = [w for cli, w in instr_lines if cli == check_li][0]
                    if ((word >> 24) & 0xFF) == 0x07:
                        patched[check_li] = patch_constu(word, val_to_idx[val])
                    break
        
        pool_lines = ["; constant pool: [" + ", ".join(str(v) for v in pool) + "]"]
    
    # Build output with pool header
    result = []
    inserted_pool = False
    for i, line in enumerate(lines):
        if i in patched:
            result.append(format_hex(patched[i]))
        else:
            result.append(line.rstrip())
        # Insert pool after first non-const comment
        if not inserted_pool and pool_lines and line.strip().startswith(";") and "const" not in line:
            for pline in pool_lines:
                result.append(pline)
            inserted_pool = True
    
    return result


def main():
    lines = sys.stdin.readlines()
    result = fixup(lines)
    for line in result:
        print(line)


if __name__ == "__main__":
    main()
