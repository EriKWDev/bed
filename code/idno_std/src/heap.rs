//! Per-world byte heap for dynamically sized entity fields. Entities stay
//! fixed-size `Copy` records holding offset handles (`HeapString`, `HeapVec`,
//! `HeapMap`); payloads live here and inline on the wire.
//!
//! Thread-safe: allocation is an atomic bump plus per-class lock-free free
//! stacks over a never-moving reservation (the commit lock is taken only
//! when the bump crosses the committed watermark), and reads go straight
//! through the cached base pointer. `&Heap` implements std `Allocator`
//! with the same constructors the arenas have (`heap.vec()`,
//! `heap.vec_from(..)`, `heap.alloc(..)`), so parallel workers may grow
//! world containers concurrently — and `take_vec`/`adopt_vec` convert
//! between a record's POD handle and a live `Vec` over the same block, no
//! copy. Block CONTENTS are exclusive to the handle holder, the contract
//! every allocator has.

use std::alloc::{AllocError, Allocator, Layout};
use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

const MIN_CLASS_SHIFT: u32 = 4;
const MAX_CLASS_SHIFT: u32 = 26;
const CLASS_COUNT: usize = (MAX_CLASS_SHIFT - MIN_CLASS_SHIFT + 1) as usize;
// iOS: see the SLAB_RESERVE_BYTES note in sync::collection — the virtual
// address budget there is a fraction of macOS's (a real iPhone refused
// 2 GiB maps), so heaps reserve 256 MiB there (= 4 max-class blocks).
const HEAP_RESERVE: usize = if cfg!(target_os = "ios") { 256 << 20 } else { 1 << 32 };

use platform::Reservation;

pub struct Heap {
    mem: Mutex<Reservation>,
    /// The reservation base, cached once — it never moves — so reads and
    /// in-block writes go straight to memory, no lock, no atomics.
    base: NonNull<u8>,
    /// Committed watermark cache: allocation takes the commit lock only
    /// when the bump crosses this.
    committed: AtomicUsize,
    bump: AtomicUsize,
    /*
        NOTE: Per-class Treiber stacks. Each head word is
              generation << 32 | block offset, and the generation bumps on
              every successful push AND pop, so a block that is popped,
              reused and freed again can never satisfy a stale CAS (the
              classic ABA tag). The freed block's first 4 bytes hold the
              next offset, accessed as an AtomicU32; a stale popper can
              still read those bytes while the block's new owner writes
              plain data over them — the value is discarded when the
              generation CAS fails. That mixed access is the standard
              intrusive-free-list caveat every such allocator carries
              (crossbeam, mimalloc); it is benign on every target we ship.
    */
    free_heads: [AtomicU64; CLASS_COUNT],
}

/*
    NOTE: Send/Sync are manual only because of the cached NonNull base.
          All allocator state is atomic or behind the commit lock; block
          contents are exclusive to whoever holds the handle (the same
          contract std allocators have — two owners of one block is a bug
          on any allocator).
*/
unsafe impl Send for Heap {}
unsafe impl Sync for Heap {}

const FREE_GENERATION_SHIFT: u32 = 32;

impl Default for Heap {
    fn default() -> Self {
        let reservation = Reservation::new(HEAP_RESERVE);
        Self {
            base: reservation.base(),
            committed: AtomicUsize::new(reservation.committed()),
            mem: Mutex::new(reservation),
            // Offset 0 is the reserved empty handle.
            bump: AtomicUsize::new(16),
            free_heads: [const { AtomicU64::new(0) }; CLASS_COUNT],
        }
    }
}

fn class_of(len: u32) -> u32 {
    let bits = len.max(1 << MIN_CLASS_SHIFT).next_power_of_two();
    bits.trailing_zeros() - MIN_CLASS_SHIFT
}

fn class_size(class: u32) -> usize {
    1 << (class + MIN_CLASS_SHIFT)
}

