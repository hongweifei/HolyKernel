//! 静态堆线性分配器。
//!
//! 从固定大小的静态数组按对齐切分区间，`dealloc` 不回收；适合启动期与
//! 生命周期与进程相同的内核对象。

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

/// 固定容量的静态堆。
#[repr(align(4096))]
pub struct BumpHeap<const N: usize> {
    data: UnsafeCell<[u8; N]>,
    next: AtomicUsize,
}

// SAFETY: 分配只通过原子游标推进，返回的区间互不重叠。
unsafe impl<const N: usize> Sync for BumpHeap<N> {}

impl<const N: usize> BumpHeap<N> {
    /// 构造空堆。
    pub const fn new() -> Self {
        Self {
            data: UnsafeCell::new([0; N]),
            next: AtomicUsize::new(0),
        }
    }
}

impl<const N: usize> Default for BumpHeap<N> {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: `alloc` 从静态堆中切出不重叠区间；`dealloc` 不回收，符合
// 启动期分配器的语义。
unsafe impl<const N: usize> GlobalAlloc for BumpHeap<N> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let base = self.data.get() as usize;
        let mut current = self.next.load(Ordering::Relaxed);
        loop {
            let start = (base + current + layout.align() - 1) & !(layout.align() - 1);
            let end = start + layout.size();
            if end > base + N {
                return core::ptr::null_mut();
            }
            let offset = end - base;
            match self.next.compare_exchange_weak(
                current,
                offset,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return start as *mut u8,
                Err(actual) => current = actual,
            }
        }
    }

    unsafe fn dealloc(&self, _pointer: *mut u8, _layout: Layout) {}
}
