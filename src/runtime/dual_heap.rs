//! Dual-region generational heap for the Nulang actor runtime.
//!
//! This module provides a generational garbage collector that separates
//! short-lived (nursery) and long-lived (tenured) objects. New allocations
//! go into the fast bump-allocated nursery; objects that survive multiple
//! minor GC cycles are promoted to the tenured region backed by [`ActorHeap`].
//!
//! # Architecture
//!
//! ```text
//!  +------------------------------------------------------------+
//!  |                          DualHeap                           |
//!  |   +----------------------+  +----------------------------+ |
//!  |   |   NurseryRegion      |  |    TenuredRegion           | |
//!  |   |  (bump allocator)    |  |  (ActorHeap w/ free lists) | |
//!  |   |                      |  |                            | |
//!  |   |  +------+---------+  |  |  +------+---------------+  | |
//!  |   |  | NObj | payload |  |  |  | Hdr  |    payload    |  | |
//!  |   |  +------+---------+  |  |  +------+---------------+  | |
//!  |   |  | NObj | payload |  |  |  | Hdr  |    payload    |  | |
//!  |   |  +------+---------+  |  |  +------+---------------+  | |
//!  |   |  | ...  |  ...    |  |  |  | ...  |    ...        |  | |
//!  |   |  +------+---------+  |  |  +------+---------------+  | |
//!  |   +----------------------+  +----------------------------+ |
//!  +------------------------------------------------------------+
//! ```
//!
//! # Allocation path
//!
//! 1. [`DualHeap::alloc`] tries the nursery bump allocator first.
//! 2. If the nursery is full, allocation falls back to the tenured region.
//! 3. Large allocations can bypass the nursery (configurable).
//!
//! # Minor GC
//!
//! When the nursery exceeds 80% occupancy:
//! 1. Trace from the root set (register values, stack roots).
//! 2. For each reachable nursery object, increment `survival_count`.
//! 3. If `survival_count >= promotion_threshold` (default: 2): copy the
//!    payload to the tenured region.
//! 4. Reachable objects below the threshold are copied into a fresh nursery.
//! 5. Unreachable objects are discarded (the old nursery space is reclaimed).
//!
//! # Safety
//!
//! All pointer-manipulating methods are `unsafe` where required by the
//! [`OrcaHeap`] trait. The caller (the runtime scheduler) is responsible
//! for ensuring that:
//! * Payload pointers are valid and owned by this heap.
//! * No data races occur (each actor has its own heap).
//! * The root set is accurate during minor GC.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::heap::{ActorHeap, GcColor, OrcaHeader, SizeClass, TypeTag};
use super::gc::OrcaHeap as OrcaHeapTrait;
use super::gc::OrcaHeader as GcOrcaHeader;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Required alignment for all allocations (header + payload).
const ALIGN: usize = 8;

/// Default nursery size (256 KiB).
pub const DEFAULT_NURSERY_SIZE: usize = 256 * 1024;

/// Default tenured size (4 MiB).
pub const DEFAULT_TENURED_SIZE: usize = 4 * 1024 * 1024;

/// Default promotion threshold — objects must survive this many minor GCs
/// before being promoted to the tenured region.
pub const DEFAULT_PROMOTION_THRESHOLD: u8 = 2;

/// Nursery fullness threshold that triggers minor GC (80%).
const NURSERY_GC_THRESHOLD: f64 = 0.80;

// ---------------------------------------------------------------------------
// Alignment helper
// ---------------------------------------------------------------------------

/// Round `size` up to the next multiple of `ALIGN` (8).
#[inline(always)]
const fn align_up(size: usize) -> usize {
    (size + ALIGN - 1) & !(ALIGN - 1)
}

/// Simple size classification (mirrors `heap::classify_total_size` which is
/// module-private).  Used by the nursery to set a size class on the header.
fn classify_size(total_size: usize) -> SizeClass {
    match total_size {
        0..=64 => SizeClass::Small,
        65..=128 => SizeClass::Medium,
        129..=256 => SizeClass::Large,
        _ => SizeClass::Huge,
    }
}

// ---------------------------------------------------------------------------
// NurseryObject
// ---------------------------------------------------------------------------

/// Header embedded before every allocation in the nursery region.
///
/// Extends [`OrcaHeader`] with a `survival_count` for generational tracking
/// and a `next` pointer for the intrusive singly-linked list.
///
/// # Memory layout (verified by `test_nursery_object_size`)
///
/// ```text
///  offset | field
///  -------+---------------
///    0    | header         (OrcaHeader — 48 bytes)
///   48    | next           (*mut NurseryObject — 8 bytes)
///   56    | survival_count (u8)
///   57    | _pad           ([u8; 7])
///  -------+---------------
///   64    | TOTAL
///  -------+---------------
///   64    | payload starts here (8-byte aligned)
/// ```
#[repr(C)]
pub struct NurseryObject {
    /// Standard ORCA header (ref count, type tag, size class, etc.).
    pub header: OrcaHeader,
    /// Next pointer for the nursery's intrusive singly-linked list.
    pub next: *mut NurseryObject,
    /// How many minor GCs this object has survived (0 = newly allocated).
    pub survival_count: u8,
    /// Padding to align the total struct to 64 bytes.
    _pad: [u8; 7],
}

impl NurseryObject {
    /// Total size of the nursery object header in bytes.
    pub const SIZE: usize = 64;

    /// Create a new nursery object header in already-zeroed memory.
    ///
    /// # Safety
    /// The memory at `nobj_ptr` must be at least `NurseryObject::SIZE` bytes
    /// and should be zero-initialized before calling this function.
    unsafe fn initialize_at(
        nobj_ptr: *mut NurseryObject,
        actor_id: u64,
        size_class: SizeClass,
        type_tag: TypeTag,
        total_size: usize,
    ) {
        let nobj = &mut *nobj_ptr;
        nobj.header.ref_count = AtomicU32::new(1);
        nobj.header.foreign_count = AtomicU32::new(0);
        nobj.header.sticky = AtomicBool::new(false);
        nobj.header.size_class = size_class;
        nobj.header.gc_color = GcColor::White;
        nobj.header.type_tag = type_tag;
        // _pad is already zero from write_bytes
        nobj.header.actor_id = actor_id;
        nobj.header.size = total_size;
        // live_next and live_prev are already null from write_bytes
        nobj.next = std::ptr::null_mut();
        nobj.survival_count = 0;
        // _pad is already zero from write_bytes
    }
}

