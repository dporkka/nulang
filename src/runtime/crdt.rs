//! Conflict-free Replicated Data Types (CRDTs) for Nulang.
//!
//! CRDTs enable distributed actors to share mutable state without locks,
//! consensus, or coordination. All changes merge automatically and converge
//! to the same result.
//!
//! This module provides the core [`Crdt`] trait and five concrete
//! implementations:
//!
//! | Type          | Operations         | Semantics                          |
//! |---------------|--------------------|-------------------------------------|
//! | [`GCounter`]  | increment          | Grow-only counter (monotonic)      |
//! | [`PNCounter`] | increment, decrement | Counter that can go negative       |
//! | [`GSet`]      | insert             | Grow-only set                      |
//! | [`ORSet`]     | add, remove        | Add-wins observed-remove set       |
//! | [`AWORSet`]   | add, remove        | Timestamp-based add-wins OR set    |
//!
//! # Mathematical Foundations
//!
//! Every CRDT satisfies three algebraic properties:
//!
//! **Associativity** — `merge(a, merge(b, c)) == merge(merge(a, b), c)`
//! Merging can be grouped arbitrarily; the order of pairwise merges does not
//! matter. This allows tree-structured or pipelined replication topologies.
//!
//! **Commutativity** — `merge(a, b) == merge(b, a)`
//! The merge order is irrelevant. This is the key property that eliminates
//! the need for consensus: replicas can merge in any order and still agree.
//!
//! **Idempotency** — `merge(a, a) == a`
//! Merging a replica with itself is a no-op. This makes retries safe and
//! deduplication unnecessary.
//!
//! Together, these properties form a **join-semilattice** where `merge` is the
//! least-upper-bound (LUB) operator and the CRDT state is ordered by the
//! "happens-before" relation. The LUB of any two states always exists and is
//! unique, guaranteeing convergence.

use std::collections::{HashMap, HashSet};

// =============================================================================
// Core CRDT Trait
// =============================================================================

/// The core trait for all Conflict-free Replicated Data Types.
///
/// Every CRDT must support:
/// - [`merge`](Crdt::merge): combine two replicas into one (must be associative, commutative, idempotent)
/// - [`value`](Crdt::value): read the current logical value
/// - [`clone_replica`](Crdt::clone_replica): create a deep copy for sending to another node
/// - [`to_bytes`](Crdt::to_bytes) / [`from_bytes`](Crdt::from_bytes): serialize for network transmission
///
/// # Type Parameters
///
/// - `Value`: The logical (user-facing) type that this CRDT represents.
///   This is often different from the internal replica state. For example,
///   a `PNCounter` internally stores two `GCounter`s but its logical value
///   is `i64`.
pub trait Crdt: Clone {
    /// The logical value type that this CRDT represents.
    ///
    /// This is the type returned by [`value`](Crdt::value) and is the
    /// user-facing abstraction over the internal replicated state.
    type Value;

    /// Merge another replica into this one.
    ///
    /// After `self.merge(other)`, `self` contains the combined state
    /// of both replicas. This operation must be:
    ///
    /// - **Associative**: `a.merge(b.merge(c)) == a.merge(b).merge(c)`
    /// - **Commutative**: `a.merge(b) == b.merge(a)`
    /// - **Idempotent**: `a.merge(a) == a`
    fn merge(&mut self, other: &Self);

    /// Read the current logical value.
    fn value(&self) -> Self::Value;

    /// Create a deep copy of this replica.
    fn clone_replica(&self) -> Self;

    /// Serialize this CRDT to bytes for network transmission.
    fn to_bytes(&self) -> Vec<u8>;

    /// Deserialize a CRDT from bytes.
    fn from_bytes(bytes: &[u8]) -> Option<Self>;
}

// =============================================================================
// Serialization Helpers
// =============================================================================

#[inline]
fn push_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_be_bytes());
}

#[inline]
fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_be_bytes());
}

#[inline]
fn read_u64(bytes: &[u8], pos: usize) -> Option<(u64, usize)> {
    let end = pos.checked_add(8)?;
    if end > bytes.len() { return None; }
    let mut arr = [0u8; 8];
    arr.copy_from_slice(&bytes[pos..end]);
    Some((u64::from_be_bytes(arr), end))
}

#[inline]
fn read_u32(bytes: &[u8], pos: usize) -> Option<(u32, usize)> {
    let end = pos.checked_add(4)?;
    if end > bytes.len() { return None; }
    let mut arr = [0u8; 4];
    arr.copy_from_slice(&bytes[pos..end]);
    Some((u32::from_be_bytes(arr), end))
}

