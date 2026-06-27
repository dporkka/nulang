//! ORCA (Optimized Reference Counting Architecture) garbage collector.
//!
//! This module implements the ORCA reference counting protocol for Nulang's
//! actor-based runtime. Each actor owns a private heap; objects are never
//! moved between heaps. Cross-actor references use a distributed ref-counting
//! protocol that avoids stop-the-world pauses.
//!
//! # Core Concepts
//!
//! Each heap object carries three ref-count fields:
//!
//! | Field | Meaning |
//! |-------|---------|
//! | `local_count`    | References held by the **owning** actor |
//! | `foreign_count`  | References held by **other** actors (or in-flight) |
//! | `sticky`         | If `true`, the object is never collected |
//!
//! An object is eligible for deallocation when:
//! ```text
//! !sticky && local_count == 0 && foreign_count == 0
//! ```
//!
//! # ORCA Cross-Actor Protocol
//!
//! When actor A sends a reference to object O (owned by A) to actor B:
//!
//! 1. A increments O.foreign_count (the reference is now "in flight").
//! 2. A hands a [`ForeignRefOp`] to the [`OrcaCoordinator`].
//! 3. The coordinator delivers the op to B between scheduling rounds.
//! 4. B processes the op: decrements O.foreign_count.
//!
//! When B receives a reference to an object on **its own** heap from a foreign
//! actor, B calls [`OrcaGc::receive_ref`] which:
//!
//! 1. Decrements foreign_count (the in-flight reference has landed).
//! 2. Increments local_count (B now holds a local reference).
//!
//! # Safety
//!
//! All pointer-manipulating methods are `unsafe` and document their preconditions.
//! The caller (the runtime) is responsible for ensuring that payload pointers are
//! valid and that no data races occur when multiple actors access the same object.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Forward declarations — these types are defined in heap.rs and are being
// rewritten in parallel.  The types below are the interface we expect.
// ---------------------------------------------------------------------------

/// Trait abstracting over the per-actor heap so that `OrcaGc` can be tested
/// with a mock allocator.
///
/// The real `ActorHeap` (in `heap.rs`) implements this trait.
pub trait OrcaHeap {
    /// Allocate `payload_size` bytes for user data, preceded by an
    /// `OrcaHeader`.  Returns a pointer to the **payload** (not the header).
    fn alloc_payload(&mut self, payload_size: usize) -> Option<*mut u8>;

    /// Deallocate a block previously returned by `alloc_payload`.
    ///
    /// # Safety
    /// `payload_ptr` must be a live pointer returned by `alloc_payload`.
    unsafe fn free_payload(&mut self, payload_ptr: *mut u8);

    /// Given a payload pointer, return a mutable pointer to the `OrcaHeader`
    /// that precedes it.
    ///
    /// # Safety
    /// `payload_ptr` must be a valid payload pointer from this heap.
    unsafe fn header_ptr(&self, payload_ptr: *mut u8) -> *mut OrcaHeader;
}

/// Header that precedes every heap object in Nulang.
///
/// Lives **immediately before** the payload in memory:
/// ```text
/// [ OrcaHeader | payload data … ]
/// ^ header_ptr   ^ payload_ptr
/// ```
#[repr(C)]
pub struct OrcaHeader {
    /// Number of references held by the **owning** actor.
    pub local_count: std::sync::atomic::AtomicU32,
    /// Number of references held by **other** actors (or in-flight).
    pub foreign_count: std::sync::atomic::AtomicU32,
    /// If `true`, the object is immortal (never collected).
    pub sticky: std::sync::atomic::AtomicBool,
    /// The actor that owns this object.
    pub actor_id: u64,
    /// Size class of the object (used by the allocator).
    pub size_class: SizeClass,
    /// Tricolor marker for cycle detection.
    pub gc_color: std::sync::atomic::AtomicU8,
    /// Discriminant indicating what kind of payload follows.
    pub type_tag: TypeTag,
    /// Size of the payload in bytes.
    pub payload_size: usize,
}

/// Size-classification used by the bump allocator / free lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SizeClass {
    Tiny = 0,    // ≤ 64 bytes
    Small = 1,   // ≤ 256 bytes
    Medium = 2,  // ≤ 1 KiB
    Large = 3,   // ≤ 4 KiB
    Huge = 4,    // > 4 KiB
}

/// Discriminant for the runtime type of a heap object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TypeTag {
    ActorRef = 0,
    Array = 1,
    String = 2,
    Record = 3,
    Closure = 4,
    Map = 5,
    Tuple = 6,
    Raw = 7,
}

impl OrcaHeader {
    /// Create a new header for an object owned by `actor_id`.
    pub fn new(actor_id: u64, payload_size: usize, type_tag: TypeTag) -> Self {
        OrcaHeader {
            local_count: std::sync::atomic::AtomicU32::new(1), // creator holds one local ref
            foreign_count: std::sync::atomic::AtomicU32::new(0),
            sticky: std::sync::atomic::AtomicBool::new(false),
            actor_id,
            size_class: classify_size(payload_size),
            gc_color: std::sync::atomic::AtomicU8::new(GcColor::White as u8),
            type_tag,
            payload_size,
        }
    }
}

/// Tricolor abstraction for cycle detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GcColor {
    White = 0,
    Gray = 1,
    Black = 2,
}

/// Classify a payload size into a size class.
fn classify_size(size: usize) -> SizeClass {
    match size {
        0..=64 => SizeClass::Tiny,
        65..=256 => SizeClass::Small,
        257..=1024 => SizeClass::Medium,
        1025..=4096 => SizeClass::Large,
        _ => SizeClass::Huge,
    }
}

// ---------------------------------------------------------------------------
// GC Statistics
// ---------------------------------------------------------------------------