// ---------------------------------------------------------------------------
// NurseryRegion
// ---------------------------------------------------------------------------

/// Fast bump allocator for short-lived objects.
///
/// `NurseryRegion` manages a contiguous memory block using pure bump
/// allocation.  There are no free lists — the nursery is reset en masse
/// during minor GC.  All live objects are tracked via an intrusive
/// singly-linked list embedded in [`NurseryObject`].
///
/// # Thread safety
///
/// `NurseryRegion` is **not** `Sync` — it is designed to be owned by a
/// single actor.  It **is** `Send` so that an actor (and its heap) can be
/// migrated between scheduler threads.
pub struct NurseryRegion {
    /// Base pointer of the contiguous backing memory.
    base: *mut u8,
    /// Bump pointer — next free byte.
    current: *mut u8,
    /// One-past-the-end pointer.
    limit: *mut u8,
    /// Total size of the backing block.
    total_size: usize,
    /// Bytes committed by the bump pointer.
    used_bytes: usize,
    /// Head of the intrusive singly-linked list of live objects.
    live_head: *mut NurseryObject,
    /// Number of allocations performed.
    alloc_count: usize,
}

// NurseryRegion can be sent between scheduler threads.
unsafe impl Send for NurseryRegion {}

impl NurseryRegion {
    // ------------------------------------------------------------------
    // Construction
    // ------------------------------------------------------------------

    /// Create a new nursery region with the given total backing size.
    ///
    /// # Panics
    /// Panics if `size` is zero.
    pub fn new(size: usize) -> Self {
        assert!(size > 0, "NurseryRegion size must be > 0");
        let layout = std::alloc::Layout::from_size_align(size, ALIGN)
            .expect("invalid NurseryRegion layout");
        let base = unsafe { std::alloc::alloc(layout) };
        if base.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        NurseryRegion {
            base,
            current: base,
            limit: unsafe { base.add(size) },
            total_size: size,
            used_bytes: 0,
            live_head: std::ptr::null_mut(),
            alloc_count: 0,
        }
    }

    // ------------------------------------------------------------------
    // Allocation
    // ------------------------------------------------------------------

    /// Allocate an object with the given payload size, type tag, and actor ID.
    ///
    /// Returns a pointer to the **payload** (the writable region just past
    /// the [`NurseryObject`]).  The nursery object header is automatically
    /// prepended and initialised.
    ///
    /// Returns `None` if the nursery is full.
    pub fn alloc(
        &mut self,
        payload_size: usize,
        type_tag: TypeTag,
        actor_id: u64,
    ) -> Option<*mut u8> {
        let aligned_payload = align_up(payload_size);
        let total_size = NurseryObject::SIZE + aligned_payload;

        unsafe {
            let new_current = self.current.add(total_size);
            if new_current > self.limit {
                return None; // Nursery is full.
            }

            let nobj_ptr = self.current as *mut NurseryObject;
            let payload_ptr = self.current.add(NurseryObject::SIZE);
            self.current = new_current;
            self.used_bytes += total_size;

            // Zero the nursery object header before initialising fields.
            std::ptr::write_bytes(nobj_ptr as *mut u8, 0, NurseryObject::SIZE);

            let size_class = classify_size(total_size);
            NurseryObject::initialize_at(nobj_ptr, actor_id, size_class, type_tag, total_size);

            // Insert at head of singly-linked list (O(1)).
            (*nobj_ptr).next = self.live_head;
            self.live_head = nobj_ptr;

            self.alloc_count += 1;
            Some(payload_ptr)
        }
    }

    // ------------------------------------------------------------------
    // Header recovery
    // ------------------------------------------------------------------

    /// Recover the [`NurseryObject`] pointer from a payload pointer.
    ///
    /// # Safety
    /// `payload_ptr` must point to the payload region of a valid nursery
    /// allocation (i.e. it must have been returned by `alloc`).
    pub unsafe fn nursery_object_of(payload_ptr: *mut u8) -> *mut NurseryObject {
        (payload_ptr as *mut NurseryObject).offset(-1)
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Total bytes committed by the bump allocator.
    pub fn used(&self) -> usize {
        self.used_bytes
    }

    /// Remaining free space in the nursery.
    pub fn free_bytes(&self) -> usize {
        self.total_size - self.used_bytes
    }

    /// Number of objects currently allocated in the nursery.
    pub fn alloc_count(&self) -> usize {
        self.alloc_count
    }

    /// Total size of the nursery backing block.
    pub fn total_size(&self) -> usize {
        self.total_size
    }

    /// Returns `true` if there is no remaining free space.
    pub fn is_full(&self) -> bool {
        self.free_bytes() == 0
    }

    /// Returns the occupancy ratio (0.0 = empty, 1.0 = full).
    pub fn occupancy(&self) -> f64 {
        if self.total_size == 0 {
            0.0
        } else {
            self.used_bytes as f64 / self.total_size as f64
        }
    }

    // ------------------------------------------------------------------
    // Iteration
    // ------------------------------------------------------------------

    /// Iterate over all nursery objects (live allocations).
    ///
    /// Calls `callback` with a pointer to each [`NurseryObject`] in the
    /// nursery.  The order follows the allocation order (newest first
    /// because of LIFO list insertion).
    pub fn iter_live<F>(&self, mut callback: F)
    where
        F: FnMut(*mut NurseryObject),
    {
        let mut current = self.live_head;
        while !current.is_null() {
            unsafe {
                callback(current);
                current = (*current).next;
            }
        }
    }

    /// Iterate over all nursery objects and their payloads.
    ///
    /// Calls `callback` with `(nursery_object_ptr, payload_ptr, payload_size)`.
    pub fn iter_live_with_payload<F>(&self, mut callback: F)
    where
        F: FnMut(*mut NurseryObject, *mut u8, usize),
    {
        self.iter_live(|nobj_ptr| unsafe {
            let payload_ptr = (nobj_ptr as *mut u8).add(NurseryObject::SIZE);
            let payload_size = (*nobj_ptr).header.size - NurseryObject::SIZE;
            callback(nobj_ptr, payload_ptr, payload_size);
        });
    }

    // ------------------------------------------------------------------
    // Reset
    // ------------------------------------------------------------------

    /// Reset the nursery to a pristine state.
    ///
    /// * The bump pointer returns to `base`.
    /// * The live-object list is cleared.
    /// * Statistics are zeroed.
    ///
    /// All objects previously allocated in the nursery become invalid — the
    /// caller must ensure no outstanding references remain (or that they have
    /// been updated after promotion / copying).
    pub fn reset(&mut self) {
        self.current = self.base;
        self.used_bytes = 0;
        self.live_head = std::ptr::null_mut();
        self.alloc_count = 0;
    }
}

// ---------------------------------------------------------------------------
// Drop
// ---------------------------------------------------------------------------

impl Drop for NurseryRegion {
    fn drop(&mut self) {
        let layout =
            std::alloc::Layout::from_size_align(self.total_size, ALIGN).unwrap();
        // SAFETY: `self.base` was allocated with the exact same layout in
        // `NurseryRegion::new`.  After dealloc the pointer must not be used.
        unsafe {
            std::alloc::dealloc(self.base, layout);
        }
    }
}

// ---------------------------------------------------------------------------
// TenuredRegion
// ---------------------------------------------------------------------------

/// Thin wrapper around [`ActorHeap`] that serves as the tenured (old)
/// generation in the dual-heap system.
///
/// The tenured region uses the full `ActorHeap` machinery (size-class free
/// lists, doubly-linked live-object list) to efficiently manage long-lived
/// objects.
pub struct TenuredRegion {
    heap: ActorHeap,
    total_size: usize,
}

impl TenuredRegion {
    // ------------------------------------------------------------------
    // Construction
    // ------------------------------------------------------------------

