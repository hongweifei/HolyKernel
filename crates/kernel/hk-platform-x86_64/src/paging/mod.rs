//! 内核页表操作。
//!
//! 沿用 Limine 建立的页表，按需为 MMIO 建立 4 KiB 映射：缺失的中间级页表从物理
//! 帧分配器取页并清零，页表本身经 HHDM 访问。
//!
//! 页表项属性与 CoW/恢复策略在 `flags`，页表机制在 `table`，写时复制在 `cow`；
//! 框架策略（页表、CoW、恢复页各从哪个 `FrameKind` 取页）由 `hk-mem::PagePool` 提供。

mod cow;
mod flags;
mod table;

use core::sync::atomic::{AtomicU64, Ordering};

use spin::Mutex;
use x86_64::registers::control::Cr3;

use hk_hal::{PAGE_SIZE, PhysAddr, PhysRange};
use hk_mem::{FrameKind, PagePool};
use hk_paging::FrameRefs;

pub(crate) use flags::map_flags;
pub use flags::{PageFlags, restore_flags};
pub use hk_mem::FrameBank as PageAllocator;
pub use table::{PageTable, PagingError, has_1g_pages, init_cpu};

/// 内核全局物理帧银行。
pub static FRAME_BANK: PageAllocator = PageAllocator::empty();

/// 用户帧的 COW 共享引用。
static FRAME_REFS: Mutex<FrameRefs> = Mutex::new(FrameRefs::new());

/// 把全局帧银行注册为 HMP 的唯一物理后端。
///
/// 之后所有 Capacity 分配都从这里取页并按 `CapacityId` 归因，物理内存不再
/// 有两本并行的账。
pub fn register_backend() {
    hk_mem::hmp::set_physical_backend(&FRAME_BANK);
}

static DIRECT_MAP: AtomicU64 = AtomicU64::new(0);
static KERNEL_ROOT: AtomicU64 = AtomicU64::new(0);

/// 设置页表运行时使用的 direct map 基址，并记录内核页表根。
pub fn init_runtime(direct_map: u64) {
    KERNEL_ROOT.store(read_cr3().get(), Ordering::Release);
    DIRECT_MAP.store(direct_map, Ordering::Release);
}

/// 返回 direct map 基址；未初始化时为 0。
pub fn direct_map_base() -> u64 {
    DIRECT_MAP.load(Ordering::Acquire)
}

/// 把一段物理区间清零；供 Frame 归还 Capacity 前擦除内容。
pub fn scrub_phys(phys: PhysAddr, len: usize) {
    let base = direct_map_base();
    if base == 0 || len == 0 {
        return;
    }
    let pointer = base.saturating_add(phys.get()) as *mut u8;
    // SAFETY: 物理区间经 direct map 可写；调用方保证区间来自 Capacity 且未被映射。
    unsafe {
        core::ptr::write_bytes(pointer, 0, len);
    }
}

/// 判断物理页是否全为零。
pub fn page_is_zero(phys: PhysAddr) -> bool {
    let base = direct_map_base();
    let address = base.saturating_add(phys.get()) as *const u64;
    for word in 0..hk_hal::PAGE_SIZE / size_of::<u64>() {
        // SAFETY: 物理页经 direct map 映射为可读，按 64 位读取不超过一页。
        if unsafe { core::ptr::read_volatile(address.add(word)) } != 0 {
            return false;
        }
    }
    true
}

/// 返回启动时记录的内核页表句柄；与当前 CR3 无关。
pub fn kernel_table() -> PageTable {
    PageTable {
        direct_map: DIRECT_MAP.load(Ordering::Acquire),
        root: PhysAddr::new(KERNEL_ROOT.load(Ordering::Acquire)),
        pcid: 0,
    }
}

/// 取出一个检查点恢复帧。
pub fn take_restore_page() -> Option<PhysAddr> {
    PagePool::new(&FRAME_BANK).restore()
}

/// 归还一个检查点恢复帧。
pub fn give_restore_page(frame: PhysAddr) {
    PagePool::new(&FRAME_BANK).free_restore(frame);
}

/// 解析用户写保护页错误；成功复制页帧时返回 `true`。
pub fn resolve_cow(address: u64) -> bool {
    let direct_map = DIRECT_MAP.load(Ordering::Acquire);
    if direct_map == 0 {
        return false;
    }
    let table = PageTable {
        direct_map,
        root: read_cr3(),
        pcid: 0,
    };
    table.resolve_cow(address)
}

/// 释放地址空间：共享帧只递减引用，最后一个引用持有者归还物理帧。
pub fn release_user(table: &PageTable) {
    let mut refs = FRAME_REFS.lock();
    for frame in table.user_frames() {
        if refs.release(frame) == 0 {
            FRAME_BANK.free_for(FrameKind::Cow, PhysRange::new(frame, PAGE_SIZE as u64));
        }
    }
}

pub(crate) fn frame_refs() -> &'static Mutex<FrameRefs> {
    &FRAME_REFS
}

/// 判断当前页表中地址是否为用户可访问页。
pub fn is_user_mapped(address: u64) -> bool {
    let direct_map = DIRECT_MAP.load(Ordering::Acquire);
    if direct_map == 0 {
        return false;
    }
    let table = PageTable {
        direct_map,
        root: read_cr3(),
        pcid: 0,
    };
    table.is_user_mapped(address)
}

pub(crate) fn read_cr3() -> PhysAddr {
    let root = Cr3::read_raw().0.start_address().as_u64();
    PhysAddr::new(root & table::ADDRESS_MASK)
}
