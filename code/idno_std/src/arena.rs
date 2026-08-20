//! Bump arenas: thread-local scratch [`Arena`] scoped by [`Arena::temp`],
//! and the shared [`FrameArena`] rewound only at the frame boundary. Both
//! implement std [`Allocator`].

use std::alloc::{AllocError, Allocator, Global, Layout};
use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

const ARENA_PAGE_SIZE: usize = 4096;

struct Chunk {
    ptr: NonNull<[u8]>,
    size: usize,
    layout: Layout,
}

impl Chunk {
    fn new(size: usize) -> Self {
        let size = size.next_multiple_of(ARENA_PAGE_SIZE);
        let layout = Layout::from_size_align(size, ARENA_PAGE_SIZE).unwrap();
        Self {
            ptr: Global.allocate(layout).unwrap(),
            size,
            layout,
        }
    }
}

impl Drop for Chunk {
    fn drop(&mut self) {
        unsafe { Global.deallocate(self.ptr.cast(), self.layout) }
    }
}

// Scratch arena

pub struct Arena {
    chunk_size: usize,
    chunks: RefCell<Vec<Chunk>>,
    current_chunk: Cell<usize>,
    active_ptr: Cell<NonNull<u8>>,
    remaining: Cell<usize>,
    temp_depth: Cell<usize>,
}

impl Arena {
    pub fn new(chunk_size_bytes: usize) -> Self {
        let chunk_size = chunk_size_bytes.next_multiple_of(ARENA_PAGE_SIZE);
        let chunk = Chunk::new(chunk_size);
        let ptr = chunk.ptr;
        Self {
            chunk_size,
            chunks: RefCell::new(vec![chunk]),
            current_chunk: Cell::new(0),
            active_ptr: Cell::new(ptr.cast()),
            remaining: Cell::new(chunk_size),
            temp_depth: Cell::new(0),
        }
    }

    pub fn temp(&self) -> TempArena<'_> {
        let mark = self.mark();
        self.temp_depth.set(mark.temp_depth + 1);
        TempArena { arena: self, mark }
    }

    /*
        NOTE: Reset invalidates every outstanding allocation. Callers must
              establish the boundary; temp scopes additionally make an
              accidental reset fail immediately.
    */
    pub unsafe fn reset(&self) {
        assert_eq!(
            self.temp_depth.get(),
            0,
            "cannot reset a scratch arena while a temp scope is live"
        );
        let mut chunks = self.chunks.borrow_mut();
        chunks.truncate(1);
        let first = &chunks[0];
        self.current_chunk.set(0);
        self.active_ptr.set(first.ptr.cast());
        self.remaining.set(first.size);
    }

    #[inline]
    pub fn assert_no_temp_scope(&self) {
        assert_eq!(
            self.temp_depth.get(),
            0,
            "scratch temp scope crossed its boundary"
        );
    }

    fn mark(&self) -> ArenaMarker {
        ArenaMarker {
            chunk_index: self.current_chunk.get(),
            ptr: self.active_ptr.get(),
            remaining: self.remaining.get(),
            temp_depth: self.temp_depth.get(),
        }
    }

    fn reset_to(&self, mark: &ArenaMarker) {
        self.current_chunk.set(mark.chunk_index);
        self.active_ptr.set(mark.ptr);
        self.remaining.set(mark.remaining);
    }

    fn alloc_impl(&self, layout: Layout, zeroed: bool) -> Result<NonNull<[u8]>, AllocError> {
        let size = layout.size();
        let align = layout.align();

        let current = self.active_ptr.get();
        let offset = current.align_offset(align);
        if offset != usize::MAX && offset + size <= self.remaining.get() {
            return Ok(self.commit(current, offset, size, zeroed));
        }

        let mut chunks = self.chunks.borrow_mut();
        let mut next = self.current_chunk.get() + 1;
        while next < chunks.len() {
            let chunk = &chunks[next];
            let ptr: NonNull<u8> = chunk.ptr.cast();
            let offset = ptr.align_offset(align);
            if offset != usize::MAX && offset + size <= chunk.size {
                self.current_chunk.set(next);
                self.active_ptr.set(ptr);
                self.remaining.set(chunk.size);
                return Ok(self.commit(ptr, offset, size, zeroed));
            }
            next += 1;
        }

        let new_size = (size + align - 1).max(self.chunk_size);
        let chunk = Chunk::new(new_size);
        let ptr: NonNull<u8> = chunk.ptr.cast();
        chunks.push(chunk);
        self.current_chunk.set(chunks.len() - 1);
        self.active_ptr.set(ptr);
        self.remaining.set(new_size);
        let offset = ptr.align_offset(align);
        Ok(self.commit(ptr, offset, size, zeroed))
    }

    #[inline]
    fn commit(&self, base: NonNull<u8>, offset: usize, size: usize, zeroed: bool) -> NonNull<[u8]> {
        let aligned = unsafe { base.add(offset) };
        self.active_ptr.set(unsafe { aligned.add(size) });
        self.remaining.set(self.remaining.get() - offset - size);
        if zeroed {
            unsafe { aligned.write_bytes(0, size) };
        }
        NonNull::slice_from_raw_parts(aligned, size)
    }

    fn grow_impl(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
        zeroed: bool,
    ) -> Result<NonNull<[u8]>, AllocError> {
        let old_size = old_layout.size();
        let new_size = new_layout.size();

        if unsafe { ptr.add(old_size) } == self.active_ptr.get()
            && ptr.align_offset(new_layout.align()) == 0
        {
            let additional = new_size - old_size;
            if additional <= self.remaining.get() {
                self.active_ptr
                    .set(unsafe { self.active_ptr.get().add(additional) });
                self.remaining.set(self.remaining.get() - additional);
                if zeroed {
                    unsafe { ptr.add(old_size).write_bytes(0, additional) };
                }
                return Ok(NonNull::slice_from_raw_parts(ptr, new_size));
            }
        }

        let new_alloc = self.alloc_impl(new_layout, zeroed)?;
        unsafe {
            new_alloc
                .cast::<u8>()
                .copy_from_nonoverlapping(ptr, old_size);
        }
        Ok(new_alloc)
    }

    fn rewind_tail(&self, ptr: NonNull<u8>, size: usize) {
        self.active_ptr.set(ptr);
        self.remaining.set(self.remaining.get() + size);
    }
}

