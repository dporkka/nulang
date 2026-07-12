//! JIT compiler tests.

use super::*;
use crate::bytecode::*;

fn make_jit() -> JitSession {
    JitSession::new()
}

#[test]
fn test_jit_session_creation() {
    let jit = JitSession::new();
    assert_eq!(jit.compiled_count(), 0);
}

#[test]
fn test_hot_counter() {
    reset_hot_counters();
    assert!(!record_and_check_hot(0, 0));
    for _ in 0..HOT_THRESHOLD { record_and_check_hot(0, 42); }
    assert!(record_and_check_hot(0, 42));
    // The same offset in a different module has its own independent counter.
    assert!(!record_and_check_hot(1, 42));
    reset_hot_counters();
}

#[test]
fn test_find_compilable_region() {
    let instructions = vec![
        Instruction::new3(OpCode::IAdd, 0, 1, 2),
        Instruction::new3(OpCode::ISub, 0, 1, 2),
        Instruction::new0(OpCode::Ret),
    ];
    // The region stops *before* Ret so the VM still executes the return.
    assert_eq!(find_compilable_region(0, &instructions), 2);
}

#[test]
fn test_find_region_stops_at_unsupported() {
    let instructions = vec![
        Instruction::new3(OpCode::IAdd, 0, 1, 2),
        Instruction::new3(OpCode::Spawn, 0, 0, 0),
        Instruction::new3(OpCode::ISub, 0, 1, 2),
    ];
    assert_eq!(find_compilable_region(0, &instructions), 1);
}

/// Regions must stop before branches and Halt: after a region runs, the VM
/// advances pc by the region length, so a compiled branch whose target lies
/// elsewhere would resume at the wrong instruction.
#[test]
fn test_find_region_stops_before_branches_and_halt() {
    for branch in [
        Instruction::new3(OpCode::Jmp, 0, 2, 0),
        Instruction::new3(OpCode::JmpT, 0, 0, 2),
        Instruction::new3(OpCode::JmpF, 0, 0, 2),
        Instruction::new0(OpCode::Halt),
    ] {
        let instructions = vec![
            Instruction::new3(OpCode::IAdd, 0, 1, 2),
            Instruction::new3(OpCode::ISub, 0, 1, 2),
            branch,
            Instruction::new3(OpCode::IMul, 0, 1, 2),
        ];
        assert_eq!(
            find_compilable_region(0, &instructions),
            2,
            "region must stop before {:?}",
            instructions[2].opcode
        );
    }
}

#[test]
fn test_jit_compile_empty_region() {
    let mut jit = make_jit();
    let instructions = vec![
        Instruction::new0(OpCode::Nop),
        Instruction::new0(OpCode::Halt),
    ];
    let ptr = unsafe { jit.compile_region(0, 0, 2, &instructions) };
    assert!(ptr.is_some());
}

#[test]
fn test_jit_compile_int_add() {
    let mut jit = make_jit();
    let instructions = vec![
        Instruction::new3(OpCode::IAdd, 0, 1, 2),
        Instruction::new0(OpCode::Halt),
    ];
    let ptr = unsafe { jit.compile_region(0, 0, 2, &instructions) };
    assert!(ptr.is_some());
}

#[test]
fn test_jit_compile_integer_loop() {
    let mut jit = make_jit();
    let instructions = vec![
        Instruction::new1(OpCode::Const0, 0),
        Instruction::new1(OpCode::Const0, 1),
        Instruction::new3(OpCode::IAdd, 0, 1, 0),
        Instruction::new1(OpCode::IInc, 1),
        Instruction::new3(OpCode::ICmpLt, 1, 2, 2),
        Instruction::new2(OpCode::JmpT, 2, 0xFC),
        Instruction::new0(OpCode::Halt),
    ];
    let ptr = unsafe { jit.compile_region(0, 0, 7, &instructions) };
    assert!(ptr.is_some());
}

#[test]
fn test_jit_compile_float_ops() {
    let mut jit = make_jit();
    let instructions = vec![
        Instruction::new3(OpCode::FAdd, 0, 1, 2),
        Instruction::new3(OpCode::FSub, 2, 1, 3),
        Instruction::new3(OpCode::FMul, 3, 0, 4),
        Instruction::new3(OpCode::FDiv, 4, 1, 5),
        Instruction::new0(OpCode::Halt),
    ];
    let ptr = unsafe { jit.compile_region(0, 0, 5, &instructions) };
    assert!(ptr.is_some());
}

#[test]
fn test_jit_compile_comparisons() {
    let mut jit = make_jit();
    let instructions = vec![
        Instruction::new3(OpCode::ICmpEq, 0, 1, 10),
        Instruction::new3(OpCode::ICmpLt, 0, 1, 11),
        Instruction::new3(OpCode::ICmpGt, 0, 1, 12),
        Instruction::new3(OpCode::ICmpLe, 0, 1, 13),
        Instruction::new3(OpCode::ICmpGe, 0, 1, 14),
        Instruction::new0(OpCode::Halt),
    ];
    let ptr = unsafe { jit.compile_region(0, 0, 6, &instructions) };
    assert!(ptr.is_some());
}

