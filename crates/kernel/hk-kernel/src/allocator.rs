//! 内核堆分配器。
//!
//! 启动期 bootstrap 小堆只负责最早期的分配；System Capacity 建立后，主堆由
//! 固定上限的 chunk 组成，chunk 按需从 FrameBank 取页、完全空闲后归还。
//! 这样内核堆不再受启动期一段连续区间限制，同时所有 chunk 仍计入 System
//! Capacity 与 FrameBank 归因。
//!
//! 增长回调会调用 FrameBank，而 FrameBank 的元数据本身也可能使用内核堆。
//! 为打断这条递归，增长期间的嵌套分配统一落到应急堆；应急堆只增不减，容量
//! 有界，不会被普通分配长期占用。

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use linked_list_allocator::Heap;
use spin::Mutex;

/// 单个主堆 chunk 的最小长度。
const MIN_CHUNK_BYTES: usize = 1024 * 1024;
/// 主堆 chunk 上限；配合 1 MiB chunk 形成可配置的内核堆上限。
const MAX_CHUNKS: usize = 256;
/// 增长期间的应急堆大小。
const EMERGENCY_BYTES: usize = 1024 * 1024;

/// 增长回调：请求 `len` 字节，返回 `(direct map 虚拟基址, 实际长度)`。
pub type GrowFn = fn(usize) -> Option<(usize, usize)>;
/// 归还回调：把一段不再使用的 chunk 还给 FrameBank。
pub type ShrinkFn = fn(usize, usize);

#[repr(align(4096))]
struct Aligned<const N: usize>([u8; N]);

struct ChunkSlot {
    heap: Heap,
    base: usize,
    len: usize,
    active: bool,
    releasable: bool,
}

impl ChunkSlot {
    const fn empty() -> Self {
        Self {
            heap: Heap::empty(),
            base: 0,
            len: 0,
            active: false,
            releasable: false,
        }
    }
}

struct HeapState {
    bootstrap: Heap,
    bootstrap_ready: bool,
    emergency: Heap,
    emergency_ready: bool,
    chunks: [ChunkSlot; MAX_CHUNKS],
    grow: Option<GrowFn>,
    shrink: Option<ShrinkFn>,
}

impl HeapState {
    fn chunk_alloc(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        for slot in &mut self.chunks {
            if !slot.active {
                continue;
            }
            if let Ok(pointer) = slot.heap.allocate_first_fit(layout) {
                return Some(pointer);
            }
        }
        None
    }

    fn chunk_for(&mut self, address: usize) -> Option<usize> {
        self.chunks.iter().position(|slot| {
            slot.active && address >= slot.base && address < slot.base.saturating_add(slot.len)
        })
    }

    fn add_chunk(&mut self, base: usize, len: usize, releasable: bool) -> bool {
        let Some(index) = self.chunks.iter().position(|slot| !slot.active) else {
            return false;
        };
        let slot = &mut self.chunks[index];
        // SAFETY: 回调保证该区间连续、可写、未交给其他分配器。
        unsafe {
            slot.heap.init(base as *mut u8, len);
        }
        slot.base = base;
        slot.len = len;
        slot.active = true;
        slot.releasable = releasable;
        true
    }

    fn used(&self) -> usize {
        self.chunks
            .iter()
            .filter(|slot| slot.active)
            .map(|slot| slot.heap.used())
            .sum()
    }

    fn chunk_count(&self) -> usize {
        self.chunks.iter().filter(|slot| slot.active).count()
    }

    fn capacity(&self) -> usize {
        self.chunks
            .iter()
            .filter(|slot| slot.active)
            .map(|slot| slot.len)
            .sum()
    }
}

/// 全局增长标记：增长回调内的嵌套分配不得再次触发增长。
static GROWING: AtomicBool = AtomicBool::new(false);
/// 增长期间应急堆已经初始化的地址；0 表示尚未初始化。
static EMERGENCY_BASE: AtomicUsize = AtomicUsize::new(0);

/// 启动期小堆加按需增长的 FrameBank 主堆。
pub struct KernelHeap<const N: usize> {
    bootstrap: UnsafeCell<Aligned<N>>,
    emergency: UnsafeCell<Aligned<EMERGENCY_BYTES>>,
    state: Mutex<HeapState>,
}

// SAFETY: 所有堆状态都在同一把自旋锁下访问；静态区间由结构体独占。
unsafe impl<const N: usize> Sync for KernelHeap<N> {}