unsafe impl Allocator for Arena {
    #[inline(always)]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.alloc_impl(layout, false)
    }
    #[inline(always)]
    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.alloc_impl(layout, true)
    }
    #[inline(always)]
    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        self.grow_impl(ptr, old_layout, new_layout, false)
    }
    #[inline(always)]
    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        self.grow_impl(ptr, old_layout, new_layout, true)
    }
    #[inline(always)]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        if unsafe { ptr.add(layout.size()) } == self.active_ptr.get() {
            self.rewind_tail(ptr, layout.size());
        }
    }
    #[inline(always)]
    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        if unsafe { ptr.add(old_layout.size()) } == self.active_ptr.get() {
            let reclaimed = old_layout.size() - new_layout.size();
            self.active_ptr
                .set(unsafe { self.active_ptr.get().sub(reclaimed) });
            self.remaining.set(self.remaining.get() + reclaimed);
        }
        Ok(NonNull::slice_from_raw_parts(ptr, new_layout.size()))
    }
}

struct ArenaMarker {
    chunk_index: usize,
    ptr: NonNull<u8>,
    remaining: usize,
    temp_depth: usize,
}

pub struct TempArena<'a> {
    arena: &'a Arena,
    mark: ArenaMarker,
}

impl Drop for TempArena<'_> {
    fn drop(&mut self) {
        assert_eq!(
            self.arena.temp_depth.get(),
            self.mark.temp_depth + 1,
            "scratch temp scopes must drop in reverse creation order"
        );
        self.arena.reset_to(&self.mark);
        self.arena.temp_depth.set(self.mark.temp_depth);
    }
}

unsafe impl Allocator for TempArena<'_> {
    #[inline(always)]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.arena.alloc_impl(layout, false)
    }
    #[inline(always)]
    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.arena.alloc_impl(layout, true)
    }
    #[inline(always)]
    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        self.arena.grow_impl(ptr, old_layout, new_layout, false)
    }
    #[inline(always)]
    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        self.arena.grow_impl(ptr, old_layout, new_layout, true)
    }
    #[inline(always)]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        unsafe { self.arena.deallocate(ptr, layout) }
    }
    #[inline(always)]
    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        unsafe { self.arena.shrink(ptr, old_layout, new_layout) }
    }
}

// Frame arena

struct FrameChunk {
    ptr: NonNull<u8>,
    size: usize,
    offset: AtomicUsize,
    layout: Layout,
}

impl FrameChunk {
    fn new(size: usize) -> Box<Self> {
        let size = size.next_multiple_of(ARENA_PAGE_SIZE);
        let layout = Layout::from_size_align(size, ARENA_PAGE_SIZE).unwrap();
        Box::new(Self {
            ptr: Global.allocate(layout).unwrap().cast(),
            size,
            offset: AtomicUsize::new(0),
            layout,
        })
    }