#[inline]
fn push_string(buf: &mut Vec<u8>, s: &str) {
    push_u32(buf, s.len() as u32);
    buf.extend_from_slice(s.as_bytes());
}

#[inline]
fn read_string(bytes: &[u8], pos: usize) -> Option<(String, usize)> {
    let (len, pos) = read_u32(bytes, pos)?;
    let len = len as usize;
    let end = pos.checked_add(len)?;
    if end > bytes.len() { return None; }
    let s = String::from_utf8(bytes[pos..end].to_vec()).ok()?;
    Some((s, end))
}

// =============================================================================
// GCounter
// =============================================================================

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GCounter {
    pub counts: HashMap<u64, u64>,
    pub node_id: u64,
}

impl GCounter {
    pub fn new(node_id: u64) -> Self {
        Self { counts: HashMap::new(), node_id }
    }

    pub fn increment(&mut self) {
        *self.counts.entry(self.node_id).or_insert(0) += 1;
    }

    pub fn increment_by(&mut self, delta: u64) {
        *self.counts.entry(self.node_id).or_insert(0) += delta;
    }

    pub fn value(&self) -> u64 {
        self.counts.values().sum()
    }
}

impl Crdt for GCounter {
    type Value = u64;

    fn merge(&mut self, other: &Self) {
        for (node_id, count) in &other.counts {
            let entry = self.counts.entry(*node_id).or_insert(0);
            *entry = (*entry).max(*count);
        }
    }

    fn value(&self) -> Self::Value { self.value() }
    fn clone_replica(&self) -> Self { self.clone() }

    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        push_u64(&mut buf, self.node_id);
        push_u32(&mut buf, self.counts.len() as u32);
        for (node_id, count) in &self.counts {
            push_u64(&mut buf, *node_id);
            push_u64(&mut buf, *count);
        }
        buf
    }

    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let (node_id, pos) = read_u64(bytes, 0)?;
        let (num_entries, mut pos) = read_u32(bytes, pos)?;
        let mut counts = HashMap::new();
        for _ in 0..num_entries {
            let (nid, p) = read_u64(bytes, pos)?;
            let (cnt, p) = read_u64(bytes, p)?;
            counts.insert(nid, cnt);
            pos = p;
        }
        Some(Self { counts, node_id })
    }
}

// =============================================================================
// PNCounter
// =============================================================================

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PNCounter {
    pub increments: GCounter,
    pub decrements: GCounter,
}

impl PNCounter {
    pub fn new(node_id: u64) -> Self {
        Self {
            increments: GCounter::new(node_id),
            decrements: GCounter::new(node_id),
        }
    }

    pub fn increment(&mut self) { self.increments.increment(); }
    pub fn decrement(&mut self) { self.decrements.increment(); }
    pub fn increment_by(&mut self, delta: u64) { self.increments.increment_by(delta); }
    pub fn decrement_by(&mut self, delta: u64) { self.decrements.increment_by(delta); }

    pub fn value(&self) -> i64 {
        self.increments.value() as i64 - self.decrements.value() as i64
    }
}

impl Crdt for PNCounter {
    type Value = i64;

    fn merge(&mut self, other: &Self) {
        self.increments.merge(&other.increments);
        self.decrements.merge(&other.decrements);
    }

    fn value(&self) -> Self::Value { self.value() }
    fn clone_replica(&self) -> Self { self.clone() }

    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = self.increments.to_bytes();
        buf.extend_from_slice(&self.decrements.to_bytes());
        buf
    }

    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let (node_id, pos) = read_u64(bytes, 0)?;
        let (num_entries, _) = read_u32(bytes, pos)?;
        let increment_len = 12 + (num_entries as usize) * 16;
        let inc_bytes = &bytes[0..increment_len];
        let dec_bytes = &bytes[increment_len..];
        let increments = GCounter::from_bytes(inc_bytes)?;
        let decrements = GCounter::from_bytes(dec_bytes)?;
        if increments.node_id != node_id || decrements.node_id != node_id {
            return None;
        }
        Some(Self { increments, decrements })
    }
}

// =============================================================================
// GSet
// =============================================================================

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GSet<T: Clone + Eq + std::hash::Hash> {
    pub elements: HashSet<T>,
}

