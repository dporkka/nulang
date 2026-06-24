//! CRDT Manager for Nulang.
//!
//! The `CrdtManager` owns all local CRDT replicas and handles inter-node
//! synchronization. Actors interact with CRDTs through `CrdtHandle`s, which
//! are lightweight references to the actual CRDT stored in the manager.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

// Import from sibling modules
use super::crdt::{Crdt, GCounter, PNCounter, GSet, ORSet, AWORSet};
use super::crdt_reg::{LWWRegister, MVRegister, RGA};

// ---------------------------------------------------------------------------
// CrdtId -- globally unique identifier for a replicated CRDT
// ---------------------------------------------------------------------------

/// A globally unique identifier for a CRDT instance.
///
/// CRDTs with the same `CrdtId` on different nodes are replicas of each
/// other and will be synced automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CrdtId(pub u64);

static CRDT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

impl CrdtId {
    pub fn new() -> Self {
        CrdtId(CRDT_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

// ---------------------------------------------------------------------------
// CrdtType -- what kind of CRDT this is
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// CrdtOp -- serialized operation for inter-node sync
// ---------------------------------------------------------------------------

/// A serialized CRDT operation sent between nodes.
#[derive(Debug, Clone, PartialEq)]
pub struct CrdtOp {
    pub crdt_id: CrdtId,
    pub crdt_type: CrdtType,
    /// Serialized CRDT state (from Crdt::to_bytes).
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
        if bytes.len() < 13 {
            return None;
        }
        let crdt_id = CrdtId(u64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3],
            bytes[4], bytes[5], bytes[6], bytes[7],
        ]));
        let crdt_type = match bytes[8] {
            0 => CrdtType::GCounter,
            1 => CrdtType::PNCounter,
            2 => CrdtType::GSet,
            3 => CrdtType::ORSet,
            4 => CrdtType::AWORSet,
            5 => CrdtType::LWWRegister,
            6 => CrdtType::MVRegister,
            7 => CrdtType::RGA,
            _ => return None,
        };
        let payload_len = u32::from_be_bytes([bytes[9], bytes[10], bytes[11], bytes[12]]) as usize;
        if bytes.len() < 13 + payload_len {
            return None;
        }
        let payload = bytes[13..13 + payload_len].to_vec();
        Some(CrdtOp { crdt_id, crdt_type, payload })
    }
}

// ---------------------------------------------------------------------------
// CrdtEntry -- a stored CRDT in the manager
// ---------------------------------------------------------------------------

/// Internal wrapper for a CRDT stored in the manager.
#[derive(Debug, Clone)]
pub enum CrdtEntry {
    GCounter(GCounter),
    PNCounter(PNCounter),
    GSet(GSet<String>),
    ORSet(ORSet<String>),
    AWORSet(AWORSet<String>),
    LWWRegister(LWWRegister<String>),
    MVRegister(MVRegister<String>),
    RGA(RGA<String>),
}

impl CrdtEntry {
    /// Serialize this entry's payload (without the wrapper).
    pub fn payload_bytes(&self) -> Vec<u8> {
        match self {
            CrdtEntry::GCounter(c) => c.to_bytes(),
            CrdtEntry::PNCounter(c) => c.to_bytes(),
            CrdtEntry::GSet(c) => c.to_bytes(),
            CrdtEntry::ORSet(c) => c.to_bytes(),
            CrdtEntry::AWORSet(c) => c.to_bytes(),
            CrdtEntry::LWWRegister(c) => c.to_bytes(),
            CrdtEntry::MVRegister(c) => c.to_bytes(),
            CrdtEntry::RGA(c) => c.to_bytes(),
        }
    }

    /// Get the CRDT type.
    pub fn crdt_type(&self) -> CrdtType {
        match self {
            CrdtEntry::GCounter(_) => CrdtType::GCounter,
            CrdtEntry::PNCounter(_) => CrdtType::PNCounter,
            CrdtEntry::GSet(_) => CrdtType::GSet,
            CrdtEntry::ORSet(_) => CrdtType::ORSet,
            CrdtEntry::AWORSet(_) => CrdtType::AWORSet,
            CrdtEntry::LWWRegister(_) => CrdtType::LWWRegister,
            CrdtEntry::MVRegister(_) => CrdtType::MVRegister,
            CrdtEntry::RGA(_) => CrdtType::RGA,
        }
    }

