//! 服务进程的线性堆。
//!
//! 服务没有通用堆管理器：启动时从任务 Arena 一次性取一段内存，按对齐向上
//! 分配；释放为空操作，适合一次性装载协议缓冲与设备结构的服务器。

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

/// 线性堆分配器。
pub struct BumpHeap {
    size: usize,
    base: AtomicUsize,
    next: AtomicUsize,
}

impl BumpHeap {
    /// 以指定字节数构造分配器；首次分配时向内核申请。
    pub const fn new(size: usize) -> Self {
        Self {
            size,
            base: AtomicUsize::new(0),
            next: AtomicUsize::new(0),
        }
    }
}

// SAFETY: 分配与释放都只在服务自己的用户态任务内调用；`base`/`next` 用原子
// 维护，地址范围由一次性 SYS_ALLOC 决定，分配结果不越界或重复。
unsafe impl GlobalAlloc for BumpHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut base = self.base.load(Ordering::Acquire);
        if base == 0 {
            base = crate::alloc(self.size, 16) as usize;
            if base == 0 {
                return core::ptr::null_mut();
            }
            self.base.store(base, Ordering::Release);
            self.next.store(base, Ordering::Release);
        }
        let mut current = self.next.load(Ordering::Acquire);
        loop {
            let aligned = (current + layout.align() - 1) & !(layout.align() - 1);
            let Some(end) = aligned.checked_add(layout.size()) else {
                return core::ptr::null_mut();
            };
            if end > base + self.size {
                return core::ptr::null_mut();
            }
            match self
                .next
                .compare_exchange(current, end, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return aligned as *mut u8,
                Err(actual) => current = actual,
            }
        }
    }

    unsafe fn dealloc(&self, _pointer: *mut u8, _layout: Layout) {}
}
