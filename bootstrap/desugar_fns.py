#!/usr/bin/env python3
"""desugar_fns.py — Preprocess multi-fn Nulang programs for compile_hex.nula.

Reads a .nula source file from stdin and transforms top-level fn definitions
into let-binding chains, so the bytecode compiler can handle multi-function
programs.

Transformation:
    fn add(x) => x + 1
    fn double(x) => x * 2
    add(double(3))

Becomes:
    let add = fn(x) => x + 1 in
    let double = fn(x) => x * 2 in
    add(double(3))

Usage:
    python3 bootstrap/desugar_fns.py < myprogram.nula |
      nulang bootstrap/compile_hex.nula |
      python3 bootstrap/fixup_hex.py |
      python3 bootstrap/hex2nbc.py > out.nbc

- Handles `fn name(x) => body` (arrow syntax) and `fn name(x) { body }` (block syntax)
- fn definitions must be at the top level (not nested in expressions)
- The last expression is the main body
"""

import sys
import re


def desugar(source: str) -> str:
    """Transform top-level fn definitions into let chains."""
    lines = source.split('\n')
    result_lines = []
    fn_defs = []
    
    i = 0
    while i < len(lines):
        line = lines[i]
        stripped = line.strip()
        
        # Check if this line starts a fn definition
        if stripped.startswith('fn ') and '(' in stripped and ('=>' in stripped or '}' in stripped):
            # Single-line fn definition
            fn_defs.append(stripped)
            i += 1
        elif stripped.startswith('fn '):
            # Multi-line fn definition — collect until we find =>
            fn_text = [line]
            i += 1
            while i < len(lines):
                fn_text.append(lines[i])
                if '=>' in lines[i] or '}' in lines[i]:
                    i += 1
                    break
                i += 1
            fn_defs.append(' '.join(fn_text).strip())
        else:
            # Not a fn definition — collect remaining as body
            result_lines.extend(lines[i:])
            break
    
    if not fn_defs:
        return source  # No fn definitions found
    
    # Transform fn definitions into let bindings
    let_bindings = []
    for fn_def in fn_defs:
        # Try arrow syntax: fn name(x) => body
        match = re.match(r'fn\s+(\w+)\s*\((.*?)\)\s*=>\s*(.*)', fn_def)
        if match:
            name = match.group(1)
            params = match.group(2)
            body = match.group(3)
            let_bindings.append(f'let {name} = fn({params}) => {body} in')
            continue
        # Try block syntax: fn name(x) { body }
        match = re.match(r'fn\s+(\w+)\s*\((.*?)\)\s*\{\s*([^}]*)\s*\}', fn_def)
        if match:
            name = match.group(1)
            params = match.group(2)
            body = match.group(3)
            let_bindings.append(f'let {name} = fn({params}) => {body} in')
            continue
    # Build output: let bindings + remaining body
    body_text = ' '.join(line.strip() for line in result_lines if line.strip())
    
    # Join everything on one line (IO.read reads only one line)
    if body_text:
        return ' '.join(let_bindings) + ' ' + body_text
    else:
        return ' '.join(let_bindings[:-1]) + ' ' + let_bindings[-1].replace(' in', '')


def main():
    source = sys.stdin.read()
    result = desugar(source)
    print(result)


if __name__ == '__main__':
    main()
