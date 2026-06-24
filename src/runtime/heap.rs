//! Per-actor heap with bump allocator.

/// Actor-local heap allocator.
pub struct ActorHeap {
    memory: Vec<u8>,
    offset: usize,
    size_class: usize,
}

impl ActorHeap {
    pub fn new(size_class: usize) -> Self {
        let capacity = match size_class {
            0 => 256,
            1 => 1024,
            2 => 4096,
            3 => 16 * 1024,
            4 => 64 * 1024,
            _ => 256 * 1024,
        };
        ActorHeap {
            memory: vec![0; capacity],
            offset: 0,
            size_class,
        }
    }

    /// Allocate `size` bytes, returning offset into memory.
    pub fn alloc(&mut self, size: usize) -> Option<usize> {
        let aligned = (size + 7) & !7; // 8-byte alignment
        if self.offset + aligned > self.memory.len() {
            return None; // Out of memory
        }
        let addr = self.offset;
        self.offset += aligned;
        Some(addr)
    }

    pub fn reset(&mut self) {
        self.offset = 0;
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.memory[..self.offset]
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.memory[..self.offset]
    }

    pub fn capacity(&self) -> usize {
        self.memory.len()
    }

    pub fn used(&self) -> usize {
        self.offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heap_alloc() {
        let mut heap = ActorHeap::new(0);
        let a1 = heap.alloc(16).unwrap();
        let a2 = heap.alloc(8).unwrap();
        assert!(a2 > a1);
        assert_eq!(heap.used(), 24);
    }

    #[test]
    fn test_heap_oom() {
        let mut heap = ActorHeap::new(0);
        assert!(heap.alloc(300).is_none());
    }
}
