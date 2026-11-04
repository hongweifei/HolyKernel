//! 内核堆分配器。
//!
//! 静态数组由通用堆管理器维护，释放会回收；IPC 消息载荷等短生命周期
//! 分配不会永久占用内核堆。线性分配器保留给启动期一次性分配。

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use linked_list_allocator::Heap;
use spin::Mutex;

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

/// 可回收的静态堆。
#[repr(align(4096))]
pub struct FreeHeap<const N: usize> {
    data: UnsafeCell<[u8; N]>,
    heap: Mutex<Heap>,
    ready: AtomicBool,
}

// SAFETY: 堆只在 `Mutex` 下访问；静态数组在结构体内独占。
unsafe impl<const N: usize> Sync for FreeHeap<N> {}

impl<const N: usize> FreeHeap<N> {
    /// 构造空堆；首次分配时初始化。
    pub const fn new() -> Self {
        Self {
            data: UnsafeCell::new([0; N]),
            heap: Mutex::new(Heap::empty()),
            ready: AtomicBool::new(false),
        }
    }
}

impl<const N: usize> Default for FreeHeap<N> {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: 分配与释放都在自旋锁下进行；静态数组区间由本结构独占。
unsafe impl<const N: usize> GlobalAlloc for FreeHeap<N> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut heap = self.heap.lock();
        if !self.ready.swap(true, Ordering::AcqRel) {
            // SAFETY: 静态数组由本结构体独占，大小与对齐满足堆管理器要求。
            unsafe {
                heap.init(self.data.get() as *mut u8, N);
            }
        }
        heap.allocate_first_fit(layout)
            .map_or(core::ptr::null_mut(), |pointer| pointer.as_ptr())
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if let Some(pointer) = NonNull::new(pointer) {
            // SAFETY: 指针来自本分配器，layout 与分配时一致。
            unsafe {
                self.heap.lock().deallocate(pointer, layout);
            }
        }
    }
}