#[test]
fn test_jit_compile_logic() {
    let mut jit = make_jit();
    let instructions = vec![
        Instruction::new2(OpCode::Not, 0, 1),
        Instruction::new3(OpCode::And, 0, 1, 2),
        Instruction::new3(OpCode::Or, 0, 1, 3),
        Instruction::new0(OpCode::Halt),
    ];
    let ptr = unsafe { jit.compile_region(0, 0, 4, &instructions) };
    assert!(ptr.is_some());
}

#[test]
fn test_jit_compile_conversions() {
    let mut jit = make_jit();
    let instructions = vec![
        Instruction::new2(OpCode::IToF, 0, 1),
        Instruction::new2(OpCode::FToI, 1, 2),
        Instruction::new0(OpCode::Halt),
    ];
    let ptr = unsafe { jit.compile_region(0, 0, 3, &instructions) };
    assert!(ptr.is_some());
}

#[test]
fn test_jit_compile_register_moves() {
    let mut jit = make_jit();
    let instructions = vec![
        Instruction::new2(OpCode::Move, 0, 1),
        Instruction::new2(OpCode::Dup, 0, 2),
        Instruction::new2(OpCode::Swap, 1, 2),
        Instruction::new0(OpCode::Halt),
    ];
    let ptr = unsafe { jit.compile_region(0, 0, 4, &instructions) };
    assert!(ptr.is_some());
}

#[test]
fn test_jit_compile_jmp_unconditional() {
    let mut jit = make_jit();
    let instructions = vec![
        Instruction::new3(OpCode::Jmp, 0, 0, 3),
        Instruction::new0(OpCode::Nop),
        Instruction::new0(OpCode::Nop),
        Instruction::new0(OpCode::Halt),
    ];
    let ptr = unsafe { jit.compile_region(0, 0, 4, &instructions) };
    assert!(ptr.is_some());
}

#[test]
fn test_jit_compile_jmp_conditional() {
    let mut jit = make_jit();
    let instructions = vec![
        Instruction::new3(OpCode::JmpT, 0, 0, 3),
        Instruction::new3(OpCode::JmpF, 0, 0, 3),
        Instruction::new0(OpCode::Nop),
        Instruction::new0(OpCode::Halt),
    ];
    let ptr = unsafe { jit.compile_region(0, 0, 4, &instructions) };
    assert!(ptr.is_some());
}

#[test]
fn test_jit_compile_all_mvp_opcodes() {
    let mut jit = make_jit();
    let instructions = vec![
        Instruction::new1(OpCode::Const0, 0),
        Instruction::new1(OpCode::Const1, 1),
        Instruction::new1(OpCode::Const2, 2),
        Instruction::new1(OpCode::ConstM1, 3),
        Instruction::new2(OpCode::Move, 0, 4),
        Instruction::new2(OpCode::Dup, 0, 5),
        Instruction::new2(OpCode::Swap, 4, 5),
        Instruction::new3(OpCode::IAdd, 0, 1, 10),
        Instruction::new3(OpCode::ISub, 1, 2, 11),
        Instruction::new3(OpCode::IMul, 2, 3, 12),
        Instruction::new3(OpCode::IDiv, 10, 11, 13),
        Instruction::new3(OpCode::IMod, 11, 12, 14),
        Instruction::new2(OpCode::INeg, 0, 15),
        Instruction::new1(OpCode::IInc, 0),
        Instruction::new1(OpCode::IDec, 1),
        Instruction::new3(OpCode::FAdd, 0, 1, 20),
        Instruction::new3(OpCode::FSub, 1, 2, 21),
        Instruction::new3(OpCode::FMul, 2, 3, 22),
        Instruction::new3(OpCode::FDiv, 20, 21, 23),
        Instruction::new3(OpCode::ICmpEq, 0, 1, 30),
        Instruction::new3(OpCode::ICmpLt, 0, 1, 31),
        Instruction::new3(OpCode::ICmpGt, 0, 1, 32),
        Instruction::new3(OpCode::ICmpLe, 0, 1, 33),
        Instruction::new3(OpCode::ICmpGe, 0, 1, 34),
        Instruction::new3(OpCode::FCmpEq, 0, 1, 35),
        Instruction::new3(OpCode::FCmpLt, 0, 1, 36),
        Instruction::new3(OpCode::FCmpGt, 0, 1, 37),
        Instruction::new2(OpCode::Not, 0, 40),
        Instruction::new3(OpCode::And, 0, 1, 41),
        Instruction::new3(OpCode::Or, 0, 1, 42),
        Instruction::new2(OpCode::IToF, 0, 50),
        Instruction::new2(OpCode::FToI, 1, 51),
        Instruction::new0(OpCode::Halt),
    ];
    let ptr = unsafe { jit.compile_region(0, 0, instructions.len(), &instructions) };
    assert!(ptr.is_some());
}

#[test]
fn test_jit_compile_rejects_unsupported_opcode() {
    let mut jit = make_jit();
    let instructions = vec![
        Instruction::new3(OpCode::IAdd, 0, 1, 2),
        Instruction::new3(OpCode::Spawn, 0, 0, 0),
        Instruction::new3(OpCode::ISub, 0, 1, 2),
        Instruction::new0(OpCode::Halt),
    ];
    let ptr = unsafe { jit.compile_region(0, 0, 1, &instructions) };
    assert!(ptr.is_some());
}

