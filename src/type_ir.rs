use crate::types::{Capability, PrimitiveType};
use blake3::Hasher;

/// NTIR Opcodes
const T_NONE: u8 = 0x01;
const T_BOOL: u8 = 0x02;
const T_I64: u8 = 0x03;
const T_F64: u8 = 0x04;
const T_STR: u8 = 0x05;

const CAP_ISO: u8 = 0x10;
const CAP_TRN: u8 = 0x11;
const CAP_REF: u8 = 0x12;
const CAP_VAL: u8 = 0x13;
const CAP_BOX: u8 = 0x14;
const CAP_TAG: u8 = 0x15;

const NODE_RECORD: u8 = 0x20;
const NODE_TUPLE: u8 = 0x21;
const NODE_VARIANT: u8 = 0x22;

const REF_CYCLE: u8 = 0x30;

#[derive(Debug, Clone, PartialEq)]
pub enum NtirNode {
    Primitive(PrimitiveType),
    Capability(Capability, Box<NtirNode>),
    Record(Vec<(String, NtirNode)>),
    Tuple(Vec<NtirNode>),
    Variant(Vec<(String, NtirNode)>),
    Cycle(u64),
}

impl NtirNode {
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Hasher::new();
        hasher.update(b"NTIR"); // Magic bytes
        hasher.update(&[0x00, 0x01]); // Version 1

        self.serialize_into(&mut hasher);

        hasher.finalize().into()
    }

    fn serialize_into(&self, hasher: &mut Hasher) {
        match self {
            NtirNode::Primitive(p) => {
                match p {
                    PrimitiveType::Unit => hasher.update(&[T_NONE]),
                    PrimitiveType::Bool => hasher.update(&[T_BOOL]),
                    PrimitiveType::Int => hasher.update(&[T_I64]),
                    PrimitiveType::Float => hasher.update(&[T_F64]),
                    PrimitiveType::String => hasher.update(&[T_STR]),
                    _ => hasher.update(&[T_NONE]), // Never, Address, Nil mapping
                };
            }
            NtirNode::Capability(cap, inner) => {
                let op = match cap {
                    Capability::Iso | Capability::LinearIso => CAP_ISO,
                    Capability::Trn => CAP_TRN,
                    Capability::Ref => CAP_REF,
                    Capability::Val | Capability::Linear => CAP_VAL,
                    Capability::Box => CAP_BOX,
                    Capability::Tag => CAP_TAG,
                };
                hasher.update(&[op]);
                inner.serialize_into(hasher);
            }
            NtirNode::Record(fields) => {
                hasher.update(&[NODE_RECORD]);
                write_leb128(hasher, fields.len() as u64);
                for (name, ty) in fields {
                    write_leb128(hasher, name.len() as u64);
                    hasher.update(name.as_bytes());
                    ty.serialize_into(hasher);
                }
            }
            NtirNode::Tuple(elems) => {
                hasher.update(&[NODE_TUPLE]);
                write_leb128(hasher, elems.len() as u64);
                for elem in elems {
                    elem.serialize_into(hasher);
                }
            }
            NtirNode::Variant(tags) => {
                hasher.update(&[NODE_VARIANT]);
                write_leb128(hasher, tags.len() as u64);
                for (name, ty) in tags {
                    write_leb128(hasher, name.len() as u64);
                    hasher.update(name.as_bytes());
                    ty.serialize_into(hasher);
                }
            }
            NtirNode::Cycle(dist) => {
                hasher.update(&[REF_CYCLE]);
                write_leb128(hasher, *dist);
            }
        }
    }
}

fn write_leb128(hasher: &mut Hasher, mut val: u64) {
    loop {
        let mut byte = (val & 0x7F) as u8;
        val >>= 7;
        if val != 0 {
            byte |= 0x80;
        }
        hasher.update(&[byte]);
        if val == 0 {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Type;

    #[test]
    fn test_record_hash_order_independence() {
        let t1 = Type::Record(vec![
            ("a".to_string(), Type::Primitive(PrimitiveType::Int)),
            ("b".to_string(), Type::Primitive(PrimitiveType::String)),
        ]);
        let t2 = Type::Record(vec![
            ("b".to_string(), Type::Primitive(PrimitiveType::String)),
            ("a".to_string(), Type::Primitive(PrimitiveType::Int)),
        ]);

        assert_eq!(t1.to_ntir().hash(), t2.to_ntir().hash());
    }
}