    #[inline]
    fn try_alloc(&self, layout: Layout) -> Option<NonNull<[u8]>> {
        let mut current = self.offset.load(Ordering::Relaxed);
        loop {
            let base = self.ptr.addr().get() + current;
            let aligned = base.next_multiple_of(layout.align());
            let end = aligned - self.ptr.addr().get() + layout.size();
            if end > self.size {
                return None;
            }
            match self.offset.compare_exchange_weak(
                current,
                end,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    let ptr = unsafe { self.ptr.add(aligned - self.ptr.addr().get()) };
                    return Some(NonNull::slice_from_raw_parts(ptr, layout.size()));
                }
                Err(actual) => current = actual,
            }
        }
    }
}

impl Drop for FrameChunk {
    fn drop(&mut self) {
        unsafe { Global.deallocate(self.ptr, self.layout) }
    }
}

pub struct FrameArena {
    chunk_size: usize,
    chunks: Mutex<Vec<Box<FrameChunk>>>,
    current: std::sync::atomic::AtomicPtr<FrameChunk>,
}

unsafe impl Send for FrameArena {}
unsafe impl Sync for FrameArena {}

impl FrameArena {
    pub fn new(chunk_size_bytes: usize) -> Self {
        let chunk = FrameChunk::new(chunk_size_bytes);
        let current =
            std::sync::atomic::AtomicPtr::new(&*chunk as *const FrameChunk as *mut FrameChunk);
        Self {
            chunk_size: chunk.size,
            chunks: Mutex::new(vec![chunk]),
            current,
        }
    }

    /// # Safety
    /// No frame allocation may remain live and no thread may be allocating.
    pub unsafe fn reset(&self) {
        let mut chunks = self.chunks.lock().unwrap();
        chunks.truncate(1);
        chunks[0].offset.store(0, Ordering::Relaxed);
        self.current.store(
            &*chunks[0] as *const FrameChunk as *mut FrameChunk,
            Ordering::Release,
        );
    }

    fn alloc_slow(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let mut chunks = self.chunks.lock().unwrap();
        let current = unsafe { &*self.current.load(Ordering::Acquire) };
        if let Some(allocation) = current.try_alloc(layout) {
            return Ok(allocation);
        }
        let chunk = FrameChunk::new((layout.size() + layout.align()).max(self.chunk_size));
        let allocation = chunk.try_alloc(layout).ok_or(AllocError)?;
        self.current.store(
            &*chunk as *const FrameChunk as *mut FrameChunk,
            Ordering::Release,
        );
        chunks.push(chunk);
        Ok(allocation)
    }
}

unsafe impl Allocator for FrameArena {
    #[inline]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let current = unsafe { &*self.current.load(Ordering::Acquire) };
        match current.try_alloc(layout) {
            Some(allocation) => Ok(allocation),
            None => self.alloc_slow(layout),
        }
    }

    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        let new_alloc = self.allocate(new_layout)?;
        unsafe {
            new_alloc
                .cast::<u8>()
                .copy_from_nonoverlapping(ptr, old_layout.size())
        };
        Ok(new_alloc)
    }

    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        let new_alloc = self.allocate(new_layout)?;
        unsafe {
            let destination = new_alloc.cast::<u8>();
            destination.copy_from_nonoverlapping(ptr, old_layout.size());
            destination
                .add(old_layout.size())
                .write_bytes(0, new_layout.size() - old_layout.size());
        }
        Ok(new_alloc)
    }

    unsafe fn deallocate(&self, _ptr: NonNull<u8>, _layout: Layout) {}

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        _old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        Ok(NonNull::slice_from_raw_parts(ptr, new_layout.size()))
    }
}

pub type FrameVec<T> = Vec<T, &'static FrameArena>;

// Collection constructors, shared with every house allocator whose `&Self`
// implements `Allocator` — the arenas here and the world `Heap`.