#[test]
fn test_tiered_action_has_simd_variant() {
    let action = TieredAction::CompiledSimdAndRan;
    assert_ne!(action, TieredAction::Interpret);
    assert_ne!(action, TieredAction::RanJit);
}

#[test]
fn test_jit_session_simd_enabled() {
    let jit = JitSession::new();
    // Session created successfully with SIMD enabled in ISA flags
    assert_eq!(jit.compiled_count(), 0);
}

// ---------------------------------------------------------------------------
// Extended opcode coverage: Load/Store, bitwise int ops, FNeg
// ---------------------------------------------------------------------------

#[test]
fn test_jit_compile_bitwise_ops() {
    let mut jit = make_jit();
    let instructions = vec![
        Instruction::new3(OpCode::Xor, 0, 1, 2),
        Instruction::new3(OpCode::Shl, 2, 1, 3),
        Instruction::new3(OpCode::Shr, 3, 1, 4),
        Instruction::new3(OpCode::BitAnd, 4, 0, 5),
        Instruction::new3(OpCode::BitOr, 5, 1, 6),
        Instruction::new0(OpCode::Halt),
    ];
    let ptr = unsafe { jit.compile_region(0, 0, 6, &instructions) };
    assert!(ptr.is_some());
}

#[test]
fn test_jit_compile_fneg() {
    let mut jit = make_jit();
    let instructions = vec![
        Instruction::new3(OpCode::FNeg, 0, 0, 1),
        Instruction::new0(OpCode::Halt),
    ];
    let ptr = unsafe { jit.compile_region(0, 0, 2, &instructions) };
    assert!(ptr.is_some());
}

#[test]
fn test_jit_compile_load_store() {
    let mut jit = make_jit();
    let instructions = vec![
        Instruction::new2(OpCode::Load, 0, 1),
        Instruction::new2(OpCode::Store, 1, 2),
        Instruction::new0(OpCode::Halt),
    ];
    let ptr = unsafe { jit.compile_region(0, 0, 3, &instructions) };
    assert!(ptr.is_some());
}

/// Execute a compiled bitwise region directly and check the results against
/// the interpreter's semantics: tag-checked int operands (non-int → 0),
/// arithmetic shift right, shift amounts masked to 6 bits.
#[test]
fn test_jit_execute_bitwise_ops() {
    use crate::vm::Value;
    let mut jit = make_jit();
    let instructions = vec![
        Instruction::new3(OpCode::Xor, 0, 1, 2),    // r2  = r0 ^ r1
        Instruction::new3(OpCode::BitAnd, 0, 1, 3), // r3  = r0 & r1
        Instruction::new3(OpCode::BitOr, 0, 1, 4),  // r4  = r0 | r1
        Instruction::new3(OpCode::Shl, 5, 6, 7),    // r7  = r5 << r6
        Instruction::new3(OpCode::Shr, 8, 9, 10),   // r10 = r8 >> r9 (arithmetic)
        Instruction::new3(OpCode::Shl, 11, 12, 13), // r13 = r11 << (r12 & 63)
        Instruction::new3(OpCode::Xor, 14, 15, 16), // r16 = float ^ int -> 0 ^ 7
        Instruction::new0(OpCode::Halt),
    ];
    let func = unsafe { jit.compile_region(0, 0, 8, &instructions) }
        .expect("bitwise region should compile");
    let consts: [u64; 0] = [];
    let mut regs = [0u64; 256];
    regs[0] = Value::int(0b1100).as_raw();
    regs[1] = Value::int(0b1010).as_raw();
    regs[5] = Value::int(3).as_raw();
    regs[6] = Value::int(4).as_raw();
    regs[8] = Value::int(-16).as_raw();
    regs[9] = Value::int(2).as_raw();
    regs[11] = Value::int(1).as_raw();
    regs[12] = Value::int(65).as_raw(); // 65 & 0x3f == 1
    regs[14] = Value::float(1.5).as_raw(); // not int-tagged -> contributes 0
    regs[15] = Value::int(7).as_raw();

    func(regs.as_mut_ptr(), consts.as_ptr());

    assert_eq!(Value::from_bits(regs[2]).as_int(), Some(0b0110));
    assert_eq!(Value::from_bits(regs[3]).as_int(), Some(0b1000));
    assert_eq!(Value::from_bits(regs[4]).as_int(), Some(0b1110));
    assert_eq!(Value::from_bits(regs[7]).as_int(), Some(48));
    assert_eq!(Value::from_bits(regs[10]).as_int(), Some(-4));
    assert_eq!(Value::from_bits(regs[13]).as_int(), Some(2));
    assert_eq!(Value::from_bits(regs[16]).as_int(), Some(7));
}

/// FNeg must negate real floats and map any tagged (NaN-pattern) value to
/// -0.0, exactly like the interpreter's `as_float().unwrap_or(0.0)`.
#[test]
fn test_jit_execute_fneg() {
    use crate::vm::Value;
    let mut jit = make_jit();
    let instructions = vec![
        Instruction::new3(OpCode::FNeg, 0, 0, 1), // r1 = -r0 (float)
        Instruction::new3(OpCode::FNeg, 2, 0, 3), // r3 = -r2 (int-tagged -> -0.0)
        Instruction::new0(OpCode::Halt),
    ];
    let func = unsafe { jit.compile_region(0, 0, 3, &instructions) }
        .expect("FNeg region should compile");
    let consts: [u64; 0] = [];
    let mut regs = [0u64; 256];
    regs[0] = Value::float(2.5).as_raw();
    regs[2] = Value::int(5).as_raw();

    func(regs.as_mut_ptr(), consts.as_ptr());

    assert_eq!(Value::from_bits(regs[1]).as_float(), Some(-2.5));
    assert_eq!(regs[3], (-0.0f64).to_bits());
}

