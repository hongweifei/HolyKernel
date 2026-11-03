//! 内核启动期堆分配器。
//!
//! 从静态堆线性分配，`dealloc` 不回收；启动与内核对象生命周期都落在同一段
//! 内存上，语义简单且可预测。

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

/// 内核堆大小。
const HEAP_SIZE: usize = 1024 * 1024;

#[repr(align(4096))]
struct Heap(UnsafeCell<[u8; HEAP_SIZE]>);

// SAFETY: 堆内存只由 `BumpAllocator` 通过原子游标分配，返回的区间互不重叠。
unsafe impl Sync for Heap {}

static HEAP: Heap = Heap(UnsafeCell::new([0; HEAP_SIZE]));

struct BumpAllocator {
    next: AtomicUsize,
}

// SAFETY: `alloc` 从静态堆中切出不重叠区间；`dealloc` 不回收，符合启动期
// 分配器的语义。
unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let base = HEAP.0.get() as usize;
        let mut current = self.next.load(Ordering::Relaxed);
        loop {
            let start = (base + current + layout.align() - 1) & !(layout.align() - 1);
            let end = start + layout.size();
            if end > base + HEAP_SIZE {
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

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator {
    next: AtomicUsize::new(0),
};