#[inline]
fn assert_heap_element<T>() {
    assert!(size_of::<T>() > 0, "heap containers do not support zero-sized elements");
    assert!(align_of::<T>() <= 16, "heap blocks are 16-aligned");
}

impl Heap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self.bump.get_mut() = 16;
        for head in &mut self.free_heads {
            *head.get_mut() = 0;
        }
    }

    /// Highest byte reached by this heap. Freed blocks remain below this
    /// watermark and may be reused, so this is committed address-space demand,
    /// not the sum of currently live payload lengths.
    pub fn allocated_high_watermark_bytes(&self) -> usize {
        self.bump.load(Ordering::Acquire)
    }

    #[inline]
    fn base(&self) -> NonNull<u8> {
        self.base
    }

    /// The freed-block intrusive next pointer (first 4 bytes; blocks are
    /// 16-aligned and at least 16 bytes).
    #[inline]
    fn free_next_of(&self, offset: u32) -> &AtomicU32 {
        unsafe { &*(self.base.add(offset as usize).as_ptr() as *const AtomicU32) }
    }

    fn ensure_committed(&self, end: usize) {
        if end <= self.committed.load(Ordering::Acquire) {
            return;
        }
        let mut reservation = self.mem.lock().unwrap();
        if end > reservation.committed() {
            reservation.commit_to(end);
        }
        self.committed.store(reservation.committed(), Ordering::Release);
    }

    fn alloc_block(&self, len: u32) -> u32 {
        if len == 0 {
            return 0;
        }
        let class = class_of(len);
        assert!(class < CLASS_COUNT as u32, "heap block too large");

        let head = &self.free_heads[class as usize];
        let mut head_word = head.load(Ordering::Acquire);
        loop {
            let offset = head_word as u32;
            if offset == 0 {
                break;
            }
            let next = self.free_next_of(offset).load(Ordering::Relaxed);
            let bumped_generation = (head_word >> FREE_GENERATION_SHIFT).wrapping_add(1);
            let replacement = bumped_generation << FREE_GENERATION_SHIFT | next as u64;
            match head.compare_exchange_weak(head_word, replacement, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return offset,
                Err(current) => head_word = current,
            }
        }

        let size = class_size(class);
        let offset = self.bump.fetch_add(size, Ordering::Relaxed);
        self.ensure_committed(offset + size);
        offset as u32
    }

    fn free_block(&self, offset: u32, len: u32) {
        if offset == 0 || len == 0 {
            return;
        }
        let class = class_of(len);
        let head = &self.free_heads[class as usize];
        let mut head_word = head.load(Ordering::Acquire);
        loop {
            self.free_next_of(offset).store(head_word as u32, Ordering::Relaxed);
            let bumped_generation = (head_word >> FREE_GENERATION_SHIFT).wrapping_add(1);
            let replacement = bumped_generation << FREE_GENERATION_SHIFT | offset as u64;
            match head.compare_exchange_weak(head_word, replacement, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return,
                Err(current) => head_word = current,
            }
        }
    }

    #[inline]
    fn bytes_at(&self, offset: u32, len: usize) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.base().add(offset as usize).as_ptr(), len) }
    }

    /// Callers must not hold two overlapping mutable views; block ownership
    /// follows the single record holding the handle.
    #[allow(clippy::mut_from_ref)]
    #[inline]
    fn bytes_at_mut(&self, offset: u32, len: usize) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.base().add(offset as usize).as_ptr(), len) }
    }

    pub fn alloc_bytes(&self, data: &[u8]) -> HeapString {
        let len = u32::try_from(data.len()).expect("heap block too large");
        let offset = self.alloc_block(len);
        self.bytes_at_mut(offset, data.len()).copy_from_slice(data);
        HeapString { offset, len }
    }

    pub fn alloc_str(&self, s: &str) -> HeapString {
        self.alloc_bytes(s.as_bytes())
    }

    pub fn free(&self, handle: HeapString) {
        self.free_block(handle.offset, handle.len);
    }

    pub fn bytes(&self, handle: HeapString) -> &[u8] {
        self.bytes_at(handle.offset, handle.len as usize)
    }

    #[allow(clippy::mut_from_ref)]
    pub fn bytes_mut(&self, handle: HeapString) -> &mut [u8] {
        self.bytes_at_mut(handle.offset, handle.len as usize)
    }

    pub fn str(&self, handle: HeapString) -> &str {
        std::str::from_utf8(self.bytes(handle)).unwrap_or("")
    }

    pub fn clone_block(&self, handle: HeapString) -> HeapString {
        if handle.len == 0 {
            return HeapString::EMPTY;
        }
        let offset = self.alloc_block(handle.len);
        unsafe {
            std::ptr::copy_nonoverlapping(self.base().add(handle.offset as usize).as_ptr(), self.base().add(offset as usize).as_ptr(), handle.len as usize);
        }
        HeapString { offset, len: handle.len }
    }

    fn resize_block(&self, handle: HeapString, new_len: u32) -> HeapString {
        if new_len == 0 {
            self.free(handle);
            return HeapString::EMPTY;
        }
        if handle.offset != 0 && class_of(new_len) == class_of(handle.len.max(1)) {
            return HeapString { offset: handle.offset, len: new_len };
        }
        let offset = self.alloc_block(new_len);
        let keep = handle.len.min(new_len) as usize;
        if keep > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(self.base().add(handle.offset as usize).as_ptr(), self.base().add(offset as usize).as_ptr(), keep);
            }
        }
        self.free(handle);
        HeapString { offset, len: new_len }
    }

    fn truncate_block(&self, handle: HeapString, new_len: u32) -> HeapString {
        self.resize_block(handle, new_len.min(handle.len))
    }

    pub fn slice<T: Copy>(&self, handle: HeapVec<T>) -> &[T] {
        assert_heap_element::<T>();
        unsafe { std::slice::from_raw_parts(self.base().add(handle.raw.offset as usize).as_ptr() as *const T, handle.raw.len as usize / size_of::<T>()) }
    }

    #[allow(clippy::mut_from_ref)]
    pub fn slice_mut<T: Copy>(&self, handle: HeapVec<T>) -> &mut [T] {
        assert_heap_element::<T>();
        unsafe { std::slice::from_raw_parts_mut(self.base().add(handle.raw.offset as usize).as_ptr() as *mut T, handle.raw.len as usize / size_of::<T>()) }
    }

    pub fn alloc_slice<T: Copy>(&self, values: &[T]) -> HeapVec<T> {
        assert_heap_element::<T>();
        HeapVec { raw: self.alloc_bytes(crate::utils::bytes_of_slice(values)), _marker: std::marker::PhantomData }
    }

    /// Allocate one optional structured value. The returned POD handle can
    /// live directly in a world record; an empty handle represents `None`.
    pub fn alloc_box<T: Copy>(&self, value: T) -> HeapBox<T> {
        assert_heap_element::<T>();
        HeapBox { raw: self.alloc_bytes(crate::utils::bytes_of(&value)), _marker: std::marker::PhantomData }
    }

    pub fn boxed<T: Copy>(&self, handle: HeapBox<T>) -> Option<&T> {
        assert_heap_element::<T>();
        if handle.is_empty() {
            return None;
        }
        assert_eq!(handle.raw.len as usize, size_of::<T>(), "invalid HeapBox payload size");
        Some(unsafe { &*(self.base().add(handle.raw.offset as usize).as_ptr() as *const T) })
    }

    #[allow(clippy::mut_from_ref)]
    pub fn boxed_mut<T: Copy>(&self, handle: HeapBox<T>) -> Option<&mut T> {
        assert_heap_element::<T>();
        if handle.is_empty() {
            return None;
        }
        assert_eq!(handle.raw.len as usize, size_of::<T>(), "invalid HeapBox payload size");
        Some(unsafe { &mut *(self.base().add(handle.raw.offset as usize).as_ptr() as *mut T) })
    }

    /*
        NOTE: Handles and heap Vecs are the SAME blocks: `&Heap` implements
              `Allocator` over the size classes the handles use, so a handle
              opens as a real `Vec` and a heap `Vec` freezes into a handle
              without copying (adopt's shrink to the len's class is the one
              possible copy). Records still STORE handles — a Thing must stay
              Copy/POD for memcpy rows, undo snapshots and the wire, and a
              Vec's pointer/Drop can be neither — the Vec form is the editing
              view, not the resting state.
    */

    /// Opens a handle as a real heap-allocated `Vec`: the full std API, no
    /// copy. The handle is dead afterwards — `adopt_vec` the Vec back into
    /// the record, or let it drop and free the block.
    pub fn take_vec<T: Copy>(&self, handle: HeapVec<T>) -> WVec<'_, T> {
        assert_heap_element::<T>();
        if handle.raw.offset == 0 {
            return Vec::new_in(self);
        }
        let capacity = class_size(class_of(handle.raw.len)) / size_of::<T>();
        unsafe { Vec::from_raw_parts_in(self.base().add(handle.raw.offset as usize).as_ptr() as *mut T, handle.len(), capacity, self) }
    }

    /// Freezes a heap `Vec` into a POD handle, in place; the block shrinks
    /// to the len's size class, so an overgrown build costs at most one
    /// copy. A `Vec` from any other allocator is a bug, not a copy.
    pub fn adopt_vec<T: Copy>(&self, vec: WVec<'_, T>) -> HeapVec<T> {
        assert!(std::ptr::eq(*vec.allocator(), self), "vec was not allocated in this heap");
        if vec.capacity() == 0 {
            return HeapVec::EMPTY;
        }
        let len_bytes = (vec.len() * size_of::<T>()) as u32;
        let capacity_bytes = (vec.capacity() * size_of::<T>()) as u32;
        let offset = unsafe { vec.as_ptr().cast::<u8>().offset_from(self.base().as_ptr()) } as u32;
        std::mem::forget(vec);
        HeapVec::from_raw(self.resize_block(HeapString { offset, len: capacity_bytes }, len_bytes))
    }
}

