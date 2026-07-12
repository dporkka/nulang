//! Register and Sequence CRDTs for Nulang.
//!
//! Provides LWWRegister (last-write-wins), MVRegister (multi-value), and
//! RGA (replicated growable array / collaborative text).

use std::collections::HashSet;
use std::hash::Hash;

// Lamport clock infrastructure is shared with the other CRDTs; the canonical
// definitions live in `crdt` (and are re-exported from the runtime root).
use super::crdt::{LamportClock, LamportTime};

// ---------------------------------------------------------------------------
// 1. LWWRegister
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct LWWRegister<T: Clone> {
    pub value: T,
    pub timestamp: LamportTime,
    pub clock: LamportClock,
}

impl<T: Clone> LWWRegister<T> {
    pub fn new(node_id: u64, initial: T) -> Self {
        let mut clock = LamportClock::new(node_id);
        let timestamp = clock.tick();
        Self { value: initial, timestamp, clock }
    }

    pub fn write(&mut self, value: T) {
        self.timestamp = self.clock.tick();
        self.value = value;
    }

    pub fn read(&self) -> &T { &self.value }
    pub fn value(&self) -> T { self.value.clone() }

    pub fn merge(&mut self, other: &Self) {
        if other.timestamp > self.timestamp {
            self.value = other.value.clone();
            self.timestamp = other.timestamp;
        }
        self.clock.counter = self.clock.counter.max(other.clock.counter);
    }

    /// Delta relative to `base`: the whole register when it holds a newer
    /// write than `base` (a register's delta *is* the winning write), or
    /// `None` when the timestamp did not advance.
    pub fn delta_since(&self, base: &Self) -> Option<Self> {
        if self.timestamp > base.timestamp { Some(self.clone()) } else { None }
    }
}

impl LWWRegister<String> {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.clock.node_id.to_be_bytes());
        buf.extend_from_slice(&self.timestamp.counter.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes()); // type tag 0 = String
        let bytes = self.value.as_bytes();
        buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        buf.extend_from_slice(bytes);
        buf
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 24 { return None; }
        let node_id = u64::from_be_bytes(bytes[0..8].try_into().ok()?);
        let ts_counter = u64::from_be_bytes(bytes[8..16].try_into().ok()?);
        let type_tag = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        if type_tag != 0 { return None; }
        let str_len = u32::from_be_bytes(bytes[20..24].try_into().ok()?) as usize;
        if bytes.len() < 24 + str_len { return None; }
        let value = String::from_utf8(bytes[24..24 + str_len].to_vec()).ok()?;
        let mut clock = LamportClock::new(node_id);
        clock.counter = ts_counter;
        let timestamp = LamportTime { counter: ts_counter, node_id };
        Some(Self { value, timestamp, clock })
    }
}

// ---------------------------------------------------------------------------
// 2. MVRegister
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct MVRegister<T: Clone + Eq + Hash> {
    pub values: HashSet<(T, LamportTime)>,
    pub clock: LamportClock,
}

impl<T: Clone + Eq + Hash> MVRegister<T> {
    pub fn new(node_id: u64) -> Self {
        Self { values: HashSet::new(), clock: LamportClock::new(node_id) }
    }

    pub fn write(&mut self, value: T) {
        let ts = self.clock.tick();
        self.values.retain(|(_, t)| *t >= ts);
        self.values.insert((value, ts));
    }

