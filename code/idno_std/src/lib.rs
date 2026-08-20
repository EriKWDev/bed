//! idno_std — the idno standard library: one sane default for the questions
//! every project asks.
//!
//! - *I want a fast hash map, which should I use?* [`FastHashMap`]
//!   (deterministic rapidhash) for any key, [`IntMap`] (a
//!   one-multiply mixer) for structured integer keys, [`HighEntropyMap`]
//!   for keys that are already uniform hashes.
//! - *Where do temporaries live?* [`mem()`] — thread-local scratch arenas
//!   scoped with `scratch().temp()`, and one shared [`FrameArena`] whose
//!   allocations are guaranteed to survive until the frame boundary.
//! - *I want a thread pool, which should I use?* [`threads()`].
//! - Small math and byte helpers every crate reaches for live in [`utils`].

#![feature(allocator_api)]

use std::cell::Cell;
use std::sync::OnceLock;

pub mod arena;
pub use bitfield;
pub mod dynamics;
pub mod logging;
pub mod paths;
pub mod random;
pub mod shutdown;
pub mod time;
pub mod utils;

pub use arena::{Arena, FrameArena, FrameVec, TempArena};
pub use micropool;

// Hashing

pub type RapidHasher<'s> = rapidhash::fast::RapidHasher<'s>;

/*
    NOTE: The seed is FIXED so hashing is deterministic across runs and
          machines — hash values may participate in replicated or
          serialized state.
*/
#[derive(Clone)]
pub struct DeterministicRapidBuildHasher(pub rapidhash::fast::SeedableState<'static>);

impl Default for DeterministicRapidBuildHasher {
    fn default() -> Self {
        Self(rapidhash::fast::SeedableState::fixed())
    }
}

impl std::hash::BuildHasher for DeterministicRapidBuildHasher {
    type Hasher = RapidHasher<'static>;
    fn build_hasher(&self) -> Self::Hasher {
        std::hash::BuildHasher::build_hasher(&self.0)
    }
}

pub fn quick_hash<H: std::hash::Hash>(it: H) -> u64 {
    use std::hash::Hasher;
    let mut hasher = RapidHasher::default_const();
    it.hash(&mut hasher);
    hasher.finish()
}

#[derive(Default)]
pub struct MixHasher(u64);

impl std::hash::Hasher for MixHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, _bytes: &[u8]) {
        unreachable!("MixHasher only supports integer keys");
    }

    #[inline]
    fn write_u32(&mut self, v: u32) {
        self.0 = utils::hash_multiply_xorshift_u64(v as u64);
    }

    #[inline]
    fn write_u64(&mut self, v: u64) {
        self.0 = utils::hash_multiply_xorshift_u64(v);
    }

    #[inline]
    fn write_usize(&mut self, v: usize) {
        self.0 = utils::hash_multiply_xorshift_u64(v as u64);
    }

    #[inline]
    fn write_u128(&mut self, v: u128) {
        self.0 = utils::hash_multiply_xorshift_u64((v as u64) ^ ((v >> 64) as u64));
    }
}

#[derive(Default)]
pub struct HighEntropyHasher(u64);

impl std::hash::Hasher for HighEntropyHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, _bytes: &[u8]) {
        unreachable!("HighEntropyHasher only supports integer keys");
    }

    #[inline]
    fn write_u32(&mut self, v: u32) {
        self.0 = v as u64;
    }

    #[inline]
    fn write_u64(&mut self, v: u64) {
        self.0 = v;
    }

    #[inline]
    fn write_u128(&mut self, v: u128) {
        self.0 = (v as u64) ^ ((v >> 64) as u64);
    }
}

type BuildOf<H> = std::hash::BuildHasherDefault<H>;

pub type FastHashMap<K, V> = std::collections::HashMap<K, V, DeterministicRapidBuildHasher>;
pub type FastHashSet<K> = std::collections::HashSet<K, DeterministicRapidBuildHasher>;