crate::arena::impl_allocator_collections!(Heap);

/// Handle to a world-heap byte block (also the backing of `HeapVec` and
/// `HeapMap`). Copying the handle does not copy the payload; ownership
/// follows the record it lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct HeapString {
    offset: u32,
    len: u32,
}

impl HeapString {
    pub const EMPTY: Self = Self { offset: 0, len: 0 };

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn append_bytes(&mut self, heap: &Heap, data: &[u8]) {
        let old_len = self.len as usize;
        let new_len = old_len.checked_add(data.len()).and_then(|len| u32::try_from(len).ok()).expect("heap block too large");
        *self = heap.resize_block(*self, new_len);
        heap.bytes_at_mut(self.offset, self.len as usize)[old_len..].copy_from_slice(data);
    }
}

#[repr(C)]
pub struct HeapVec<T: Copy> {
    raw: HeapString,
    _marker: std::marker::PhantomData<T>,
}

/// Optional, heap-owned `T` represented by the same eight-byte offset handle
/// as the other world containers. Copying this value copies ownership of the
/// handle, not its payload; ownership follows the record containing it.
#[repr(C)]
pub struct HeapBox<T: Copy> {
    raw: HeapString,
    _marker: std::marker::PhantomData<T>,
}

impl<T: Copy> Clone for HeapBox<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: Copy> Copy for HeapBox<T> {}