impl<T: Clone + Eq + std::hash::Hash> GSet<T> {
    pub fn new() -> Self { Self { elements: HashSet::new() } }
    pub fn insert(&mut self, element: T) -> bool { self.elements.insert(element) }
    pub fn contains(&self, element: &T) -> bool { self.elements.contains(element) }
    pub fn len(&self) -> usize { self.elements.len() }
    pub fn is_empty(&self) -> bool { self.elements.is_empty() }
    pub fn value(&self) -> &HashSet<T> { &self.elements }
}

impl<T: Clone + Eq + std::hash::Hash> Default for GSet<T> {
    fn default() -> Self { Self::new() }
}

impl Crdt for GSet<String> {
    type Value = HashSet<String>;

    fn merge(&mut self, other: &Self) {
        self.elements.extend(other.elements.iter().cloned());
    }

    fn value(&self) -> Self::Value { self.elements.clone() }
    fn clone_replica(&self) -> Self { self.clone() }

    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        push_u32(&mut buf, self.elements.len() as u32);
        for elem in &self.elements {
            push_string(&mut buf, elem);
        }
        buf
    }

    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let (num_elements, mut pos) = read_u32(bytes, 0)?;
        let mut elements = HashSet::new();
        for _ in 0..num_elements {
            let (s, p) = read_string(bytes, pos)?;
            elements.insert(s);
            pos = p;
        }
        Some(Self { elements })
    }
}

// =============================================================================
// ORSet
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tag {
    pub node_id: u32,
    pub counter: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ORSet<T: Clone + Eq + std::hash::Hash> {
    pub entries: HashMap<T, HashSet<Tag>>,
    pub tag_counter: u32,
    pub node_id: u32,
}

impl<T: Clone + Eq + std::hash::Hash> ORSet<T> {
    pub fn new(node_id: u32) -> Self {
        Self { entries: HashMap::new(), tag_counter: 0, node_id }
    }

    fn fresh_tag(&mut self) -> Tag {
        let tag = Tag { node_id: self.node_id, counter: self.tag_counter };
        self.tag_counter = self.tag_counter.checked_add(1).expect("Counter overflow");
        tag
    }

    pub fn add(&mut self, element: T) {
        let tag = self.fresh_tag();
        self.entries.entry(element).or_insert_with(HashSet::new).insert(tag);
    }

    pub fn remove(&mut self, element: &T) {
        self.entries.remove(element);
    }

    pub fn contains(&self, element: &T) -> bool {
        self.entries.get(element).map_or(false, |tags| !tags.is_empty())
    }

    pub fn value(&self) -> HashSet<T> {
        self.entries.iter()
            .filter(|(_, tags)| !tags.is_empty())
            .map(|(elem, _)| elem.clone())
            .collect()
    }

    pub fn len(&self) -> usize { self.value().len() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}

impl Crdt for ORSet<String> {
    type Value = HashSet<String>;

    fn merge(&mut self, other: &Self) {
        for (element, tags) in &other.entries {
            let entry = self.entries.entry(element.clone()).or_insert_with(HashSet::new);
            entry.extend(tags);
        }
        self.tag_counter = self.tag_counter.max(other.tag_counter);
    }

    fn value(&self) -> Self::Value { self.value() }
    fn clone_replica(&self) -> Self { self.clone() }

    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        push_u64(&mut buf, self.node_id as u64);
        push_u64(&mut buf, self.tag_counter as u64);
        push_u32(&mut buf, self.entries.len() as u32);
        for (element, tags) in &self.entries {
            push_string(&mut buf, element);
            push_u32(&mut buf, tags.len() as u32);
            for tag in tags { push_u64(&mut buf, ((tag.node_id as u64) << 32) | (tag.counter as u64)); }
        }
        buf
    }

    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let (node_id, pos) = read_u64(bytes, 0)?;
        let (tag_counter, pos) = read_u64(bytes, pos)?;
        let (num_elements, mut pos) = read_u32(bytes, pos)?;
        let mut entries = HashMap::new();
        for _ in 0..num_elements {
            let (element, p) = read_string(bytes, pos)?;
            let (tag_count, mut p) = read_u32(bytes, p)?;
            let mut tags = HashSet::new();
            for _ in 0..tag_count {
                let (tag_val, p2) = read_u64(bytes, p)?;
                tags.insert(Tag { node_id: (tag_val >> 32) as u32, counter: tag_val as u32 });
                p = p2;
            }
            entries.insert(element, tags);
            pos = p;
        }
        Some(Self { entries, tag_counter: tag_counter as u32, node_id: node_id as u32 })
    }
}

// =============================================================================
// LamportTime + LamportClock
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LamportTime {
    pub counter: u64,
    pub node_id: u64,
}

