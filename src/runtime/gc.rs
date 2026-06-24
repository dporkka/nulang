//! Garbage collector (reference counting for MVP).

/// Reference-counted heap object header.
#[derive(Debug)]
pub struct HeapObject {
    pub ref_count: usize,
    pub size: usize,
    pub data: Vec<u8>,
}

impl HeapObject {
    pub fn new(data: Vec<u8>) -> Self {
        HeapObject { ref_count: 1, size: data.len(), data }
    }

    pub fn retain(&mut self) {
        self.ref_count += 1;
    }

    pub fn release(&mut self) -> bool {
        self.ref_count -= 1;
        self.ref_count == 0
    }
}

/// Simple reference-counting GC.
pub struct RefCountGC {
    objects: Vec<Option<HeapObject>>,
    free_list: Vec<usize>,
}

impl RefCountGC {
    pub fn new() -> Self {
        RefCountGC {
            objects: Vec::with_capacity(1024),
            free_list: Vec::new(),
        }
    }

    pub fn allocate(&mut self, data: Vec<u8>) -> usize {
        let obj = HeapObject::new(data);
        if let Some(idx) = self.free_list.pop() {
            self.objects[idx] = Some(obj);
            idx
        } else {
            let idx = self.objects.len();
            self.objects.push(Some(obj));
            idx
        }
    }

    pub fn retain(&mut self, idx: usize) {
        if let Some(Some(obj)) = self.objects.get_mut(idx) {
            obj.retain();
        }
    }

    pub fn release(&mut self, idx: usize) -> bool {
        let should_free = if let Some(Some(obj)) = self.objects.get_mut(idx) {
            obj.release()
        } else {
            false
        };
        if should_free {
            self.objects[idx] = None;
            self.free_list.push(idx);
        }
        should_free
    }

    pub fn get(&self, idx: usize) -> Option<&HeapObject> {
        self.objects.get(idx).and_then(|o| o.as_ref())
    }

    pub fn get_mut(&mut self, idx: usize) -> Option<&mut HeapObject> {
        self.objects.get_mut(idx).and_then(|o| o.as_mut())
    }
}

impl Default for RefCountGC {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gc_allocate() {
        let mut gc = RefCountGC::new();
        let idx = gc.allocate(vec![1, 2, 3]);
        assert_eq!(gc.get(idx).unwrap().ref_count, 1);
    }

    #[test]
    fn test_gc_ref_count() {
        let mut gc = RefCountGC::new();
        let idx = gc.allocate(vec![1, 2, 3]);
        gc.retain(idx);
        assert_eq!(gc.get(idx).unwrap().ref_count, 2);
        assert!(!gc.release(idx));
        assert_eq!(gc.get(idx).unwrap().ref_count, 1);
        assert!(gc.release(idx));
        assert!(gc.get(idx).is_none());
    }
}
