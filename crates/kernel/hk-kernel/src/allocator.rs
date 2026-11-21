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

/// 每块分配尾部的护栏长度。
const GUARD: usize = 8;
/// 护栏魔数；与分配长度混合，便于区分被踩过的块。
const GUARD_MAGIC: u64 = 0xdead_beef_cafe_f00d;

/// 由用户 layout 推导后端堆 layout 与用户指针偏移。
///
/// 头部留出 `align` 字节，使 `base + head` 仍满足用户对齐要求；尾部留 8 字节。
fn guarded_layout(layout: Layout) -> Option<(Layout, usize)> {
    let head = layout.align().max(GUARD);
    let total = layout.size().checked_add(head.checked_add(GUARD)?)?;
    let inner = Layout::from_size_align(total, layout.align()).ok()?;
    Some((inner, head))
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

    /// 调试：尝试读取 `(chunk 数, 容量)`；堆锁被 panic 现场持有时返回 `None`。
    ///
    /// panic 处理器不能用会阻塞的 `chunk_count`：坏释放路径在持有堆锁时 panic，
    /// 再取同一把锁会永久自旋。
    pub fn chunk_stats_try(&self) -> Option<(usize, usize)> {
        let state = self.state.try_lock()?;
        Some((state.chunk_count(), state.capacity()))
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

    /// 分配入口：按应急堆/主堆 chunk/增长/启动堆的顺序取一块并清零。
    ///
    /// 与 [`GlobalAlloc::alloc`] 拆开，是为了让调试期的分配登记发生在锁外，
    /// 报告路径不会重入堆锁。
    unsafe fn alloc_inner(&self, layout: Layout) -> *mut u8 {
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
}

impl<const N: usize> Default for KernelHeap<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// 调试：列出与 `[start, start + len)` 重叠、且不属于 `exclude` 的活动分配。
pub fn allocation_overlaps(
    start: usize,
    len: usize,
    exclude: (usize, usize),
    out: &mut [(usize, usize)],
) -> usize {
    registry::overlaps(start, len, exclude, out)
}

/// 调试：`[start, start + len)` 当前是否仍登记为活动分配。
pub fn is_live_allocation(start: usize, len: usize) -> bool {
    registry::contains(start, len)
}

/// 调试：因大块表满而未登记的次数。
pub fn large_dropped() -> usize {
    registry::large_lost()
}

/// 调试期活动分配登记表。
///
/// 记录每次分配的地址与长度：同一地址再次分配（两个分配者拿到同一块内存）或
/// 释放未登记/长度不符的指针当场报出来。表本身是静态数组，不在分配路径上再
/// 申请内存；调用点位于堆锁之外，报告路径不会重入。
mod registry {
    use core::sync::atomic::{AtomicUsize, Ordering};

    /// 表容量；桌面稳定期的活动分配量低于此值（整表占 2 MiB `.bss`）。
    const SLOTS: usize = 131_072;
    const MASK: usize = SLOTS - 1;
    const EMPTY: usize = usize::MAX;
    const TOMB: usize = usize::MAX - 1;

    #[derive(Clone, Copy)]
    struct Slot {
        address: usize,
        size: usize,
    }

    /// 分配与释放可能来自不同 CPU，登记表本身需要串行化。
    static TABLE: spin::Mutex<[Slot; SLOTS]> = spin::Mutex::new(
        [Slot {
            address: EMPTY,
            size: 0,
        }; SLOTS],
    );
    static LIVE: AtomicUsize = AtomicUsize::new(0);

    /// 大分配（≥ 1 KiB）的区间表。
    ///
    /// 小分配（如 BTree 节点）也可能被分进一个大缓冲区内部；只扫描主表代价太高，
    /// 这里单独记住大块，任意新分配都先与它们比对。容量必须覆盖活动大块的峰值，
    /// 否则目标任务表缓冲区可能没被登记、重叠检查就会漏掉。
    const LARGE: usize = 4096;
    const LARGE_MIN: usize = 1024;

    struct LargeSlots {
        len: usize,
        slots: [(usize, usize); LARGE],
    }

    impl LargeSlots {
        const fn new() -> Self {
            Self {
                len: 0,
                slots: [(0, 0); LARGE],
            }
        }

        fn overlaps(&self, address: usize, size: usize) -> Option<(usize, usize)> {
            let end = address.saturating_add(size);
            self.slots[..self.len]
                .iter()
                .copied()
                .find(|(start, len)| {
                    let slot_end = start.saturating_add(*len);
                    *start < end && address < slot_end
                })
        }

        fn insert(&mut self, address: usize, size: usize) {
            if self.len < LARGE {
                self.slots[self.len] = (address, size);
                self.len += 1;
            } else {
                LARGE_LOST.fetch_add(1, Ordering::Relaxed);
            }
        }

        fn remove(&mut self, address: usize, size: usize) {
            if let Some(position) = self.slots[..self.len]
                .iter()
                .position(|&(start, len)| start == address && len == size)
            {
                let last = self.len - 1;
                self.slots[position] = self.slots[last];
                self.len = last;
            }
        }
    }

    static LARGE_TABLE: spin::Mutex<LargeSlots> = spin::Mutex::new(LargeSlots::new());
    /// 因大块表已满而漏登记的次数；非零说明重叠检测存在盲区。
    static LARGE_LOST: AtomicUsize = AtomicUsize::new(0);

    /// 返回因表满而漏登记的大块次数。
    pub(super) fn large_lost() -> usize {
        LARGE_LOST.load(Ordering::Relaxed)
    }

    fn hash(address: usize) -> usize {
        (address.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 20) & MASK
    }

    /// 登记一次分配。
    pub(super) fn record(address: usize, size: usize) {
        let mut large = LARGE_TABLE.lock();
        let mut table = TABLE.lock();
        // 任意分配都先与已登记的大块比对：小 BTree 节点被分进大 Vec 内部是
        // 最常见的“看不见”的重叠。
        if let Some((start, len)) = large.overlaps(address, size) {
            panic!("内核堆分配重叠：{address:#018x}+{size:#x} 与 {start:#018x}+{len:#x}");
        }
        // 大块分配还要与全部活动分配比对：它可能压在更小、更早的分配上。
        if size >= LARGE_MIN {
            let end = address.saturating_add(size);
            for slot in table.iter() {
                if slot.address == EMPTY || slot.address == TOMB {
                    continue;
                }
                let slot_end = slot.address.saturating_add(slot.size);
                if slot.address < end && address < slot_end {
                    panic!(
                        "内核堆分配重叠：{address:#018x}+{size:#x} 与 {:#018x}+{:#x}",
                        slot.address, slot.size
                    );
                }
            }
        }
        let mut index = hash(address);
        let mut tomb = None;
        for _ in 0..SLOTS {
            let slot = table[index];
            if slot.address == address {
                panic!(
                    "内核堆重复分配：{address:#018x} 已登记 {:#x} 字节，本次 {size:#x} 字节",
                    slot.size
                );
            }
            if slot.address == TOMB {
                tomb.get_or_insert(index);
            } else if slot.address == EMPTY {
                let at = tomb.unwrap_or(index);
                table[at] = Slot { address, size };
                LIVE.fetch_add(1, Ordering::Relaxed);
                if size >= LARGE_MIN {
                    large.insert(address, size);
                }
                return;
            }
            index = (index + 1) & MASK;
        }
        if let Some(at) = tomb {
            table[at] = Slot { address, size };
            LIVE.fetch_add(1, Ordering::Relaxed);
            if size >= LARGE_MIN {
                large.insert(address, size);
            }
            return;
        }
        panic!(
            "内核堆登记表已满（{} 个活动分配）",
            LIVE.load(Ordering::Relaxed)
        );
    }

    /// 注销一次释放；未登记或长度不一致说明分配器状态已被破坏。
    pub(super) fn forget(address: usize, size: usize) {
        let mut large = LARGE_TABLE.lock();
        let mut table = TABLE.lock();
        let mut index = hash(address);
        for _ in 0..SLOTS {
            let slot = table[index];
            if slot.address == EMPTY {
                break;
            }
            if slot.address == address {
                if slot.size != size {
                    panic!(
                        "内核堆释放长度不一致：{address:#018x} 分配 {:#x} 字节，释放 {size:#x} 字节",
                        slot.size
                    );
                }
                table[index].address = TOMB;
                if size >= LARGE_MIN {
                    large.remove(address, size);
                }
                LIVE.fetch_sub(1, Ordering::Relaxed);
                return;
            }
            index = (index + 1) & MASK;
        }
        panic!("内核堆释放了未登记的指针：{address:#018x}（{size:#x} 字节）");
    }

    /// 查询某个区间当前是否仍登记为活动分配。
    ///
    /// 用于验证“长期存活的大缓冲区”没有被提前释放、被其它分配复用。
    pub(super) fn contains(address: usize, size: usize) -> bool {
        // 该检查会在中断/抢占上下文运行；登记表锁可能被本 CPU 上被中断的
        // 分配路径持有，不能在这里阻塞。拿不到锁时按“未知”处理，不误报。
        let Some(table) = TABLE.try_lock() else {
            return true;
        };
        let mut index = hash(address);
        for _ in 0..SLOTS {
            let slot = table[index];
            if slot.address == EMPTY {
                return false;
            }
            if slot.address == address {
                return slot.size == size;
            }
            index = (index + 1) & MASK;
        }
        false
    }

    /// 查找本块前后 64 字节窗口内的其它活动分配。
    ///
    /// 护栏被踩时用来指认越界者：紧邻前一块会从自己的尾护栏越界到本块前护栏，
    /// 紧邻后一块则可能与本块区间重叠。
    pub(super) fn neighbors(
        user: usize,
        size: usize,
        head: usize,
        out: &mut [(usize, usize); 8],
    ) -> usize {
        let table = TABLE.lock();
        let inner_start = user.saturating_sub(head);
        let inner_end = user.saturating_add(size).saturating_add(super::GUARD);
        let low = inner_start.saturating_sub(64);
        let high = inner_end.saturating_add(64);
        let mut count = 0;
        for slot in table.iter() {
            if slot.address == EMPTY || slot.address == TOMB {
                continue;
            }
            if slot.address == user && slot.size == size {
                continue;
            }
            let end = slot.address.saturating_add(slot.size);
            if slot.address < high && low < end {
                if count < out.len() {
                    out[count] = (slot.address, slot.size);
                }
                count += 1;
            }
        }
        count
    }

    /// 找到紧邻本块之前的分配，并读出它的尾护栏字。
    ///
    /// 若它越界写进本块前护栏，它的尾护栏也一定已被踩坏——两者一起足以定案。
    pub(super) fn preceding(user: usize, head: usize) -> Option<(usize, usize, u64)> {
        let table = TABLE.lock();
        let inner_start = user.checked_sub(head)?;
        let mut best: Option<(usize, usize)> = None;
        for slot in table.iter() {
            if slot.address == EMPTY || slot.address == TOMB || slot.address == 0 {
                continue;
            }
            let end = slot.address.saturating_add(slot.size);
            // 前一块的 inner 结束于 `end + GUARD`，其 user 结束于 `end`。
            if end.saturating_add(super::GUARD) > inner_start {
                continue;
            }
            match best {
                Some((address, _)) if address >= slot.address => {}
                _ => best = Some((slot.address, slot.size)),
            }
        }
        let (address, size) = best?;
        // 前一块的尾护栏位于它的 user 结束处。
        let guard = unsafe { core::ptr::read_unaligned((address + size) as *const u64) };
        Some((address, size, guard))
    }

    /// 列出与 `[start, start + len)` 重叠的其它活动分配。
    ///
    /// `exclude` 是所有者为自己的那条记录 (起始地址, 长度)，用于排除被检查区间
    /// 本身。返回重叠条数，最多写入 `out.len()` 条。只在报告路径调用。
    pub(super) fn overlaps(
        start: usize,
        len: usize,
        exclude: (usize, usize),
        out: &mut [(usize, usize)],
    ) -> usize {
        // 只在报告路径调用；锁被本 CPU 的中断上下文持有时放弃，避免死锁。
        let Some(table) = TABLE.try_lock() else {
            return 0;
        };
        let end = start.saturating_add(len);
        let mut count = 0;
        for slot in table.iter() {
            if slot.address == EMPTY || slot.address == TOMB {
                continue;
            }
            if (slot.address, slot.size) == exclude {
                continue;
            }
            let slot_end = slot.address.saturating_add(slot.size);
            if slot.address < end && start < slot_end {
                if count < out.len() {
                    out[count] = (slot.address, slot.size);
                }
                count += 1;
            }
        }
        count
    }
}

/// 调试：保留最近若干次释放的地址/长度与调用者返回地址。
///
/// 坏释放（或节点被提前归还）之后要等到很久以后才以空洞别名/tasks 尾部损坏
/// 出现；这里在释放现场留下调用链，panic 时直接打出来。
pub mod free_log {
    use core::sync::atomic::{AtomicUsize, Ordering};

    /// 记录条数。
    pub const SLOTS: usize = 64;
    /// 每条记录的字段数：地址、长度、三个返回地址。
    pub const FIELDS: usize = 5;

    static LOG: spin::Mutex<[[u64; FIELDS]; SLOTS]> = spin::Mutex::new([[0; FIELDS]; SLOTS]);
    static INDEX: AtomicUsize = AtomicUsize::new(0);

    /// 记录一次释放；`rets` 是沿帧指针找到的调用者返回地址。
    pub(super) fn record(address: usize, size: usize, rets: [u64; 3]) {
        let mut log = LOG.lock();
        let slot = INDEX.fetch_add(1, Ordering::Relaxed) % SLOTS;
        log[slot] = [
            address as u64,
            size as u64,
            rets[0],
            rets[1],
            rets[2],
        ];
    }

    /// 返回最近 `count` 条释放记录（新的在前）。
    pub fn recent(count: usize) -> [[u64; FIELDS]; SLOTS] {
        let log = LOG.lock();
        let total = INDEX.load(Ordering::Relaxed);
        let mut out = [[0_u64; FIELDS]; SLOTS];
        for i in 0..count.min(SLOTS).min(total) {
            out[i] = log[(total - 1 - i) % SLOTS];
        }
        out
    }

    /// 查找某个地址最近一次释放记录（含调用者返回地址）。
    pub fn find(address: usize) -> Option<[u64; FIELDS]> {
        let log = LOG.lock();
        let total = INDEX.load(Ordering::Relaxed);
        for i in 0..SLOTS.min(total) {
            let record = log[(total - 1 - i) % SLOTS];
            if record[0] == address as u64 {
                return Some(record);
            }
        }
        None
    }
}

/// 调试：保留最近若干次大块分配的地址/长度与调用者返回地址。
///
/// 护栏被踩时用它指出“是哪个分配长出来的”，比只报地址更有用。
pub mod alloc_log {
    use core::sync::atomic::{AtomicUsize, Ordering};

    /// 记录条数。
    pub const SLOTS: usize = 64;
    /// 每条记录的字段数：地址、长度、三个返回地址。
    pub const FIELDS: usize = 5;

    static LOG: spin::Mutex<[[u64; FIELDS]; SLOTS]> = spin::Mutex::new([[0; FIELDS]; SLOTS]);
    static INDEX: AtomicUsize = AtomicUsize::new(0);

    /// 记录一次分配；`rets` 是沿帧指针找到的调用者返回地址。
    pub(super) fn record(address: usize, size: usize, rets: [u64; 3]) {
        let mut log = LOG.lock();
        let slot = INDEX.fetch_add(1, Ordering::Relaxed) % SLOTS;
        log[slot] = [
            address as u64,
            size as u64,
            rets[0],
            rets[1],
            rets[2],
        ];
    }

    /// 返回最近 `count` 条分配记录（新的在前）。
    pub fn recent(count: usize) -> [[u64; FIELDS]; SLOTS] {
        let log = LOG.lock();
        let total = INDEX.load(Ordering::Relaxed);
        let mut out = [[0_u64; FIELDS]; SLOTS];
        for i in 0..count.min(SLOTS).min(total) {
            out[i] = log[(total - 1 - i) % SLOTS];
        }
        out
    }

    /// 查找某个地址最近一次分配记录的索引。
    pub fn find(address: usize) -> Option<[u64; FIELDS]> {
        let log = LOG.lock();
        let total = INDEX.load(Ordering::Relaxed);
        for i in 0..SLOTS.min(total) {
            let record = log[(total - 1 - i) % SLOTS];
            if record[0] == address as u64 {
                return Some(record);
            }
        }
        None
    }
}

/// 沿当前帧指针收集调用者返回地址（最多 3 个）。
fn caller_rets() -> [u64; 3] {
    const LOW: u64 = 0xffff_ffff_8000_0000;
    const HIGH: u64 = 0xffff_ffff_c000_0000;
    let mut rets = [0_u64; 3];
    let mut frame: u64;
    // SAFETY: 只读取当前帧指针。
    unsafe {
        core::arch::asm!(
            "mov {}, rbp",
            out(reg) frame,
            options(nomem, nostack, preserves_flags)
        );
    }
    let mut slot = 0;
    let mut cursor = frame;
    for _ in 0..6 {
        if !(LOW..HIGH).contains(&cursor) {
            break;
        }
        // SAFETY: 帧指针落在内核镜像范围内，两个槽位可读。
        let (next, ret) = unsafe {
            (
                core::ptr::read_unaligned(cursor as *const u64),
                core::ptr::read_unaligned((cursor + 8) as *const u64),
            )
        };
        if ret != 0 && slot < rets.len() && (LOW..HIGH).contains(&ret) {
            rets[slot] = ret;
            slot += 1;
        }
        if next <= cursor {
            break;
        }
        cursor = next;
    }
    rets
}

// SAFETY: 分配与释放都在同一把自旋锁下进行；增长回调在锁外执行，并通过
// `GROWING` 把回调内的嵌套分配导向应急堆，避免递归增长与死锁。
unsafe impl<const N: usize> GlobalAlloc for KernelHeap<N> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let Some((inner, head)) = guarded_layout(layout) else {
            return core::ptr::null_mut();
        };
        // SAFETY: 转发给内部实现。
        let base = unsafe { self.alloc_inner(inner) };
        if base.is_null() {
            return base;
        }
        let magic = GUARD_MAGIC ^ layout.size() as u64;
        // SAFETY: inner 区间由本分配独占，两个护栏各占 8 字节。
        unsafe {
            core::ptr::write_unaligned(base as *mut u64, magic);
            core::ptr::write_unaligned(base.add(head + layout.size()) as *mut u64, magic);
        }
        let user = unsafe { base.add(head) };
        if layout.size() >= 128 {
            alloc_log::record(user as usize, layout.size(), caller_rets());
        }
        registry::record(user as usize, layout.size());
        user
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let Some(user) = NonNull::new(pointer) else {
            return;
        };
        let Some((inner, head)) = guarded_layout(layout) else {
            return;
        };
        let base = unsafe { user.as_ptr().sub(head) };
        let magic = GUARD_MAGIC ^ layout.size() as u64;
        // SAFETY: 护栏在分配时写入，位置由 layout 唯一确定。
        let (front, back) = unsafe {
            (
                core::ptr::read_unaligned(base as *const u64),
                core::ptr::read_unaligned(user.as_ptr().add(layout.size()) as *const u64),
            )
        };
        if front != magic || back != magic {
            let mut out = [(0_usize, 0_usize); 8];
            let count = registry::neighbors(
                user.as_ptr() as usize,
                layout.size(),
                head,
                &mut out,
            );
            let prev = registry::preceding(user.as_ptr() as usize, head);
            // 同一地址的上一次释放：若是悬空指针二次释放，这里就是它的调用链。
            let previous_free = free_log::find(user.as_ptr() as usize);
            // SAFETY: 本块仍是活动分配，头部若干字可读（可能落在尾护栏/填充内）。
            let own = unsafe {
                [
                    core::ptr::read_unaligned(user.as_ptr() as *const u64),
                    core::ptr::read_unaligned(user.as_ptr().add(8) as *const u64),
                    core::ptr::read_unaligned(user.as_ptr().add(16) as *const u64),
                    core::ptr::read_unaligned(user.as_ptr().add(24) as *const u64),
                ]
            };
            panic!(
                "内核堆护栏被踩：块={:#018x}+{:#x} 前={:#018x} 后={:#018x} 期望={:#018x} 前邻={:?} 邻块[{}]={:?} 本块字={:?} 上次释放={:?}",
                user.as_ptr() as usize,
                layout.size(),
                front,
                back,
                magic,
                prev,
                count,
                &out[..count.min(out.len())],
                own,
                previous_free
            );
        }
        // 记录所有非平凡释放：受害块常见是 32 字节，阈值不能抬高。
        if layout.size() >= 16 {
            free_log::record(user.as_ptr() as usize, layout.size(), caller_rets());
        }
        registry::forget(user.as_ptr() as usize, layout.size());
        let address = base as usize;
        let mut state = self.state.lock();
        if state.emergency_ready {
            let base = self.emergency.get() as usize;
            if address >= base && address < base.saturating_add(EMERGENCY_BYTES) {
                // SAFETY: 指针来自应急堆，layout 与分配时一致。
                unsafe {
                    state
                        .emergency
                        .deallocate(NonNull::new_unchecked(address as *mut u8), inner);
                }
                return;
            }
        }
        if let Some(index) = state.chunk_for(address) {
            let slot = &mut state.chunks[index];
            // SAFETY: 指针来自该 chunk，layout 与分配时一致。
            unsafe {
                slot.heap
                    .deallocate(NonNull::new_unchecked(address as *mut u8), inner);
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
        // 释放的指针必须来自 bootstrap 区间；指向已归还 chunk 的悬空引用若继续
        // 交给 bootstrap，会把外来地址插进它的空闲表，之后再释放同一地址就会
        // 变成更难定位的坏释放。
        let bootstrap_base = self.bootstrap.get() as usize;
        assert!(
            address >= bootstrap_base && address < bootstrap_base.saturating_add(N),
            "内核堆释放了不属于堆的指针：{address:#018x} len={} bootstrap={bootstrap_base:#018x}",
            layout.size()
        );
        // SAFETY: 指针来自 bootstrap 区间，layout 与分配时一致。
        unsafe {
            state
                .bootstrap
                .deallocate(NonNull::new_unchecked(address as *mut u8), inner);
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