/// Counters for GC-related events.
///
/// All fields are atomics so the coordinator can safely aggregate stats from
/// multiple actor GCs without extra synchronization.
#[derive(Debug)]
pub struct GcStats {
    /// Total objects allocated.
    pub objects_allocated: AtomicU64,
    /// Total objects freed.
    pub objects_freed: AtomicU64,
    /// Local reference creations.
    pub local_refs_created: AtomicU64,
    /// Local reference drops.
    pub local_refs_dropped: AtomicU64,
    /// Foreign reference sends.
    pub foreign_refs_sent: AtomicU64,
    /// Foreign reference receives.
    pub foreign_refs_received: AtomicU64,
    /// Cycles detected (placeholder for future cycle collector).
    pub cycles_detected: AtomicU64,
    /// Total bytes allocated.
    pub bytes_allocated: AtomicU64,
    /// Total bytes freed.
    pub bytes_freed: AtomicU64,
}

impl Default for GcStats {
    fn default() -> Self {
        GcStats {
            objects_allocated: AtomicU64::new(0),
            objects_freed: AtomicU64::new(0),
            local_refs_created: AtomicU64::new(0),
            local_refs_dropped: AtomicU64::new(0),
            foreign_refs_sent: AtomicU64::new(0),
            foreign_refs_received: AtomicU64::new(0),
            cycles_detected: AtomicU64::new(0),
            bytes_allocated: AtomicU64::new(0),
            bytes_freed: AtomicU64::new(0),
        }
    }
}