pub type HighEntropyMap<K, V> = std::collections::HashMap<K, V, BuildOf<HighEntropyHasher>>;
pub type HighEntropySet<K> = std::collections::HashSet<K, BuildOf<HighEntropyHasher>>;

pub type IntMap<K, V> = std::collections::HashMap<K, V, BuildOf<MixHasher>>;
pub type IntSet<K> = std::collections::HashSet<K, BuildOf<MixHasher>>;

/*
    NOTE: The allocator-parameterized twins, so a map/set can live in an arena or
          the Heap. They are hashbrown, not std: std's HashMap/HashSet carry no
          allocator parameter (only Vec/VecDeque/Box do). Built through the
          arena-collection API (`arena.int_map(..)`), never spelled by hand.
*/
pub type FastHashMapIn<K, V, A> = hashbrown::HashMap<K, V, DeterministicRapidBuildHasher, A>;
pub type FastHashSetIn<K, A> = hashbrown::HashSet<K, DeterministicRapidBuildHasher, A>;

pub type HighEntropyMapIn<K, V, A> = hashbrown::HashMap<K, V, BuildOf<HighEntropyHasher>, A>;
pub type HighEntropySetIn<K, A> = hashbrown::HashSet<K, BuildOf<HighEntropyHasher>, A>;

pub type IntMapIn<K, V, A> = hashbrown::HashMap<K, V, BuildOf<MixHasher>, A>;
pub type IntSetIn<K, A> = hashbrown::HashSet<K, BuildOf<MixHasher>, A>;

// Memory

static PROCESS_RUNTIME: OnceLock<ProcessRuntime> = OnceLock::new();

pub struct ProcessRuntime {
    pub memory: Memory,
    pub logging: logging::LoggingRuntime,
    pub shutdown_requested: std::sync::atomic::AtomicBool,
    pub threads: OnceLock<micropool::ThreadPool>,
}

impl Default for ProcessRuntime {
    fn default() -> Self {
        Self {
            memory: Memory::default(),
            logging: logging::LoggingRuntime::default(),
            shutdown_requested: std::sync::atomic::AtomicBool::new(false),
            threads: OnceLock::new(),
        }
    }
}

pub fn process_runtime() -> &'static ProcessRuntime {
    PROCESS_RUNTIME.get_or_init(ProcessRuntime::default)
}

pub fn mem() -> &'static Memory {
    &process_runtime().memory
}

#[cfg(any(test, feature = "many-test-threads"))]
const SCRATCH_ARENA_SLOTS: usize = 256;
#[cfg(not(any(test, feature = "many-test-threads")))]
const SCRATCH_ARENA_SLOTS: usize = 64;

pub struct Memory {
    scratch_arenas: [OnceLock<Arena>; SCRATCH_ARENA_SLOTS],
    frame: FrameArena,
    next_scratch_arena: core::sync::atomic::AtomicUsize,
}

/*
    NOTE: `Arena` itself is deliberately neither Send nor Sync. Memory gives
          each OS thread one arena behind a stable slot. Scratch state never
          crosses threads; FrameArena is independently thread-safe.
*/
unsafe impl Send for Memory {}
unsafe impl Sync for Memory {}

