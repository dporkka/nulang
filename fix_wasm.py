import re

with open("src/mir_wasm.rs", "r") as f:
    content = f.read()

# 1. Fix 256 locals
content = re.sub(
    r"let local_count = func\.locals\.len\(\) \+ func\.params\.len\(\) \+ func\.captures\.len\(\);\n\s*let wasm_locals: Vec<_> = \(0\.\.local_count\)\.map\(\|\_\| \(1u32, ValType::I64\)\)\.collect\(\);",
    r"let local_count = func.locals.len() + func.params.len() + func.captures.len();\n        let wasm_locals: Vec<_> = vec![(256u32, ValType::I64)];",
    content
)

# 2. Pass func to compile_terminator
content = re.sub(
    r"self\.compile_terminator\(&mut body, &block\.terminator, &labels, li\);",
    r"self.compile_terminator(&mut body, &block.terminator, &labels, li, func);",
    content
)
content = re.sub(
    r"fn compile_terminator\(\n\s*&self,\n\s*body: &mut Function,\n\s*term: &Terminator,\n\s*labels: &HashMap<BlockId, u32>,\n\s*cur: u32,\n\s*\)",
    r"fn compile_terminator(\n        &self,\n        body: &mut Function,\n        term: &Terminator,\n        labels: &HashMap<BlockId, u32>,\n        cur: u32,\n        func: &mir::Function,\n    )",
    content
)

# 3. Fix Branch Terminator
content = re.sub(
    r"Terminator::Branch \{ cond, then_, else_ \} => \{\n\s*body\.instruction\(&Instruction::LocalGet\(cond\.0\)\);\n\s*body\.instruction\(&Instruction::I64Const\(1\)\);\n\s*body\.instruction\(&Instruction::I64And\);",
    r"Terminator::Branch { cond, then_, else_ } => {\n                body.instruction(&Instruction::LocalGet(self.mir_local(cond, func)));\n                body.instruction(&Instruction::I64Const(1));\n                body.instruction(&Instruction::I64And);\n                body.instruction(&Instruction::I32WrapI64);",
    content
)

# 4. Fix Unary Ops
content = re.sub(
    r"RValue::Unary\(_, _\) => \{\n\s*body\.instruction\(&Instruction::I64Const\(value_layout::TAG_NIL as i64\)\);\n\s*\}",
    r"""RValue::Unary(op, a) => {
                self.compile_unary(body, *op, a, func);
            }""",
    content
)

unary_impl = """    fn compile_unary(
        &self,
        body: &mut Function,
        op: crate::ast::UnOp,
        a: &mir::LocalId,
        func: &mir::Function,
    ) {
        use crate::ast::UnOp;
        let pm = value_layout::PAYLOAD_MASK as i64;
        
        match op {
            UnOp::Neg => {
                body.instruction(&Instruction::I64Const(0));
                body.instruction(&Instruction::LocalGet(self.mir_local(a, func)));
                body.instruction(&Instruction::I64Const(pm));
                body.instruction(&Instruction::I64And);
                body.instruction(&Instruction::I64Sub);
                body.instruction(&Instruction::I64Const(pm));
                body.instruction(&Instruction::I64And);
                body.instruction(&Instruction::I64Const(value_layout::TAG_INT as i64));
                body.instruction(&Instruction::I64Or);
            }
            UnOp::Not => {
                let tf = value_layout::tag_bool(false) as i64;
                let tt = value_layout::tag_bool(true) as i64;
                body.instruction(&Instruction::LocalGet(self.mir_local(a, func)));
                body.instruction(&Instruction::I64Const(tt));
                body.instruction(&Instruction::I64Eq);
                body.instruction(&Instruction::I64ExtendI32S);
                body.instruction(&Instruction::I64Const(tf - tt));
                body.instruction(&Instruction::I64Mul);
                body.instruction(&Instruction::I64Const(tt));
                body.instruction(&Instruction::I64Add);
            }
            _ => {
                body.instruction(&Instruction::I64Const(value_layout::TAG_NIL as i64));
            }
        }
    }

"""
content = content.replace("    fn compile_const(", unary_impl + "    fn compile_const(")

# 5. Fix Binary Ops
content = re.sub(
    r"body\.instruction\(&Instruction::LocalGet\(254\)\);\n\n\s*match op \{",
    r"""body.instruction(&Instruction::LocalGet(254));

        let sign_extend_both = |b: &mut Function| {
            b.instruction(&Instruction::LocalSet(254));
            b.instruction(&Instruction::I64Const(16));
            b.instruction(&Instruction::I64Shl);
            b.instruction(&Instruction::I64Const(16));
            b.instruction(&Instruction::I64ShrS);
            b.instruction(&Instruction::LocalGet(254));
            b.instruction(&Instruction::I64Const(16));
            b.instruction(&Instruction::I64Shl);
            b.instruction(&Instruction::I64Const(16));
            b.instruction(&Instruction::I64ShrS);
        };

        match op {""",
    content
)

content = content.replace(
    "BinOp::Div => {\n                body.instruction(&Instruction::I64DivS);\n            }",
    "BinOp::Div => {\n                sign_extend_both(body);\n                body.instruction(&Instruction::I64DivS);\n            }"
)
content = content.replace(
    "BinOp::Mod => {\n                body.instruction(&Instruction::I64RemS);\n            }",
    "BinOp::Mod => {\n                sign_extend_both(body);\n                body.instruction(&Instruction::I64RemS);\n            }"
)
content = content.replace(
    "cmp @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge) => {\n                match cmp {",
    "cmp @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge) => {\n                sign_extend_both(body);\n                match cmp {"
)
content = content.replace(
    "body.instruction(&Instruction::I64Const(ti));\n        body.instruction(&Instruction::I64Or);",
    "body.instruction(&Instruction::I64Const(pm));\n        body.instruction(&Instruction::I64And);\n        body.instruction(&Instruction::I64Const(ti));\n        body.instruction(&Instruction::I64Or);"
)

with open("src/mir_wasm.rs", "w") as f:
    f.write(content)
