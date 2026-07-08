//! ORCA-compatible per-actor heap allocator for Nulang.
//!
//! Stage A1 — Nulang v0.4 ORCA Garbage Collector
//!
//! This module provides a bump allocator backed by a contiguous memory block,
//! with per-size-class intrusive free lists for fast object reuse. Every
//! allocation carries an [`OrcaHeader`] that stores reference counts, GC
//! colour, type tag, and live-object linked-list pointers.
//!
//! # Design decisions
//!
//! * **Bump allocation** for speed on the fast path (most allocations).
//! * **Size-class free lists** (Tiny → Huge) so that freed objects can be
//!   reused without touching the bump pointer.
//! * **Intrusive live list** — every live object is a node in a doubly-linked
//!   list embedded in the header. This makes `iter_live_objects` O(live) and
//!   avoids auxiliary hash maps or bitmaps.
//! * **8-byte alignment** is enforced for every allocation (header + payload).
//! * **Zero `actor_id` default** — the heap is created before the actor ID is
//!   known; callers should invoke `set_actor_id` immediately after creation.

use std::sync::atomic::{AtomicBool, AtomicU32};
#[cfg(test)]
use std::sync::atomic::Ordering;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Required alignment for every allocation (header + payload).
const ALIGN: usize = 8;

/// Number of discrete size classes.
const NUM_SIZE_CLASSES: usize = 5;

// ---------------------------------------------------------------------------
// SizeClass
// ---------------------------------------------------------------------------

/// Size classification for heap objects.
///
/// Each class represents an upper bound on the *total* allocation size
/// (header + aligned payload). Free lists are bucketed by this class so
/// that reallocation of similarly-sized objects is cache-friendly.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeClass {
    /// Up to 32 bytes total.
    Tiny = 0,
    /// 33–64 bytes total.
    Small = 1,
    /// 65–128 bytes total.
    Medium = 2,
    /// 129–256 bytes total.
    Large = 3,
    /// 257+ bytes total (unbounded).
    Huge = 4,
}

impl SizeClass {}

/// Map a *total* allocation size (header + payload, already aligned) to its
/// size class and the rounded-up block size for free-list bucketing.
fn classify_total_size(total_size: usize) -> (SizeClass, usize) {
    // Clamp to at least the header size so that even zero-payload
    // allocations have a well-defined class.
    let total_size = total_size.max(std::mem::size_of::<OrcaHeader>());
    match total_size {
        0..=32 => (SizeClass::Tiny, 32),
        33..=64 => (SizeClass::Small, 64),
        65..=128 => (SizeClass::Medium, 128),
        129..=256 => (SizeClass::Large, 256),
        n => (SizeClass::Huge, n),
    }
}

// ---------------------------------------------------------------------------
// GcColor
// ---------------------------------------------------------------------------

/// Tri-colour marker used by the ORCA cycle detector.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcColor {
    /// Object is potentially garbage (not yet visited).
    White = 0,
    /// Object has been discovered but children not yet scanned.
    Gray = 1,
    /// Object and its transitive children are reachable.
    Black = 2,
}

// ---------------------------------------------------------------------------
// TypeTag
// ---------------------------------------------------------------------------

/// Runtime type tag for heap-allocated objects.
///
/// The GC and the debugger use this to interpret payload layout.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeTag {
    /// Reference to another actor (contains a `u64` actor id).
    ActorRef = 0,
    /// Dynamically-sized array.
    Array = 1,
    /// UTF-8 string data.
    String = 2,
    /// Record / object with named fields.
    Record = 3,
    /// Function closure (captures environment).
    Closure = 4,
    /// Hash map.
    Map = 5,
    /// Fixed-size tuple.
    Tuple = 6,
    /// Raw untyped data (FFI boundaries).
    Raw = 7,
}

// ---------------------------------------------------------------------------
// OrcaHeader
// ---------------------------------------------------------------------------