impl<T: Copy> std::fmt::Debug for HeapBox<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_empty() { f.write_str("HeapBox[None]") } else { f.write_str("HeapBox[Some]") }
    }
}

impl<T: Copy> HeapBox<T> {
    pub const EMPTY: Self = Self { raw: HeapString::EMPTY, _marker: std::marker::PhantomData };
    pub fn from_raw(raw: HeapString) -> Self {
        Self { raw, _marker: std::marker::PhantomData }
    }
    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }
    pub fn is_some(&self) -> bool {
        !self.is_empty()
    }
    pub fn raw(&self) -> HeapString {
        self.raw
    }
}

impl<T: Copy> Clone for HeapVec<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: Copy> Copy for HeapVec<T> {}

impl<T: Copy> std::fmt::Debug for HeapVec<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HeapVec[{}]", self.len())
    }
}

impl<T: Copy> HeapVec<T> {
    pub const EMPTY: Self = Self { raw: HeapString::EMPTY, _marker: std::marker::PhantomData };
    pub fn from_raw(raw: HeapString) -> Self {
        Self { raw, _marker: std::marker::PhantomData }
    }
    pub fn len(&self) -> usize {
        assert_heap_element::<T>();
        self.raw.len as usize / size_of::<T>()
    }
    pub fn is_empty(&self) -> bool {
        self.raw.len == 0
    }
    pub fn raw(&self) -> HeapString {
        self.raw
    }