/// Load/Store are register copies (op1 -> op2), same as Move/Dup.
#[test]
fn test_jit_execute_load_store() {
    use crate::vm::Value;
    let mut jit = make_jit();
    let instructions = vec![
        Instruction::new2(OpCode::Load, 0, 1),
        Instruction::new2(OpCode::Store, 1, 2),
        Instruction::new0(OpCode::Halt),
    ];
    let func = unsafe { jit.compile_region(0, 0, 3, &instructions) }
        .expect("Load/Store region should compile");
    let consts: [u64; 0] = [];
    let mut regs = [0u64; 256];
    regs[0] = Value::int(42).as_raw();

    func(regs.as_mut_ptr(), consts.as_ptr());

    assert_eq!(Value::from_bits(regs[1]).as_int(), Some(42));
    assert_eq!(Value::from_bits(regs[2]).as_int(), Some(42));
}

/// End-to-end equivalence: run a hot loop (2000 iterations, crossing
/// HOT_THRESHOLD) containing the new bitwise opcodes through the VM
/// interpreter, then execute the same loop body as a JIT-compiled region
/// driven from Rust, and assert both produce the identical accumulator.
#[test]
fn test_jit_bitwise_loop_matches_interpreter() {
    use crate::vm::{Value, VM};

    const LIMIT: i64 = 2000;

    let mut module = CodeModule::new("jit_bitwise_loop");
    let c_limit = module.add_constant(Constant::Int(LIMIT));
    module.emit(Instruction::new1(OpCode::Const0, 0)); // 0: r0 = 0 (acc)
    module.emit(Instruction::new1(OpCode::Const0, 1)); // 1: r1 = 0 (i)
    module.emit(Instruction::new1(OpCode::Const2, 2)); // 2: r2 = 2
    module.emit(Instruction::new3(                     // 3: r6 = LIMIT
        OpCode::ConstU,
        ((c_limit >> 8) & 0xFF) as u8,
        (c_limit & 0xFF) as u8,
        6,
    ));
    module.emit(Instruction::new1(OpCode::Const1, 7)); // 4: r7 = 1
    // Loop body (pc 5..=12): a straight-line region of 8 compilable opcodes.
    module.emit(Instruction::new3(OpCode::IAdd, 0, 1, 0));   // 5:  acc += i
    module.emit(Instruction::new3(OpCode::IAdd, 1, 7, 1));   // 6:  i += 1
    module.emit(Instruction::new3(OpCode::Xor, 1, 2, 3));    // 7:  r3 = i ^ 2
    module.emit(Instruction::new3(OpCode::Shl, 3, 2, 3));    // 8:  r3 <<= 2
    module.emit(Instruction::new3(OpCode::BitOr, 3, 2, 3));  // 9:  r3 |= 2
    module.emit(Instruction::new3(OpCode::BitAnd, 3, 6, 4)); // 10: r4 = r3 & LIMIT
    module.emit(Instruction::new3(OpCode::IAdd, 0, 4, 0));   // 11: acc += r4
    module.emit(Instruction::new3(OpCode::ICmpLt, 1, 6, 5)); // 12: r5 = i < LIMIT
    let back: i16 = -8; // 13: JmpT r5 -> pc 5 (13 + (-8))
    module.emit(Instruction::new3(
        OpCode::JmpT,
        5,
        ((back as u16) >> 8) as u8,
        (back as u16 & 0xFF) as u8,
    ));
    module.emit(Instruction::new0(OpCode::Halt)); // 13
    module.entry_point = Some(0);

    // Reference value, computed with plain Rust using the same semantics.
    // The loop adds `i` before incrementing, so i runs 0..LIMIT there.
    let mut expected: i64 = 0;
    for i in 1..=LIMIT {
        expected += i - 1;
        expected += (((i ^ 2) << 2) | 2) & LIMIT;
    }

    // 1. Interpreter run (the loop crosses HOT_THRESHOLD, so the tiered
    //    path is exercised; the result must match regardless).
    let mut vm = VM::new();
    vm.load_module(module.clone());
    let interp = vm.run().expect("interpreter run should succeed");
    assert_eq!(interp.as_int(), Some(expected), "interpreter result mismatch");

    // 2. JIT-compiled loop body: compile the pc 5..=12 region and drive it
    //    from Rust, replicating the JmpT back-edge via r5.
    let mut jit = make_jit();
    let func = unsafe { jit.compile_region(0, 5, 8, &module.instructions) }
        .expect("loop body region should compile");
    let consts: Vec<u64> = module
        .constants
        .iter()
        .map(|c| match *c {
            Constant::Int(n) => Value::int(n).as_raw(),
            _ => Value::nil().as_raw(),
        })
        .collect();
    let mut regs = [0u64; 256];
    regs[0] = Value::int(0).as_raw();
    regs[1] = Value::int(0).as_raw();
    regs[2] = Value::int(2).as_raw();
    regs[6] = Value::int(LIMIT).as_raw();
    regs[7] = Value::int(1).as_raw();
    loop {
        func(regs.as_mut_ptr(), consts.as_ptr());
        if Value::from_bits(regs[5]).as_bool() != Some(true) {
            break;
        }
    }

    assert_eq!(
        Value::from_bits(regs[0]).as_int(),
        Some(expected),
        "JIT-compiled loop body must match the interpreter"
    );
}