/// Header prepended to every heap allocation.
///
/// The header is laid out with `#[repr(C)]` so that the payload pointer
/// returned by [`ActorHeap::alloc`] is always exactly one `OrcaHeader`
/// stride past the header base address. [`ActorHeap::header_of`] recovers
/// the header by walking backward one stride.
///
/// # Memory layout ( verified by `test_header_size` )
///
/// ```text
///  offset | field
///  -------+---------------
///    0    | ref_count      (AtomicU32)
///    4    | foreign_count  (AtomicU32)
///    8    | sticky         (AtomicBool)
///    9    | size_class     (SizeClass   u8)
///   10    | gc_color       (GcColor     u8)
///   11    | type_tag       (TypeTag     u8)
///   12    | _pad           ([u8; 4]) — aligns actor_id to 8 bytes
///   16    | actor_id       (u64)
///   24    | size           (usize — total bytes, header + aligned payload)
///   32    | payload_size   (usize — requested payload bytes)
///   40    | live_next      (*mut OrcaHeader)
///   48    | live_prev      (*mut OrcaHeader)
///  -------+---------------
///   56    | TOTAL
/// ```
#[repr(C)]
pub struct OrcaHeader {
    /// Local reference count — how many references exist *inside* the owning
    /// actor. When this drops to zero the object may be reclaimed.
    pub ref_count: AtomicU32,
    /// Foreign reference count — how many references exist in *other* actors.
    /// Part of the ORCA protocol for cross-actor reference tracking.
    pub foreign_count: AtomicU32,
    /// When `true` the object is immortal (sticky) and must never be collected.
    /// Used for global constants and pinned FFI buffers.
    pub sticky: AtomicBool,
    /// Size class bucket this object belongs to.
    pub size_class: SizeClass,
    /// GC tri-colour state.
    pub gc_color: GcColor,
    /// Runtime type tag — tells the GC how to scan this object's payload.
    pub type_tag: TypeTag,
    /// Padding to ensure `actor_id` (u64) is 8-byte aligned.
    _pad: [u8; 4],
    /// ID of the actor that owns this object.
    pub actor_id: u64,
    /// Total bytes allocated for this object (header + aligned payload).
    pub size: usize,
    /// Requested payload size in bytes (as passed to `alloc`).
    pub payload_size: usize,
    /// Intrusive next pointer for the live-object doubly-linked list.
    /// This is internal to the allocator and not part of the public ORCA spec.
    pub(crate) live_next: *mut OrcaHeader,
    /// Intrusive previous pointer for the live-object doubly-linked list.
    pub(crate) live_prev: *mut OrcaHeader,
}

impl OrcaHeader {
    /// Create a *logically* initialised header on the caller's stack.
    ///
    /// # Safety
    /// The returned value must be copied into heap-backed storage (via
    /// `ptr::write`) before any other thread can observe it.
    pub(crate) fn new(
        actor_id: u64,
        type_tag: TypeTag,
        total_size: usize,
        payload_size: usize,
    ) -> Self {
        let (size_class, _) = classify_total_size(total_size);
        OrcaHeader {
            ref_count: AtomicU32::new(1),
            foreign_count: AtomicU32::new(0),
            sticky: AtomicBool::new(false),
            size_class,
            gc_color: GcColor::White,
            type_tag,
            _pad: [0; 4],
            actor_id,
            size: total_size,
            payload_size,
            live_next: std::ptr::null_mut(),
            live_prev: std::ptr::null_mut(),
        }
    }
}

// ---------------------------------------------------------------------------
// ActorHeap
// ---------------------------------------------------------------------------

/// Per-actor heap allocator with ORCA-compatible object headers.
///
/// `ActorHeap` combines a fast bump allocator with size-class free lists.
/// All allocations are 8-byte aligned and carry an [`OrcaHeader`].  The
/// allocator maintains an intrusive doubly-linked list of *live* objects so
/// that the GC can walk all reachable objects in O(live) time.
///
/// # Thread safety
///
/// `ActorHeap` is **not** `Sync` — it is designed to be owned by a single
/// actor and accessed only while that actor is running.  It **is** `Send`
/// so that an actor (and its heap) can be migrated between scheduler threads.
#[derive(Debug)]
pub struct ActorHeap {
    /// Owning actor ID (0 until `set_actor_id` is called).
    actor_id: u64,
    /// Base pointer of the contiguous backing memory.
    base: *mut u8,
    /// Bump pointer — next free byte in the backing block.
    current: *mut u8,
    /// One-past-the-end pointer of the backing block.
    limit: *mut u8,
    /// Total size of the backing block (bytes).
    total_size: usize,
    /// Bytes committed by the bump pointer (i.e. `current - base`).
    used_bytes: usize,
    /// Per-size-class intrusive free lists.
    /// Each entry is either `null_mut()` or points to the payload of the
    /// first free block in that class.  The first 8 bytes of a free payload
    /// store a `*mut u8` to the next free block.
    free_lists: [*mut u8; NUM_SIZE_CLASSES],
    /// Head of the live-object doubly-linked list.
    live_head: *mut OrcaHeader,
    /// Tail of the live-object doubly-linked list.
    live_tail: *mut OrcaHeader,
    /// Number of objects currently in the live list.
    live_count: usize,
    /// Cumulative allocations (including reuses from free lists).
    total_allocs: usize,
    /// Cumulative frees.
    total_frees: usize,
    /// High-water mark of `used_bytes`.
    peak_used: usize,
}