impl<const N: usize> KernelHeap<N> {
    /// 构造空堆；首次分配时初始化 bootstrap。
    pub const fn new() -> Self {
        Self {
            bootstrap: UnsafeCell::new(Aligned([0; N])),
            emergency: UnsafeCell::new(Aligned([0; EMERGENCY_BYTES])),
            state: Mutex::new(HeapState {
                bootstrap: Heap::empty(),
                bootstrap_ready: false,
                emergency: Heap::empty(),
                emergency_ready: false,
                chunks: [const { ChunkSlot::empty() }; MAX_CHUNKS],
                grow: None,
                shrink: None,
            }),
        }
    }

    /// 注册按需增长与归还回调；平台在 System Capacity 建立后调用。
    pub fn set_grower(&self, grow: GrowFn, shrink: ShrinkFn) {
        let mut state = self.state.lock();
        state.grow = Some(grow);
        state.shrink = Some(shrink);
    }

    /// 以 FrameBank 供给的虚拟区间初始化一个固定 chunk。
    ///
    /// 启动期使用这一段完成 System Capacity 建立；后续增长由 `set_grower`
    /// 注册的回调负责。
    pub fn init_dynamic(&self, base: usize, len: usize) {
        let mut state = self.state.lock();
        assert!(state.chunk_count() == 0, "内核主堆重复初始化");
        assert!(state.add_chunk(base, len, false), "内核主堆 chunk 表已满");
    }

    /// 返回主堆已用字节数（不含 bootstrap 与应急堆）。
    pub fn dynamic_used(&self) -> usize {
        self.state.lock().used()
    }

    /// 返回当前主堆 chunk 数。
    pub fn chunk_count(&self) -> usize {
        self.state.lock().chunk_count()
    }

    /// 返回当前主堆 chunk 总容量。
    pub fn capacity(&self) -> usize {
        self.state.lock().capacity()
    }

    fn ensure_bootstrap(&self, state: &mut HeapState) {
        if state.bootstrap_ready {
            return;
        }
        // SAFETY: bootstrap 数组由本结构体独占，且只初始化一次。
        unsafe {
            state.bootstrap.init(self.bootstrap.get() as *mut u8, N);
        }
        state.bootstrap_ready = true;
    }

    fn ensure_emergency(&self, state: &mut HeapState) {
        if state.emergency_ready {
            return;
        }
        // SAFETY: 应急数组由本结构体独占，且只初始化一次。
        unsafe {
            state
                .emergency
                .init(self.emergency.get() as *mut u8, EMERGENCY_BYTES);
        }
        state.emergency_ready = true;
        EMERGENCY_BASE.store(self.emergency.get() as usize, Ordering::Release);
    }

    fn emergency_alloc(&self, state: &mut HeapState, layout: Layout) -> *mut u8 {
        self.ensure_emergency(state);
        state
            .emergency
            .allocate_first_fit(layout)
            .map_or(core::ptr::null_mut(), |pointer| pointer.as_ptr())
    }

    fn grow(&self, layout: Layout) -> bool {
        if GROWING.swap(true, Ordering::AcqRel) {
            return false;
        }
        let (grow, shrink) = {
            let state = self.state.lock();
            (state.grow, state.shrink)
        };
        let result = grow.map(|grow| {
            let minimum = layout.size().max(MIN_CHUNK_BYTES);
            let len = minimum
                .div_ceil(hk_hal::PAGE_SIZE)
                .saturating_mul(hk_hal::PAGE_SIZE);
            grow(len)
        });
        GROWING.store(false, Ordering::Release);
        let Some(Some((base, len))) = result else {
            return false;
        };
        if len < layout.size() || base == 0 {
            if let Some(shrink) = shrink {
                shrink(base, len);
            }
            return false;
        }
        let mut state = self.state.lock();
        if !state.add_chunk(base, len, true) {
            drop(state);
            if let Some(shrink) = shrink {
                shrink(base, len);
            }
            return false;
        }
        true
    }
}