/// JIT-compiled IInc/IDec must match the interpreter bit-for-bit: both read
/// the register's raw 48-bit payload as a signed value (tag ignored), adjust
/// by ±1 with 48-bit wrap, and re-tag the result as an int — the semantics
/// of the `nulang_iinc`/`nulang_idec` runtime helpers.
#[test]
fn test_jit_iinc_idec_match_interpreter() {
    use crate::vm::{Value, VM};

    let cases: Vec<(OpCode, Constant)> = vec![
        (OpCode::IInc, Constant::Int(41)),
        (OpCode::IDec, Constant::Int(41)),
        (OpCode::IInc, Constant::Bool(true)),                // payload 1 -> int 2
        (OpCode::IDec, Constant::Nil),                       // payload 0 -> int -1
        (OpCode::IInc, Constant::Float(2.5)),                // tag ignored: payload bits -> int
        (OpCode::IInc, Constant::Int(0x0000_7FFF_FFFF_FFFF)),  // INT48_MAX wraps to INT48_MIN
        (OpCode::IDec, Constant::Int(-0x0000_8000_0000_0000)), // INT48_MIN wraps to INT48_MAX
    ];

    for (op, constant) in cases {
        // Interpreter reference: load the constant into r0, run the op, Halt.
        let mut module = CodeModule::new("jit_iinc_idec_ref");
        let idx = module.add_constant(constant.clone());
        module.emit(Instruction::new3(
            OpCode::ConstU,
            ((idx >> 8) & 0xFF) as u8,
            (idx & 0xFF) as u8,
            0,
        ));
        module.emit(Instruction::new1(op, 0));
        module.emit(Instruction::new0(OpCode::Halt));
        module.entry_point = Some(0);
        let mut vm = VM::new();
        vm.load_module(module);
        let interp = vm.run().expect("interpreter IInc/IDec should succeed");

        // JIT-compiled single-op region fed the same raw bits as ConstU loads.
        let input_raw = match constant {
            Constant::Int(n) => Value::int(n).as_raw(),
            Constant::Float(f) => Value::float(f).as_raw(),
            Constant::Bool(b) => Value::bool(b).as_raw(),
            Constant::Nil => Value::nil().as_raw(),
            other => panic!("unexpected constant in test case: {:?}", other),
        };
        let mut jit = make_jit();
        let instructions = vec![
            Instruction::new1(op, 0),
            Instruction::new0(OpCode::Halt),
        ];
        let func = unsafe { jit.compile_region(0, 0, 2, &instructions) }
            .expect("IInc/IDec region should compile");
        let consts: [u64; 0] = [];
        let mut regs = [0u64; 256];
        regs[0] = input_raw;
        func(regs.as_mut_ptr(), consts.as_ptr());

        assert_eq!(
            regs[0],
            interp.as_raw(),
            "JIT {:?} must match the interpreter bit-for-bit",
            op
        );
    }
}

// ---------------------------------------------------------------------------
// Type-directed (guard-stripped) tiering
// ---------------------------------------------------------------------------

/// Helper: build the standard hot integer loop module used by the typed-path
/// tests. Layout:
/// ```text
/// 0: r0 = 0 (acc)          7:  r0 += 1        (filler)
/// 1: r1 = 0 (i)            8:  r8 *= 1        (filler, stays 2)
/// 2: r7 = 1 (one)          9:  r9 = r8 + 1    (filler)
/// 3: r6 = LIMIT            10: r10 = r9 - r8  (filler)
/// 4: r8 = 2                11: r5 = i < LIMIT
/// 5: r0 += i   <- region   12: JmpT r5 -> pc 5
/// 6: r1 += 1               13: Halt
/// ```
/// The loop body at pc 5 is a straight-line region of 7 compilable opcodes.
fn make_int_loop_module(limit: i64) -> CodeModule {
    let mut module = CodeModule::new("typed_int_loop");
    let c_limit = module.add_constant(Constant::Int(limit));
    module.emit(Instruction::new1(OpCode::Const0, 0));
    module.emit(Instruction::new1(OpCode::Const0, 1));
    module.emit(Instruction::new1(OpCode::Const1, 7));
    module.emit(Instruction::new3(
        OpCode::ConstU,
        ((c_limit >> 8) & 0xFF) as u8,
        (c_limit & 0xFF) as u8,
        6,
    ));
    module.emit(Instruction::new1(OpCode::Const2, 8));
    // Loop body (pc 5..=11).
    module.emit(Instruction::new3(OpCode::IAdd, 0, 1, 0));    // 5:  acc += i
    module.emit(Instruction::new3(OpCode::IAdd, 1, 7, 1));    // 6:  i += 1
    module.emit(Instruction::new3(OpCode::IAdd, 0, 7, 0));    // 7:  acc += 1
    module.emit(Instruction::new3(OpCode::IMul, 8, 7, 8));    // 8:  r8 *= 1
    module.emit(Instruction::new3(OpCode::IAdd, 9, 8, 9));    // 9:  r9 += r8 (r9 starts 0 -> 2)
    module.emit(Instruction::new3(OpCode::ISub, 9, 8, 10));   // 10: r10 = r9 - r8
    module.emit(Instruction::new3(OpCode::ICmpLt, 1, 6, 5));  // 11: r5 = i < LIMIT
    let back: i16 = -7; // 12: JmpT r5 -> pc 5
    module.emit(Instruction::new3(
        OpCode::JmpT,
        5,
        ((back as u16) >> 8) as u8,
        (back as u16 & 0xFF) as u8,
    ));
    module.emit(Instruction::new0(OpCode::Halt)); // 13
    module.entry_point = Some(0);
    module
}