    pub fn push(&mut self, heap: &Heap, value: T) {
        assert_heap_element::<T>();
        let old_len = self.raw.len as usize;
        let new_len = old_len.checked_add(size_of::<T>()).and_then(|len| u32::try_from(len).ok()).expect("heap block too large");
        self.raw = heap.resize_block(self.raw, new_len);
        unsafe {
            (heap.base().add(self.raw.offset as usize + old_len).as_ptr() as *mut T).write_unaligned(value);
        }
    }

    pub fn pop(&mut self, heap: &Heap) -> Option<T> {
        let value = heap.slice(*self).last().copied()?;
        self.raw = heap.truncate_block(self.raw, self.raw.len - size_of::<T>() as u32);
        Some(value)
    }

    pub fn remove(&mut self, heap: &Heap, index: usize) -> Option<T> {
        let value = heap.slice(*self).get(index).copied()?;
        heap.slice_mut(*self).copy_within(index + 1.., index);
        self.raw = heap.truncate_block(self.raw, self.raw.len - size_of::<T>() as u32);
        Some(value)
    }

    pub fn insert(&mut self, heap: &Heap, index: usize, value: T) {
        let count = self.len();
        let index = index.min(count);
        self.push(heap, value);
        let slice = heap.slice_mut(*self);
        slice.copy_within(index..count, index + 1);
        slice[index] = value;
    }
}

#[repr(C)]
pub struct HeapMap<K: Copy + PartialEq, V: Copy> {
    pairs: HeapVec<(K, V)>,
}

impl<K: Copy + PartialEq, V: Copy> Clone for HeapMap<K, V> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K: Copy + PartialEq, V: Copy> Copy for HeapMap<K, V> {}

impl<K: Copy + PartialEq, V: Copy> std::fmt::Debug for HeapMap<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HeapMap[{}]", self.pairs.len())
    }
}

impl<K: Copy + PartialEq, V: Copy> HeapMap<K, V> {
    pub const EMPTY: Self = Self { pairs: HeapVec::EMPTY };

    pub fn from_raw(raw: HeapString) -> Self {
        Self { pairs: HeapVec::from_raw(raw) }
    }

    pub fn len(&self) -> usize {
        self.pairs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    pub fn raw(&self) -> HeapString {
        self.pairs.raw()
    }

    pub fn get(&self, heap: &Heap, key: K) -> Option<V> {
        heap.slice(self.pairs).iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }

    pub fn iter<'h>(&self, heap: &'h Heap) -> std::slice::Iter<'h, (K, V)> {
        heap.slice(self.pairs).iter()
    }

    pub fn insert(&mut self, heap: &Heap, key: K, value: V) -> Option<V> {
        if let Some(pair) = heap.slice_mut(self.pairs).iter_mut().find(|(k, _)| *k == key) {
            let previous = pair.1;
            pair.1 = value;
            return Some(previous);
        }
        self.pairs.push(heap, (key, value));
        None
    }

    pub fn remove(&mut self, heap: &Heap, key: K) -> bool {
        let Some(index) = heap.slice(self.pairs).iter().position(|(k, _)| *k == key) else {
            return false;
        };
        self.pairs.remove(heap, index);
        true
    }
}

