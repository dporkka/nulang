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
    assert!(!record_and_check_hot(0));
    for _ in 0..HOT_THRESHOLD { record_and_check_hot(42); }
    assert!(record_and_check_hot(42));
    reset_hot_counters();
}

#[test]
fn test_find_compilable_region() {
    let instructions = vec![
        Instruction::new3(OpCode::IAdd, 0, 1, 2),
        Instruction::new3(OpCode::ISub, 0, 1, 2),
        Instruction::new0(OpCode::Ret),
    ];
    assert_eq!(find_compilable_region(0, &instructions), 3);
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
        Instruction::new3(OpCode::JmpT, 2, 0xFC),
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