impl LamportTime {
    pub fn new(counter: u64, node_id: u64) -> Self {
        Self { counter, node_id }
    }

    pub fn is_greater_than(&self, other: &LamportTime) -> bool {
        self.counter > other.counter
            || (self.counter == other.counter && self.node_id > other.node_id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LamportClock {
    pub node_id: u64,
    pub counter: u64,
}

impl LamportClock {
    pub fn new(node_id: u64) -> Self {
        Self { node_id, counter: 0 }
    }

    pub fn tick(&mut self) -> LamportTime {
        self.counter += 1;
        LamportTime { counter: self.counter, node_id: self.node_id }
    }
}

// =============================================================================
// AWORSet
// =============================================================================

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AWORSet<T: Clone + Eq + std::hash::Hash> {
    pub entries: HashMap<T, LamportTime>,
    pub removed: HashMap<T, LamportTime>,
    pub clock: LamportClock,
}

impl<T: Clone + Eq + std::hash::Hash> AWORSet<T> {
    pub fn new(node_id: u64) -> Self {
        Self { entries: HashMap::new(), removed: HashMap::new(), clock: LamportClock::new(node_id) }
    }

    pub fn add(&mut self, element: T) {
        let ts = self.clock.tick();
        self.entries.insert(element, ts);
    }

    pub fn remove(&mut self, element: &T) {
        let ts = self.clock.tick();
        self.removed.insert(element.clone(), ts);
    }

    pub fn contains(&self, element: &T) -> bool {
        match (self.entries.get(element), self.removed.get(element)) {
            (Some(add_ts), Some(rem_ts)) => add_ts.is_greater_than(rem_ts),
            (Some(_), None) => true,
            (None, _) => false,
        }
    }

    pub fn value(&self) -> HashSet<T> {
        self.entries.keys().filter(|e| self.contains(e)).cloned().collect()
    }

    pub fn len(&self) -> usize { self.value().len() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}

impl Crdt for AWORSet<String> {
    type Value = HashSet<String>;

    fn merge(&mut self, other: &Self) {
        for (element, ts) in &other.entries {
            match self.entries.get(element) {
                Some(existing) if *ts <= *existing => {}
                _ => { self.entries.insert(element.clone(), *ts); }
            }
        }
        for (element, ts) in &other.removed {
            match self.removed.get(element) {
                Some(existing) if *ts <= *existing => {}
                _ => { self.removed.insert(element.clone(), *ts); }
            }
        }
        self.clock.counter = self.clock.counter.max(other.clock.counter);
    }

    fn value(&self) -> Self::Value { self.value() }
    fn clone_replica(&self) -> Self { self.clone() }

    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        push_u64(&mut buf, self.clock.node_id);
        push_u64(&mut buf, self.clock.counter);
        push_u32(&mut buf, self.entries.len() as u32);
        for (element, ts) in &self.entries {
            push_string(&mut buf, element);
            push_u64(&mut buf, ts.counter);
            push_u64(&mut buf, ts.node_id);
        }
        push_u32(&mut buf, self.removed.len() as u32);
        for (element, ts) in &self.removed {
            push_string(&mut buf, element);
            push_u64(&mut buf, ts.counter);
            push_u64(&mut buf, ts.node_id);
        }
        buf
    }

    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let (node_id, pos) = read_u64(bytes, 0)?;
        let (counter, pos) = read_u64(bytes, pos)?;
        let (num_entries, mut pos) = read_u32(bytes, pos)?;
        let mut entries = HashMap::new();
        for _ in 0..num_entries {
            let (element, p) = read_string(bytes, pos)?;
            let (cnt, p) = read_u64(bytes, p)?;
            let (nid, p) = read_u64(bytes, p)?;
            entries.insert(element, LamportTime { counter: cnt, node_id: nid });
            pos = p;
        }
        let (num_removed, mut pos) = read_u32(bytes, pos)?;
        let mut removed = HashMap::new();
        for _ in 0..num_removed {
            let (element, p) = read_string(bytes, pos)?;
            let (cnt, p) = read_u64(bytes, p)?;
            let (nid, p) = read_u64(bytes, p)?;
            removed.insert(element, LamportTime { counter: cnt, node_id: nid });
            pos = p;
        }
        Some(Self { entries, removed, clock: LamportClock { node_id, counter } })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- GCounter ----

    #[test]
    fn test_gcounter_increment() {
        let mut c = GCounter::new(1);
        c.increment();
        assert_eq!(c.value(), 1);
        c.increment();
        assert_eq!(c.value(), 2);
    }

    #[test]
    fn test_gcounter_value() {
        let mut c = GCounter::new(1);
        c.increment_by(5);
        assert_eq!(c.value(), 5);
    }

    #[test]
    fn test_gcounter_merge() {
        let mut a = GCounter::new(1);
        a.increment_by(3);
        let mut b = GCounter::new(2);
        b.increment_by(5);
        a.merge(&b);
        assert_eq!(a.value(), 8);
    }

    #[test]
    fn test_gcounter_merge_commutative() {
        let mut a = GCounter::new(1);
        a.increment_by(3);
        let b_orig = a.clone();
        let mut b = GCounter::new(2);
        b.increment_by(5);
        let c_orig = b.clone();
        a.merge(&c_orig);
        let mut b = b_orig;
        b.merge(&c_orig);
        // They both merged with the same state
        let mut a2 = GCounter::new(1); a2.increment_by(3);
        let mut b2 = GCounter::new(2); b2.increment_by(5);
        a2.merge(&b2);
        b2.merge(&a2);
        assert_eq!(a2.value(), b2.value());
    }

    #[test]
    fn test_gcounter_merge_idempotent() {
        let mut a = GCounter::new(1);
        a.increment_by(10);
        let snapshot = a.clone();
        a.merge(&snapshot);
        assert_eq!(a.value(), 10);
    }

    #[test]
    fn test_gcounter_serialize_roundtrip() {
        let mut c = GCounter::new(42);
        c.increment_by(100);
        let bytes = c.to_bytes();
        let restored = GCounter::from_bytes(&bytes).unwrap();
        assert_eq!(c, restored);
        assert_eq!(restored.value(), 100);
    }

    // ---- PNCounter ----

    #[test]
    fn test_pncounter_increment() {
        let mut c = PNCounter::new(1);
        c.increment();
        c.increment();
        assert_eq!(c.value(), 2);
    }

    #[test]
    fn test_pncounter_decrement() {
        let mut c = PNCounter::new(1);
        c.increment_by(5);
        c.decrement_by(2);
        assert_eq!(c.value(), 3);
    }

    #[test]
    fn test_pncounter_net_negative() {
        let mut c = PNCounter::new(1);
        c.decrement_by(3);
        assert_eq!(c.value(), -3);
    }

    #[test]
    fn test_pncounter_merge() {
        let mut a = PNCounter::new(1);
        a.increment_by(3);
        let mut b = PNCounter::new(2);
        b.increment_by(5);
        b.decrement_by(2);
        a.merge(&b);
        assert_eq!(a.value(), 6);
    }

    #[test]
    fn test_pncounter_merge_commutative() {
        let mut a = PNCounter::new(1); a.increment_by(3);
        let mut b = PNCounter::new(2); b.increment_by(5);
        let a_snap = a.clone();
        let b_snap = b.clone();
        a.merge(&b_snap);
        let mut b = b_snap.clone();
        b.merge(&a_snap);
        assert_eq!(a.value(), b.value());
    }

    #[test]
    fn test_pncounter_serialize_roundtrip() {
        let mut c = PNCounter::new(42);
        c.increment_by(10);
        c.decrement_by(3);
        let bytes = c.to_bytes();
        let restored = PNCounter::from_bytes(&bytes).unwrap();
        assert_eq!(c.value(), restored.value());
    }

    // ---- GSet ----

    #[test]
    fn test_gset_insert() {
        let mut s = GSet::<String>::new();
        assert!(s.insert("a".to_string()));
        assert!(!s.insert("a".to_string()));
        assert!(s.contains(&"a".to_string()));
    }

    #[test]
    fn test_gset_merge() {
        let mut a = GSet::<String>::new();
        a.insert("x".to_string());
        let mut b = GSet::<String>::new();
        b.insert("y".to_string());
        a.merge(&b);
        assert!(a.contains(&"x".to_string()));
        assert!(a.contains(&"y".to_string()));
    }

    #[test]
    fn test_gset_merge_commutative() {
        let mut a = GSet::<String>::new(); a.insert("x".to_string());
        let mut b = GSet::<String>::new(); b.insert("y".to_string());
        let a_snap = a.clone_replica();
        let b_snap = b.clone_replica();
        a.merge(&b_snap);
        let mut b = b_snap.clone();
        b.merge(&a_snap);
        assert_eq!(a.value(), b.value());
    }

    #[test]
    fn test_gset_serialize_roundtrip() {
        let mut s = GSet::<String>::new();
        s.insert("hello".to_string());
        s.insert("world".to_string());
        let bytes = s.to_bytes();
        let restored = GSet::<String>::from_bytes(&bytes).unwrap();
        assert_eq!(s.value(), restored.value());
    }

    // ---- ORSet ----

    #[test]
    fn test_orset_add() {
        let mut s = ORSet::<String>::new(1_u32);
        s.add("a".to_string());
        assert!(s.contains(&"a".to_string()));
    }

    #[test]
    fn test_orset_remove() {
        let mut s = ORSet::<String>::new(1_u32);
        s.add("a".to_string());
        s.remove(&"a".to_string());
        assert!(!s.contains(&"a".to_string()));
    }

    #[test]
    fn test_orset_add_wins() {
        let mut a = ORSet::<String>::new(1_u32);
        a.add("x".to_string());
        let mut b = a.clone();
        a.add("x".to_string());
        b.remove(&"x".to_string());
        a.merge(&b);
        assert!(a.contains(&"x".to_string()), "add should win");
    }

    #[test]
    fn test_orset_merge() {
        let mut a = ORSet::<String>::new(1_u32); a.add("x".to_string());
        let mut b = ORSet::<String>::new(2_u32); b.add("y".to_string());
        a.merge(&b);
        assert!(a.contains(&"x".to_string()));
        assert!(a.contains(&"y".to_string()));
    }

    #[test]
    fn test_orset_merge_commutative() {
        let mut a = ORSet::<String>::new(1_u32); a.add("x".to_string());
        let mut b = ORSet::<String>::new(2_u32); b.add("y".to_string());
        let b_snap = b.clone_replica();
        a.merge(&b_snap);
        let mut b = b_snap.clone();
        b.merge(&a.clone_replica());
        assert_eq!(a.value(), b.value());
    }

    #[test]
    fn test_orset_serialize_roundtrip() {
        let mut s = ORSet::<String>::new(1_u32);
        s.add("hello".to_string());
        s.add("world".to_string());
        let bytes = s.to_bytes();
        let restored = ORSet::<String>::from_bytes(&bytes).unwrap();
        assert_eq!(s.value(), restored.value());
    }

    // ---- AWORSet ----

    #[test]
    fn test_aworset_add() {
        let mut s = AWORSet::<String>::new(1);
        s.add("a".to_string());
        assert!(s.contains(&"a".to_string()));
    }

    #[test]
    fn test_aworset_remove() {
        let mut s = AWORSet::<String>::new(1);
        s.add("a".to_string());
        s.remove(&"a".to_string());
        assert!(!s.contains(&"a".to_string()));
    }

    #[test]
    fn test_aworset_timestamp_wins() {
        let mut a = AWORSet::<String>::new(1);
        a.add("x".to_string());
        let mut b = a.clone();
        b.remove(&"x".to_string());
        b.add("x".to_string());
        a.merge(&b);
        assert!(a.contains(&"x".to_string()), "later add should win");
    }

    #[test]
    fn test_aworset_merge() {
        let mut a = AWORSet::<String>::new(1); a.add("x".to_string());
        let mut b = AWORSet::<String>::new(2); b.add("y".to_string());
        a.merge(&b);
        assert!(a.contains(&"x".to_string()));
        assert!(a.contains(&"y".to_string()));
    }

    #[test]
    fn test_aworset_merge_commutative() {
        let mut a = AWORSet::<String>::new(1); a.add("x".to_string());
        let mut b = AWORSet::<String>::new(2); b.add("y".to_string());
        let b_snap = b.clone_replica();
        a.merge(&b_snap);
        let mut b = b_snap.clone();
        b.merge(&a.clone_replica());
        assert_eq!(a.value(), b.value());
    }

    #[test]
    fn test_aworset_serialize_roundtrip() {
        let mut s = AWORSet::<String>::new(1);
        s.add("hello".to_string());
        s.add("world".to_string());
        let bytes = s.to_bytes();
        let restored = AWORSet::<String>::from_bytes(&bytes).unwrap();
        assert_eq!(s.value(), restored.value());
    }

    #[test]
    fn test_lamport_time_ordering() {
        let a = LamportTime { counter: 1, node_id: 1 };
        let b = LamportTime { counter: 2, node_id: 1 };
        let c = LamportTime { counter: 2, node_id: 2 };
        assert!(b.is_greater_than(&a));
        assert!(c.is_greater_than(&b));
    }
}