#[macro_export]
macro_rules! impl_allocator_collections {
    ($($allocator:ty),+ $(,)?) => {$(
        impl $allocator {
            pub fn alloc<T>(&self, value: T) -> Box<T, &Self> {
                Box::new_in(value, self)
            }

            pub fn vec<T>(&self, capacity: usize) -> Vec<T, &Self> {
                Vec::with_capacity_in(capacity, self)
            }

            pub fn vec_from<T>(&self, items: impl IntoIterator<Item = T>) -> Vec<T, &Self> {
                let items = items.into_iter();
                let mut vec = Vec::with_capacity_in(items.size_hint().0, self);
                vec.extend(items);
                vec
            }

            /// Copy `source` into this allocator, returning the slice that
            /// lives as long as the allocation does. Spelled `Vec::leak`
            /// underneath, which for an arena is a borrow until its rewind,
            /// never a process leak.
            pub fn slice_from<T: Copy>(&self, source: &[T]) -> &[T] {
                self.vec_from(source.iter().copied()).leak()
            }

            pub fn deque<T>(&self, capacity: usize) -> std::collections::VecDeque<T, &Self> {
                std::collections::VecDeque::with_capacity_in(capacity, self)
            }

            pub fn deque_from<T>(&self, items: impl IntoIterator<Item = T>) -> std::collections::VecDeque<T, &Self> {
                let items = items.into_iter();
                let mut deque = std::collections::VecDeque::with_capacity_in(items.size_hint().0, self);
                deque.extend(items);
                deque
            }

            pub fn fast_map<K, V>(&self, capacity: usize) -> $crate::FastHashMapIn<K, V, &Self> {
                $crate::FastHashMapIn::with_capacity_and_hasher_in(capacity, Default::default(), self)
            }

            pub fn fast_map_from<K: ::std::cmp::Eq + ::std::hash::Hash, V>(&self, items: impl IntoIterator<Item = (K, V)>) -> $crate::FastHashMapIn<K, V, &Self> {
                let items = items.into_iter();
                let mut map = $crate::FastHashMapIn::with_capacity_and_hasher_in(items.size_hint().0, Default::default(), self);
                map.extend(items);
                map
            }

            pub fn fast_set<K>(&self, capacity: usize) -> $crate::FastHashSetIn<K, &Self> {
                $crate::FastHashSetIn::with_capacity_and_hasher_in(capacity, Default::default(), self)
            }

            pub fn fast_set_from<K: ::std::cmp::Eq + ::std::hash::Hash>(&self, items: impl IntoIterator<Item = K>) -> $crate::FastHashSetIn<K, &Self> {
                let items = items.into_iter();
                let mut set = $crate::FastHashSetIn::with_capacity_and_hasher_in(items.size_hint().0, Default::default(), self);
                set.extend(items);
                set
            }

            pub fn entropy_map<K, V>(&self, capacity: usize) -> $crate::HighEntropyMapIn<K, V, &Self> {
                $crate::HighEntropyMapIn::with_capacity_and_hasher_in(capacity, Default::default(), self)
            }

            pub fn entropy_map_from<K: ::std::cmp::Eq + ::std::hash::Hash, V>(&self, items: impl IntoIterator<Item = (K, V)>) -> $crate::HighEntropyMapIn<K, V, &Self> {
                let items = items.into_iter();
                let mut map = $crate::HighEntropyMapIn::with_capacity_and_hasher_in(items.size_hint().0, Default::default(), self);
                map.extend(items);
                map
            }

            pub fn entropy_set<K>(&self, capacity: usize) -> $crate::HighEntropySetIn<K, &Self> {
                $crate::HighEntropySetIn::with_capacity_and_hasher_in(capacity, Default::default(), self)
            }

            pub fn entropy_set_from<K: ::std::cmp::Eq + ::std::hash::Hash>(&self, items: impl IntoIterator<Item = K>) -> $crate::HighEntropySetIn<K, &Self> {
                let items = items.into_iter();
                let mut set = $crate::HighEntropySetIn::with_capacity_and_hasher_in(items.size_hint().0, Default::default(), self);
                set.extend(items);
                set
            }

            pub fn int_map<K, V>(&self, capacity: usize) -> $crate::IntMapIn<K, V, &Self> {
                $crate::IntMapIn::with_capacity_and_hasher_in(capacity, Default::default(), self)
            }

            pub fn int_map_from<K: ::std::cmp::Eq + ::std::hash::Hash, V>(&self, items: impl IntoIterator<Item = (K, V)>) -> $crate::IntMapIn<K, V, &Self> {
                let items = items.into_iter();
                let mut map = $crate::IntMapIn::with_capacity_and_hasher_in(items.size_hint().0, Default::default(), self);
                map.extend(items);
                map
            }

            pub fn int_set<K>(&self, capacity: usize) -> $crate::IntSetIn<K, &Self> {
                $crate::IntSetIn::with_capacity_and_hasher_in(capacity, Default::default(), self)
            }

            pub fn int_set_from<K: ::std::cmp::Eq + ::std::hash::Hash>(&self, items: impl IntoIterator<Item = K>) -> $crate::IntSetIn<K, &Self> {
                let items = items.into_iter();
                let mut set = $crate::IntSetIn::with_capacity_and_hasher_in(items.size_hint().0, Default::default(), self);
                set.extend(items);
                set
            }
        }
    )+};
}