// ActorHeap can be sent between scheduler threads because it owns all of
// its memory and no other thread holds pointers into it.
unsafe impl Send for ActorHeap {}

/// Round `size` up to the next multiple of `ALIGN` (8).
#[inline(always)]
const fn align_up(size: usize) -> usize {
    (size + ALIGN - 1) & !(ALIGN - 1)
}

impl ActorHeap {
    /// Size of the ORCA header in bytes.
    pub const HEADER_SIZE: usize = std::mem::size_of::<OrcaHeader>();

    // ------------------------------------------------------------------
    // Construction
    // ------------------------------------------------------------------

    /// Create a new per-actor heap with the given total backing size.
    ///
    /// The backing memory is allocated with the global allocator and is
    /// 8-byte aligned.  The `actor_id` defaults to `0`; the caller should
    /// invoke [`ActorHeap::set_actor_id`] as soon as the real actor ID is
    /// known.
    ///
    /// # Panics
    ///
    /// Panics if `total_size` is zero or the layout is invalid.
    pub fn new(total_size: usize) -> Self {
        assert!(total_size > 0, "ActorHeap size must be > 0");
        let layout = std::alloc::Layout::from_size_align(total_size, ALIGN)
            .expect("invalid ActorHeap layout");
        let base = unsafe { std::alloc::alloc(layout) };
        if base.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        ActorHeap {
            actor_id: 0,
            base,
            current: base,
            limit: unsafe { base.add(total_size) },
            total_size,
            used_bytes: 0,
            free_lists: [std::ptr::null_mut(); NUM_SIZE_CLASSES],
            live_head: std::ptr::null_mut(),
            live_tail: std::ptr::null_mut(),
            live_count: 0,
            total_allocs: 0,
            total_frees: 0,
            peak_used: 0,
        }
    }

    /// Set the owning actor ID.
    ///
    /// All subsequently allocated objects will have this `actor_id` written
    /// into their header.  Existing objects are **not** updated.
    pub fn set_actor_id(&mut self, id: u64) {
        self.actor_id = id;
    }

    // ------------------------------------------------------------------
    // Allocation
    // ------------------------------------------------------------------

