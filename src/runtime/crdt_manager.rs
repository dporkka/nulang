//! CRDT Manager for Nulang.
//!
//! The `CrdtManager` owns all local CRDT replicas and handles inter-node
//! synchronization. Actors interact with CRDTs through `CrdtHandle`s, which
//! are lightweight references to the actual CRDT stored in the manager.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use super::crdt::{Crdt, GCounter, PNCounter, GSet, ORSet, AWORSet};
use super::crdt_reg::{LWWRegister, MVRegister, RGA};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CrdtId(pub u64);

static CRDT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

impl CrdtId {
    pub fn new() -> Self {
        CrdtId(CRDT_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrdtType {
    GCounter,
    PNCounter,
    GSet,
    ORSet,
    AWORSet,
    LWWRegister,
    MVRegister,
    RGA,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CrdtOp {
    pub crdt_id: CrdtId,
    pub crdt_type: CrdtType,
    pub payload: Vec<u8>,
}

impl CrdtOp {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.crdt_id.0.to_be_bytes());
        buf.push(self.crdt_type as u8);
        buf.extend_from_slice(&(self.payload.len() as u32).to_be_bytes());
        buf.extend_from_slice(&self.payload);
        buf
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 13 { return None; }
        let crdt_id = CrdtId(u64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3],
            bytes[4], bytes[5], bytes[6], bytes[7],
        ]));
        let crdt_type = match bytes[8] {
            0 => CrdtType::GCounter, 1 => CrdtType::PNCounter,
            2 => CrdtType::GSet, 3 => CrdtType::ORSet,
            4 => CrdtType::AWORSet, 5 => CrdtType::LWWRegister,
            6 => CrdtType::MVRegister, 7 => CrdtType::RGA,
            _ => return None,
        };
        let payload_len = u32::from_be_bytes([bytes[9], bytes[10], bytes[11], bytes[12]]) as usize;
        if bytes.len() < 13 + payload_len { return None; }
        let payload = bytes[13..13 + payload_len].to_vec();
        Some(CrdtOp { crdt_id, crdt_type, payload })
    }
}

#[derive(Debug, Clone)]
pub enum CrdtEntry {
    GCounter(GCounter), PNCounter(PNCounter),
    GSet(GSet<String>), ORSet(ORSet<String>), AWORSet(AWORSet<String>),
    LWWRegister(LWWRegister<String>), MVRegister(MVRegister<String>), RGA(RGA<String>),
}

impl CrdtEntry {
    pub fn payload_bytes(&self) -> Vec<u8> {
        match self {
            CrdtEntry::GCounter(c) => c.to_bytes(), CrdtEntry::PNCounter(c) => c.to_bytes(),
            CrdtEntry::GSet(c) => c.to_bytes(), CrdtEntry::ORSet(c) => c.to_bytes(),
            CrdtEntry::AWORSet(c) => c.to_bytes(), CrdtEntry::LWWRegister(c) => c.to_bytes(),
            CrdtEntry::MVRegister(c) => c.to_bytes(), CrdtEntry::RGA(c) => c.to_bytes(),
        }
    }

    pub fn crdt_type(&self) -> CrdtType {
        match self {
            CrdtEntry::GCounter(_) => CrdtType::GCounter, CrdtEntry::PNCounter(_) => CrdtType::PNCounter,
            CrdtEntry::GSet(_) => CrdtType::GSet, CrdtEntry::ORSet(_) => CrdtType::ORSet,
            CrdtEntry::AWORSet(_) => CrdtType::AWORSet, CrdtEntry::LWWRegister(_) => CrdtType::LWWRegister,
            CrdtEntry::MVRegister(_) => CrdtType::MVRegister, CrdtEntry::RGA(_) => CrdtType::RGA,
        }
    }