pub use crate::impl_allocator_collections;

impl_allocator_collections!(Arena, TempArena<'_>, FrameArena);

macro_rules! impl_arena_zeroed_bytes {
    ($($arena:ty),+ $(,)?) => {$(
        impl $arena {
            /// Zeroed bytes at a chosen alignment — `vec::<u8>` only promises 1.
            /// Arena-only: the slice rewinds with the arena; nothing frees it.
            pub fn alloc_zeroed_bytes(&self, size: usize, align: usize) -> &mut [u8] {
                let layout = Layout::from_size_align(size, align).unwrap();
                let mut allocation = self.allocate_zeroed(layout).unwrap();
                unsafe { allocation.as_mut() }
            }
        }
    )+};
}

impl_arena_zeroed_bytes!(Arena, TempArena<'_>, FrameArena);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_vector_growth_preserves_rows_when_tail_cannot_expand() {
        let arena = Arena::new(ARENA_PAGE_SIZE);
        let temp = arena.temp();
        let mut rows = temp.vec::<(u128, u128, u128)>(1);

        for value in 0..2_000u128 {
            rows.push((value, value + 1, value + 2));
        }

        assert_eq!(rows.len(), 2_000);
        for (value, row) in rows.iter().enumerate() {
            assert_eq!(*row, (value as u128, value as u128 + 1, value as u128 + 2));
        }
    }

    #[test]
    fn scratch_grow_zeroed_preserves_prefix_and_clears_extension() {
        let arena = Arena::new(ARENA_PAGE_SIZE);
        let old_layout = Layout::from_size_align(64, 16).unwrap();
        let new_layout = Layout::from_size_align(8_192, 16).unwrap();
        let allocation = arena.allocate(old_layout).unwrap();
        let pointer = allocation.cast::<u8>();
        unsafe { pointer.write_bytes(0x5a, old_layout.size()) };

        let grown = unsafe { arena.grow_zeroed(pointer, old_layout, new_layout).unwrap() };
        let bytes = unsafe { grown.as_ptr().as_ref().unwrap() };

        assert!(bytes[..old_layout.size()].iter().all(|&byte| byte == 0x5a));
        assert!(bytes[old_layout.size()..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn frame_grow_zeroed_preserves_prefix_and_clears_extension() {
        let arena = FrameArena::new(ARENA_PAGE_SIZE);
        let old_layout = Layout::from_size_align(64, 16).unwrap();
        let new_layout = Layout::from_size_align(8_192, 16).unwrap();
        let allocation = arena.allocate(old_layout).unwrap();
        let pointer = allocation.cast::<u8>();
        unsafe { pointer.write_bytes(0xa5, old_layout.size()) };

        let grown = unsafe { arena.grow_zeroed(pointer, old_layout, new_layout).unwrap() };
        let bytes = unsafe { grown.as_ptr().as_ref().unwrap() };

        assert!(bytes[..old_layout.size()].iter().all(|&byte| byte == 0xa5));
        assert!(bytes[old_layout.size()..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn frame_parallel_allocations_do_not_overlap() {
        let arena = std::sync::Arc::new(FrameArena::new(ARENA_PAGE_SIZE));
        let ranges = std::sync::Arc::new(Mutex::new(Vec::new()));
        let mut threads = Vec::new();

        for _ in 0..8 {
            let arena = std::sync::Arc::clone(&arena);
            let ranges = std::sync::Arc::clone(&ranges);
            threads.push(std::thread::spawn(move || {
                let layout = Layout::from_size_align(64, 64).unwrap();
                for _ in 0..1_000 {
                    let allocation = arena.allocate(layout).unwrap();
                    let start = allocation.cast::<u8>().addr().get();
                    ranges.lock().unwrap().push(start..start + layout.size());
                }
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }

        let mut ranges = ranges.lock().unwrap();
        ranges.sort_unstable_by_key(|range| range.start);
        assert!(ranges.windows(2).all(|pair| pair[0].end <= pair[1].start));
    }

    #[test]
    #[should_panic(expected = "scratch temp scopes must drop in reverse creation order")]
    fn temp_scopes_reject_out_of_order_drop() {
        let arena = Arena::new(ARENA_PAGE_SIZE);
        let outer = arena.temp();
        let _inner = arena.temp();
        drop(outer);
    }
}