    /// Allocate an object with the given payload size and type tag.
    ///
    /// Returns a pointer to the **payload** (the writable region just past
    /// the [`OrcaHeader`]).  The header is automatically prepended and
    /// initialised with:
    ///
    /// * `ref_count = 1`
    /// * `foreign_count = 0`
    /// * `sticky = false`
    /// * `gc_color = White`
    /// * `actor_id` = the heap's current actor ID
    /// * `size_class` computed from the total allocation size
    ///
    /// # Algorithm
    ///
    /// 1. Align `payload_size` to 8 bytes.
    /// 2. Compute `total_size = HEADER_SIZE + aligned_payload`.
    /// 3. Determine the size class.
    /// 4. Check the corresponding free list — if a block is available, pop
    ///    it, rewrite the header fields, and return the payload pointer.
    /// 5. Otherwise fall back to bump allocation from the contiguous region.
    ///
    /// Returns `None` if the backing memory is exhausted.
    pub fn alloc(&mut self, payload_size: usize, type_tag: TypeTag) -> Option<*mut u8> {
        let aligned_payload = align_up(payload_size);
        let total_size = Self::HEADER_SIZE + aligned_payload;
        let (size_class, _) = classify_total_size(total_size);
        let sc_idx = size_class as usize;

        // --- Fast path: try the free list for this size class ---
        if sc_idx < NUM_SIZE_CLASSES {
            unsafe {
                if !self.free_lists[sc_idx].is_null() {
                    // Pop the first block from the intrusive list.
                    let payload_ptr = self.free_lists[sc_idx];
                    // The first 8 bytes of the free payload hold the next pointer.
                    let next_free = *(payload_ptr as *mut *mut u8);
                    self.free_lists[sc_idx] = next_free;

                    // Rewrite header (the old header values are stale).
                    let header_ptr = Self::header_of(payload_ptr);
                    // SAFETY: payload_ptr came from a previous alloc on this
                    // heap, so header_ptr points to a valid OrcaHeader inside
                    // our backing block.
                    std::ptr::write(
                        header_ptr,
                        OrcaHeader::new(self.actor_id, type_tag, total_size, payload_size),
                    );

                    self.add_to_live_list(header_ptr);
                    self.live_count += 1;
                    self.total_allocs += 1;
                    return Some(payload_ptr);
                }
            }
        }

        // --- Slow path: bump allocation ---
        unsafe {
            let new_current = self.current.add(total_size);
            if new_current > self.limit {
                return None; // Out of memory.
            }

            let header_ptr = self.current as *mut OrcaHeader;
            let payload_ptr = self.current.add(Self::HEADER_SIZE);
            self.current = new_current;
            self.used_bytes += total_size;

            // Initialise the header in place.
            std::ptr::write(
                header_ptr,
                OrcaHeader::new(self.actor_id, type_tag, total_size, payload_size),
            );

            self.add_to_live_list(header_ptr);
            self.live_count += 1;
            self.total_allocs += 1;

            if self.used_bytes > self.peak_used {
                self.peak_used = self.used_bytes;
            }

            Some(payload_ptr)
        }
    }

    // ------------------------------------------------------------------
    // Free
    // ------------------------------------------------------------------

    /// Free an object back to the free list for its size class.
    ///
    /// `payload_ptr` must be a pointer previously returned by [`alloc`].
    /// The object is removed from the live list and its payload memory is
    /// repurposed as an intrusive linked-list node.
    ///
    /// # Safety
    ///
    /// * `payload_ptr` must be valid and must have come from `alloc` on this
    ///   exact heap.
    /// * No references to this object may remain — violating this is UB.
    /// * This method must not be called twice on the same pointer (double-free).
    pub unsafe fn free(&mut self, payload_ptr: *mut u8) {
        // Recover the header.
        let header_ptr = Self::header_of(payload_ptr);

        // Remove from the live list.
        self.remove_from_live_list(header_ptr);
        self.live_count -= 1;
        self.total_frees += 1;

        // Read the size class so we know which free list to push to.
        // We use a relaxed read because no other thread should be mutating
        // this header concurrently (ActorHeap is !Sync).
        let sc = (*header_ptr).size_class;
        let sc_idx = sc as usize;

        if sc_idx < NUM_SIZE_CLASSES {
            // Intrusive free list: the first 8 bytes of the (now dead) payload
            // store a pointer to the previous head of the free list.
            //
            // SAFETY: the payload is at least 8 bytes for every size class
            // except Tiny, and Tiny is never actually used because the header
            // alone is 56 bytes. Therefore we always have room for a pointer.
            *(payload_ptr as *mut *mut u8) = self.free_lists[sc_idx];
            self.free_lists[sc_idx] = payload_ptr;
        }
        // If the size class is somehow out of range we simply leak the block.
        // This should never happen because classify_total_size only returns
        // valid discriminants.
    }

    // ------------------------------------------------------------------
    // Header recovery
    // ------------------------------------------------------------------