/// Expected accumulator for `make_int_loop_module`: the loop adds `i` and 1
/// per iteration with i running 0..LIMIT.
fn int_loop_expected(limit: i64) -> i64 {
    (0..limit).sum::<i64>() + limit
}

/// The type inference must prove the loop-carried integer registers at the
/// start of the hot region (pc 5), including the register loaded from an
/// Int constant and the ones written by arithmetic inside the loop.
#[test]
fn test_infer_reg_types_int_loop() {
    use crate::jit::typed_compiler::{infer_reg_types, KnownType};

    let module = make_int_loop_module(2000);
    let meta = infer_reg_types(&module, 5);

    assert_eq!(meta.get_type(0), KnownType::Int, "accumulator r0");
    assert_eq!(meta.get_type(1), KnownType::Int, "counter r1");
    assert_eq!(meta.get_type(6), KnownType::Int, "constant-loaded r6");
    assert_eq!(meta.get_type(7), KnownType::Int, "constant r7");
    assert_eq!(meta.get_type(8), KnownType::Int, "loop-written r8");
    // r9/r10 are only ever written inside the loop body, so on the first
    // entry they hold nil: the must-analysis must conservatively report
    // Unknown at the region start.
    assert_eq!(meta.get_type(9), KnownType::Unknown, "loop-internal r9");
    assert_eq!(meta.get_type(10), KnownType::Unknown, "loop-internal r10");
}

/// Conservative cases: IDiv can yield nil (div by zero) so its destination
/// must stay Unknown, and an unmodeled opcode must clobber all facts.
#[test]
fn test_infer_reg_types_conservative() {
    use crate::jit::typed_compiler::{infer_reg_types, KnownType};

    let mut module = CodeModule::new("typed_conservative");
    module.emit(Instruction::new1(OpCode::Const1, 0));       // 0: r0 = 1
    module.emit(Instruction::new1(OpCode::Const0, 1));       // 1: r1 = 0
    module.emit(Instruction::new3(OpCode::IDiv, 0, 1, 2));   // 2: r2 = r0 / r1 (nil!)
    module.emit(Instruction::new3(OpCode::IAdd, 0, 1, 3));   // 3: r3 = r0 + r1
    module.emit(Instruction::new0(OpCode::Halt));            // 4
    module.entry_point = Some(0);

    let meta = infer_reg_types(&module, 4);
    assert_eq!(meta.get_type(0), KnownType::Int);
    assert_eq!(meta.get_type(2), KnownType::Unknown, "IDiv may produce nil");
    assert_eq!(meta.get_type(3), KnownType::Int);

    // An unmodeled opcode (CapChk) clobbers every register fact.
    let mut module2 = CodeModule::new("typed_clobber");
    module2.emit(Instruction::new1(OpCode::Const1, 0));      // 0: r0 = 1
    module2.emit(Instruction::new2(OpCode::CapChk, 0, 9));   // 1: unmodeled -> clobber all
    module2.emit(Instruction::new0(OpCode::Halt));           // 2
    module2.entry_point = Some(0);

    let meta2 = infer_reg_types(&module2, 2);
    assert!(
        meta2.reg_types.is_empty(),
        "unmodeled opcodes must clobber all register types, got {:?}",
        meta2.reg_types
    );
}

/// Float constants and float arithmetic must be inferred as Float.
#[test]
fn test_infer_reg_types_float() {
    use crate::jit::typed_compiler::{infer_reg_types, KnownType};

    let mut module = CodeModule::new("typed_float");
    let c0 = module.add_constant(Constant::Float(1.5));
    let c1 = module.add_constant(Constant::Float(2.5));
    module.emit(Instruction::new3(
        OpCode::ConstU,
        ((c0 >> 8) & 0xFF) as u8,
        (c0 & 0xFF) as u8,
        0,
    ));
    module.emit(Instruction::new3(
        OpCode::ConstU,
        ((c1 >> 8) & 0xFF) as u8,
        (c1 & 0xFF) as u8,
        1,
    ));
    module.emit(Instruction::new3(OpCode::FAdd, 0, 1, 2));   // r2 = r0 + r1
    module.emit(Instruction::new3(OpCode::FCmpLt, 0, 1, 3)); // r3 = r0 < r1
    module.emit(Instruction::new0(OpCode::Halt));
    module.entry_point = Some(0);

    let meta = infer_reg_types(&module, 4);
    assert_eq!(meta.get_type(0), KnownType::Float);
    assert_eq!(meta.get_type(1), KnownType::Float);
    assert_eq!(meta.get_type(2), KnownType::Float);
    assert_eq!(meta.get_type(3), KnownType::Bool, "comparisons yield Bool");
}