/// Any std container can live in world memory: `Vec::new_in(&world.heap)`.
/// Sound because the reservation address never moves and blocks recycle
/// exactly through the same size classes.
unsafe impl Allocator for &Heap {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        if layout.align() > 16 {
            return Err(AllocError);
        }
        let handle_len = layout.size().max(1) as u32;
        let offset = self.alloc_block(handle_len);
        Ok(NonNull::slice_from_raw_parts(unsafe { self.base().add(offset as usize) }, class_size(class_of(handle_len))))
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        let offset = unsafe { ptr.as_ptr().offset_from(self.base().as_ptr()) } as u32;
        self.free_block(offset, layout.size().max(1) as u32);
    }
}

/*
    NOTE: HeapReader is the read-only face handed to render visits through
          WorldView. The heap itself is thread-safe — parallel gamelogic
          may allocate and grow containers through `&Heap` — but a VISIT
          must not mutate the world at all: every world mutation is a
          replicated edit, or clients diverge. The reader states that
          boundary in the type; visits that produce data use the
          sanctioned side channels (MeshUpdateQueue, the recorder batch).
*/
#[derive(Clone, Copy)]
pub struct HeapReader<'h> {
    heap: &'h Heap,
}

impl<'h> HeapReader<'h> {
    pub fn bytes(&self, handle: HeapString) -> &'h [u8] {
        self.heap.bytes(handle)
    }

    pub fn str(&self, handle: HeapString) -> &'h str {
        self.heap.str(handle)
    }

    pub fn slice<T: Copy>(&self, handle: HeapVec<T>) -> &'h [T] {
        self.heap.slice(handle)
    }

    pub fn boxed<T: Copy>(&self, handle: HeapBox<T>) -> Option<&'h T> {
        self.heap.boxed(handle)
    }
}

impl Heap {
    pub fn reader(&self) -> HeapReader<'_> {
        HeapReader { heap: self }
    }
}

pub type WVec<'h, T> = Vec<T, &'h Heap>;

#[cfg(test)]
mod heap_collection_tests {
    use super::*;

    #[test]
    fn heap_speaks_the_arena_collection_api() {
        let heap = Heap::new();
        let mut numbers = heap.vec::<u64>(4);
        numbers.extend([1, 2, 3]);
        assert_eq!(numbers.as_slice(), &[1, 2, 3]);
        assert_eq!(*heap.alloc(7u32), 7);
        assert_eq!(heap.vec_from(0..5u32).as_slice(), &[0, 1, 2, 3, 4]);

        let mut queue = heap.deque_from(0..3u32);
        queue.push_front(9);
        assert_eq!(queue.iter().copied().collect::<Vec<_>>(), vec![9, 0, 1, 2]);

        let counts = heap.int_map_from([(1u64, "a"), (2, "b")]);
        assert_eq!(counts.get(&2), Some(&"b"));

        let names = heap.fast_map_from([("mesh", 3u32)]);
        assert_eq!(names.get("mesh"), Some(&3));

        let seen = heap.int_set_from([10u64, 20, 20, 30]);
        assert_eq!(seen.len(), 3);
        assert!(seen.contains(&20));

        let tags = heap.fast_set_from(["a", "b", "a"]);
        assert_eq!(tags.len(), 2);
    }

    #[test]
    fn handles_and_vecs_are_the_same_blocks() {
        let heap = Heap::new();
        let handle = heap.alloc_slice(&[1u32, 2, 3]);

        let mut open = heap.take_vec(handle);
        open.push(4);
        open.retain(|&value| value != 2);
        let handle = heap.adopt_vec(open);

        assert_eq!(heap.slice(handle), &[1, 3, 4]);
    }