    /// Merge another entry of the same type into this one.
    /// Returns false if the types don't match.
    pub fn merge_entry(&mut self, other: &CrdtEntry) -> bool {
        use super::crdt::Crdt;
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

// ---------------------------------------------------------------------------
// CrdtManager
// ---------------------------------------------------------------------------

/// Manages all local CRDT replicas and handles inter-node synchronization.
pub struct CrdtManager {
    /// This node's ID.
    node_id: u64,
    /// Owned CRDT replicas: CrdtId -> entry.
    entries: HashMap<CrdtId, CrdtEntry>,
    /// Pending operations to sync to other nodes.
    pending_ops: Vec<CrdtOp>,
    /// How many ops have been synced total.
    ops_synced: u64,
}

impl CrdtManager {
    pub fn new(node_id: u64) -> Self {
        CrdtManager {
            node_id,
            entries: HashMap::new(),
            pending_ops: Vec::new(),
            ops_synced: 0,
        }
    }

    // -- Factory methods for creating CRDTs ---------------------------

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
        let set = ORSet::new(self.node_id);
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

    // -- CRUD operations ----------------------------------------------

    /// Get a mutable reference to a GCounter by ID.
    pub fn get_gcounter_mut(&mut self, id: CrdtId) -> Option<&mut GCounter> {
        match self.entries.get_mut(&id) {
            Some(CrdtEntry::GCounter(c)) => Some(c),
            _ => None,
        }
    }

    pub fn get_pncounter_mut(&mut self, id: CrdtId) -> Option<&mut PNCounter> {
        match self.entries.get_mut(&id) {
            Some(CrdtEntry::PNCounter(c)) => Some(c),
            _ => None,
        }
    }

    pub fn get_gset_mut(&mut self, id: CrdtId) -> Option<&mut GSet<String>> {
        match self.entries.get_mut(&id) {
            Some(CrdtEntry::GSet(c)) => Some(c),
            _ => None,
        }
    }

    pub fn get_orset_mut(&mut self, id: CrdtId) -> Option<&mut ORSet<String>> {
        match self.entries.get_mut(&id) {
            Some(CrdtEntry::ORSet(c)) => Some(c),
            _ => None,
        }
    }

    pub fn get_aworset_mut(&mut self, id: CrdtId) -> Option<&mut AWORSet<String>> {
        match self.entries.get_mut(&id) {
            Some(CrdtEntry::AWORSet(c)) => Some(c),
            _ => None,
        }
    }

    pub fn get_lwwregister_mut(&mut self, id: CrdtId) -> Option<&mut LWWRegister<String>> {
        match self.entries.get_mut(&id) {
            Some(CrdtEntry::LWWRegister(c)) => Some(c),
            _ => None,
        }
    }

    pub fn get_mvregister_mut(&mut self, id: CrdtId) -> Option<&mut MVRegister<String>> {
        match self.entries.get_mut(&id) {
            Some(CrdtEntry::MVRegister(c)) => Some(c),
            _ => None,
        }
    }

    pub fn get_rga_mut(&mut self, id: CrdtId) -> Option<&mut RGA<String>> {
        match self.entries.get_mut(&id) {
            Some(CrdtEntry::RGA(c)) => Some(c),
            _ => None,
        }
    }

    // -- Sync operations ----------------------------------------------

    /// Apply an incoming CRDT operation from another node.
    ///
    /// Looks up the local replica by ID and merges the remote state into it.
    /// If no local replica exists, creates one from the remote state.
    pub fn apply_op(&mut self, op: CrdtOp) {
        if let Some(entry) = self.entries.get_mut(&op.crdt_id) {
            // Deserialize the remote state and merge it
            let merged = match entry {
                CrdtEntry::GCounter(c) => {
                    if let Some(remote) = GCounter::from_bytes(&op.payload) {
                        c.merge(&remote);
                        true
                    } else { false }
                }
                CrdtEntry::PNCounter(c) => {
                    if let Some(remote) = PNCounter::from_bytes(&op.payload) {
                        c.merge(&remote);
                        true
                    } else { false }
                }
                CrdtEntry::GSet(c) => {
                    if let Some(remote) = GSet::<String>::from_bytes(&op.payload) {
                        c.merge(&remote);
                        true
                    } else { false }
                }
                CrdtEntry::ORSet(c) => {
                    if let Some(remote) = ORSet::<String>::from_bytes(&op.payload) {
                        c.merge(&remote);
                        true
                    } else { false }
                }
                CrdtEntry::AWORSet(c) => {
                    if let Some(remote) = AWORSet::<String>::from_bytes(&op.payload) {
                        c.merge(&remote);
                        true
                    } else { false }
                }
                CrdtEntry::LWWRegister(c) => {
                    if let Some(remote) = LWWRegister::<String>::from_bytes(&op.payload) {
                        c.merge(&remote);
                        true
                    } else { false }
                }
                CrdtEntry::MVRegister(c) => {
                    if let Some(remote) = MVRegister::<String>::from_bytes(&op.payload) {
                        c.merge(&remote);
                        true
                    } else { false }
                }
                CrdtEntry::RGA(c) => {
                    if let Some(remote) = RGA::<String>::from_bytes(&op.payload) {
                        c.merge(&remote);
                        true
                    } else { false }
                }
            };
            if merged {
                self.ops_synced += 1;
            }
        } else {
            // No local replica -- create one from the remote state
            let entry = match op.crdt_type {
                CrdtType::GCounter => {
                    GCounter::from_bytes(&op.payload).map(CrdtEntry::GCounter)
                }
                CrdtType::PNCounter => {
                    PNCounter::from_bytes(&op.payload).map(CrdtEntry::PNCounter)
                }
                CrdtType::GSet => {
                    GSet::<String>::from_bytes(&op.payload).map(CrdtEntry::GSet)
                }
                CrdtType::ORSet => {
                    ORSet::<String>::from_bytes(&op.payload).map(CrdtEntry::ORSet)
                }
                CrdtType::AWORSet => {
                    AWORSet::<String>::from_bytes(&op.payload).map(CrdtEntry::AWORSet)
                }
                CrdtType::LWWRegister => {
                    LWWRegister::<String>::from_bytes(&op.payload).map(CrdtEntry::LWWRegister)
                }
                CrdtType::MVRegister => {
                    MVRegister::<String>::from_bytes(&op.payload).map(CrdtEntry::MVRegister)
                }
                CrdtType::RGA => {
                    RGA::<String>::from_bytes(&op.payload).map(CrdtEntry::RGA)
                }
            };
            if let Some(e) = entry {
                self.entries.insert(op.crdt_id, e);
                self.ops_synced += 1;
            }
        }
    }

    /// Generate sync operations for all local CRDTs.
    ///
    /// Returns a list of `CrdtOp`s that should be sent to other nodes.
    pub fn generate_sync_ops(&mut self) -> Vec<CrdtOp> {
        let mut ops = Vec::new();
        for (id, entry) in &self.entries {
            ops.push(CrdtOp {
                crdt_id: *id,
                crdt_type: entry.crdt_type(),
                payload: entry.payload_bytes(),
            });
        }
        ops
    }

    /// Queue a local mutation for sync.
    ///
    /// After an actor modifies a CRDT, this method captures the updated
    /// state so it can be sent to other nodes.
    pub fn queue_sync(&mut self, id: CrdtId) {
        if let Some(entry) = self.entries.get(&id) {
            self.pending_ops.push(CrdtOp {
                crdt_id: id,
                crdt_type: entry.crdt_type(),
                payload: entry.payload_bytes(),
            });
        }
    }

    /// Take all pending ops (for sending to the network layer).
    pub fn drain_pending_ops(&mut self) -> Vec<CrdtOp> {
        std::mem::take(&mut self.pending_ops)
    }

    /// Number of CRDTs managed.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn ops_synced(&self) -> u64 {
        self.ops_synced
    }
}
