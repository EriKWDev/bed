//! Virtual-memory-backed containers over [`platform::Reservation`]: data at
//! stable addresses that never realloc-and-copy, in host-owned mappings that
//! outlive the application image using them.

use platform::Reservation;

/// A growable array over a reservation: stable addresses, no realloc, commit
/// on demand. Elements must be trivially movable and are never dropped.
pub struct VirtualVec<T: Copy> {
    mem: Reservation,
    len: usize,
    _marker: std::marker::PhantomData<T>,
}

impl<T: Copy> VirtualVec<T> {
    pub fn new(max_elements: usize) -> Self {
        Self { mem: Reservation::new(max_elements * size_of::<T>()), len: 0, _marker: std::marker::PhantomData }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    #[inline]
    fn ptr(&self) -> *mut T {
        self.mem.base().as_ptr() as *mut T
    }

    pub fn push(&mut self, value: T) {
        self.mem.commit_to((self.len + 1) * size_of::<T>());
        unsafe { self.ptr().add(self.len).write(value) };
        self.len += 1;
    }

    pub fn extend_from_slice(&mut self, values: &[T]) {
        self.mem.commit_to((self.len + values.len()) * size_of::<T>());
        unsafe {
            std::ptr::copy_nonoverlapping(values.as_ptr(), self.ptr().add(self.len), values.len());
        }
        self.len += values.len();
    }

    /// Append `count` elements copied from possibly-unaligned raw bytes.
    pub fn extend_from_raw(&mut self, bytes: &[u8], count: usize) {
        debug_assert!(bytes.len() >= count * size_of::<T>());
        self.mem.commit_to((self.len + count) * size_of::<T>());
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.ptr().add(self.len) as *mut u8, count * size_of::<T>());
        }
        self.len += count;
    }

    #[inline]
    pub fn swap_remove(&mut self, index: usize) -> T {
        debug_assert!(index < self.len);
        unsafe {
            let removed = self.ptr().add(index).read();
            self.len -= 1;
            if index != self.len {
                self.ptr().add(index).write(self.ptr().add(self.len).read());
            }
            removed
        }
    }

    /// Append `count` zeroed elements (fresh commits are already zero pages;
    /// recycled tail bytes are cleared here).
    pub fn extend_zeroed(&mut self, count: usize) {
        self.mem.commit_to((self.len + count) * size_of::<T>());
        unsafe { (self.ptr().add(self.len) as *mut u8).write_bytes(0, count * size_of::<T>()) };
        self.len += count;
    }

    /// Swap-remove a whole stride of elements: the last `stride` elements
    /// move into the removed row's place. `len` must be a multiple of
    /// `stride`, `row` counts in strides.
    pub fn swap_remove_stride(&mut self, row: usize, stride: usize) {
        let rows = self.len / stride;
        debug_assert!(row < rows && self.len % stride == 0);
        let last = rows - 1;
        if row != last {
            unsafe {
                std::ptr::copy_nonoverlapping(self.ptr().add(last * stride), self.ptr().add(row * stride), stride);
            }
        }
        self.len -= stride;
    }
}

impl<T: Copy> std::ops::Deref for VirtualVec<T> {
    type Target = [T];
    #[inline]
    fn deref(&self) -> &[T] {
        unsafe { std::slice::from_raw_parts(self.ptr(), self.len) }
    }
}

impl<T: Copy> std::ops::DerefMut for VirtualVec<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr(), self.len) }
    }
}
