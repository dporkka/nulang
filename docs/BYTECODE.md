# Nulang VM Bytecode Reference

> Discovered and verified during RFC 0003 implementation (2026-08-09).

## Instruction Encoding

Every instruction is 4 bytes (u32 big-endian):
```
byte 0: opcode (u8)
byte 1: op1    (u8)
byte 2: op2    (u8)
byte 3: op3    (u8)
```

Hex string format: `"OOP1P2P3"` where OO=opcode, P1=op1, P2=op2, P3=op3.

## VM Execution Model

### PC Management
- **PC is pre-incremented** before opcode dispatch (`vm.rs:3719`)
- Jump offsets account for this: `new_pc = (pc + 1) + offset - 1 = pc + offset`
- After dispatch, the next `step()` call uses the updated PC

### Function Calls
- `Call func_reg, argc, dst` — calls function at index in func_reg
  - Copies r0..r[argc-1] from caller to callee frame
  - Sets `return_dst = dst` on callee frame
- `RetVal reg` — returns value in reg to caller
  - Writes `regs[reg]` to caller's `regs[return_dst]`
  - Pops callee frame
- `function_table: Vec<usize>` — bytecode offsets, indexed by function number
- `entry_point: Option<usize>` — **direct instruction offset**, NOT function table index

### Registers
- 256 registers per frame (`[Value; 256]`)
- Initialized to `Value::nil()` (tag TAG_NIL)
- Arguments passed in r0, r1, ...
- Return value convention: r0

## Verified Opcodes

### Constants
| Opcode | Hex | Operands | Description |
|--------|-----|----------|-------------|
| Const0 | 0x03 | op1=dst | r[dst] = Int(0) |
| Const1 | 0x04 | op1=dst | r[dst] = Int(1) |
| ConstU | 0x07 | op1:op2=const_idx, op3=dst | r[dst] = const[const_idx] |

### Arithmetic (Integer)
| Opcode | Hex | Operands | Description |
|--------|-----|----------|-------------|
| IAdd | 0x20 | op1=a, op2=b, op3=dst | r[dst] = r[a] + r[b] |
| ISub | 0x21 | op1=a, op2=b, op3=dst | r[dst] = r[a] - r[b] |
| IMul | 0x22 | op1=a, op2=b, op3=dst | r[dst] = r[a] * r[b] |

### Comparison
| Opcode | Hex | Operands | Description |
|--------|-----|----------|-------------|
| ICmpEq | 0x40 | op1=a, op2=b, op3=dst | r[dst] = bool(a == b) |
| ICmpLt | 0x41 | op1=a, op2=b, op3=dst | r[dst] = bool(a < b) |
| ICmpLe | 0x43 | op1=a, op2=b, op3=dst | r[dst] = bool(a <= b) |
| ICmpGe | 0x44 | op1=a, op2=b, op3=dst | r[dst] = bool(a >= b) |

### Control Flow
| Opcode | Hex | Operands | Description |
|--------|-----|----------|-------------|
| Jmp | 0x50 | op1:op2=offset(i16) | PC += offset |
| JmpF | 0x52 | op1=cond_reg, op2:op3=offset | Jump if !r[cond_reg].as_bool() |
| Call | 0x54 | op1=func_reg, op2=argc, op3=dst | Call function |
| RetVal | 0x57 | op1=reg | Return r[reg] to caller |

### Register Moves
| Opcode | Hex | Operands | Description |
|--------|-----|----------|-------------|
| Move | 0x12 | op1=src, op2=dst | r[dst] = r[src] |

## Key Behaviors

### as_bool() for JmpF
- `Value::bool(b)` → `Some(b)` (TAG_BOOL)
- `Value::int(n)` → **None** (TAG_INT, not checked!)
- JmpF uses `as_bool().unwrap_or(false)` → Int values treated as **false**
- `Const0` gives `Int(0)` → JmpF jumps (as_bool→None→false→!false=true)

### entry_point vs function_table
- `entry_point` is a **direct bytecode offset**, not a function table index
- `run()` sets `frame.pc = entry_point`
- `function_table` is only used by `Call` to resolve function indices to offsets

### Jump Offset Formula
```
offset = target_pc - jmp_pc
```
Where `jmp_pc` is the instruction index of the Jmp/JmpF instruction.
The VM computes: `new_pc = (jmp_pc + 1) + offset - 1 = jmp_pc + offset`

## Verified Programs

| # | Program | Instructions | entry_point | Result |
|---|---------|-------------|-------------|--------|
| 0 | `42` | 2 | 0 | 42 |
| 1 | `1+2` | 4 | 0 | 3 |
| 2 | `if 1<2 then 10 else 20` | 8 | 0 | 10 |
| 3 | `double(21)` | 7 | 3 | 42 |
| 4 | `fact(6)` | 15 | 11 | 720 |

## Bootstrap Self-Hosting Pipeline

```
echo 'expr' | nulang bootstrap/compile_hex.nula |
  python3 bootstrap/fixup_hex.py |
  python3 bootstrap/hex2nbc.py > out.nbc
nulang out.nbc
```

11/11 checks pass (`bootstrap/verify.sh`), including:
- Arithmetic, let, if, not, lambda, recursion (fib), multi-fn

## References
- `src/bytecode.rs` — opcode definitions
- `src/vm.rs` — VM dispatch (line 3719: PC increment, line 3721: opcode match)
- `src/vm.rs:4423` — Move/Load/Store/Dup dispatch
- `src/vm.rs:3308` — step_call
- `src/vm.rs:3763` — RetVal dispatch
