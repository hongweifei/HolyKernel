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

struct HeapState {
    bootstrap: Heap,
    dynamic: Heap,
    bootstrap_ready: bool,
    dynamic_ready: bool,
    dynamic_base: usize,
    dynamic_len: usize,
}

/// 启动期小堆加 FrameBank 主堆。
pub struct KernelHeap<const N: usize> {
    bootstrap: UnsafeCell<[u8; N]>,
    state: Mutex<HeapState>,
}

// SAFETY: 两段堆都在同一把自旋锁下访问，bootstrap 数组由结构体独占。
unsafe impl<const N: usize> Sync for KernelHeap<N> {}

impl<const N: usize> KernelHeap<N> {
    /// 构造空堆；首次分配时初始化 bootstrap。
    pub const fn new() -> Self {
        Self {
            bootstrap: UnsafeCell::new([0; N]),
            state: Mutex::new(HeapState {
                bootstrap: Heap::empty(),
                dynamic: Heap::empty(),
                bootstrap_ready: false,
                dynamic_ready: false,
                dynamic_base: 0,
                dynamic_len: 0,
            }),
        }
    }

    /// 以 FrameBank 供给的虚拟区间初始化主堆。
    pub fn init_dynamic(&self, base: usize, len: usize) {
        let mut state = self.state.lock();
        assert!(!state.dynamic_ready, "内核主堆重复初始化");
        // SAFETY: 调用方保证区间连续、可写且只交给本分配器。
        unsafe {
            state.dynamic.init(base as *mut u8, len);
        }
        state.dynamic_base = base;
        state.dynamic_len = len;
        state.dynamic_ready = true;
    }

    /// 返回主堆已用字节数。
    pub fn dynamic_used(&self) -> usize {
        let state = self.state.lock();
        state.dynamic.used()
    }
}

impl<const N: usize> Default for KernelHeap<N> {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: 分配与释放都在同一把自旋锁下进行，bootstrap 与 dynamic 区间互不重叠。
unsafe impl<const N: usize> GlobalAlloc for KernelHeap<N> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut state = self.state.lock();
        if !state.bootstrap_ready {
            // SAFETY: bootstrap 数组由本结构体独占。
            unsafe {
                state.bootstrap.init(self.bootstrap.get() as *mut u8, N);
            }
            state.bootstrap_ready = true;
        }
        if state.dynamic_ready
            && let Ok(pointer) = state.dynamic.allocate_first_fit(layout)
        {
            // SAFETY: 分配器返回独占区间；保持静态堆原有的清零语义。
            unsafe {
                core::ptr::write_bytes(pointer.as_ptr(), 0, layout.size());
            }
            return pointer.as_ptr();
        }
        let pointer = state
            .bootstrap
            .allocate_first_fit(layout)
            .map_or(core::ptr::null_mut(), |pointer| pointer.as_ptr());
        if !pointer.is_null() {
            // SAFETY: 分配器返回独占区间；保持静态堆原有的清零语义。
            unsafe {
                core::ptr::write_bytes(pointer, 0, layout.size());
            }
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let Some(pointer) = NonNull::new(pointer) else {
            return;
        };
        let mut state = self.state.lock();
        let address = pointer.as_ptr() as usize;
        if state.dynamic_ready
            && address >= state.dynamic_base
            && address < state.dynamic_base.saturating_add(state.dynamic_len)
        {
            // SAFETY: 指针落在主堆区间，layout 与分配时一致。
            unsafe {
                state.dynamic.deallocate(pointer, layout);
            }
        } else {
            // SAFETY: 指针落在 bootstrap 区间，layout 与分配时一致。
            unsafe {
                state.bootstrap.deallocate(pointer, layout);
            }
        }
    }
}

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
        let pointer = heap
            .allocate_first_fit(layout)
            .map_or(core::ptr::null_mut(), |pointer| pointer.as_ptr());
        if !pointer.is_null() {
            let base = self.data.get() as usize;
            let start = pointer as usize;
            let end = start.saturating_add(layout.size());
            assert!(
                start >= base && end <= base + N,
                "内核堆越界分配：{start:#x}..{end:#x}"
            );
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if let Some(pointer) = NonNull::new(pointer) {
            let base = self.data.get() as usize;
            let start = pointer.as_ptr() as usize;
            let end = start.saturating_add(layout.size());
            assert!(
                start >= base && end <= base + N,
                "内核堆越界释放：{start:#x}..{end:#x}"
            );
            // SAFETY: 指针来自本分配器，layout 与分配时一致。
            unsafe {
                self.heap.lock().deallocate(pointer, layout);
            }
        }
    }
}