    /// Recover the [`OrcaHeader`] pointer from a payload pointer.
    ///
    /// # Safety
    ///
    /// `payload_ptr` must point to the payload region of a valid allocation
    /// on this heap (i.e. it must have been returned by `alloc`).  The
    /// header is located exactly `HEADER_SIZE` bytes before the payload
    /// because of the `#[repr(C)]` layout.
    pub unsafe fn header_of(payload_ptr: *mut u8) -> *mut OrcaHeader {
        // Cast to *mut OrcaHeader and offset by -1.  Because OrcaHeader is
        // 56 bytes, this subtracts 56 bytes from the address, landing exactly
        // on the header that was laid out immediately before the payload.
        (payload_ptr as *mut OrcaHeader).offset(-1)
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Total bytes committed by the bump allocator (`current - base`).
    pub fn used(&self) -> usize {
        self.used_bytes
    }

    /// Remaining free space in the bump region.
    pub fn free_bytes(&self) -> usize {
        self.total_size - self.used_bytes
    }

    /// Number of objects currently alive (allocated but not freed).
    pub fn live_count(&self) -> usize {
        self.live_count
    }

    /// Total number of objects sitting in free lists (available for reuse).
    pub fn free_list_count(&self) -> usize {
        let mut count = 0usize;
        for sc_idx in 0..NUM_SIZE_CLASSES {
            let mut cursor = self.free_lists[sc_idx];
            while !cursor.is_null() {
                count += 1;
                // SAFETY: cursor is a payload pointer from a previous free()
                // on this heap. The first 8 bytes hold the next pointer.
                unsafe {
                    cursor = *(cursor as *mut *mut u8);
                }
            }
        }
        count
    }

    // ------------------------------------------------------------------
    // Iteration
    // ------------------------------------------------------------------

    /// Iterate over all live objects.
    ///
    /// Calls `callback` with `(header_ptr, payload_ptr, payload_size)` for
    /// every object that is currently in the live list (i.e. allocated and
    /// not yet freed).  The order follows the allocation order because the
    /// live list is append-only.
    ///
    /// # Usage in GC mark phase
    ///
    /// ```ignore
    /// heap.iter_live_objects(|header, payload, size| {
    ///     unsafe {
    ///         (*header).gc_color = GcColor::Gray;
    ///         // enqueue payload for scanning ...
    ///     }
    /// });
    /// ```
    pub fn iter_live_objects<F>(&self, mut callback: F)
    where
        F: FnMut(*mut OrcaHeader, *mut u8, usize),
    {
        let mut current = self.live_head;
        while !current.is_null() {
            unsafe {
                // Payload starts one header stride past the header.
                let payload_ptr = current.add(1) as *mut u8;
                let payload_size = (*current).size - Self::HEADER_SIZE;
                callback(current, payload_ptr, payload_size);
                current = (*current).live_next;
            }
        }
    }

    // ------------------------------------------------------------------
    // Reset
    // ------------------------------------------------------------------

    /// Reset the heap to a pristine state.
    ///
    /// * The bump pointer returns to `base`.
    /// * All free lists are discarded.
    /// * The live-object list is cleared.
    /// * Statistics are zeroed.
    ///
    /// This is used when an actor is restarted by a supervisor.  Any
    /// outstanding pointers into this heap become dangling — the caller
    /// must ensure none exist.
    pub fn reset(&mut self) {
        self.current = self.base;
        self.used_bytes = 0;
        self.live_head = std::ptr::null_mut();
        self.live_tail = std::ptr::null_mut();
        self.live_count = 0;
        self.free_lists = [std::ptr::null_mut(); NUM_SIZE_CLASSES];
        self.total_allocs = 0;
        self.total_frees = 0;
        self.peak_used = 0;
    }

    // ==================================================================
    // Internal helpers
    // ==================================================================

    /// Append `header_ptr` to the tail of the live-object doubly-linked list.
    ///
    /// # Safety
    /// `header_ptr` must point to a valid, writable `OrcaHeader` inside this
    /// heap's backing block.  This function is called only from `alloc`.
    unsafe fn add_to_live_list(&mut self, header_ptr: *mut OrcaHeader) {
        (*header_ptr).live_next = std::ptr::null_mut();
        (*header_ptr).live_prev = self.live_tail;

        if self.live_tail.is_null() {
            // First object.
            self.live_head = header_ptr;
        } else {
            (*self.live_tail).live_next = header_ptr;
        }
        self.live_tail = header_ptr;
    }

    /// Remove `header_ptr` from the live-object doubly-linked list.
    ///
    /// # Safety
    /// `header_ptr` must be a current member of the live list.
    unsafe fn remove_from_live_list(&mut self, header_ptr: *mut OrcaHeader) {
        let prev = (*header_ptr).live_prev;
        let next = (*header_ptr).live_next;

        if prev.is_null() {
            self.live_head = next;
        } else {
            (*prev).live_next = next;
        }

        if next.is_null() {
            self.live_tail = prev;
        } else {
            (*next).live_prev = prev;
        }
    }
}

// ---------------------------------------------------------------------------
// OrcaHeap trait implementation
// ---------------------------------------------------------------------------

use super::gc::OrcaHeap;

impl OrcaHeap for ActorHeap {
    /// Allocate payload bytes with the Raw type tag.
    ///
    /// Delegates to [`ActorHeap::alloc`] using [`TypeTag::Raw`] as the
    /// default type tag.  The ORCA GC (in `gc.rs`) calls this when it needs
    /// to allocate an object whose type will be determined later.
    fn alloc_payload(&mut self, payload_size: usize) -> Option<*mut u8> {
        self.alloc(payload_size, TypeTag::Raw)
    }

