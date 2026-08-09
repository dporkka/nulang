// bootstrap/FORMAT.md — Emitter JSON format specification
//
// The bootstrap emitter (bootstrap/emitter.nula) outputs JSON in this
// format. The Rust host converter reads this JSON and produces a .nbc
// binary via CodeModule::from_bootstrap_json().
//
// ## JSON Schema
//
// {
//   "name": "module_name",           // string, module identifier
//   "instructions": [                // array of hex strings, 8 chars each
//     "07000000",                    //   4 bytes big-endian: opcode|op1|op2|op3
//     "57000000"
//   ],
//   "constants": [                   // array of constant objects
//     {"type": "Int", "value": 42},  //   supported types: Int, Float, Bool, String
//     {"type": "String", "value": "hello"}
//   ],
//   "entry_point": 0,                // optional, default 0
//   "function_table": [0],           // optional, code offsets for named functions
//   "exports": [                     // optional, public symbols for library linking
//     {
//       "name": "add",
//       "kind": "function",          // "function" or "constant"
//       "index": 0,                  // index into function_table or constants
//       "type_sig": "fn(Int,Int)->Int"
//     }
//   ]
// }
//
// ## Constant types
//
// {"type": "Int", "value": <i64>}
// {"type": "Float", "value": <f64>}
// {"type": "Bool", "value": <true|false>}
// {"type": "String", "value": "<text>"}
//
// ## Instructions
//
// Each instruction is an 8-character hex string encoding 4 bytes
// in big-endian order. The bytes are: opcode (u8), op1 (u8), op2 (u8),
// op3 (u8). The host decodes via Instruction::decode(u32::from_str_radix(hex, 16)).

// This file is documentation only. The implementation is in:
//   src/bytecode.rs — CodeModule::from_bootstrap_json()