impl<const N: usize> Default for KernelHeap<N> {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: 分配与释放都在同一把自旋锁下进行；增长回调在锁外执行，并通过
// `GROWING` 把回调内的嵌套分配导向应急堆，避免递归增长与死锁。
unsafe impl<const N: usize> GlobalAlloc for KernelHeap<N> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut state = self.state.lock();
        if GROWING.load(Ordering::Acquire) {
            let pointer = self.emergency_alloc(&mut state, layout);
            if !pointer.is_null() {
                // SAFETY: 返回区间由本分配器独占。
                unsafe {
                    core::ptr::write_bytes(pointer, 0, layout.size());
                }
            }
            return pointer;
        }
        if let Some(pointer) = state.chunk_alloc(layout) {
            // SAFETY: 分配器返回独占区间；保持静态堆原有的清零语义。
            unsafe {
                core::ptr::write_bytes(pointer.as_ptr(), 0, layout.size());
            }
            return pointer.as_ptr();
        }
        let grower = state.grow;
        drop(state);
        if grower.is_some() && self.grow(layout) {
            let mut state = self.state.lock();
            if let Some(pointer) = state.chunk_alloc(layout) {
                // SAFETY: 分配器返回独占区间；保持清零语义。
                unsafe {
                    core::ptr::write_bytes(pointer.as_ptr(), 0, layout.size());
                }
                return pointer.as_ptr();
            }
        }
        let mut state = self.state.lock();
        self.ensure_bootstrap(&mut state);
        let pointer = state
            .bootstrap
            .allocate_first_fit(layout)
            .map_or(core::ptr::null_mut(), |pointer| pointer.as_ptr());
        if !pointer.is_null() {
            // SAFETY: 分配器返回独占区间；保持清零语义。
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
        let address = pointer.as_ptr() as usize;
        let mut state = self.state.lock();
        if state.emergency_ready {
            let base = self.emergency.get() as usize;
            if address >= base && address < base.saturating_add(EMERGENCY_BYTES) {
                // SAFETY: 指针来自应急堆，layout 与分配时一致。
                unsafe {
                    state.emergency.deallocate(pointer, layout);
                }
                return;
            }
        }
        if let Some(index) = state.chunk_for(address) {
            let slot = &mut state.chunks[index];
            // SAFETY: 指针来自该 chunk，layout 与分配时一致。
            unsafe {
                slot.heap.deallocate(pointer, layout);
            }
            if slot.releasable && slot.heap.used() == 0 {
                let base = slot.base;
                let len = slot.len;
                slot.active = false;
                slot.base = 0;
                slot.len = 0;
                slot.releasable = false;
                let shrink = state.shrink;
                drop(state);
                if let Some(shrink) = shrink {
                    shrink(base, len);
                }
                return;
            }
            return;
        }
        // SAFETY: 其余指针都来自 bootstrap 区间。
        unsafe {
            state.bootstrap.deallocate(pointer, layout);
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

#[cfg(test)]
mod growth_tests {
    use core::alloc::{GlobalAlloc, Layout};
    use core::cell::UnsafeCell;
    use core::sync::atomic::{AtomicUsize, Ordering};

    use super::KernelHeap;

    #[repr(align(4096))]
    struct Arena(UnsafeCell<[u8; 2 * 1024 * 1024]>);
    // SAFETY: 测试只通过内核堆分配器访问这块内存。
    unsafe impl Sync for Arena {}

    static INITIAL: Arena = Arena(UnsafeCell::new([0; 2 * 1024 * 1024]));
    static GROWN: Arena = Arena(UnsafeCell::new([0; 2 * 1024 * 1024]));
    static SHRUNK: AtomicUsize = AtomicUsize::new(0);
    static HEAP: KernelHeap<64> = KernelHeap::new();

    fn grow(_len: usize) -> Option<(usize, usize)> {
        Some((GROWN.0.get() as *mut u8 as usize, 2 * 1024 * 1024))
    }

    fn shrink(_base: usize, _len: usize) {
        SHRUNK.fetch_add(1, Ordering::AcqRel);
    }

    #[test]
    fn grows_and_returns_idle_chunks() {
        HEAP.init_dynamic(INITIAL.0.get() as *mut u8 as usize, 4096);
        HEAP.set_grower(grow, shrink);
        let layout = Layout::from_size_align(128 * 1024, 8).expect("layout");
        // SAFETY: layout 非零，分配器返回独占区间。
        let pointer = unsafe { HEAP.alloc(layout) };
        assert!(!pointer.is_null());
        let address = pointer as usize;
        assert!(
            (GROWN.0.get() as *mut u8 as usize
                ..GROWN.0.get() as *mut u8 as usize + 2 * 1024 * 1024)
                .contains(&address)
        );
        assert_eq!(HEAP.chunk_count(), 2);
        assert!(HEAP.dynamic_used() >= 128 * 1024);
        // SAFETY: 指针与 layout 与分配时一致。
        unsafe {
            HEAP.dealloc(pointer, layout);
        }
        assert_eq!(HEAP.chunk_count(), 1);
        assert!(SHRUNK.load(Ordering::Acquire) >= 1);
    }
}