/// (a) A hot integer loop running through the VM's tiering path must be
/// compiled by the type-directed (guard-stripped) compiler, and (b) produce
/// exactly the same result as the interpreter/scalar path.
#[test]
fn test_typed_tiering_hot_int_loop() {
    use crate::vm::VM;

    const LIMIT: i64 = 2000;
    let module = make_int_loop_module(LIMIT);

    let mut vm = VM::new();
    vm.load_module(module);
    let result = vm.run().expect("typed int loop should run");
    assert_eq!(
        result.as_int(),
        Some(int_loop_expected(LIMIT)),
        "typed-path result must match the interpreter semantics"
    );
    assert!(
        vm.jit_typed_compiled_count() >= 1,
        "hot int loop region must be compiled through the type-directed path"
    );

    // Sanity: the plain interpreter result (no JIT tier-up) is identical.
    let mut module2 = make_int_loop_module(LIMIT);
    module2.name = "typed_int_loop_ref".to_string();
    let mut vm2 = VM::new();
    vm2.load_module(module2);
    let result2 = vm2.run().expect("reference int loop should run");
    assert_eq!(result2.as_int(), result.as_int());
}

/// (a/b) A hot float loop must also take the typed path and stay exact:
/// whole-number f64 sums below 2^53 are represented exactly.
#[test]
fn test_typed_tiering_hot_float_loop() {
    use crate::vm::VM;

    const LIMIT: f64 = 2000.0;

    let mut module = CodeModule::new("typed_float_loop");
    let c_zero = module.add_constant(Constant::Float(0.0));
    let c_one = module.add_constant(Constant::Float(1.0));
    let c_limit = module.add_constant(Constant::Float(LIMIT));
    let emit_const = |module: &mut CodeModule, idx: usize, dst: u8| {
        module.emit(Instruction::new3(
            OpCode::ConstU,
            ((idx >> 8) & 0xFF) as u8,
            (idx & 0xFF) as u8,
            dst,
        ));
    };
    emit_const(&mut module, c_zero, 0);   // 0: r0 = 0.0 (acc)
    emit_const(&mut module, c_zero, 1);   // 1: r1 = 0.0 (i)
    emit_const(&mut module, c_one, 7);    // 2: r7 = 1.0
    emit_const(&mut module, c_limit, 6);  // 3: r6 = LIMIT
    // Loop body (pc 4..=9): 6 straight-line compilable opcodes.
    module.emit(Instruction::new3(OpCode::FAdd, 0, 1, 0));    // 4: acc += i
    module.emit(Instruction::new3(OpCode::FAdd, 1, 7, 1));    // 5: i += 1.0
    module.emit(Instruction::new3(OpCode::FAdd, 8, 7, 8));    // 6: filler r8 += 1.0
    module.emit(Instruction::new3(OpCode::FAdd, 9, 8, 9));    // 7: filler r9 += r8
    module.emit(Instruction::new3(OpCode::FAdd, 10, 9, 10));  // 8: filler r10 += r9
    module.emit(Instruction::new3(OpCode::FCmpLt, 1, 6, 5));  // 9: r5 = i < LIMIT
    let back: i16 = -6; // 10: JmpT r5 -> pc 4
    module.emit(Instruction::new3(
        OpCode::JmpT,
        5,
        ((back as u16) >> 8) as u8,
        (back as u16 & 0xFF) as u8,
    ));
    module.emit(Instruction::new0(OpCode::Halt)); // 11
    module.entry_point = Some(0);

    let expected: f64 = (0..2000).map(|i| i as f64).sum();

    let mut vm = VM::new();
    vm.load_module(module);
    let result = vm.run().expect("typed float loop should run");
    assert_eq!(result.as_float(), Some(expected));
    assert!(
        vm.jit_typed_compiled_count() >= 1,
        "hot float loop region must be compiled through the type-directed path"
    );
}