    /// Free a payload previously returned by [`alloc_payload`].
    ///
    /// Delegates directly to [`ActorHeap::free`].
    ///
    /// # Safety
    /// `payload_ptr` must be a live pointer returned by `alloc_payload` on
    /// this exact heap.
    unsafe fn free_payload(&mut self, payload_ptr: *mut u8) {
        self.free(payload_ptr);
    }

    /// Recover the [`OrcaHeader`] pointer from a payload pointer.
    ///
    /// # Safety
    /// `payload_ptr` must be a valid payload pointer from this heap.
    unsafe fn header_ptr(&self, payload_ptr: *mut u8) -> *mut OrcaHeader {
        ActorHeap::header_of(payload_ptr)
    }
}

// ---------------------------------------------------------------------------
// Drop
// ---------------------------------------------------------------------------

impl Drop for ActorHeap {
    fn drop(&mut self) {
        let layout =
            std::alloc::Layout::from_size_align(self.total_size, ALIGN).unwrap();
        // SAFETY: `self.base` was allocated with the exact same layout in
        // `ActorHeap::new`.  After dealloc the pointer must not be used,
        // which is fine because the heap is being dropped.
        unsafe {
            std::alloc::dealloc(self.base, layout);
        }
    }
}

// =============================================================================
// Unit Tests
// =============================================================================

#[test]
fn test_alloc_and_write() {
    let mut heap = ActorHeap::new(64 * 1024);
    heap.set_actor_id(42);

    let payload = heap.alloc(16, TypeTag::String).expect("alloc failed");
    assert!(!payload.is_null());

    // SAFETY: payload is a valid, unique allocation on this heap.
    unsafe {
        let data = std::slice::from_raw_parts_mut(payload, 16);
        data.copy_from_slice(b"Hello, Nulang!!!");
        assert_eq!(&data[..6], b"Hello,");
    }
}

#[test]
fn test_alloc_multiple() {
    let mut heap = ActorHeap::new(64 * 1024);
    let mut ptrs = Vec::new();

    for i in 0..10 {
        let p = heap.alloc(32, TypeTag::Tuple).expect("alloc failed");
        ptrs.push(p);
        // Write a marker so we can distinguish objects.
        unsafe { *(p as *mut u64) = i as u64; }
    }

    // Distinct pointers.
    let mut sorted = ptrs.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), ptrs.len());

    // Markers intact.
    for (i, p) in ptrs.iter().enumerate() {
        unsafe {
            assert_eq!(*(*p as *mut u64), i as u64);
        }
    }
}

#[test]
fn test_free_and_reuse() {
    let mut heap = ActorHeap::new(64 * 1024);

    // Allocate and remember the payload pointer.
    let p1 = heap.alloc(64, TypeTag::Record).unwrap();

    // Free it — the block should go to the appropriate free list.
    unsafe { heap.free(p1); }

    // Allocate again with the same size class. We should get the same
    // payload pointer back because the free list is LIFO.
    let p2 = heap.alloc(64, TypeTag::Record).unwrap();
    assert_eq!(p1, p2, "freed block should be reused");
}

