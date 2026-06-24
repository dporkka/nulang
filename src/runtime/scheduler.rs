//! Work-stealing scheduler with M:N threading.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Work-stealing task queue.
pub struct WorkQueue {
    tasks: Vec<Box<dyn FnOnce() + Send>>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl WorkQueue {
    pub fn new() -> Self {
        WorkQueue {
            tasks: Vec::with_capacity(256),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    pub fn push(&mut self, task: Box<dyn FnOnce() + Send>) {
        self.tasks.push(task);
        self.tail.fetch_add(1, Ordering::Release);
    }

    pub fn pop(&mut self) -> Option<Box<dyn FnOnce() + Send>> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == 0 {
            return None;
        }
        let new_tail = tail - 1;
        self.tail.store(new_tail, Ordering::Relaxed);
        // In a real implementation, we'd synchronize with steal
        self.tasks.pop()
    }

    pub fn steal(&self) -> Option<Box<dyn FnOnce() + Send>> {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        if head >= tail {
            return None;
        }
        // In a real implementation, use CAS
        None
    }
}

/// Thread pool scheduler.
pub struct Scheduler {
    worker_count: usize,
    shutdown: Arc<AtomicBool>,
    global_queue: Vec<WorkQueue>,
}

impl Scheduler {
    pub fn new(worker_count: usize) -> Self {
        let mut queues = Vec::new();
        for _ in 0..worker_count {
            queues.push(WorkQueue::new());
        }
        Scheduler {
            worker_count,
            shutdown: Arc::new(AtomicBool::new(false)),
            global_queue: queues,
        }
    }

    pub fn start(&mut self) {
        // Placeholder: would spawn worker threads
    }

    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Release);
    }

    pub fn submit(&mut self, worker: usize, task: Box<dyn FnOnce() + Send>) {
        if worker < self.global_queue.len() {
            self.global_queue[worker].push(task);
        }
    }

    pub fn worker_count(&self) -> usize {
        self.worker_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scheduler_creation() {
        let scheduler = Scheduler::new(4);
        assert_eq!(scheduler.worker_count(), 4);
    }
}