    /// Create a new tenured region with the given total backing size.
    pub fn new(total_size: usize) -> Self {
        TenuredRegion {
            heap: ActorHeap::new(total_size),
            total_size,
        }
    }

    /// Set the owning actor ID on the underlying heap.
    pub fn set_actor_id(&mut self, id: u64) {
        self.heap.set_actor_id(id);
    }

    // ------------------------------------------------------------------
    // Allocation
    // ------------------------------------------------------------------

    /// Allocate an object in the tenured region.
    ///
    /// Delegates to [`ActorHeap::alloc`].  Returns a pointer to the payload.
    pub fn alloc(&mut self, payload_size: usize, type_tag: TypeTag) -> Option<*mut u8> {
        self.heap.alloc(payload_size, type_tag)
    }

    // ------------------------------------------------------------------
    // Free
    // ------------------------------------------------------------------

    /// Free an object back to the tenured region's free lists.
    ///
    /// # Safety
    /// `payload_ptr` must be a live pointer returned by `alloc` on this
    /// tenured region.
    pub unsafe fn free(&mut self, payload_ptr: *mut u8) {
        self.heap.free(payload_ptr);
    }

    // ------------------------------------------------------------------
    // Iteration
    // ------------------------------------------------------------------

    /// Iterate over all live objects in the tenured region.
    ///
    /// Calls `callback` with `(header_ptr, payload_ptr, payload_size)`.
    pub fn iter_live<F>(&self, callback: F)
    where
        F: FnMut(*mut OrcaHeader, *mut u8, usize),
    {
        self.heap.iter_live_objects(callback);
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Total bytes committed in the tenured region.
    pub fn used(&self) -> usize {
        self.heap.used()
    }

    /// Remaining free space in the tenured region.
    pub fn free_bytes(&self) -> usize {
        self.heap.free_bytes()
    }

    /// Number of live objects in the tenured region.
    pub fn live_count(&self) -> usize {
        self.heap.live_count()
    }

    /// Total size of the tenured backing block.
    pub fn total_size(&self) -> usize {
        self.total_size
    }

    /// Number of objects in free lists (available for reuse).
    pub fn free_list_count(&self) -> usize {
        self.heap.free_list_count()
    }

    // ------------------------------------------------------------------
    // Reset
    // ------------------------------------------------------------------

    /// Reset the tenured region to a pristine state.
    ///
    /// Delegates to [`ActorHeap::reset`].
    pub fn reset(&mut self) {
        self.heap.reset();
    }
}

// ---------------------------------------------------------------------------
// DualHeapStats
// ---------------------------------------------------------------------------

/// Snapshot of allocation and GC statistics for a [`DualHeap`].
#[derive(Debug, Clone, Default)]
pub struct DualHeapStats {
    /// Bytes currently used in the nursery.
    pub nursery_used: usize,
    /// Total size of the nursery backing block.
    pub nursery_total: usize,
    /// Number of objects currently in the nursery.
    pub nursery_alloc_count: usize,
    /// Bytes currently used in the tenured region.
    pub tenured_used: usize,
    /// Total size of the tenured backing block.
    pub tenured_total: usize,
    /// Number of live objects in the tenured region.
    pub tenured_live_count: usize,
    /// Number of minor GC cycles performed.
    pub minor_gc_count: u64,
    /// Number of objects promoted to tenured.
    pub promoted_count: u64,
    /// Current promotion threshold.
    pub promotion_threshold: u8,
}

// ---------------------------------------------------------------------------
// DualHeap
// ---------------------------------------------------------------------------

/// Dual-region generational heap allocator.
///
/// `DualHeap` combines a fast bump-allocated nursery for short-lived objects
/// with a full-featured tenured region (backed by [`ActorHeap`]) for
/// long-lived data.  The generational hypothesis — that most objects die
/// young — means that most allocations never touch the tenured region,
/// reducing fragmentation and free-list pressure.
///
/// # Generational GC
///
/// When the nursery fills up (configurable threshold, default 80%), a minor
/// GC is triggered:
/// 1. Trace from the root set to find all reachable nursery objects.
/// 2. For each reachable object, increment its `survival_count`.
/// 3. Objects whose `survival_count` meets or exceeds the promotion
///    threshold are copied to the tenured region.
/// 4. Reachable objects below the threshold are copied into a fresh nursery.
/// 5. Unreachable objects are discarded when the old nursery space is reclaimed.
///
/// # Thread safety
///
/// `DualHeap` is **not** `Sync` — it is designed to be owned by a single
/// actor.  It **is** `Send` so that an actor can be migrated between
/// scheduler threads.
pub struct DualHeap {
    /// Fast nursery for short-lived objects.
    nursery: NurseryRegion,
    /// Tenured region for long-lived objects.
    tenured: TenuredRegion,
    /// Owning actor ID (0 until `set_actor_id` is called).
    actor_id: u64,
    /// Number of minor GC cycles performed.
    minor_gc_count: u64,
    /// Number of objects promoted to tenured.
    promoted_count: u64,
    /// Nursery backing size.
    nursery_size: usize,
    /// Tenured backing size.
    tenured_size: usize,
    /// Objects must survive this many minor GCs to be promoted.
    promotion_threshold: u8,
}

// DualHeap can be sent between scheduler threads.
unsafe impl Send for DualHeap {}

impl DualHeap {
    // ------------------------------------------------------------------
    // Construction
    // ------------------------------------------------------------------