#[test]
fn test_size_classes() {
    let mut heap = ActorHeap::new(64 * 1024);

    // Payload sizes that result in different total-size buckets.
    // HEADER_SIZE = 48, ALIGN = 8.
    // total = 48 + align_up(payload)
    let cases: Vec<(usize, SizeClass)> = vec![
        (1,   SizeClass::Small),   // total = 64 → Small (33-64)
        (16,  SizeClass::Medium),  // total = 72 → Medium (65-128)
        (17,  SizeClass::Medium),  // total = 80 → Medium
        (80,  SizeClass::Large),   // total = 136 → Large (129-256)
        (81,  SizeClass::Large),   // total = 144 → Large
        (200, SizeClass::Large),   // total = 256 → Large
        (210, SizeClass::Huge),    // total = 272 → Huge (257+)
    ];

    for (payload_size, expected_class) in cases {
        let p = heap.alloc(payload_size, TypeTag::Raw).unwrap();
        unsafe {
            let header = ActorHeap::header_of(p);
            assert_eq!(
                (*header).size_class, expected_class,
                "payload_size={} got {:?}, expected {:?}",
                payload_size, (*header).size_class, expected_class
            );
        }
    }
}

#[test]
fn test_header_integrity() {
    let mut heap = ActorHeap::new(64 * 1024);
    heap.set_actor_id(99);

    let p = heap.alloc(42, TypeTag::Closure).unwrap();
    unsafe {
        let h = &*ActorHeap::header_of(p);
        assert_eq!(h.ref_count.load(Ordering::Relaxed), 1);
        assert_eq!(h.foreign_count.load(Ordering::Relaxed), 0);
        assert_eq!(h.sticky.load(Ordering::Relaxed), false);
        assert_eq!(h.actor_id, 99);
        assert_eq!(h.type_tag, TypeTag::Closure);
        assert_eq!(h.gc_color, GcColor::White);
        // Total size = HEADER_SIZE + align_up(42) = 48 + 48 = 96
        assert_eq!(h.size, ActorHeap::HEADER_SIZE + 48);
    }
}

#[test]
fn test_header_of_roundtrip() {
    let mut heap = ActorHeap::new(64 * 1024);
    let p = heap.alloc(100, TypeTag::Array).unwrap();

    unsafe {
        let h = ActorHeap::header_of(p);
        // The payload pointer from the header.
        let p2 = h.add(1) as *mut u8;
        assert_eq!(p, p2, "header_of roundtrip failed");
    }
}

#[test]
fn test_free_list_reuse() {
    let mut heap = ActorHeap::new(64 * 1024);

    // Allocate several objects.
    let a = heap.alloc(64, TypeTag::Tuple).unwrap();
    let b = heap.alloc(64, TypeTag::Tuple).unwrap();
    let c = heap.alloc(64, TypeTag::Tuple).unwrap();

    // Free in reverse order.
    unsafe {
        heap.free(c);
        heap.free(b);
        heap.free(a);
    }

    // All three should go back to the free list.
    assert_eq!(heap.free_list_count(), 3);

    // Re-allocate — they should come back in LIFO order.
    // Free order was c,b,a (a was freed last), so a is popped first.
    let r1 = heap.alloc(64, TypeTag::Tuple).unwrap();
    let r2 = heap.alloc(64, TypeTag::Tuple).unwrap();
    let r3 = heap.alloc(64, TypeTag::Tuple).unwrap();

    assert_eq!(r1, a, "last-freed object should be reused first (LIFO)");
    assert_eq!(r2, b);
    assert_eq!(r3, c);

    // Free list should be empty now.
    assert_eq!(heap.free_list_count(), 0);
}

#[test]
fn test_heap_exhaustion() {
    // A tiny heap: can fit at most one 48-byte header + 8-byte payload.
    let mut heap = ActorHeap::new(64);

    let p1 = heap.alloc(8, TypeTag::Raw);
    assert!(p1.is_some());

    // Second allocation should fail — not enough contiguous space.
    let p2 = heap.alloc(8, TypeTag::Raw);
    assert!(p2.is_none(), "heap should be exhausted");
}

#[test]
fn test_reset() {
    let mut heap = ActorHeap::new(64 * 1024);

    heap.alloc(100, TypeTag::Map).unwrap();
    heap.alloc(200, TypeTag::Record).unwrap();
    assert_eq!(heap.live_count(), 2);
    assert!(heap.used() > 0);

    // Free one object so it goes into the free list.
    let p = heap.alloc(64, TypeTag::String).unwrap();
    unsafe { heap.free(p); }
    assert!(heap.free_list_count() > 0);

    // Reset everything.
    heap.reset();

    assert_eq!(heap.live_count(), 0);
    assert_eq!(heap.used(), 0);
    assert_eq!(heap.free_list_count(), 0);
    assert_eq!(heap.free_bytes(), 64 * 1024);

    // Should be able to allocate again from a clean slate.
    let q = heap.alloc(300, TypeTag::Array);
    assert!(q.is_some());
}

