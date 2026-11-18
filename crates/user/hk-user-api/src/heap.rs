//! 服务进程的堆。
//!
//! 首次分配时从任务 Arena 取一段内存交给通用堆管理器；释放会回收，适合
//! 文件系统缓存、日志事务快照等反复分配的服务。

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;

use linked_list_allocator::Heap;
use spin::Mutex;

/// 可回收的堆分配器。
pub struct FreeHeap {
    size: usize,
    heap: Mutex<Heap>,
}

impl FreeHeap {
    /// 以指定字节数构造分配器；首次分配时向内核申请。
    pub const fn new(size: usize) -> Self {
        Self {
            size,
            heap: Mutex::new(Heap::empty()),
        }
    }
}

// SAFETY: 分配与释放都只在服务自己的用户态任务内调用；Arena 区间由一次
// SYS_ALLOC 独占，堆管理器在自旋锁下维护空闲链表。
unsafe impl GlobalAlloc for FreeHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut heap = self.heap.lock();
        if heap.size() == 0 {
            let base = crate::alloc(self.size, 16);
            if base.is_null() {
                return core::ptr::null_mut();
            }
            // SAFETY: 区间由 SYS_ALLOC 返回并属于当前任务，大小与对齐满足要求。
            unsafe {
                heap.init(base, self.size);
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