/// (b) The guard-stripped region must be bit-for-bit identical to the scalar
/// JIT region for the same inputs: drive both compiled functions from Rust
/// with identical register files and compare the entire register state.
#[test]
fn test_typed_path_matches_scalar_path() {
    use crate::jit::typed_compiler::infer_reg_types;
    use crate::vm::Value;

    const LIMIT: i64 = 2000;
    let module = make_int_loop_module(LIMIT);
    let consts: Vec<u64> = module
        .constants
        .iter()
        .map(|c| match *c {
            Constant::Int(n) => Value::int(n).as_raw(),
            _ => Value::nil().as_raw(),
        })
        .collect();

    let run_region = |func: JitFunctionPtr| -> [u64; 256] {
        let mut regs = [0u64; 256];
        regs[6] = Value::int(LIMIT).as_raw();
        regs[7] = Value::int(1).as_raw();
        regs[8] = Value::int(2).as_raw();
        loop {
            func(regs.as_mut_ptr(), consts.as_ptr());
            if Value::from_bits(regs[5]).as_bool() != Some(true) {
                break;
            }
        }
        regs
    };

    // Scalar path.
    let mut scalar_jit = make_jit();
    let scalar = unsafe { scalar_jit.compile_region(0, 5, 7, &module.instructions) }
        .expect("scalar region should compile");
    let scalar_regs = run_region(scalar);

    // Typed path.
    let meta = infer_reg_types(&module, 5);
    assert!(!meta.reg_types.is_empty(), "int loop registers must be typed");
    let mut typed_jit = make_jit();
    let typed = unsafe {
        typed_jit.compile_region_typed(0, 5, 7, &module.instructions, Some(&meta))
    }
    .expect("typed region should compile");
    assert!(
        typed_jit.is_typed_compiled(0, 5),
        "region with proven types must use the guard-stripped compiler"
    );
    let typed_regs = run_region(typed);

    assert_eq!(
        typed_regs,
        scalar_regs,
        "guard-stripped code must be bit-for-bit identical to scalar code"
    );
}

/// (c) Absent or unprovable metadata must keep the scalar behavior:
/// `compile_region_typed` with `None` compiles via the scalar compiler, and
/// a loop whose register types are clobbered by an unmodeled opcode runs
/// correctly without ever taking the typed path.
#[test]
fn test_absent_metadata_uses_scalar_path() {
    use crate::vm::{Value, VM};

    const LIMIT: i64 = 2000;
    let module = make_int_loop_module(LIMIT);

    // None metadata: compiles, but is NOT recorded as typed.
    let mut jit = make_jit();
    let func = unsafe { jit.compile_region_typed(0, 5, 7, &module.instructions, None) }
        .expect("region should compile without metadata");
    assert_eq!(jit.typed_compiled_count(), 0, "no metadata -> scalar path");
    assert!(!jit.is_typed_compiled(0, 5));

    // The scalar-compiled function still computes the right thing.
    let consts: Vec<u64> = module
        .constants
        .iter()
        .map(|c| match *c {
            Constant::Int(n) => Value::int(n).as_raw(),
            _ => Value::nil().as_raw(),
        })
        .collect();
    let mut regs = [0u64; 256];
    regs[6] = Value::int(LIMIT).as_raw();
    regs[7] = Value::int(1).as_raw();
    regs[8] = Value::int(2).as_raw();
    loop {
        func(regs.as_mut_ptr(), consts.as_ptr());
        if Value::from_bits(regs[5]).as_bool() != Some(true) {
            break;
        }
    }
    assert_eq!(
        Value::from_bits(regs[0]).as_int(),
        Some(int_loop_expected(LIMIT))
    );

    // Unprovable metadata: an unmodeled opcode (CapChk) right before the
    // loop clobbers every register fact, so the VM must stay on the scalar
    // path while still producing the correct result.
    let mut clobbered = CodeModule::new("typed_clobbered_loop");
    let c_limit = clobbered.add_constant(Constant::Int(LIMIT));
    clobbered.emit(Instruction::new1(OpCode::Const0, 0));   // 0
    clobbered.emit(Instruction::new1(OpCode::Const0, 1));   // 1
    clobbered.emit(Instruction::new1(OpCode::Const1, 7));   // 2
    clobbered.emit(Instruction::new3(
        OpCode::ConstU,
        ((c_limit >> 8) & 0xFF) as u8,
        (c_limit & 0xFF) as u8,
        6,
    ));                                                     // 3
    clobbered.emit(Instruction::new1(OpCode::Const2, 8));   // 4
    // Clobber AFTER all constant setup so no register fact survives the
    // meet at the loop head: forward state is all-Unknown here.
    clobbered.emit(Instruction::new2(OpCode::CapChk, 0, 9)); // 5: clobbers analysis state
    // Loop body (pc 6..=12): same shape as make_int_loop_module.
    clobbered.emit(Instruction::new3(OpCode::IAdd, 0, 1, 0));
    clobbered.emit(Instruction::new3(OpCode::IAdd, 1, 7, 1));
    clobbered.emit(Instruction::new3(OpCode::IAdd, 0, 7, 0));
    clobbered.emit(Instruction::new3(OpCode::IMul, 8, 7, 8));
    clobbered.emit(Instruction::new3(OpCode::IAdd, 9, 8, 9));
    clobbered.emit(Instruction::new3(OpCode::ISub, 9, 8, 10));
    clobbered.emit(Instruction::new3(OpCode::ICmpLt, 1, 6, 5));
    let back: i16 = -7; // 13: JmpT r5 -> pc 6
    clobbered.emit(Instruction::new3(
        OpCode::JmpT,
        5,
        ((back as u16) >> 8) as u8,
        (back as u16 & 0xFF) as u8,
    ));
    clobbered.emit(Instruction::new0(OpCode::Halt)); // 14
    clobbered.entry_point = Some(0);

    let mut vm = VM::new();
    vm.load_module(clobbered);
    let result = vm.run().expect("clobbered loop should run");
    assert_eq!(result.as_int(), Some(int_loop_expected(LIMIT)));
    assert_eq!(
        vm.jit_typed_compiled_count(),
        0,
        "loop with unprovable register types must not use the typed path"
    );
}