    pub fn read(&self) -> HashSet<T> {
        self.values.iter()
            .map(|(_, t)| *t)
            .max()
            .map(|max_ts| {
                self.values.iter()
                    .filter(|(_, t)| *t == max_ts)
                    .map(|(v, _)| v.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn is_conflicted(&self) -> bool { self.read().len() > 1 }

    pub fn merge(&mut self, other: &Self) {
        self.clock.counter = self.clock.counter.max(other.clock.counter);
        for (val, ts) in &other.values {
            self.values.insert((val.clone(), *ts));
        }
        if !self.values.is_empty() {
            let max_ts = self.values.iter().map(|(_, t)| *t).max().unwrap();
            self.values.retain(|(_, t)| *t == max_ts);
        }
    }

    /// Delta relative to `base`: the `(value, timestamp)` pairs not present
    /// in `base`. `None` when no value was added. Because `merge` prunes to
    /// the maximum timestamp, a receiver holding `base` ends up with the
    /// same retained set whether it merges this delta or the full state.
    pub fn delta_since(&self, base: &Self) -> Option<Self> {
        let values: HashSet<(T, LamportTime)> =
            self.values.difference(&base.values).cloned().collect();
        if values.is_empty() { None } else { Some(Self { values, clock: self.clock }) }
    }
}

impl MVRegister<String> {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.clock.node_id.to_be_bytes());
        buf.extend_from_slice(&self.clock.counter.to_be_bytes());
        buf.extend_from_slice(&(self.values.len() as u32).to_be_bytes());
        for (value, ts) in &self.values {
            buf.extend_from_slice(&ts.counter.to_be_bytes());
            buf.extend_from_slice(&ts.node_id.to_be_bytes());
            let bytes = value.as_bytes();
            buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            buf.extend_from_slice(bytes);
        }
        buf
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 20 { return None; }
        let node_id = u64::from_be_bytes(bytes[0..8].try_into().ok()?);
        let clock_counter = u64::from_be_bytes(bytes[8..16].try_into().ok()?);
        let count = u32::from_be_bytes(bytes[16..20].try_into().ok()?) as usize;
        let mut clock = LamportClock::new(node_id);
        clock.counter = clock_counter;
        let mut values = HashSet::new();
        let mut offset = 20;
        for _ in 0..count {
            if bytes.len() < offset + 16 { return None; }
            let ts_counter = u64::from_be_bytes(bytes[offset..offset+8].try_into().ok()?);
            let ts_node_id = u64::from_be_bytes(bytes[offset+8..offset+16].try_into().ok()?);
            offset += 16;
            if bytes.len() < offset + 4 { return None; }
            let str_len = u32::from_be_bytes(bytes[offset..offset+4].try_into().ok()?) as usize;
            offset += 4;
            if bytes.len() < offset + str_len { return None; }
            let value = String::from_utf8(bytes[offset..offset+str_len].to_vec()).ok()?;
            offset += str_len;
            values.insert((value, LamportTime::new(ts_counter, ts_node_id)));
        }
        Some(Self { values, clock })
    }
}

// ---------------------------------------------------------------------------
// 3. RGA
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ElementId { pub node_id: u64, pub counter: u64 }

#[derive(Debug, Clone)]
pub struct RGAElement<T: Clone> {
    pub id: ElementId,
    pub parent: Option<ElementId>,
    pub value: Option<T>,
    pub timestamp: LamportTime,
}

impl<T: Clone + PartialEq> PartialEq for RGAElement<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.parent == other.parent && self.value == other.value && self.timestamp == other.timestamp
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RGA<T: Clone + PartialEq> {
    pub elements: Vec<RGAElement<T>>,
    pub clock: LamportClock,
}

impl<T: Clone + PartialEq> RGA<T> {
    pub fn new(node_id: u64) -> Self { Self { elements: Vec::new(), clock: LamportClock::new(node_id) } }

    pub fn insert_after(&mut self, parent: Option<ElementId>, value: T) -> ElementId {
        let id = ElementId { node_id: self.clock.node_id, counter: self.clock.tick().counter };
        let ts = LamportTime { counter: id.counter, node_id: id.node_id };
        let elem = RGAElement { id, parent, value: Some(value), timestamp: ts };
        self.insert_element_sorted(elem);
        id
    }

    pub fn insert_at(&mut self, index: usize, value: T) -> ElementId {
        let parent = if index == 0 {
            None
        } else {
            self.elements.iter()
                .filter(|e| e.value.is_some())
                .nth(index - 1)
                .map(|e| e.id)
        };
        self.insert_after(parent, value)
    }

    pub fn delete(&mut self, id: ElementId) {
        if let Some(elem) = self.elements.iter_mut().find(|e| e.id == id) { elem.value = None; }
    }

    pub fn delete_at(&mut self, index: usize) {
        if let Some(id) = self.elements.iter()
            .filter(|e| e.value.is_some())
            .nth(index)
            .map(|e| e.id) {
            self.delete(id);
        }
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        self.elements.iter().filter(|e| e.value.is_some()).nth(index).and_then(|e| e.value.as_ref())
    }

    pub fn len(&self) -> usize { self.elements.iter().filter(|e| e.value.is_some()).count() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }

    pub fn value(&self) -> Vec<T> {
        self.elements.iter().filter(|e| e.value.is_some()).map(|e| e.value.as_ref().unwrap().clone()).collect()
    }

    pub fn merge(&mut self, other: &Self) {
        for other_elem in &other.elements {
            if let Some(existing) = self.elements.iter_mut().find(|e| e.id == other_elem.id) {
                if other_elem.timestamp > existing.timestamp { *existing = other_elem.clone(); }
            } else {
                self.insert_element_sorted(other_elem.clone());
            }
        }
        self.clock.counter = self.clock.counter.max(other.clock.counter);
    }

    /// Delta relative to `base`: the elements whose id is unknown to `base`
    /// or whose timestamp advanced (e.g. a tombstone). `None` when nothing
    /// changed. New elements keep their relative order so merging the delta
    /// produces the same sorted sequence as merging the full state.
    pub fn delta_since(&self, base: &Self) -> Option<Self> {
        let elements: Vec<RGAElement<T>> = self.elements.iter()
            .filter(|e| {
                base.elements.iter()
                    .find(|b| b.id == e.id)
                    .map_or(true, |b| e.timestamp > b.timestamp)
            })
            .cloned()
            .collect();
        if elements.is_empty() { None } else { Some(Self { elements, clock: self.clock }) }
    }

    fn insert_element_sorted(&mut self, elem: RGAElement<T>) {
        let pos = self.find_insert_position(&elem);
        self.elements.insert(pos, elem);
    }

    fn find_insert_position(&self, elem: &RGAElement<T>) -> usize {
        let parent_pos = elem.parent.and_then(|pid| self.elements.iter().position(|e| e.id == pid));
        match parent_pos {
            None => {
                if elem.parent.is_none() {
                    let mut pos = 0;
                    for (i, e) in self.elements.iter().enumerate() {
                        if e.parent.is_none() && e.timestamp <= elem.timestamp { pos = i + 1; } else { break; }
                    }
                    pos
                } else { self.elements.len() }
            }
            Some(pidx) => {
                let mut pos = pidx + 1;
                for (i, e) in self.elements.iter().enumerate().skip(pidx + 1) {
                    if e.parent == elem.parent && e.timestamp <= elem.timestamp { pos = i + 1; }
                    else if e.parent == elem.parent && e.timestamp > elem.timestamp { break; }
                    else if e.parent != elem.parent { break; }
                }
                pos
            }
        }
    }
}

impl RGA<String> {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.clock.node_id.to_be_bytes());
        buf.extend_from_slice(&self.clock.counter.to_be_bytes());
        buf.extend_from_slice(&(self.elements.len() as u32).to_be_bytes());
        for elem in &self.elements {
            buf.extend_from_slice(&elem.id.node_id.to_be_bytes());
            buf.extend_from_slice(&elem.id.counter.to_be_bytes());
            if let Some(p) = elem.parent {
                buf.push(1);
                buf.extend_from_slice(&p.node_id.to_be_bytes());
                buf.extend_from_slice(&p.counter.to_be_bytes());
            } else { buf.push(0); }
            if let Some(ref v) = elem.value {
                buf.push(1);
                let bytes = v.as_bytes();
                buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
                buf.extend_from_slice(bytes);
            } else { buf.push(0); }
            buf.extend_from_slice(&elem.timestamp.counter.to_be_bytes());
            buf.extend_from_slice(&elem.timestamp.node_id.to_be_bytes());
        }
        buf
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 20 { return None; }
        let node_id = u64::from_be_bytes(bytes[0..8].try_into().ok()?);
        let clock_counter = u64::from_be_bytes(bytes[8..16].try_into().ok()?);
        let count = u32::from_be_bytes(bytes[16..20].try_into().ok()?) as usize;
        let mut clock = LamportClock::new(node_id);
        clock.counter = clock_counter;
        let mut elements = Vec::with_capacity(count);
        let mut offset = 20;
        for _ in 0..count {
            if bytes.len() < offset + 16 { return None; }
            let id_node_id = u64::from_be_bytes(bytes[offset..offset+8].try_into().ok()?);
            let id_counter = u64::from_be_bytes(bytes[offset+8..offset+16].try_into().ok()?);
            offset += 16;
            if bytes.len() < offset + 1 { return None; }
            let has_parent = bytes[offset] != 0; offset += 1;
            let parent = if has_parent {
                if bytes.len() < offset + 16 { return None; }
                let p_node_id = u64::from_be_bytes(bytes[offset..offset+8].try_into().ok()?);
                let p_counter = u64::from_be_bytes(bytes[offset+8..offset+16].try_into().ok()?);
                offset += 16;
                Some(ElementId { node_id: p_node_id, counter: p_counter })
            } else { None };
            if bytes.len() < offset + 1 { return None; }
            let has_value = bytes[offset] != 0; offset += 1;
            let value = if has_value {
                if bytes.len() < offset + 4 { return None; }
                let str_len = u32::from_be_bytes(bytes[offset..offset+4].try_into().ok()?) as usize;
                offset += 4;
                if bytes.len() < offset + str_len { return None; }
                let s = String::from_utf8(bytes[offset..offset+str_len].to_vec()).ok()?;
                offset += str_len;
                Some(s)
            } else { None };
            if bytes.len() < offset + 16 { return None; }
            let ts_counter = u64::from_be_bytes(bytes[offset..offset+8].try_into().ok()?);
            let ts_node_id = u64::from_be_bytes(bytes[offset+8..offset+16].try_into().ok()?);
            offset += 16;
            elements.push(RGAElement { id: ElementId { node_id: id_node_id, counter: id_counter }, parent, value, timestamp: LamportTime { counter: ts_counter, node_id: ts_node_id } });
        }
        Some(RGA { elements, clock })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lww_write_read() {
        let mut reg = LWWRegister::new(1, "hello".to_string());
        assert_eq!(reg.read(), "hello");
        reg.write("world".to_string());
        assert_eq!(reg.read(), "world");
    }

    #[test]
    fn test_lww_merge_takes_higher_timestamp() {
        let mut a = LWWRegister::new(1, "alice".to_string());
        a.write("alice-v2".to_string());
        let mut b = LWWRegister::new(2, "bob".to_string());
        b.write("bob-v1".to_string());
        b.write("bob-v2".to_string());
        b.write("bob-v3".to_string());
        a.merge(&b);
        assert_eq!(a.read(), "bob-v3");
    }

    #[test]
    fn test_lww_merge_commutative() {
        let mut a = LWWRegister::new(1, "a".to_string());
        let mut b = LWWRegister::new(2, "b".to_string());
        b.write("b-v2".to_string());
        let a_snap = a.clone();
        a.merge(&b);
        let mut b2 = b.clone();
        b2.merge(&a_snap);
        assert_eq!(a.read(), b2.read());
    }

    #[test]
    fn test_lww_merge_idempotent() {
        let mut a = LWWRegister::new(1, "test".to_string());
        a.write("updated".to_string());
        let snap = a.clone();
        a.merge(&snap);
        assert_eq!(a.read(), snap.read());
    }

    #[test]
    fn test_lww_concurrent_write() {
        let mut a = LWWRegister::new(1, "initial".to_string());
        let mut b = LWWRegister::new(2, "initial".to_string());
        a.write("from-a".to_string());
        b.write("from-b".to_string());
        let mut a2 = a.clone(); a2.merge(&b);
        assert_eq!(a2.read(), "from-b");
    }

    #[test]
    fn test_lww_serialize_roundtrip() {
        let mut reg = LWWRegister::new(42, "hello world".to_string());
        reg.write("serialized".to_string());
        let bytes = reg.to_bytes();
        let restored = LWWRegister::from_bytes(&bytes).unwrap();
        assert_eq!(reg.value, restored.value);
        assert_eq!(reg.timestamp, restored.timestamp);
    }

    #[test]
    fn test_mv_write_read() {
        let mut reg = MVRegister::new(1);
        reg.write("hello".to_string());
        assert!(reg.read().contains("hello"));
    }

    #[test]
    fn test_mv_concurrent_writes() {
        let mut reg = MVRegister::new(1);
        let ts = LamportTime::new(5, 1);
        reg.values.insert(("conflict-a".to_string(), ts));
        reg.values.insert(("conflict-b".to_string(), ts));
        let vals = reg.read();
        assert_eq!(vals.len(), 2);
        assert!(reg.is_conflicted());
    }

    #[test]
    fn test_mv_merge_resolves() {
        let mut a = MVRegister::new(1);
        let mut b = MVRegister::new(2);
        a.write("old".to_string());
        b.write("new".to_string());
        a.merge(&b);
        assert!(a.read().contains("new"));
        assert_eq!(a.read().len(), 1);
    }

    #[test]
    fn test_mv_not_conflicted_after_single_write() {
        let mut reg = MVRegister::new(1);
        reg.write("only".to_string());
        assert!(!reg.is_conflicted());
    }

    #[test]
    fn test_mv_serialize_roundtrip() {
        let mut reg = MVRegister::new(7);
        reg.write("first".to_string());
        reg.write("second".to_string());
        let bytes = reg.to_bytes();
        let restored = MVRegister::from_bytes(&bytes).unwrap();
        assert_eq!(reg.read(), restored.read());
    }

    #[test]
    fn test_rga_insert() {
        let mut rga = RGA::new(1);
        let id = rga.insert_after(None, "hello".to_string());
        assert_eq!(rga.len(), 1);
        assert_eq!(rga.value(), vec!["hello".to_string()]);
        assert_eq!(id.node_id, 1);
    }

    #[test]
    fn test_rga_insert_multiple() {
        let mut rga = RGA::new(1);
        rga.insert_after(None, "a".to_string());
        let id_a = rga.elements.iter()
            .filter(|e| e.value.is_some())
            .next()
            .map(|e| e.id)
            .expect("expected at least one live element");
        rga.insert_after(Some(id_a), "b".to_string());
        rga.insert_after(Some(id_a), "c".to_string());
        assert_eq!(rga.len(), 3);
    }

    #[test]
    fn test_rga_delete() {
        let mut rga = RGA::new(1);
        let id = rga.insert_after(None, "to-delete".to_string());
        rga.delete(id);
        assert_eq!(rga.len(), 0);
        assert_eq!(rga.elements.len(), 1);
    }

    #[test]
    fn test_rga_len_after_delete() {
        let mut rga = RGA::new(1);
        rga.insert_after(None, "a".to_string());
        let id_b = rga.insert_after(None, "b".to_string());
        rga.insert_after(None, "c".to_string());
        rga.delete(id_b);
        assert_eq!(rga.len(), 2);
    }

    #[test]
    fn test_rga_insert_at_index() {
        let mut rga = RGA::new(1);
        rga.insert_at(0, "first".to_string());
        rga.insert_at(0, "before-first".to_string());
        assert_eq!(rga.len(), 2);
    }

    #[test]
    fn test_rga_merge() {
        let mut a = RGA::new(1);
        let mut b = RGA::new(2);
        a.insert_after(None, "from-a".to_string());
        b.insert_after(None, "from-b".to_string());
        a.merge(&b);
        assert_eq!(a.len(), 2);
        let vals = a.value();
        assert!(vals.contains(&"from-a".to_string()));
        assert!(vals.contains(&"from-b".to_string()));
    }

    #[test]
    fn test_rga_merge_commutative() {
        let mut a = RGA::new(1); a.insert_after(None, "alice".to_string());
        let mut b = RGA::new(2); b.insert_after(None, "bob".to_string());
        let a_snap = a.clone();
        a.merge(&b);
        let mut b2 = b.clone(); b2.merge(&a_snap);
        let mut va = a.value(); let mut vb = b2.value();
        va.sort(); vb.sort();
        assert_eq!(va, vb);
    }

    #[test]
    fn test_rga_concurrent_insert() {
        let mut a = RGA::new(1);
        let mut b = RGA::new(2);
        let common_id = a.insert_after(None, "base".to_string());
        b.elements = a.elements.clone(); b.clock.counter = a.clock.counter;
        a.insert_after(Some(common_id), "a-first".to_string());
        b.insert_after(Some(common_id), "b-first".to_string());
        let mut am = a.clone(); am.merge(&b);
        assert_eq!(am.len(), 3);
        let vals = am.value();
        let pos_a = vals.iter().position(|v| v == "a-first").unwrap();
        let pos_b = vals.iter().position(|v| v == "b-first").unwrap();
        assert!(pos_a < pos_b);
    }

    #[test]
    fn test_rga_serialize_roundtrip() {
        let mut rga = RGA::new(3);
        rga.insert_after(None, "one".to_string());
        rga.insert_after(None, "two".to_string());
        let id = rga.insert_after(None, "three".to_string());
        rga.delete(id);
        let bytes = rga.to_bytes();
        let restored = RGA::from_bytes(&bytes).unwrap();
        assert_eq!(rga.value(), restored.value());
        assert_eq!(rga.len(), restored.len());
    }

    // ---- Delta-state replication ----
    //
    // Merging the delta into a replica that already holds `base` must
    // produce exactly the same state as merging the full state.

    #[test]
    fn test_lww_delta_since_unchanged() {
        let reg = LWWRegister::new(1, "hello".to_string());
        assert!(reg.delta_since(&reg.clone()).is_none());
    }

    #[test]
    fn test_lww_delta_merge_equals_full_merge() {
        let base = LWWRegister::new(1, "v1".to_string());
        let mut full = base.clone();
        full.write("v2".to_string());

        let delta = full.delta_since(&base).expect("newer write");

        let mut via_delta = base.clone();
        via_delta.merge(&delta);
        let mut via_full = base.clone();
        via_full.merge(&full);
        assert_eq!(via_delta, via_full);
    }

    #[test]
    fn test_mv_delta_merge_equals_full_merge() {
        let mut base = MVRegister::new(1);
        base.write("old".to_string());
        let mut full = base.clone();
        full.write("new".to_string());

        let delta = full.delta_since(&base).expect("new value");
        assert_eq!(delta.values.len(), 1);

        let mut via_delta = base.clone();
        via_delta.merge(&delta);
        let mut via_full = base.clone();
        via_full.merge(&full);
        assert_eq!(via_delta, via_full);
    }

    #[test]
    fn test_mv_delta_carries_concurrent_conflict() {
        // A conflict set (two values at the same max timestamp) that grew
        // since the base must travel whole so the receiver sees the same
        // conflict as a full-state merge would show.
        let mut base = MVRegister::new(1);
        base.write("base".to_string());
        let mut full = base.clone();
        let ts = full.values.iter().map(|(_, t)| *t).max().unwrap();
        full.values.insert(("conflict".to_string(), ts));

        let delta = full.delta_since(&base).expect("conflict value added");
        let mut via_delta = base.clone();
        via_delta.merge(&delta);
        let mut via_full = base.clone();
        via_full.merge(&full);
        assert_eq!(via_delta, via_full);
        assert_eq!(via_delta.read().len(), 2);
    }

    #[test]
    fn test_rga_delta_merge_equals_full_merge() {
        let mut base = RGA::new(1);
        base.insert_after(None, "a".to_string());
        let mut full = base.clone();
        full.insert_at(1, "b".to_string());
        full.insert_at(2, "c".to_string());

        let delta = full.delta_since(&base).expect("elements added");
        assert_eq!(delta.elements.len(), 2);

        let mut via_delta = base.clone();
        via_delta.merge(&delta);
        let mut via_full = base.clone();
        via_full.merge(&full);
        assert_eq!(via_delta, via_full);
    }

    #[test]
    fn test_rga_delta_carries_tombstone() {
        let mut base = RGA::new(1);
        let id_a = base.insert_after(None, "a".to_string());
        let id_b = base.insert_after(None, "b".to_string());
        // Simulate a replica that merged a tombstone for `a` from elsewhere:
        // the element id is known to `base` but its timestamp advanced.
        let mut full = base.clone();
        if let Some(elem) = full.elements.iter_mut().find(|e| e.id == id_a) {
            elem.value = None;
            elem.timestamp = LamportTime::new(elem.timestamp.counter + 10, 99);
        }
        full.delete(id_b);

        let delta = full.delta_since(&base).expect("tombstone travels");
        let mut via_delta = base.clone();
        via_delta.merge(&delta);
        let mut via_full = base.clone();
        via_full.merge(&full);
        assert_eq!(via_delta, via_full);
    }
}