    /// Create a new dual heap with the given nursery and tenured sizes.
    ///
    /// Uses the default promotion threshold (2).
    ///
    /// # Panics
    /// Panics if either size is zero.
    pub fn new(nursery_size: usize, tenured_size: usize) -> Self {
        DualHeap {
            nursery: NurseryRegion::new(nursery_size),
            tenured: TenuredRegion::new(tenured_size),
            actor_id: 0,
            minor_gc_count: 0,
            promoted_count: 0,
            nursery_size,
            tenured_size,
            promotion_threshold: DEFAULT_PROMOTION_THRESHOLD,
        }
    }

    /// Create a new dual heap with default sizes (256 KiB nursery, 4 MiB tenured).
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_NURSERY_SIZE, DEFAULT_TENURED_SIZE)
    }

    /// Set the owning actor ID.
    ///
    /// All subsequently allocated objects will have this `actor_id` written
    /// into their header.  Existing objects are **not** updated.
    pub fn set_actor_id(&mut self, id: u64) {
        self.actor_id = id;
        self.tenured.set_actor_id(id);
    }

    /// Set the promotion threshold.
    ///
    /// Objects must survive at least `threshold` minor GCs before being
    /// promoted to the tenured region.
    pub fn set_promotion_threshold(&mut self, threshold: u8) {
        self.promotion_threshold = threshold;
    }

    // ------------------------------------------------------------------
    // Allocation
    // ------------------------------------------------------------------

    /// Allocate an object with the given payload size and type tag.
    ///
    /// First attempts allocation in the nursery (fast bump path).  If the
    /// nursery is full, falls back to the tenured region.
    ///
    /// Returns a pointer to the **payload** (past the header).  Returns
    /// `None` if both regions are exhausted.
    pub fn alloc(&mut self, payload_size: usize, type_tag: TypeTag) -> Option<*mut u8> {
        // Try nursery first (fast path).
        if let Some(ptr) = self.nursery.alloc(payload_size, type_tag, self.actor_id) {
            return Some(ptr);
        }

        // Fall back to tenured region.
        self.tenured.alloc(payload_size, type_tag)
    }

    // ------------------------------------------------------------------
    // Pointer classification
    // ------------------------------------------------------------------

    /// Returns `true` if `payload_ptr` points into the nursery region.
    ///
    /// This is used by [`OrcaHeap::free_payload`] and [`OrcaHeap::header_ptr`]
    /// to route operations to the correct region.  If a pointer is not in the
    /// nursery, it is assumed to be in the tenured region.
    pub fn ptr_in_nursery(&self, payload_ptr: *mut u8) -> bool {
        payload_ptr >= self.nursery.base && payload_ptr < self.nursery.limit
    }

    // ------------------------------------------------------------------
    // Minor GC
    // ------------------------------------------------------------------

    /// Run a generational (minor) garbage collection on the nursery.
    ///
    /// # Algorithm
    /// 1. Build the transitive closure of reachable nursery objects starting
    ///    from `root_set` (payload pointers that are directly reachable).
    /// 2. For each reachable nursery object, increment `survival_count`.
    /// 3. If `survival_count >= promotion_threshold`: copy the payload to
    ///    the tenured region (promotion).
    /// 4. Reachable objects below the threshold are retained in the nursery.
    /// 5. Reset the nursery — unreachable objects are reclaimed.
    /// 6. Retained objects are re-allocated in the fresh nursery.
    ///
    /// # Returns
    /// A vector of `(old_payload_ptr, new_payload_ptr)` mappings for objects
    /// that were moved (promoted to tenured or re-allocated in the fresh
    /// nursery).  The caller should update any references accordingly.
    ///
    /// # Safety
    /// The `root_set` must contain all payload pointers that are directly
    /// reachable from registers, the stack, or global roots.  Missing roots
    /// will cause live objects to be incorrectly reclaimed.
    pub fn minor_gc(&mut self, root_set: &[*mut u8]) -> Vec<(*mut u8, *mut u8)> {
        self.minor_gc_count += 1;

        // Step 1: Find all reachable nursery objects (transitive closure).
        let reachable = self.find_reachable_nursery_objects(root_set);

        // Step 2: Classify reachable objects and collect payload data.
        let mut retained: Vec<(*mut u8, TypeTag, usize, u8, Vec<u8>)> = Vec::new();
        // (old_payload_ptr, type_tag, payload_size, survival_count, payload_bytes)

        let mut moved_mappings: Vec<(*mut u8, *mut u8)> = Vec::new();

        for &nobj_ptr in &reachable {
            unsafe {
                let nobj = &mut *nobj_ptr;
                let payload_ptr = (nobj_ptr as *mut u8).add(NurseryObject::SIZE);
                let payload_size = nobj.header.size.saturating_sub(NurseryObject::SIZE);

                nobj.survival_count += 1;

                if nobj.survival_count >= self.promotion_threshold {
                    // --------------------------------------------------
                    // Promote to tenured region
                    // --------------------------------------------------
                    if let Some(new_payload) = self.tenured.alloc(payload_size, nobj.header.type_tag)
                    {
                        // Copy payload from nursery to tenured.
                        std::ptr::copy_nonoverlapping(payload_ptr, new_payload, payload_size);

                        // Set actor_id on the new header.
                        let new_header = ActorHeap::header_of(new_payload);
                        (*new_header).actor_id = self.actor_id;

                        moved_mappings.push((payload_ptr, new_payload));
                        self.promoted_count += 1;
                    }
                } else {
                    // --------------------------------------------------
                    // Retain in nursery — copy payload for re-allocation
                    // --------------------------------------------------
                    let mut payload_bytes = vec![0u8; payload_size];
                    std::ptr::copy_nonoverlapping(
                        payload_ptr,
                        payload_bytes.as_mut_ptr(),
                        payload_size,
                    );
                    retained.push((
                        payload_ptr,
                        nobj.header.type_tag,
                        payload_size,
                        nobj.survival_count,
                        payload_bytes,
                    ));
                }
            }
        }

        // Step 3: Reset nursery (discards all old objects, live or dead).
        self.nursery.reset();

        // Step 4: Re-allocate retained objects in fresh nursery.
        for (old_payload_ptr, type_tag, payload_size, survival_count, payload_bytes) in retained {
            if payload_size == 0 {
                continue;
            }
            if let Some(new_payload) = self.nursery.alloc(payload_size, type_tag, self.actor_id) {
                unsafe {
                    // Copy payload into the fresh nursery allocation.
                    std::ptr::copy_nonoverlapping(payload_bytes.as_ptr(), new_payload, payload_size);

                    // Restore the survival count on the new nursery object.
                    let new_nobj_ptr = NurseryRegion::nursery_object_of(new_payload);
                    (*new_nobj_ptr).survival_count = survival_count;
                }
                moved_mappings.push((old_payload_ptr, new_payload));
            }
        }

        moved_mappings
    }