    pub fn merge_entry(&mut self, other: &CrdtEntry) -> bool {
        match (self, other) {
            (CrdtEntry::GCounter(a), CrdtEntry::GCounter(b)) => { a.merge(b); true }
            (CrdtEntry::PNCounter(a), CrdtEntry::PNCounter(b)) => { a.merge(b); true }
            (CrdtEntry::GSet(a), CrdtEntry::GSet(b)) => { a.merge(b); true }
            (CrdtEntry::ORSet(a), CrdtEntry::ORSet(b)) => { a.merge(b); true }
            (CrdtEntry::AWORSet(a), CrdtEntry::AWORSet(b)) => { a.merge(b); true }
            (CrdtEntry::LWWRegister(a), CrdtEntry::LWWRegister(b)) => { a.merge(b); true }
            (CrdtEntry::MVRegister(a), CrdtEntry::MVRegister(b)) => { a.merge(b); true }
            (CrdtEntry::RGA(a), CrdtEntry::RGA(b)) => { a.merge(b); true }
            _ => false,
        }
    }
}

pub struct CrdtManager {
    node_id: u64,
    entries: HashMap<CrdtId, CrdtEntry>,
    pending_ops: Vec<CrdtOp>,
    ops_synced: u64,
}

impl CrdtManager {
    pub fn new(node_id: u64) -> Self {
        CrdtManager { node_id, entries: HashMap::new(), pending_ops: Vec::new(), ops_synced: 0 }
    }

    pub fn create_gcounter(&mut self) -> (CrdtId, GCounter) {
        let id = CrdtId::new();
        let counter = GCounter::new(self.node_id);
        self.entries.insert(id, CrdtEntry::GCounter(counter.clone()));
        (id, counter)
    }

    pub fn create_pncounter(&mut self) -> (CrdtId, PNCounter) {
        let id = CrdtId::new();
        let counter = PNCounter::new(self.node_id);
        self.entries.insert(id, CrdtEntry::PNCounter(counter.clone()));
        (id, counter)
    }

    pub fn create_gset(&mut self) -> (CrdtId, GSet<String>) {
        let id = CrdtId::new();
        let set = GSet::new();
        self.entries.insert(id, CrdtEntry::GSet(set.clone()));
        (id, set)
    }

    pub fn create_orset(&mut self) -> (CrdtId, ORSet<String>) {
        let id = CrdtId::new();
        let set = ORSet::new(self.node_id as u32);
        self.entries.insert(id, CrdtEntry::ORSet(set.clone()));
        (id, set)
    }

    pub fn create_aworset(&mut self) -> (CrdtId, AWORSet<String>) {
        let id = CrdtId::new();
        let set = AWORSet::new(self.node_id);
        self.entries.insert(id, CrdtEntry::AWORSet(set.clone()));
        (id, set)
    }

    pub fn create_lwwregister(&mut self, initial: String) -> (CrdtId, LWWRegister<String>) {
        let id = CrdtId::new();
        let reg = LWWRegister::new(self.node_id, initial);
        self.entries.insert(id, CrdtEntry::LWWRegister(reg.clone()));
        (id, reg)
    }

    pub fn create_mvregister(&mut self) -> (CrdtId, MVRegister<String>) {
        let id = CrdtId::new();
        let reg = MVRegister::new(self.node_id);
        self.entries.insert(id, CrdtEntry::MVRegister(reg.clone()));
        (id, reg)
    }

    pub fn create_rga(&mut self) -> (CrdtId, RGA<String>) {
        let id = CrdtId::new();
        let rga = RGA::new(self.node_id);
        self.entries.insert(id, CrdtEntry::RGA(rga.clone()));
        (id, rga)
    }

    pub fn get_gcounter_mut(&mut self, id: CrdtId) -> Option<&mut GCounter> {
        match self.entries.get_mut(&id) { Some(CrdtEntry::GCounter(c)) => Some(c), _ => None }
    }
    pub fn get_pncounter_mut(&mut self, id: CrdtId) -> Option<&mut PNCounter> {
        match self.entries.get_mut(&id) { Some(CrdtEntry::PNCounter(c)) => Some(c), _ => None }
    }
    pub fn get_gset_mut(&mut self, id: CrdtId) -> Option<&mut GSet<String>> {
        match self.entries.get_mut(&id) { Some(CrdtEntry::GSet(c)) => Some(c), _ => None }
    }
    pub fn get_orset_mut(&mut self, id: CrdtId) -> Option<&mut ORSet<String>> {
        match self.entries.get_mut(&id) { Some(CrdtEntry::ORSet(c)) => Some(c), _ => None }
    }
    pub fn get_aworset_mut(&mut self, id: CrdtId) -> Option<&mut AWORSet<String>> {
        match self.entries.get_mut(&id) { Some(CrdtEntry::AWORSet(c)) => Some(c), _ => None }
    }
    pub fn get_lwwregister_mut(&mut self, id: CrdtId) -> Option<&mut LWWRegister<String>> {
        match self.entries.get_mut(&id) { Some(CrdtEntry::LWWRegister(c)) => Some(c), _ => None }
    }
    pub fn get_mvregister_mut(&mut self, id: CrdtId) -> Option<&mut MVRegister<String>> {
        match self.entries.get_mut(&id) { Some(CrdtEntry::MVRegister(c)) => Some(c), _ => None }
    }
    pub fn get_rga_mut(&mut self, id: CrdtId) -> Option<&mut RGA<String>> {
        match self.entries.get_mut(&id) { Some(CrdtEntry::RGA(c)) => Some(c), _ => None }
    }