#[test]
fn test_live_object_iteration() {
    let mut heap = ActorHeap::new(64 * 1024);

    let p1 = heap.alloc(16, TypeTag::String).unwrap();
    let p2 = heap.alloc(32, TypeTag::Tuple).unwrap();
    let p3 = heap.alloc(48, TypeTag::Map).unwrap();

    // Free the middle object.
    unsafe { heap.free(p2); }
    assert_eq!(heap.live_count(), 2);

    // Collect all live object payload pointers.
    let mut live = Vec::new();
    heap.iter_live_objects(|_header, payload, _size| {
        live.push(payload);
    });

    assert_eq!(live.len(), 2);
    assert!(live.contains(&p1));
    assert!(!live.contains(&p2));
    assert!(live.contains(&p3));
}

#[test]
fn test_large_allocation() {
    let mut heap = ActorHeap::new(64 * 1024);

    // 512 bytes payload → total = 48 + 512 = 560 → Huge size class.
    let p = heap.alloc(512, TypeTag::Array).unwrap();
    unsafe {
        let h = &*ActorHeap::header_of(p);
        assert_eq!(h.size_class, SizeClass::Huge);
        assert_eq!(h.size, ActorHeap::HEADER_SIZE + 512);
        assert_eq!(h.type_tag, TypeTag::Array);
    }

    // Write and verify the full payload.
    unsafe {
        let buf = std::slice::from_raw_parts_mut(p, 512);
        for (i, slot) in buf.iter_mut().enumerate() {
            *slot = (i % 256) as u8;
        }
        for i in 0..512 {
            assert_eq!(buf[i], (i % 256) as u8);
        }
    }
}

#[test]
fn test_alignment() {
    let mut heap = ActorHeap::new(64 * 1024);

    // Allocate many objects with odd payload sizes and verify every
    // returned pointer is 8-byte aligned.
    let sizes = [1, 3, 7, 8, 9, 15, 17, 31, 33, 63, 65, 127, 129, 255];
    let mut ptrs = Vec::new();

    for &sz in &sizes {
        let p = heap.alloc(sz, TypeTag::Raw).unwrap();
        assert_eq!(
            p as usize % ALIGN,
            0,
            "payload pointer {:p} is not {}-byte aligned (size={})",
            p, ALIGN, sz
        );
        ptrs.push(p);
    }

    // Verify headers are also properly aligned.
    for p in &ptrs {
        unsafe {
            let h = ActorHeap::header_of(*p);
            assert_eq!(
                h as usize % ALIGN,
                0,
                "header pointer {:p} is not {}-byte aligned",
                h, ALIGN
            );
        }
    }
}

#[test]
fn test_header_size_is_56() {
    // The whole design assumes HEADER_SIZE == 56. If this ever changes,
    // classify_total_size and the memory layout comments must be updated.
    assert_eq!(ActorHeap::HEADER_SIZE, 56);
}

#[test]
fn test_alloc_zero_payload() {
    let mut heap = ActorHeap::new(64 * 1024);

    // Zero-byte payload should still allocate a header.
    let p = heap.alloc(0, TypeTag::Raw).unwrap();
    unsafe {
        let h = &*ActorHeap::header_of(p);
        assert_eq!(h.size, ActorHeap::HEADER_SIZE);
        assert_eq!(h.ref_count.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn test_statistics() {
    let mut heap = ActorHeap::new(64 * 1024);

    assert_eq!(heap.used(), 0);
    assert_eq!(heap.free_bytes(), 64 * 1024);
    assert_eq!(heap.live_count(), 0);
    assert_eq!(heap.free_list_count(), 0);

    let p1 = heap.alloc(100, TypeTag::Raw).unwrap();
    let _p2 = heap.alloc(200, TypeTag::Raw).unwrap();

    assert_eq!(heap.live_count(), 2);
    assert!(heap.used() >= ActorHeap::HEADER_SIZE * 2 + 100 + 200);

    unsafe { heap.free(p1); }
    assert_eq!(heap.live_count(), 1);
    assert_eq!(heap.free_list_count(), 1);
}