    /// Build the transitive closure of reachable nursery objects from the
    /// given root set.
    ///
    /// Uses a conservative stack-scanning approach: every 8-byte-aligned word
    /// in a reachable object's payload is checked to see if it could be a
    /// pointer to another nursery object.  This is sound (may retain more
    /// than necessary) but not precise (may have false positives).
    fn find_reachable_nursery_objects(
        &self,
        root_set: &[*mut u8],
    ) -> Vec<*mut NurseryObject> {
        // Collect all nursery objects into a set for fast membership testing.
        let mut all_nursery_objects: HashSet<*mut NurseryObject> = HashSet::new();
        self.nursery.iter_live(|nobj_ptr| {
            all_nursery_objects.insert(nobj_ptr);
        });

        if all_nursery_objects.is_empty() {
            return Vec::new();
        }

        let mut reachable: HashSet<*mut NurseryObject> = HashSet::new();
        let mut worklist: Vec<*mut u8> = root_set.iter().copied().collect();

        while let Some(payload_ptr) = worklist.pop() {
            if payload_ptr.is_null() {
                continue;
            }

            // Check if this payload pointer falls inside the nursery.
            if !self.ptr_in_nursery(payload_ptr) {
                continue;
            }

            // Recover the nursery object header.
            let nobj_ptr = unsafe { NurseryRegion::nursery_object_of(payload_ptr) };

            // Verify this is actually a valid nursery object.
            if !all_nursery_objects.contains(&nobj_ptr) {
                continue;
            }

            // If we haven't visited this object yet, scan its payload.
            if reachable.insert(nobj_ptr) {
                unsafe {
                    let nobj = &*nobj_ptr;
                    let payload_size = nobj.header.size.saturating_sub(NurseryObject::SIZE);

                    // Conservatively scan every 8-byte-aligned word.
                    let step = std::mem::size_of::<usize>();
                    for offset in (0..payload_size.saturating_sub(step - 1)).step_by(step) {
                        let word_ptr = payload_ptr.add(offset) as *mut usize;
                        let word = *word_ptr;

                        // Check if this word could be a nursery payload pointer.
                        let candidate = word as *mut u8;
                        if self.ptr_in_nursery(candidate) {
                            let candidate_nobj =
                                NurseryRegion::nursery_object_of(candidate);
                            if all_nursery_objects.contains(&candidate_nobj)
                                && !reachable.contains(&candidate_nobj)
                            {
                                worklist.push(candidate);
                            }
                        }
                    }
                }
            }
        }

        reachable.into_iter().collect()
    }

    // ------------------------------------------------------------------
    // GC triggering
    // ------------------------------------------------------------------

    /// Returns `true` if the nursery is more than 80% full and a minor GC
    /// should be triggered.
    pub fn should_minor_gc(&self) -> bool {
        self.nursery.occupancy() >= NURSERY_GC_THRESHOLD
    }

    // ------------------------------------------------------------------
    // Statistics
    // ------------------------------------------------------------------

    /// Return a snapshot of allocation and GC statistics.
    pub fn stats(&self) -> DualHeapStats {
        DualHeapStats {
            nursery_used: self.nursery.used(),
            nursery_total: self.nursery_size,
            nursery_alloc_count: self.nursery.alloc_count(),
            tenured_used: self.tenured.used(),
            tenured_total: self.tenured_size,
            tenured_live_count: self.tenured.live_count(),
            minor_gc_count: self.minor_gc_count,
            promoted_count: self.promoted_count,
            promotion_threshold: self.promotion_threshold,
        }
    }

    // ------------------------------------------------------------------
    // Queries (forwarded)
    // ------------------------------------------------------------------

    /// Total bytes used across both regions.
    pub fn total_used(&self) -> usize {
        self.nursery.used() + self.tenured.used()
    }

    /// Total free bytes across both regions.
    pub fn total_free(&self) -> usize {
        self.nursery.free_bytes() + self.tenured.free_bytes()
    }

    /// Number of live objects in the tenured region.
    pub fn tenured_live_count(&self) -> usize {
        self.tenured.live_count()
    }

    /// Number of objects in the nursery.
    pub fn nursery_alloc_count(&self) -> usize {
        self.nursery.alloc_count()
    }

    // ------------------------------------------------------------------
    // Reset
    // ------------------------------------------------------------------

    /// Reset both regions to a pristine state.
    ///
    /// All objects become invalid — the caller must ensure no outstanding
    /// references remain.
    pub fn reset(&mut self) {
        self.nursery.reset();
        self.tenured.reset();
        self.minor_gc_count = 0;
        self.promoted_count = 0;
    }
}

// ---------------------------------------------------------------------------
// OrcaHeap trait implementation
// ---------------------------------------------------------------------------

impl OrcaHeapTrait for DualHeap {
    /// Allocate payload bytes.  Preferentially goes to the nursery.
    fn alloc_payload(&mut self, payload_size: usize) -> Option<*mut u8> {
        self.alloc(payload_size, TypeTag::Raw)
    }

