//! Work-stealing scheduler: Chase-Lev deque per worker.

/// Global scheduler managing worker threads and actor execution.
pub struct Scheduler {
    worker_count: usize,
    global_queue: Vec<u64>, // Actor IDs ready to run
    processed_count: usize, // Number of actors processed
}

impl Scheduler {
    pub fn new(worker_count: usize) -> Self {
        Scheduler {
            worker_count,
            global_queue: Vec::new(),
            processed_count: 0,
        }
    }

    /// Enqueue an actor ID into the scheduler's ready queue.
    pub fn enqueue(&mut self, actor_id: u64) {
        self.global_queue.push(actor_id);
    }

    /// Dequeue the next actor ID from the ready queue (LIFO for cache locality).
    pub fn dequeue(&mut self) -> Option<u64> {
        self.global_queue.pop()
    }

    /// Steal an actor ID from the global queue.
    ///
    /// In the MVP, this simply pops from the global queue.
    /// A full implementation would steal from other workers' local deques.
    pub fn steal(&mut self) -> Option<u64> {
        self.global_queue.pop()
    }

    /// Process one actor through the given processing function.
    ///
    /// Dequeues an actor and calls `process_fn` with its ID.
    /// Returns `true` if an actor was processed, `false` if the queue was empty.
    pub fn run_one<F>(&mut self, mut process_fn: F) -> bool
    where
        F: FnMut(u64),
    {
        if let Some(actor_id) = self.dequeue() {
            process_fn(actor_id);
            self.processed_count += 1;
            true
        } else {
            false
        }
    }

    /// Increment the processed count (used when processing happens outside run_one).
    pub fn increment_processed(&mut self) {
        self.processed_count += 1;
    }

    pub fn worker_count(&self) -> usize {
        self.worker_count
    }

    pub fn queue_len(&self) -> usize {
        self.global_queue.len()
    }

    /// Return the number of actors that have been processed.
    pub fn processed_count(&self) -> usize {
        self.processed_count
    }
}