    #[test]
    fn heap_vec_mutation_updates_its_handle_in_place() {
        let heap = Heap::new();
        let mut values = HeapVec::EMPTY;

        for value in 0..40u32 {
            values.push(&heap, value);
        }
        values.insert(&heap, 4, 99);
        assert_eq!(values.remove(&heap, 4), Some(99));
        assert_eq!(values.pop(&heap), Some(39));
        assert_eq!(heap.slice(values), &(0..39u32).collect::<Vec<_>>());
    }

    #[test]
    fn adopt_shrinks_an_overgrown_build_to_its_len_class() {
        let heap = Heap::new();
        let mut build = heap.vec::<u8>(1024);
        build.extend_from_slice(&[9; 20]);
        let handle = heap.adopt_vec(build);
        assert_eq!(heap.slice(handle), &[9; 20]);

        let reopened = heap.take_vec(handle);
        assert!(reopened.capacity() >= 20 && reopened.capacity() < 1024, "block came back in the len's class, not the build's");
    }

    #[test]
    fn parallel_workers_allocate_grow_and_free_concurrently() {
        let heap = Heap::new();
        std::thread::scope(|scope| {
            for thread_index in 0..8u32 {
                let heap = &heap;
                scope.spawn(move || {
                    let mut held: Vec<(HeapVec<u32>, u32, u32)> = Vec::new();
                    for round in 0..500u32 {
                        let stamp = thread_index << 16 | round;
                        let element_count = round % 23;
                        let mut grown = heap.vec::<u32>(1);
                        grown.extend((0..element_count).map(|i| stamp + i));
                        held.push((heap.adopt_vec(grown), stamp, element_count));

                        if round % 3 == 0 && held.len() > 4 {
                            let (handle, stamp, element_count) = held.swap_remove((round as usize * 7) % held.len());
                            let slice = heap.slice(handle);
                            assert_eq!(slice.len(), element_count as usize);
                            assert!(slice.iter().enumerate().all(|(i, &value)| value == stamp + i as u32), "another worker scribbled an owned block");
                            heap.free(handle.raw());
                        }
                    }
                    for (handle, stamp, element_count) in held {
                        let slice = heap.slice(handle);
                        assert_eq!(slice.len(), element_count as usize);
                        assert!(slice.iter().enumerate().all(|(i, &value)| value == stamp + i as u32));
                    }
                });
            }
        });
    }

    #[test]
    fn readers_share_read_access_across_threads() {
        let heap = Heap::new();
        let names: Vec<(HeapString, String)> = (0..64)
            .map(|i| {
                let name = format!("name_{i}");
                (heap.alloc_str(&name), name)
            })
            .collect();
        let reader = heap.reader();
        std::thread::scope(|scope| {
            for chunk in names.chunks(16) {
                scope.spawn(move || {
                    for (handle, expected) in chunk {
                        assert_eq!(reader.str(*handle), expected);
                    }
                });
            }
        });
    }

    #[test]
    fn empty_vecs_and_handles_bridge_both_ways() {
        let heap = Heap::new();
        let opened_empty = heap.take_vec(HeapVec::<u32>::EMPTY);
        assert!(opened_empty.is_empty());
        assert!(heap.adopt_vec(opened_empty).is_empty());

        let mut cleared = heap.take_vec(heap.alloc_slice(&[1u32]));
        cleared.clear();
        assert!(heap.adopt_vec(cleared).is_empty(), "adopting an emptied vec frees its block");
    }

    #[test]
    fn heap_box_is_optional_and_stores_one_value() {
        let heap = Heap::new();
        let empty = HeapBox::<[u32; 2]>::EMPTY;
        assert!(heap.boxed(empty).is_none());

        let handle = heap.alloc_box([3, 7]);
        assert_eq!(heap.boxed(handle), Some(&[3, 7]));
        heap.boxed_mut(handle).unwrap()[1] = 9;
        assert_eq!(heap.boxed(handle), Some(&[3, 9]));
        heap.free(handle.raw());
    }
}