    /// Free a payload previously returned by `alloc_payload`.
    ///
    /// # Safety
    /// `payload_ptr` must be a live pointer returned by `alloc_payload` on
    /// this exact heap.
    ///
    /// For nursery objects, individual free is a no-op — the object will be
    /// reclaimed during the next minor GC.  For tenured objects, delegates to
    /// [`ActorHeap::free`].
    unsafe fn free_payload(&mut self, payload_ptr: *mut u8) {
        if self.ptr_in_nursery(payload_ptr) {
            // Nursery objects are not individually freed.
            // They are bulk-reclaimed during minor GC.
        } else {
            self.tenured.free(payload_ptr);
        }
    }

    /// Recover the GC header pointer from a payload pointer.
    ///
    /// # Safety
    /// `payload_ptr` must be a valid payload pointer from this heap.
    unsafe fn header_ptr(&self, payload_ptr: *mut u8) -> *mut GcOrcaHeader {
        if self.ptr_in_nursery(payload_ptr) {
            let nobj_ptr = NurseryRegion::nursery_object_of(payload_ptr);
            &mut (*nobj_ptr).header as *mut OrcaHeader as *mut GcOrcaHeader
        } else {
            ActorHeap::header_of(payload_ptr) as *mut GcOrcaHeader
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod dual_heap_tests {
    use super::*;
    use std::sync::atomic::Ordering;

    // ------------------------------------------------------------------
    // Test 1: Basic nursery allocation
    // ------------------------------------------------------------------

    #[test]
    fn test_nursery_alloc() {
        let mut nursery = NurseryRegion::new(64 * 1024);

        let p = nursery.alloc(16, TypeTag::String, 1).expect("alloc failed");
        assert!(!p.is_null());

        // Write and verify data.
        unsafe {
            let data = std::slice::from_raw_parts_mut(p, 16);
            data.copy_from_slice(b"Hello, Nulang!!!");
            assert_eq!(&data[..6], b"Hello,");
        }

        // Verify the nursery object header.
        unsafe {
            let nobj = &*NurseryRegion::nursery_object_of(p);
            assert_eq!(nobj.header.ref_count.load(Ordering::Relaxed), 1);
            assert_eq!(nobj.header.actor_id, 1);
            assert_eq!(nobj.header.type_tag, TypeTag::String);
            assert_eq!(nobj.survival_count, 0);
        }

        assert_eq!(nursery.alloc_count(), 1);
        assert!(nursery.used() > 0);
    }

    // ------------------------------------------------------------------
    // Test 2: Nursery reset clears all objects
    // ------------------------------------------------------------------

    #[test]
    fn test_nursery_reset() {
        let mut nursery = NurseryRegion::new(64 * 1024);

        // Allocate several objects.
        let p1 = nursery.alloc(16, TypeTag::Tuple, 1).unwrap();
        let p2 = nursery.alloc(32, TypeTag::Array, 1).unwrap();
        let p3 = nursery.alloc(48, TypeTag::Map, 1).unwrap();

        assert_eq!(nursery.alloc_count(), 3);
        assert!(nursery.used() > 0);

        // Reset.
        nursery.reset();

        assert_eq!(nursery.alloc_count(), 0);
        assert_eq!(nursery.used(), 0);
        assert_eq!(nursery.free_bytes(), 64 * 1024);

        // Should be able to allocate again from a clean slate.
        let q = nursery.alloc(100, TypeTag::Record, 1);
        assert!(q.is_some());
    }

    // ------------------------------------------------------------------
    // Test 3: DualHeap allocation goes to nursery
    // ------------------------------------------------------------------

    #[test]
    fn test_dual_heap_alloc() {
        let mut heap = DualHeap::new(64 * 1024, 256 * 1024);
        heap.set_actor_id(42);

        let p = heap.alloc(16, TypeTag::String).expect("alloc failed");
        assert!(!p.is_null());

        // Should be in the nursery.
        assert!(heap.ptr_in_nursery(p));

        // Verify header.
        unsafe {
            let nobj = &*NurseryRegion::nursery_object_of(p);
            assert_eq!(nobj.header.actor_id, 42);
            assert_eq!(nobj.header.type_tag, TypeTag::String);
        }

        let stats = heap.stats();
        assert_eq!(stats.nursery_alloc_count, 1);
        assert_eq!(stats.tenured_live_count, 0);
    }

    // ------------------------------------------------------------------
    // Test 4: When nursery is full, allocation falls back to tenured
    // ------------------------------------------------------------------

    #[test]
    fn test_nursery_full_fallback() {
        // Tiny nursery: can only hold a few objects.
        let mut heap = DualHeap::new(256, 64 * 1024);
        heap.set_actor_id(1);

        // Fill the nursery only — stop when allocations start going to tenured.
        let mut total_nursery_allocs = 0;
        loop {
            if let Some(p) = heap.alloc(16, TypeTag::Tuple) {
                if !heap.ptr_in_nursery(p) {
                    // This one went to tenured — nursery is already full.
                    break;
                }
                total_nursery_allocs += 1;
                if total_nursery_allocs > 100 {
                    panic!("nursery should have filled up by now");
                }
            } else {
                panic!("tenured should not be full yet");
            }
        }

        // Nursery should now be full.
        assert!(heap.nursery.is_full());

        // Next allocation should fall back to tenured.
        let p_tenured = heap.alloc(16, TypeTag::String);
        assert!(p_tenured.is_some(), "should fall back to tenured");

        let p_tenured = p_tenured.unwrap();
        assert!(!heap.ptr_in_nursery(p_tenured), "should be in tenured");

        let stats = heap.stats();
        assert!(stats.tenured_live_count >= 1);
    }

    // ------------------------------------------------------------------
    // Test 5: Minor GC promotes reachable objects
    // ------------------------------------------------------------------

    #[test]
    fn test_minor_gc_promotes_survivors() {
        let mut heap = DualHeap::new(64 * 1024, 256 * 1024);
        heap.set_actor_id(1);
        heap.set_promotion_threshold(1); // Promote after 1 survival

        // Allocate an object.
        let p = heap.alloc(16, TypeTag::String).unwrap();

        // Run minor GC with this object as a root.
        let mappings = heap.minor_gc(&[p]);

        // Should have promoted the object.
        assert_eq!(heap.stats().promoted_count, 1);
        assert_eq!(heap.stats().minor_gc_count, 1);

        // The object should now be in tenured.
        assert_eq!(heap.tenured.live_count(), 1);

        // Nursery should be empty (object was promoted, not retained).
        assert_eq!(heap.nursery.alloc_count(), 0);

        // Verify the mapping points to the tenured region.
        assert!(!mappings.is_empty());
        let (_, new_ptr) = mappings[0];
        assert!(!heap.ptr_in_nursery(new_ptr));
    }

    // ------------------------------------------------------------------
    // Test 6: Minor GC reclaims unreferenced objects
    // ------------------------------------------------------------------

    #[test]
    fn test_minor_gc_reclaims_unreferenced() {
        let mut heap = DualHeap::new(64 * 1024, 256 * 1024);
        heap.set_actor_id(1);

        // Allocate several objects without keeping roots.
        let _p1 = heap.alloc(16, TypeTag::String).unwrap();
        let _p2 = heap.alloc(32, TypeTag::Tuple).unwrap();
        let _p3 = heap.alloc(48, TypeTag::Array).unwrap();

        assert_eq!(heap.nursery.alloc_count(), 3);

        // Run minor GC with an empty root set — all objects are garbage.
        let mappings = heap.minor_gc(&[]);

        // Nothing promoted, nothing retained.
        assert_eq!(heap.stats().promoted_count, 0);
        assert!(mappings.is_empty());

        // Nursery should be empty.
        assert_eq!(heap.nursery.alloc_count(), 0);
        assert_eq!(heap.nursery.used(), 0);

        // Tenured should also be empty.
        assert_eq!(heap.tenured.live_count(), 0);
    }

    // ------------------------------------------------------------------
    // Test 7: should_minor_gc threshold detection
    // ------------------------------------------------------------------

    #[test]
    fn test_should_minor_gc() {
        let mut heap = DualHeap::new(1024, 64 * 1024);
        heap.set_actor_id(1);

        // Initially empty — should not trigger.
        assert!(!heap.should_minor_gc());

        // Fill past 80%.
        while heap.nursery.occupancy() < NURSERY_GC_THRESHOLD {
            let _ = heap.alloc(16, TypeTag::Tuple);
        }

        // Should now trigger.
        assert!(heap.should_minor_gc());
    }

    // ------------------------------------------------------------------
    // Test 8: Stats are accurate
    // ------------------------------------------------------------------

    #[test]
    fn test_stats() {
        let mut heap = DualHeap::new(64 * 1024, 256 * 1024);
        heap.set_actor_id(7);
        heap.set_promotion_threshold(3);

        // Allocate some objects.
        let p1 = heap.alloc(16, TypeTag::String).unwrap();
        let _p2 = heap.alloc(32, TypeTag::Tuple).unwrap();
        let _p3 = heap.alloc(48, TypeTag::Array).unwrap();

        let stats_before = heap.stats();
        assert_eq!(stats_before.nursery_alloc_count, 3);
        assert_eq!(stats_before.nursery_total, 64 * 1024);
        assert_eq!(stats_before.tenured_total, 256 * 1024);
        assert_eq!(stats_before.promotion_threshold, 3);
        assert_eq!(stats_before.minor_gc_count, 0);
        assert_eq!(stats_before.promoted_count, 0);

        // Run a minor GC with one root.
        heap.minor_gc(&[p1]);

        let stats_after = heap.stats();
        assert_eq!(stats_after.minor_gc_count, 1);
        assert_eq!(stats_after.promotion_threshold, 3);
        // p1 survived but wasn't promoted (threshold = 3).
        assert_eq!(stats_after.promoted_count, 0);
    }

    // ------------------------------------------------------------------
    // Test 9: OrcaHeap trait implementation
    // ------------------------------------------------------------------

    #[test]
    fn test_orca_heap_trait() {
        let mut heap = DualHeap::new(64 * 1024, 256 * 1024);
        heap.set_actor_id(99);

        // alloc_payload — should go to nursery.
        let p = heap.alloc_payload(32).expect("alloc_payload failed");
        assert!(!p.is_null());
        assert!(heap.ptr_in_nursery(p), "should be in nursery");

        // header_ptr — returns a gc::OrcaHeader pointer.
        // The first 3 fields (local_count, foreign_count, sticky) are at
        // compatible offsets with heap::OrcaHeader (ref_count, foreign_count,
        // sticky).  actor_id and type_tag are NOT at compatible offsets.
        unsafe {
            let header = &*heap.header_ptr(p);
            // local_count in gc header maps to ref_count in heap header (offset 0).
            assert_eq!(header.local_count.load(Ordering::Relaxed), 1);
            // foreign_count is at compatible offset 4.
            assert_eq!(header.foreign_count.load(Ordering::Relaxed), 0);
            // sticky is at compatible offset 8.
            assert!(!header.sticky.load(Ordering::Relaxed));
        }

        // Verify actor_id and type_tag through the native heap header path.
        unsafe {
            let nobj = &*NurseryRegion::nursery_object_of(p);
            assert_eq!(nobj.header.actor_id, 99);
            assert_eq!(nobj.header.type_tag, TypeTag::Raw);
        }

        // free_payload (nursery object — no-op, but should not crash).
        unsafe {
            heap.free_payload(p);
        }

        // Allocate in tenured by filling nursery first.
        let mut tenured_ptr = None;
        loop {
            if let Some(p) = heap.alloc_payload(8) {
                if !heap.ptr_in_nursery(p) {
                    tenured_ptr = Some(p);
                    break;
                }
            } else {
                break;
            }
        }

        // If we got a tenured allocation, test free on it.
        if let Some(tp) = tenured_ptr {
            assert!(!heap.ptr_in_nursery(tp), "should be in tenured");
            unsafe {
                let header_before = &*heap.header_ptr(tp);
                assert_eq!(header_before.local_count.load(Ordering::Relaxed), 1);
                heap.free_payload(tp);
            }
            // After freeing the tenured object, the live count should drop.
            // (The free list now has 1 reusable block.)
            assert_eq!(heap.tenured.free_list_count(), 1);
        }
    }

    // ------------------------------------------------------------------
    // Test 10: Multiple minor GC cycles
    // ------------------------------------------------------------------

    #[test]
    fn test_multiple_minor_gcs() {
        let mut heap = DualHeap::new(64 * 1024, 256 * 1024);
        heap.set_actor_id(1);
        heap.set_promotion_threshold(2); // Default threshold

        // Allocate a long-lived object.
        let mut p = heap.alloc(16, TypeTag::String).unwrap();

        // GC cycle 1: object survives, survival_count = 1 (below threshold).
        let mappings1 = heap.minor_gc(&[p]);
        assert_eq!(heap.stats().minor_gc_count, 1);
        assert_eq!(heap.stats().promoted_count, 0);

        // Update pointer from mappings.
        p = mappings1.iter().find(|(old, _)| *old == p).map(|(_, new)| *new).unwrap_or(p);

        // GC cycle 2: object survives, survival_count = 2 (meets threshold).
        let mappings2 = heap.minor_gc(&[p]);
        assert_eq!(heap.stats().minor_gc_count, 2);
        assert_eq!(heap.stats().promoted_count, 1);

        // Verify the object was promoted to tenured.
        let promoted_ptr = mappings2.iter().find(|(old, _)| *old == p).map(|(_, new)| *new);
        assert!(promoted_ptr.is_some());
        let promoted_ptr = promoted_ptr.unwrap();
        assert!(!heap.ptr_in_nursery(promoted_ptr));

        // Tenured should have 1 live object.
        assert_eq!(heap.tenured.live_count(), 1);
    }

    // ------------------------------------------------------------------
    // Test 11: Promotion threshold
    // ------------------------------------------------------------------

    #[test]
    fn test_promotion_threshold() {
        let mut heap = DualHeap::new(64 * 1024, 256 * 1024);
        heap.set_actor_id(1);
        heap.set_promotion_threshold(3); // Must survive 3 GCs

        let mut p = heap.alloc(16, TypeTag::String).unwrap();

        // After 1st GC: survival_count = 1 — not promoted.
        let m1 = heap.minor_gc(&[p]);
        assert_eq!(heap.stats().promoted_count, 0);
        p = m1.iter().find(|(old, _)| *old == p).map(|(_, new)| *new).unwrap_or(p);

        // After 2nd GC: survival_count = 2 — not promoted.
        let m2 = heap.minor_gc(&[p]);
        assert_eq!(heap.stats().promoted_count, 0);
        p = m2.iter().find(|(old, _)| *old == p).map(|(_, new)| *new).unwrap_or(p);

        // After 3rd GC: survival_count = 3 — promoted!
        let m3 = heap.minor_gc(&[p]);
        assert_eq!(heap.stats().promoted_count, 1);
        assert_eq!(heap.stats().minor_gc_count, 3);

        // Verify promoted object is in tenured.
        let promoted_ptr = m3.iter().find(|(old, _)| *old == p).map(|(_, new)| *new);
        assert!(promoted_ptr.is_some());
        assert!(!heap.ptr_in_nursery(promoted_ptr.unwrap()));
    }

    // ------------------------------------------------------------------
    // Test 12: Nursery object size is 64 bytes
    // ------------------------------------------------------------------

    #[test]
    fn test_nursery_object_size_is_64() {
        assert_eq!(NurseryObject::SIZE, 64);
        assert_eq!(std::mem::size_of::<NurseryObject>(), 64);
    }

    // ------------------------------------------------------------------
    // Test 13: Nursery exhaustion returns None
    // ------------------------------------------------------------------

    #[test]
    fn test_nursery_exhaustion() {
        let mut nursery = NurseryRegion::new(128);

        // First allocation should succeed (64-byte header + some payload).
        let p1 = nursery.alloc(32, TypeTag::Raw, 1);
        assert!(p1.is_some());

        // Keep allocating until full.
        loop {
            if nursery.alloc(32, TypeTag::Raw, 1).is_none() {
                break;
            }
        }

        // Should be full now.
        assert!(nursery.is_full());

        // Next allocation should fail.
        let p_fail = nursery.alloc(32, TypeTag::Raw, 1);
        assert!(p_fail.is_none());
    }

    // ------------------------------------------------------------------
    // Test 14: Mixed nursery and tenured allocations
    // ------------------------------------------------------------------

    #[test]
    fn test_mixed_allocations() {
        let mut heap = DualHeap::new(512, 64 * 1024);
        heap.set_actor_id(1);

        let mut nursery_ptrs = Vec::new();
        let mut tenured_ptrs = Vec::new();

        // Allocate many objects; some will go to nursery, some to tenured.
        for i in 0..20 {
            let p = heap.alloc(16, TypeTag::Tuple).unwrap();
            if heap.ptr_in_nursery(p) {
                nursery_ptrs.push(p);
            } else {
                // Mark tenured objects with an identifier.
                unsafe { *(p as *mut u64) = i as u64; }
                tenured_ptrs.push(p);
            }
        }

        // Should have both nursery and tenured allocations.
        assert!(!nursery_ptrs.is_empty(), "should have nursery allocs");
        assert!(!tenured_ptrs.is_empty(), "should have tenured allocs");

        // Verify tenured objects are accessible.
        for (i, &p) in tenured_ptrs.iter().enumerate() {
            unsafe {
                let val = *(p as *mut u64);
                assert_eq!(val, i as u64 + nursery_ptrs.len() as u64);
            }
        }
    }

    // ------------------------------------------------------------------
    // Test 15: DualHeap reset clears everything
    // ------------------------------------------------------------------

    #[test]
    fn test_dual_heap_reset() {
        let mut heap = DualHeap::new(64 * 1024, 256 * 1024);
        heap.set_actor_id(1);

        // Allocate in both regions.
        let p1 = heap.alloc(16, TypeTag::String).unwrap();
        assert!(heap.ptr_in_nursery(p1));

        // Fill nursery and get a tenured alloc.
        let mut p2 = None;
        for _ in 0..1000 {
            if let Some(p) = heap.alloc(16, TypeTag::Tuple) {
                if !heap.ptr_in_nursery(p) {
                    p2 = Some(p);
                    break;
                }
            }
        }

        // Reset everything.
        heap.reset();

        assert_eq!(heap.nursery.alloc_count(), 0);
        assert_eq!(heap.tenured.live_count(), 0);
        assert_eq!(heap.stats().minor_gc_count, 0);
        assert_eq!(heap.stats().promoted_count, 0);
    }
}
