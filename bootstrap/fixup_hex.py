#!/usr/bin/env python3
"""fixup_hex.py — Patch placeholder jump offsets in hex bytecode output.

Reads the marked hex output from compile_hex.nula (stdin) and produces
corrected hex output (stdout) with proper JmpF/Jmp relative offsets.

Markers used by compile_hex.nula:
  ; JmpF -> else   — next line is JmpF, target is the line after ; else:
  ; Jmp -> end     — next line is Jmp, target is either ; end: or end of input
  ; then:          — then-branch starts here
  ; else:          — else-branch starts here (target of JmpF)
  ; end:           — optional explicit end marker
"""

import sys
import re


def parse_hex(s: str) -> int:
    """Parse 8-char hex string to integer."""
    return int(s.strip(), 16)


def format_hex(w: int) -> str:
    """Format integer as 8-char hex string."""
    return f"{w & 0xFFFFFFFF:08x}"


def instr(opcode: int, op1: int, op2: int, op3: int) -> int:
    """Build instruction word."""
    return (opcode << 24) | (op1 << 16) | (op2 << 8) | op3


def patch_jmpf(word: int, offset: int) -> int:
    """Patch JmpF instruction with correct offset. offset must fit in i16."""
    cond = (word >> 16) & 0xFF
    return instr(0x52, cond, (offset >> 8) & 0xFF, offset & 0xFF)


def patch_jmp(word: int, offset: int) -> int:
    """Patch Jmp instruction with correct offset."""
    return instr(0x50, (offset >> 8) & 0xFF, offset & 0xFF, 0)


def fixup(lines: list[str]) -> list[str]:
    """Process lines and return corrected lines with patched offsets."""
    # First pass: collect instruction lines and their indices
    instr_lines = []  # list of (line_index, instruction_word)
    markers = {}      # line_index -> marker_type
    
    for i, line in enumerate(lines):
        stripped = line.strip()
        if not stripped:
            continue
        if stripped.startswith(";"):
            markers[i] = stripped
        elif re.match(r'^[0-9a-fA-F]{8}$', stripped):
            instr_lines.append((i, parse_hex(stripped)))
    
    # Build instruction index map: original line -> instruction index
    line_to_ic = {}
    for ic, (li, _) in enumerate(instr_lines):
        line_to_ic[li] = ic
    
    # Second pass: find JmpF and Jmp markers and compute offsets
    jmpf_info = []  # list of (line_idx, ic)
    jmp_info = []   # list of (line_idx, ic)
    
    for li, marker in sorted(markers.items()):
        if "JmpF" in marker:
            # Find the next instruction line after this marker
            for check_li in range(li + 1, len(lines)):
                if check_li in line_to_ic:
                    jmpf_info.append((check_li, line_to_ic[check_li]))
                    break
        elif "Jmp -> end" in marker:
            for check_li in range(li + 1, len(lines)):
                if check_li in line_to_ic:
                    jmp_info.append((check_li, line_to_ic[check_li]))
                    break
    
    # Third pass: for each JmpF, find the matching else marker
    # JmpF target = instruction after ; else:
    patched = {}  # line_idx -> new word
    
    else_markers = [li for li, m in markers.items() if m.startswith("; else:")]
    end_markers = [li for li, m in markers.items() if m.startswith("; end:")]
    
    for jf_li, jf_ic in reversed(jmpf_info):
        # Find the next ; else: marker after this JmpF
        target_ic = None
        for em in else_markers:
            if em > jf_li:
                # Find the first instruction after the else marker
                for check_li in range(em + 1, len(lines)):
                    if check_li in line_to_ic:
                        target_ic = line_to_ic[check_li]
                        break
                break
        
        if target_ic is not None:
            offset = target_ic - jf_ic - 1
            old_word = [w for li, w in instr_lines if li == jf_li][0]
            patched[jf_li] = patch_jmpf(old_word, offset)
    
    for jp_li, jp_ic in reversed(jmp_info):
        target_ic = None
        # Find next ; end: marker first, then else, then end of list
        for em in end_markers:
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
            offset = target_ic - jp_ic - 1
            old_word = [w for li, w in instr_lines if li == jp_li][0]
            patched[jp_li] = patch_jmp(old_word, offset)
    
    # Build output
    result = []
    for i, line in enumerate(lines):
        if i in patched:
            result.append(format_hex(patched[i]))
        else:
            result.append(line.rstrip())
    
    return result


def main():
    lines = sys.stdin.readlines()
    result = fixup(lines)
    for line in result:
        print(line)


if __name__ == "__main__":
    main()