impl Default for Memory {
    fn default() -> Self {
        Self {
            scratch_arenas: [const { OnceLock::new() }; SCRATCH_ARENA_SLOTS],
            frame: FrameArena::new(32 * 1024 * 1024),
            next_scratch_arena: core::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl Memory {
    /*
        NOTE: The scratch arena slot is the platform thread-context index:
              unique among live threads, stable for a logical thread across
              hot reloads (a respawn from a kept context lands on the same
              arena), and recycled when a context is released. Arenas stay in
              the process runtime rather than a non-POD thread-context local
              so retained allocation capacity survives hot reload. Using the
              context's POD index preserves the logical-thread identity.
              std::ThreadId cannot provide this identity — a reloaded image's
              std re-mints ids for the same OS threads.
    */
    #[inline]
    pub fn thread_index(&self) -> usize {
        thread_local! {
            static MINE: Cell<Option<usize>> = const { Cell::new(None) };
        }

        MINE.with(|cell| {
            if let Some(index) = cell.get() {
                index
            } else {
                let index = self
                    .next_scratch_arena
                    .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                assert!(
                    index < self.scratch_arenas.len(),
                    "called from: {:?}",
                    std::panic::Location::caller()
                );
                cell.set(Some(index));
                index
            }
        })
    }

    #[inline]
    pub fn scratch(&self) -> &Arena {
        self.scratch_arenas[self.thread_index()].get_or_init(|| Arena::new(4 * 1024 * 1024))
    }

    #[inline]
    pub fn frame(&self) -> &FrameArena {
        &self.frame
    }

    /// # Safety
    /// Frame boundary only: no frame allocation may remain live and no worker
    /// may be using the frame arena. Scratch temps rewind at their lexical
    /// boundary; only the calling thread is checked here.
    pub unsafe fn frame_reset(&self) {
        if let Some(scratch) = self.scratch_arenas[self.thread_index()].get() {
            scratch.assert_no_temp_scope();
        }
        unsafe { self.frame.reset() };
    }
}

// Threads

pub fn threads() -> &'static micropool::ThreadPool {
    process_runtime().threads.get_or_init(|| {
        profiling::scope!("init micro threads");
        let thread_count = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .saturating_sub(1)
            .max(1);
        micropool::ThreadPoolBuilder::default()
            .num_threads(thread_count)
            .spawn_handler(move |i, worker| {
                std::thread::spawn(move || {
                    let _name = format!("micro-{}/{thread_count}", i + 1);
                    profiling::register_thread!(&_name);
                    (worker)();
                })
            })
            .build()
    })
}

pub struct SendPtr<T>(pub *mut T);

unsafe impl<T> Send for SendPtr<T> {}
unsafe impl<T> Sync for SendPtr<T> {}

impl<T> Clone for SendPtr<T> {
    fn clone(&self) -> Self {
        Self(self.0)
    }
}

impl<T> Copy for SendPtr<T> {}

impl<T> SendPtr<T> {
    /*
        NOTE: Taking `self` by value keeps closures capturing the wrapper,
              not the raw field — disjoint capture would drop the Send/Sync
              blessing.
    */
    #[inline]
    pub fn at(self, index: usize) -> *mut T {
        unsafe { self.0.add(index) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_runtime_is_stable() {
        let resolved = process_runtime() as *const ProcessRuntime;
        assert_eq!(process_runtime() as *const ProcessRuntime, resolved);
        let temporary = mem().scratch().temp();
        let mut values = temporary.vec::<u64>(8);
        values.extend([1, 2, 3]);
        assert_eq!(values.iter().sum::<u64>(), 6);
    }

    #[test]
    fn frame_reset_does_not_touch_another_threads_scratch_scope() {
        let memory = std::sync::Arc::new(Memory::default());
        let entered = std::sync::Arc::new(std::sync::Barrier::new(2));
        let reset = std::sync::Arc::new(std::sync::Barrier::new(2));
        let worker_memory = std::sync::Arc::clone(&memory);
        let worker_entered = std::sync::Arc::clone(&entered);
        let worker_reset = std::sync::Arc::clone(&reset);

        let worker = std::thread::spawn(move || {
            let temp = worker_memory.scratch().temp();
            let mut values = temp.vec_from([1, 2, 3]);
            worker_entered.wait();
            worker_reset.wait();
            values.push(4);
            assert_eq!(&values[..], &[1, 2, 3, 4]);
        });

        entered.wait();
        unsafe { memory.frame_reset() };
        reset.wait();
        worker.join().unwrap();
    }

    #[test]
    #[should_panic(expected = "scratch temp scope crossed its boundary")]
    fn frame_reset_rejects_a_calling_thread_scratch_scope() {
        let memory = Memory::default();
        let _temp = memory.scratch().temp();
        unsafe { memory.frame_reset() };
    }
}