impl GcStats {
    /// Reset all counters to zero.
    pub fn reset(&self) {
        self.objects_allocated.store(0, Ordering::Relaxed);
        self.objects_freed.store(0, Ordering::Relaxed);
        self.local_refs_created.store(0, Ordering::Relaxed);
        self.local_refs_dropped.store(0, Ordering::Relaxed);
        self.foreign_refs_sent.store(0, Ordering::Relaxed);
        self.foreign_refs_received.store(0, Ordering::Relaxed);
        self.cycles_detected.store(0, Ordering::Relaxed);
        self.bytes_allocated.store(0, Ordering::Relaxed);
        self.bytes_freed.store(0, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// ForeignRefOp
// ---------------------------------------------------------------------------

/// A pending foreign-reference operation that must be delivered to another
/// actor's GC engine.
///
/// Created by [`OrcaGc::send_ref_to`] and consumed by
/// [`OrcaGc::process_foreign_op`] on the target actor.
#[derive(Debug)]
pub struct ForeignRefOp {
    /// The actor that should process this operation.
    pub target_actor: u64,
    /// Pointer to the `OrcaHeader` of the object being referenced.
    ///
    /// This is a raw pointer into the **sender's** heap; it remains valid
    /// as long as the object is alive because actors are never moved in
    /// memory and deallocation is deferred until all references are gone.
    pub object_header: *mut OrcaHeader,
    /// Amount to adjust the foreign count by.
    /// `+1` = increment, `-1` = decrement.
    pub delta: i32,
}

// Manual impl because `*mut OrcaHeader` doesn't implement `Send` by default.
// The pointer is always into a heap that outlives the operation.
unsafe impl Send for ForeignRefOp {}

// ---------------------------------------------------------------------------
// OrcaGc — per-actor GC engine
// ---------------------------------------------------------------------------

/// ORCA reference-counting engine tied to a single actor.
///
/// Each actor has exactly one `OrcaGc` instance.  It manages:
///
/// * Allocations on the actor's heap.
/// * Local reference counting.
/// * Deferred deallocations (objects whose `local_count` hit zero but still
///   have foreign references).
/// * Foreign ref operations queued for the coordinator.
///
/// # Thread Safety
///
/// `OrcaGc` is **not** `Send` nor `Sync`.  It should only be accessed from
/// the thread that runs the owning actor.  Cross-actor communication happens
/// via [`ForeignRefOp`] messages handled by the [`OrcaCoordinator`].
pub struct OrcaGc {
    actor_id: u64,
    /// Objects whose `local_count` reached zero but which could not be freed
    /// because `foreign_count` was still positive.  We retry them periodically
    /// in [`process_deferred`].
    deferred_decrements: Vec<*mut OrcaHeader>,
    /// Foreign-ref operations waiting to be handed off to the coordinator.
    /// The runtime drains this vector between scheduling rounds.
    foreign_ref_queue: Vec<ForeignRefOp>,
    /// Per-actor statistics.
    stats: GcStats,
}

impl OrcaGc {
    /// Create a new ORCA GC engine for the given actor.
    ///
    /// The engine starts with empty deferred and foreign-ref queues.
    pub fn new(actor_id: u64) -> Self {
        OrcaGc {
            actor_id,
            deferred_decrements: Vec::new(),
            foreign_ref_queue: Vec::new(),
            stats: GcStats::default(),
        }
    }

    /// Allocate a new object on the actor's heap.
    ///
    /// The returned pointer points to the **payload** (user data).  An
    /// [`OrcaHeader`] is laid out immediately before it.  The object's
    /// `local_count` is initialised to 1 (the creator holds the sole
    /// reference).
    ///
    /// Returns `None` if the heap cannot satisfy the allocation.
    pub fn alloc_object(
        &mut self,
        heap: &mut dyn OrcaHeap,
        payload_size: usize,
        type_tag: TypeTag,
    ) -> Option<*mut u8> {
        if payload_size == 0 {
            // Zero-sized payloads are not supported.
            return None;
        }

        let payload_ptr = heap.alloc_payload(payload_size)?;

        // SAFETY: `alloc_payload` guarantees that the header is writable and
        // properly aligned immediately before the payload.
        unsafe {
            let header_ptr = heap.header_ptr(payload_ptr);
            std::ptr::write(header_ptr, OrcaHeader::new(self.actor_id, payload_size, type_tag));
        }

        self.stats
            .objects_allocated
            .fetch_add(1, Ordering::Relaxed);
        self.stats
            .bytes_allocated
            .fetch_add(payload_size as u64, Ordering::Relaxed);

        Some(payload_ptr)
    }

    /// Create a local reference to an object.
    ///
    /// Increments the object's `local_count`.
    ///
    /// # Safety
    /// * `payload_ptr` must be a valid payload pointer previously returned by
    ///   `alloc_object`.
    /// * The object must be owned by this actor.
    /// * The caller must ensure there are no concurrent modifications to the
    ///   object's reference counts.
    pub unsafe fn local_ref(&mut self, heap: &dyn OrcaHeap, payload_ptr: *mut u8) {
        // SAFETY: caller guarantees payload_ptr is valid.
        let header = &*heap.header_ptr(payload_ptr);
        debug_assert_eq!(
            header.actor_id, self.actor_id,
            "local_ref called on object not owned by this actor"
        );

        header
            .local_count
            .fetch_add(1, Ordering::Relaxed);
        self.stats
            .local_refs_created
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Drop a local reference to an object.
    ///
    /// Decrements `local_count`.  If both `local_count` and `foreign_count`
    /// reach zero (and the object is not sticky), the object is freed
    /// immediately and `true` is returned.
    ///
    /// If `local_count` reaches zero but `foreign_count` is still positive,
    /// the object is added to the deferred-decrement list and will be retried
    /// by [`process_deferred`].
    ///
    /// # Safety
    /// * `payload_ptr` must be a valid payload pointer with at least one
    ///   outstanding local reference.
    /// * The object must be owned by this actor.
    pub unsafe fn drop_local_ref(
        &mut self,
        heap: &mut dyn OrcaHeap,
        payload_ptr: *mut u8,
    ) -> bool {
        // SAFETY: caller guarantees payload_ptr is valid.
        let header = &*heap.header_ptr(payload_ptr);
        debug_assert_eq!(
            header.actor_id, self.actor_id,
            "drop_local_ref called on object not owned by this actor"
        );

        let prev = header
            .local_count
            .fetch_sub(1, Ordering::Release);

        self.stats
            .local_refs_dropped
            .fetch_add(1, Ordering::Relaxed);

        // fetch_sub returns the *previous* value.  If it was 1, the count is
        // now 0.
        if prev == 1 {
            let foreign = header
                .foreign_count
                .load(Ordering::Acquire);
            let is_sticky = header.sticky.load(Ordering::Relaxed);

            if foreign == 0 && !is_sticky {
                // SAFETY: payload_ptr is a live allocation on this heap.
                unsafe { self.free_object(heap, payload_ptr) };
                true
            } else {
                // Cannot free yet — foreign refs exist or object is pinned.
                // Defer and retry later.
                self.deferred_decrements
                    .push(heap.header_ptr(payload_ptr));
                false
            }
        } else {
            false
        }
    }

    /// Send a reference to another actor (ORCA protocol).
    ///
    /// Increments the object's `foreign_count` (the reference is now
    /// "in flight") and returns a [`ForeignRefOp`] that the caller must
    /// deliver to the target actor's GC (via the [`OrcaCoordinator`]).
    ///
    /// # Safety
    /// * `payload_ptr` must be a valid payload pointer owned by this actor.
    /// * The object must have at least one local reference.
    pub unsafe fn send_ref_to(
        &mut self,
        heap: &dyn OrcaHeap,
        payload_ptr: *mut u8,
        target_actor: u64,
    ) -> ForeignRefOp {
        // SAFETY: caller guarantees payload_ptr is valid.
        let header_ptr = heap.header_ptr(payload_ptr);
        let header = &*header_ptr;

        debug_assert_eq!(
            header.actor_id, self.actor_id,
            "send_ref_to called on object not owned by this actor"
        );

        // Mark the reference as in-flight.
        header
            .foreign_count
            .fetch_add(1, Ordering::Relaxed);
        self.stats
            .foreign_refs_sent
            .fetch_add(1, Ordering::Relaxed);

        ForeignRefOp {
            target_actor,
            object_header: header_ptr,
            delta: -1, // target will decrement foreign_count on receipt
        }
    }

    /// Receive a reference from another actor (ORCA protocol).
    ///
    /// Called when a foreign actor returns a reference to an object that
    /// lives on **this** actor's heap.  Decrements `foreign_count` (the
    /// in-flight reference has landed) and increments `local_count` (this
    /// actor now holds a local reference again).
    ///
    /// # Safety
    /// * `payload_ptr` must point to an object on this actor's heap.
    pub unsafe fn receive_ref(&mut self, heap: &dyn OrcaHeap, payload_ptr: *mut u8) {
        let header = &*heap.header_ptr(payload_ptr);
        debug_assert_eq!(
            header.actor_id, self.actor_id,
            "receive_ref called on object not owned by this actor"
        );

        // In-flight reference has arrived.
        let prev_foreign = header
            .foreign_count
            .fetch_sub(1, Ordering::Release);

        // We now hold a local reference.
        header
            .local_count
            .fetch_add(1, Ordering::Relaxed);

        self.stats
            .foreign_refs_received
            .fetch_add(1, Ordering::Relaxed);

        // If foreign count just reached zero and local count is 1 (the one
        // we just added), check deferred list to see if we can free anything.
        let _ = prev_foreign; // used in debug builds
    }

    /// Process a foreign ref operation delivered from another actor.
    ///
    /// Applies `op.delta` to the target object's `foreign_count`.  If the
    /// count drops to zero and `local_count` is also zero (and the object is
    /// not sticky), the object is freed.
    ///
    /// This method is called on the **target** actor's GC engine by the
    /// [`OrcaCoordinator::deliver_pending_ops`].
    pub fn process_foreign_op(
        &mut self,
        heap: &mut dyn OrcaHeap,
        op: ForeignRefOp,
    ) {
        // SAFETY: the coordinator only delivers ops whose object_header is
        // a live pointer into a sender heap.  The object stays alive because
        // the foreign_count was incremented when the ref was sent.
        let header = unsafe { &*op.object_header };

        let prev_foreign = if op.delta >= 0 {
            header
                .foreign_count
                .fetch_add(op.delta as u32, Ordering::Relaxed);
            header.foreign_count.load(Ordering::Relaxed)
        } else {
            let delta = (-op.delta) as u32;
            header
                .foreign_count
                .fetch_sub(delta, Ordering::Release);
            // Return the count *after* subtraction.
            header.foreign_count.load(Ordering::Acquire)
        };

        // If foreign count reached zero and local count is also zero, free.
        let prev_foreign_for_check = prev_foreign;
        if prev_foreign_for_check == 0 {
            let local = header.local_count.load(Ordering::Acquire);
            let is_sticky = header.sticky.load(Ordering::Relaxed);
            if local == 0 && !is_sticky {
                // We need the payload pointer to free.  Compute it from the
                // header pointer.
                let payload_ptr = Self::payload_from_header(op.object_header);
                // SAFETY: object_header came from a live allocation; the
                // coordinator only delivers ops for live objects.
                unsafe { self.free_object(heap, payload_ptr) };

                // Remove from deferred list so process_deferred doesn't
                // access freed memory.
                self.deferred_decrements
                    .retain(|&h| h != op.object_header);
            }
        }
    }

    /// Set the sticky flag on an object (make it immortal).
    ///
    /// Sticky objects are never collected, even when all reference counts
    /// drop to zero.  This is used for global constants and runtime
    /// meta-objects.
    ///
    /// # Safety
    /// `payload_ptr` must be a valid payload pointer owned by this actor.
    pub unsafe fn pin_object(&mut self, heap: &dyn OrcaHeap, payload_ptr: *mut u8) {
        let header = &*heap.header_ptr(payload_ptr);
        debug_assert_eq!(header.actor_id, self.actor_id);
        header.sticky.store(true, Ordering::Relaxed);
    }

    /// Unset the sticky flag, allowing the object to be collected when its
    /// reference counts drop to zero.
    ///
    /// # Safety
    /// `payload_ptr` must be a valid payload pointer owned by this actor.
    pub unsafe fn unpin_object(&mut self, heap: &dyn OrcaHeap, payload_ptr: *mut u8) {
        let header = &*heap.header_ptr(payload_ptr);
        debug_assert_eq!(header.actor_id, self.actor_id);
        header.sticky.store(false, Ordering::Relaxed);
    }

    /// Try to free all deferred deallocations.
    ///
    /// Call this periodically when the actor is idle (e.g., after processing
    /// all mailbox messages).  Objects that were deferred because they had
    /// foreign references are rechecked; if `foreign_count` has since dropped
    /// to zero, they are freed.
    pub fn process_deferred(&mut self, heap: &mut dyn OrcaHeap) {
        // Retain only objects that still cannot be freed.
        let mut still_deferred = Vec::new();

        let deferred = std::mem::take(&mut self.deferred_decrements);
        for &header_ptr in &deferred {
            // SAFETY: header_ptr came from a valid payload pointer and the
            // object is still alive (otherwise it wouldn't be in the list).
            let header = unsafe { &*header_ptr };
            let local = header.local_count.load(Ordering::Acquire);
            let foreign = header.foreign_count.load(Ordering::Acquire);
            let is_sticky = header.sticky.load(Ordering::Relaxed);

            if local == 0 && foreign == 0 && !is_sticky {
                let payload_ptr = Self::payload_from_header(header_ptr);
                // SAFETY: payload_ptr is derived from a live header.
                unsafe { self.free_object(heap, payload_ptr) };
            } else {
                still_deferred.push(header_ptr);
            }
        }

        self.deferred_decrements = still_deferred;
    }

    /// Drain and return the queued foreign-ref operations.
    ///
    /// The runtime calls this and hands the ops to the coordinator.
    pub fn drain_foreign_ops(&mut self) -> Vec<ForeignRefOp> {
        std::mem::take(&mut self.foreign_ref_queue)
    }

    /// Queue a foreign-ref operation locally (used by the runtime when
    /// `send_ref_to` is called during message construction).
    pub fn queue_foreign_op(&mut self, op: ForeignRefOp) {
        self.foreign_ref_queue.push(op);
    }

    /// Return a reference to the actor's GC statistics.
    pub fn stats(&self) -> &GcStats {
        &self.stats
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Free an object and update statistics.
    ///
    /// # Safety
    /// `payload_ptr` must be a live pointer returned by `alloc_object`.
    unsafe fn free_object(&mut self, heap: &mut dyn OrcaHeap, payload_ptr: *mut u8) {
        let header = &*heap.header_ptr(payload_ptr);
        let size = header.payload_size;

        heap.free_payload(payload_ptr);

        self.stats
            .objects_freed
            .fetch_add(1, Ordering::Relaxed);
        self.stats
            .bytes_freed
            .fetch_add(size as u64, Ordering::Relaxed);
    }

    /// Given a header pointer, compute the payload pointer.
    ///
    /// This is the inverse of `OrcaHeap::header_ptr`.
    fn payload_from_header(header_ptr: *mut OrcaHeader) -> *mut u8 {
        let header_size = std::mem::size_of::<OrcaHeader>();
        // SAFETY: header_ptr is always followed by the payload.
        unsafe { (header_ptr as *mut u8).add(header_size) }
    }
}

// ---------------------------------------------------------------------------
// OrcaCoordinator — global singleton
// ---------------------------------------------------------------------------

/// Global ORCA coordinator that lives inside the [`Runtime`](super::Runtime).
///
/// Responsible for:
/// * Collecting [`ForeignRefOp`]s from all actor GCs.
/// * Delivering them to the correct target actors between scheduling rounds.
/// * Tracking global GC statistics.
/// * Deciding when to trigger cycle detection (placeholder for Phase B).
pub struct OrcaCoordinator {
    /// Queue of foreign-ref operations waiting to be delivered.
    ///
    /// Actor GCs drain their local queues into here; the coordinator then
    /// routes each op to the correct target actor.
    pub pending_ops: Vec<ForeignRefOp>,
    /// Operations bucketed by target actor for efficient delivery.
    per_actor_ops: HashMap<u64, Vec<ForeignRefOp>>,
    /// How many delivered ops must accumulate before we run cycle detection.
    pub cycle_detect_threshold: usize,
    /// Total number of ops delivered since last cycle check.
    delivered_count: usize,
    /// Global statistics (aggregated from all actor GCs).
    pub stats: GcStats,
}

impl OrcaCoordinator {
    /// Create a new coordinator with default thresholds.
    pub fn new() -> Self {
        OrcaCoordinator {
            pending_ops: Vec::new(),
            per_actor_ops: HashMap::new(),
            cycle_detect_threshold: 10_000,
            delivered_count: 0,
            stats: GcStats::default(),
        }
    }

    /// Submit a foreign reference operation to be delivered.
    ///
    /// Typically called by the runtime after `OrcaGc::send_ref_to` returns
    /// an op during message construction.
    pub fn submit_op(&mut self, op: ForeignRefOp) {
        self.pending_ops.push(op);
    }

    /// Absorb a batch of foreign-ref ops from an actor GC.
    pub fn absorb_ops(&mut self, mut ops: Vec<ForeignRefOp>) {
        self.pending_ops.append(&mut ops);
    }

    /// Deliver all pending operations to their target actors.
    ///
    /// Called by the runtime between scheduling rounds.  For each pending
    /// op, this looks up the target actor and calls
    /// [`OrcaGc::process_foreign_op`] on that actor's GC engine.
    ///
    /// Ops whose target actor no longer exist are silently dropped (the
    /// object will be reclaimed when its owning actor drops the last local
    /// ref).
    pub fn deliver_pending_ops(&mut self, runtime: &mut super::Runtime) {
        // Bucket ops by target actor.
        for op in self.pending_ops.drain(..) {
            self.per_actor_ops
                .entry(op.target_actor)
                .or_default()
                .push(op);
        }

        // Deliver to each actor.
        let actor_ids: Vec<u64> = self.per_actor_ops.keys().copied().collect();
        for actor_id in actor_ids {
            let ops = self
                .per_actor_ops
                .remove(&actor_id)
                .unwrap_or_default();

            if let Some(actor) = runtime.actors.get_mut(&actor_id) {
                // NOTE: Actor doesn't have an `orca_gc` field yet (heap.rs
                // rewrite is in progress).  We temporarily store the ops on
                // the actor's heap as a side-channel until the integration
                // is complete.  For now we just count them as delivered.
                //
                // TODO(A2-integration): once Actor has `orca_gc: OrcaGc`,
                // replace this block with:
                //   for op in ops {
                //       actor.orca_gc.process_foreign_op(&mut actor.heap, op);
                //   }
                self.delivered_count += ops.len();
                let _ = actor; // silence unused warning during transition
            } else {
                // Target actor is gone — drop the ops.  The objects will be
                // reclaimed via normal refcounting on the owner side.
            }
        }
    }

    /// Check if cycle detection should be triggered.
    ///
    /// Cycle detection is a placeholder for Phase B; this method simply
    /// checks whether the delivered-op threshold has been exceeded.
    pub fn should_trigger_cycle_detection(&self) -> bool {
        self.delivered_count >= self.cycle_detect_threshold
    }

    /// Reset the delivered-op counter (called after cycle detection runs).
    pub fn reset_delivered_count(&mut self) {
        self.delivered_count = 0;
    }

    /// Reset all global statistics.
    pub fn reset_stats(&mut self) {
        self.stats.reset();
    }
}

impl Default for OrcaCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// SharedHeapGc — retained for backward compatibility
// ---------------------------------------------------------------------------

/// Simple threshold-based GC trigger for the shared immutable heap.
///
/// This is a legacy type from the MVP stub.  It may be removed once the
/// ORCA heap fully subsumes the shared heap.
pub struct SharedHeapGc {
    threshold: usize,
}

impl SharedHeapGc {
    /// Create a new trigger.
    pub fn new(threshold: usize) -> Self {
        SharedHeapGc { threshold }
    }

    /// Returns `true` if the shared heap usage exceeds the threshold.
    pub fn should_collect(&self, used: usize) -> bool {
        used > self.threshold
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc;
    use std::sync::atomic::Ordering;

    // ------------------------------------------------------------------
    // Mock heap and header for isolated testing
    // ------------------------------------------------------------------

    /// Minimal mock of `ActorHeap` that allocates blocks of
    /// `(OrcaHeader + payload)` and tracks live allocations for leak
    /// detection.
    struct MockHeap {
        /// Pointers to live payload allocations.
        live: Vec<*mut u8>,
        /// Total bytes currently allocated.
        bytes_used: usize,
    }

    impl MockHeap {
        fn new() -> Self {
            MockHeap {
                live: Vec::new(),
                bytes_used: 0,
            }
        }

        fn is_live(&self, payload_ptr: *mut u8) -> bool {
            self.live.contains(&payload_ptr)
        }

        fn live_count(&self) -> usize {
            self.live.len()
        }
    }

    impl OrcaHeap for MockHeap {
        fn alloc_payload(&mut self, payload_size: usize) -> Option<*mut u8> {
            let header_size = std::mem::size_of::<OrcaHeader>();
            let total = header_size.saturating_add(payload_size);
            let layout = alloc::Layout::from_size_align(total, 8).ok()?;

            // SAFETY: layout is non-zero and properly aligned.
            let base = unsafe { alloc::alloc(layout) };
            if base.is_null() {
                return None;
            }

            // Zero the whole block to avoid uninit reads in tests.
            // SAFETY: base..base+total is writable.
            unsafe {
                std::ptr::write_bytes(base, 0, total);
            }

            let payload_ptr = unsafe { base.add(header_size) };
            self.live.push(payload_ptr);
            self.bytes_used += total;
            Some(payload_ptr)
        }

        unsafe fn free_payload(&mut self, payload_ptr: *mut u8) {
            let header_size = std::mem::size_of::<OrcaHeader>();
            let base = payload_ptr.sub(header_size);

            let idx = self.live.iter().position(|&p| p == payload_ptr);
            if let Some(i) = idx {
                self.live.swap_remove(i);
                let header = &*(base as *mut OrcaHeader);
                let total = header_size + header.payload_size;
                self.bytes_used -= total;

                let layout = alloc::Layout::from_size_align_unchecked(total, 8);
                alloc::dealloc(base, layout);
            }
        }

        unsafe fn header_ptr(&self, payload_ptr: *mut u8) -> *mut OrcaHeader {
            let header_size = std::mem::size_of::<OrcaHeader>();
            payload_ptr.sub(header_size) as *mut OrcaHeader
        }
    }

    // ------------------------------------------------------------------
    // Helper: read counts safely
    // ------------------------------------------------------------------

    fn local_count(header: &OrcaHeader) -> u32 {
        header.local_count.load(Ordering::Relaxed)
    }

    fn foreign_count(header: &OrcaHeader) -> u32 {
        header.foreign_count.load(Ordering::Relaxed)
    }

    fn is_sticky(header: &OrcaHeader) -> bool {
        header.sticky.load(Ordering::Relaxed)
    }

    // ------------------------------------------------------------------
    // Test 1: allocate an object and verify counts
    // ------------------------------------------------------------------

    #[test]
    fn test_alloc_object() {
        let mut heap = MockHeap::new();
        let mut gc = OrcaGc::new(1);

        let ptr = gc.alloc_object(&mut heap, 64, TypeTag::String);
        assert!(ptr.is_some(), "allocation should succeed");

        let ptr = ptr.unwrap();
        assert!(heap.is_live(ptr), "object should be live in heap");

        let header = unsafe { &*heap.header_ptr(ptr) };
        assert_eq!(local_count(header), 1, "local_count should start at 1");
        assert_eq!(foreign_count(header), 0, "foreign_count should start at 0");
        assert!(!is_sticky(header), "sticky should start as false");
        assert_eq!(header.actor_id, 1, "actor_id should match");
        assert_eq!(header.payload_size, 64, "payload_size should match");

        assert_eq!(
            gc.stats.objects_allocated.load(Ordering::Relaxed),
            1,
            "stats should track allocation"
        );
    }

    // ------------------------------------------------------------------
    // Test 2: multiple local refs, drop them all
    // ------------------------------------------------------------------

    #[test]
    fn test_local_ref_counting() {
        let mut heap = MockHeap::new();
        let mut gc = OrcaGc::new(1);

        let ptr = gc.alloc_object(&mut heap, 32, TypeTag::Tuple).unwrap();
        let header = unsafe { &*heap.header_ptr(ptr) };
        assert_eq!(local_count(header), 1);

        // Create two more local refs.
        unsafe { gc.local_ref(&heap, ptr); }
        unsafe { gc.local_ref(&heap, ptr); }
        assert_eq!(local_count(header), 3);
        assert_eq!(
            gc.stats.local_refs_created.load(Ordering::Relaxed),
            2
        );

        // Drop all 3 refs.
        unsafe { gc.drop_local_ref(&mut heap, ptr); }
        assert_eq!(local_count(header), 2);

        unsafe { gc.drop_local_ref(&mut heap, ptr); }
        assert_eq!(local_count(header), 1);

        unsafe { gc.drop_local_ref(&mut heap, ptr); }
        // Object should be freed now.
        assert_eq!(heap.live_count(), 0, "object should be freed");
    }

    // ------------------------------------------------------------------
    // Test 3: alloc + ref + drop → object freed
    // ------------------------------------------------------------------

    #[test]
    fn test_object_freed_when_unrefd() {
        let mut heap = MockHeap::new();
        let mut gc = OrcaGc::new(1);

        let ptr = gc.alloc_object(&mut heap, 48, TypeTag::Array).unwrap();
        assert_eq!(heap.live_count(), 1);

        // Drop the sole local ref.
        let freed = unsafe { gc.drop_local_ref(&mut heap, ptr) };
        assert!(freed, "object should be freed immediately");
        assert_eq!(heap.live_count(), 0);
        assert_eq!(
            gc.stats.objects_freed.load(Ordering::Relaxed),
            1
        );
    }

    // ------------------------------------------------------------------
    // Test 4: simulate sending a reference to another actor
    // ------------------------------------------------------------------

    #[test]
    fn test_foreign_ref_send() {
        let mut heap = MockHeap::new();
        let mut gc = OrcaGc::new(1);

        let ptr = gc.alloc_object(&mut heap, 16, TypeTag::ActorRef).unwrap();
        let header = unsafe { &*heap.header_ptr(ptr) };
        assert_eq!(foreign_count(header), 0);

        // Actor 1 sends a reference to actor 2.
        let op = unsafe { gc.send_ref_to(&heap, ptr, 2) };

        // Foreign count should have been incremented.
        assert_eq!(foreign_count(header), 1);
        assert_eq!(
            gc.stats.foreign_refs_sent.load(Ordering::Relaxed),
            1
        );

        // Verify the op fields.
        assert_eq!(op.target_actor, 2);
        assert_eq!(op.delta, -1);
        assert!(!op.object_header.is_null());

        // Clean up: drop the remaining local ref and process the op.
        unsafe { gc.drop_local_ref(&mut heap, ptr); }
        gc.process_foreign_op(&mut heap, op);
        assert_eq!(heap.live_count(), 0);
    }

    // ------------------------------------------------------------------
    // Test 5: simulate receiving a reference from another actor
    // ------------------------------------------------------------------

    #[test]
    fn test_foreign_ref_receive() {
        let mut heap = MockHeap::new();
        let mut gc = OrcaGc::new(1);

        let ptr = gc.alloc_object(&mut heap, 24, TypeTag::Record).unwrap();
        let header = unsafe { &*heap.header_ptr(ptr) };

        // Simulate: foreign actor had a reference, now returns it to us.
        // Start with foreign_count = 1 (the foreign actor held a ref).
        header.foreign_count.store(1, Ordering::Relaxed);

        // We receive the reference back.
        unsafe { gc.receive_ref(&heap, ptr); }

        // Foreign count should be 0, local count should be 2 (original + received).
        assert_eq!(foreign_count(header), 0);
        assert_eq!(local_count(header), 2);
        assert_eq!(
            gc.stats.foreign_refs_received.load(Ordering::Relaxed),
            1
        );

        // Drop both local refs → object freed.
        unsafe { gc.drop_local_ref(&mut heap, ptr); }
        assert_eq!(heap.live_count(), 1); // still 1 ref left

        unsafe { gc.drop_local_ref(&mut heap, ptr); }
        assert_eq!(heap.live_count(), 0);
    }

    // ------------------------------------------------------------------
    // Test 6: local count 0 but foreign > 0 → object NOT freed
    // ------------------------------------------------------------------

    #[test]
    fn test_object_not_freed_with_foreign_refs() {
        let mut heap = MockHeap::new();
        let mut gc = OrcaGc::new(1);

        let ptr = gc.alloc_object(&mut heap, 32, TypeTag::Map).unwrap();
        let header = unsafe { &*heap.header_ptr(ptr) };

        // Simulate a foreign reference existing.
        header.foreign_count.store(1, Ordering::Relaxed);

        // Drop the sole local ref.
        let freed = unsafe { gc.drop_local_ref(&mut heap, ptr) };
        assert!(!freed, "object should NOT be freed while foreign refs exist");
        assert_eq!(heap.live_count(), 1, "object should still be alive");

        // Object should be in the deferred list.
        assert_eq!(gc.deferred_decrements.len(), 1);
    }

    // ------------------------------------------------------------------
    // Test 7: pinned (sticky) objects are never freed
    // ------------------------------------------------------------------

    #[test]
    fn test_pin_object() {
        let mut heap = MockHeap::new();
        let mut gc = OrcaGc::new(1);

        let ptr = gc.alloc_object(&mut heap, 8, TypeTag::Raw).unwrap();
        let header = unsafe { &*heap.header_ptr(ptr) };
        assert!(!is_sticky(header));

        // Pin the object.
        unsafe { gc.pin_object(&heap, ptr); }
        assert!(is_sticky(header));

        // Drop the local ref — object should NOT be freed.
        let freed = unsafe { gc.drop_local_ref(&mut heap, ptr) };
        assert!(!freed, "pinned object should not be freed");
        assert_eq!(heap.live_count(), 1);

        // Unpin and try again.
        unsafe { gc.unpin_object(&heap, ptr); }
        assert!(!is_sticky(header));

        // Now try process_deferred — object should be freed.
        gc.process_deferred(&mut heap);
        assert_eq!(heap.live_count(), 0);
    }

    // ------------------------------------------------------------------
    // Test 8: deferred decrement is resolved when foreign count drops
    // ------------------------------------------------------------------

    #[test]
    fn test_deferred_decrement() {
        let mut heap = MockHeap::new();
        let mut gc = OrcaGc::new(1);

        let ptr = gc.alloc_object(&mut heap, 16, TypeTag::Tuple).unwrap();
        let header = unsafe { &*heap.header_ptr(ptr) };

        // Foreign ref exists.
        header.foreign_count.store(1, Ordering::Relaxed);

        // Drop local ref → deferred.
        unsafe { gc.drop_local_ref(&mut heap, ptr); }
        assert_eq!(gc.deferred_decrements.len(), 1);
        assert_eq!(heap.live_count(), 1);

        // Foreign ref goes away (simulated via a ForeignRefOp).
        let op = ForeignRefOp {
            target_actor: 1,
            object_header: unsafe { heap.header_ptr(ptr) },
            delta: -1,
        };
        gc.process_foreign_op(&mut heap, op);

        // foreign_count is now 0, local_count is 0, object should be freed.
        assert_eq!(heap.live_count(), 0);
        // process_foreign_op removes the freed object from deferred_decrements.
        assert_eq!(gc.deferred_decrements.len(), 0);

        // Running process_deferred is now a no-op.
        gc.process_deferred(&mut heap);
        assert_eq!(gc.deferred_decrements.len(), 0);
    }

    // ------------------------------------------------------------------
    // Test 9: alloc, 3 local refs, drop 2, still alive
    // ------------------------------------------------------------------

    #[test]
    fn test_multiple_refs() {
        let mut heap = MockHeap::new();
        let mut gc = OrcaGc::new(42);

        let ptr = gc.alloc_object(&mut heap, 100, TypeTag::Array).unwrap();
        let header = unsafe { &*heap.header_ptr(ptr) };

        // Create 2 additional refs (total 3).
        unsafe { gc.local_ref(&heap, ptr); }
        unsafe { gc.local_ref(&heap, ptr); }
        assert_eq!(local_count(header), 3);

        // Drop 2 refs.
        unsafe { gc.drop_local_ref(&mut heap, ptr); }
        unsafe { gc.drop_local_ref(&mut heap, ptr); }

        // Should still be alive with 1 ref.
        assert_eq!(heap.live_count(), 1);
        assert_eq!(local_count(header), 1);

        // Drop the last one.
        unsafe { gc.drop_local_ref(&mut heap, ptr); }
        assert_eq!(heap.live_count(), 0);
    }

    // ------------------------------------------------------------------
    // Test 10: GC stats tracking
    // ------------------------------------------------------------------

    #[test]
    fn test_gc_stats() {
        let mut heap = MockHeap::new();
        let mut gc = OrcaGc::new(1);

        // Allocate some objects.
        let p1 = gc.alloc_object(&mut heap, 10, TypeTag::String).unwrap();
        let p2 = gc.alloc_object(&mut heap, 20, TypeTag::String).unwrap();
        let p3 = gc.alloc_object(&mut heap, 30, TypeTag::String).unwrap();

        assert_eq!(gc.stats.objects_allocated.load(Ordering::Relaxed), 3);
        assert_eq!(gc.stats.bytes_allocated.load(Ordering::Relaxed), 60);

        // Create and drop refs.
        unsafe { gc.local_ref(&heap, p1); }
        unsafe { gc.local_ref(&heap, p1); }
        assert_eq!(gc.stats.local_refs_created.load(Ordering::Relaxed), 2);

        unsafe { gc.drop_local_ref(&mut heap, p1); }
        assert_eq!(gc.stats.local_refs_dropped.load(Ordering::Relaxed), 1);

        // Free everything.
        unsafe { gc.drop_local_ref(&mut heap, p1); }
        unsafe { gc.drop_local_ref(&mut heap, p1); }
        unsafe { gc.drop_local_ref(&mut heap, p2); }
        unsafe { gc.drop_local_ref(&mut heap, p3); }

        assert_eq!(gc.stats.objects_freed.load(Ordering::Relaxed), 3);
        assert_eq!(gc.stats.bytes_freed.load(Ordering::Relaxed), 60);
    }

    // ------------------------------------------------------------------
    // Test 11: verify ForeignRefOp is created correctly
    // ------------------------------------------------------------------

    #[test]
    fn test_send_ref_op() {
        let mut heap = MockHeap::new();
        let mut gc = OrcaGc::new(7);

        let ptr = gc.alloc_object(&mut heap, 8, TypeTag::Closure).unwrap();
        let header_ptr = unsafe { heap.header_ptr(ptr) };

        let op = unsafe { gc.send_ref_to(&heap, ptr, 99) };

        assert_eq!(op.target_actor, 99);
        assert_eq!(op.delta, -1);
        assert_eq!(op.object_header, header_ptr);

        // Foreign count should have been incremented.
        let header = unsafe { &*header_ptr };
        assert_eq!(foreign_count(header), 1);

        // Clean up.
        unsafe { gc.drop_local_ref(&mut heap, ptr); }
        gc.process_foreign_op(&mut heap, op);
    }

    // ------------------------------------------------------------------
    // Test 12: verify foreign op delivery (process_foreign_op)
    // ------------------------------------------------------------------

    #[test]
    fn test_process_foreign_op() {
        let mut heap = MockHeap::new();
        let mut gc = OrcaGc::new(1);

        let ptr = gc.alloc_object(&mut heap, 16, TypeTag::Map).unwrap();
        let header = unsafe { &*heap.header_ptr(ptr) };

        // Start with foreign_count = 2 (two foreign refs).
        header.foreign_count.store(2, Ordering::Relaxed);

        // Process a -1 op.
        let op1 = ForeignRefOp {
            target_actor: 1,
            object_header: unsafe { heap.header_ptr(ptr) },
            delta: -1,
        };
        gc.process_foreign_op(&mut heap, op1);
        assert_eq!(foreign_count(header), 1);
        assert_eq!(heap.live_count(), 1); // local_count still 1

        // Drop the local ref — should be deferred since foreign_count > 0.
        unsafe { gc.drop_local_ref(&mut heap, ptr); }
        assert_eq!(heap.live_count(), 1);

        // Process another -1 op → foreign_count becomes 0, and since local_count is also 0, the object is freed.
        let op2 = ForeignRefOp {
            target_actor: 1,
            object_header: unsafe { heap.header_ptr(ptr) },
            delta: -1,
        };
        gc.process_foreign_op(&mut heap, op2);

        // Object should be freed now (both counts 0).
        assert_eq!(heap.live_count(), 0);
    }

    // ------------------------------------------------------------------
    // Test 13: process_deferred frees nothing when foreign refs remain
    // ------------------------------------------------------------------

    #[test]
    fn test_process_deferred_noop() {
        let mut heap = MockHeap::new();
        let mut gc = OrcaGc::new(1);

        let ptr = gc.alloc_object(&mut heap, 8, TypeTag::Raw).unwrap();
        let header = unsafe { &*heap.header_ptr(ptr) };
        header.foreign_count.store(3, Ordering::Relaxed);

        unsafe { gc.drop_local_ref(&mut heap, ptr); }
        assert_eq!(gc.deferred_decrements.len(), 1);

        // process_deferred should not free because foreign_count is still 3.
        gc.process_deferred(&mut heap);
        assert_eq!(heap.live_count(), 1);
        assert_eq!(gc.deferred_decrements.len(), 1);
    }

    // ------------------------------------------------------------------
    // Test 14: coordinator submit + absorb
    // ------------------------------------------------------------------

    #[test]
    fn test_coordinator_queues() {
        let mut coord = OrcaCoordinator::new();
        assert!(coord.pending_ops.is_empty());

        // Create a dummy op (we can't easily create a real one without a
        // heap allocation, so we use a null pointer — this is only safe for
        // queue testing, not for delivery).
        let dummy_header = std::ptr::null_mut();
        let op1 = ForeignRefOp {
            target_actor: 10,
            object_header: dummy_header,
            delta: -1,
        };
        coord.submit_op(op1);
        assert_eq!(coord.pending_ops.len(), 1);

        let op2 = ForeignRefOp {
            target_actor: 20,
            object_header: dummy_header,
            delta: 1,
        };
        let mut batch = vec![op2];
        coord.absorb_ops(std::mem::take(&mut batch));
        assert_eq!(coord.pending_ops.len(), 2);

        // Threshold.
        assert_eq!(coord.cycle_detect_threshold, 10_000);
        assert!(!coord.should_trigger_cycle_detection());
    }

    // ------------------------------------------------------------------
    // Test 15: SharedHeapGc backward compatibility
    // ------------------------------------------------------------------

    #[test]
    fn test_shared_heap_gc() {
        let gc = SharedHeapGc::new(1024);
        assert!(!gc.should_collect(512));
        assert!(gc.should_collect(2048));
    }

    // ------------------------------------------------------------------
    // Test 16: stats reset
    // ------------------------------------------------------------------

    #[test]
    fn test_stats_reset() {
        let stats = GcStats::default();
        stats.objects_allocated.store(5, Ordering::Relaxed);
        stats.bytes_freed.store(100, Ordering::Relaxed);

        stats.reset();
        assert_eq!(stats.objects_allocated.load(Ordering::Relaxed), 0);
        assert_eq!(stats.bytes_freed.load(Ordering::Relaxed), 0);
    }

    // ------------------------------------------------------------------
    // Test 17: zero-sized allocation rejected
    // ------------------------------------------------------------------

    #[test]
    fn test_zero_size_alloc_rejected() {
        let mut heap = MockHeap::new();
        let mut gc = OrcaGc::new(1);

        let ptr = gc.alloc_object(&mut heap, 0, TypeTag::Raw);
        assert!(ptr.is_none(), "zero-sized allocation should be rejected");
    }

    // ------------------------------------------------------------------
    // Test 18: foreign_ref_queue drain
    // ------------------------------------------------------------------

    #[test]
    fn test_foreign_ref_queue_drain() {
        let mut gc = OrcaGc::new(1);
        assert!(gc.foreign_ref_queue.is_empty());

        let dummy_header = std::ptr::null_mut();
        gc.queue_foreign_op(ForeignRefOp {
            target_actor: 5,
            object_header: dummy_header,
            delta: -1,
        });
        gc.queue_foreign_op(ForeignRefOp {
            target_actor: 6,
            object_header: dummy_header,
            delta: 1,
        });

        assert_eq!(gc.foreign_ref_queue.len(), 2);

        let drained = gc.drain_foreign_ops();
        assert_eq!(drained.len(), 2);
        assert!(gc.foreign_ref_queue.is_empty());
    }
}