    pub fn apply_op(&mut self, op: CrdtOp) {
        if let Some(entry) = self.entries.get_mut(&op.crdt_id) {
            let merged = match entry {
                CrdtEntry::GCounter(c) => GCounter::from_bytes(&op.payload).map(|r| { c.merge(&r); }).is_some(),
                CrdtEntry::PNCounter(c) => PNCounter::from_bytes(&op.payload).map(|r| { c.merge(&r); }).is_some(),
                CrdtEntry::GSet(c) => GSet::<String>::from_bytes(&op.payload).map(|r| { c.merge(&r); }).is_some(),
                CrdtEntry::ORSet(c) => ORSet::<String>::from_bytes(&op.payload).map(|r| { c.merge(&r); }).is_some(),
                CrdtEntry::AWORSet(c) => AWORSet::<String>::from_bytes(&op.payload).map(|r| { c.merge(&r); }).is_some(),
                CrdtEntry::LWWRegister(c) => LWWRegister::<String>::from_bytes(&op.payload).map(|r| { c.merge(&r); }).is_some(),
                CrdtEntry::MVRegister(c) => MVRegister::<String>::from_bytes(&op.payload).map(|r| { c.merge(&r); }).is_some(),
                CrdtEntry::RGA(c) => RGA::<String>::from_bytes(&op.payload).map(|r| { c.merge(&r); }).is_some(),
            };
            if merged { self.ops_synced += 1; }
        } else {
            let entry = match op.crdt_type {
                CrdtType::GCounter => GCounter::from_bytes(&op.payload).map(CrdtEntry::GCounter),
                CrdtType::PNCounter => PNCounter::from_bytes(&op.payload).map(CrdtEntry::PNCounter),
                CrdtType::GSet => GSet::<String>::from_bytes(&op.payload).map(CrdtEntry::GSet),
                CrdtType::ORSet => ORSet::<String>::from_bytes(&op.payload).map(CrdtEntry::ORSet),
                CrdtType::AWORSet => AWORSet::<String>::from_bytes(&op.payload).map(CrdtEntry::AWORSet),
                CrdtType::LWWRegister => LWWRegister::<String>::from_bytes(&op.payload).map(CrdtEntry::LWWRegister),
                CrdtType::MVRegister => MVRegister::<String>::from_bytes(&op.payload).map(CrdtEntry::MVRegister),
                CrdtType::RGA => RGA::<String>::from_bytes(&op.payload).map(CrdtEntry::RGA),
            };
            if let Some(e) = entry { self.entries.insert(op.crdt_id, e); self.ops_synced += 1; }
        }
    }

    pub fn generate_sync_ops(&mut self) -> Vec<CrdtOp> {
        self.entries.iter().map(|(id, entry)| CrdtOp {
            crdt_id: *id, crdt_type: entry.crdt_type(), payload: entry.payload_bytes(),
        }).collect()
    }

    pub fn queue_sync(&mut self, id: CrdtId) {
        if let Some(entry) = self.entries.get(&id) {
            self.pending_ops.push(CrdtOp { crdt_id: id, crdt_type: entry.crdt_type(), payload: entry.payload_bytes() });
        }
    }

    pub fn drain_pending_ops(&mut self) -> Vec<CrdtOp> {
        std::mem::take(&mut self.pending_ops)
    }

    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn ops_synced(&self) -> u64 { self.ops_synced }
}
