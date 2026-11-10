//! 内核页表操作。
//!
//! 沿用 Limine 建立的页表，按需为 MMIO 建立 4 KiB 映射：缺失的中间级页表从物理
//! 帧分配器取页并清零，页表本身经 HHDM 访问。
//!
//! 页表项属性与 CoW/恢复策略在 `flags`，页表机制在 `table`，写时复制在 `cow`；
//! 框架策略（页表、CoW、恢复页各从哪个 `FrameKind` 取页）由 `hk-mem::PageAllocator` 提供。

mod cow;
mod flags;
mod table;

use core::sync::atomic::{AtomicU64, Ordering};

use spin::Mutex;
use x86_64::registers::control::Cr3;

use hk_hal::{PAGE_SIZE, PhysAddr, PhysRange};
use hk_mem::{FrameBank, FrameKind, PageAllocator};
use hk_paging::FrameRefs;

pub use flags::{PageFlags, restore_flags};
pub use table::{PageTable, PagingError, has_1g_pages, init_cpu};

/// 内核全局物理帧银行。
pub static FRAME_BANK: FrameBank = FrameBank::empty();

/// 用户帧的 COW 共享引用。
static FRAME_REFS: Mutex<FrameRefs> = Mutex::new(FrameRefs::new());

/// 物理页在用户地址空间中的映射数。
static MAPPING_REFS: Mutex<FrameRefs> = Mutex::new(FrameRefs::new());

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

/// 增加一页的用户映射引用；达到追踪上限时返回 `false`。
pub fn mapping_acquire(phys: PhysAddr) -> bool {
    MAPPING_REFS.lock().acquire(phys)
}

/// 释放一页的用户映射引用，返回剩余映射数。
pub fn mapping_release(phys: PhysAddr) -> u32 {
    MAPPING_REFS.lock().release(phys)
}

/// 返回一页当前的用户映射数。
pub fn mapping_count(phys: PhysAddr) -> u32 {
    MAPPING_REFS.lock().count(phys)
}

/// 判断物理页是否仍被多个地址空间共享。
pub fn is_shared_frame(phys: PhysAddr) -> bool {
    mapping_count(phys) > 1
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
    PageAllocator::new(&FRAME_BANK).restore()
}

/// 归还一个检查点恢复帧。
pub fn give_restore_page(frame: PhysAddr) {
    PageAllocator::new(&FRAME_BANK).free_restore(frame);
}

/// COW 缺页的解析结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CowResolution {
    /// 只剩当前映射，直接恢复写权限。
    Writable,
    /// 复制出新的私有页。
    Copied(PhysAddr),
}

/// 解析用户写保护页错误。
pub fn resolve_cow(address: u64) -> Option<CowResolution> {
    let direct_map = DIRECT_MAP.load(Ordering::Acquire);
    if direct_map == 0 {
        return None;
    }
    let table = PageTable {
        direct_map,
        root: read_cr3(),
        pcid: 0,
    };
    table.resolve_cow(address)
}

/// 释放地址空间：先按 PTE 释放映射与 COW 引用，再回收最后一个映射者。
pub fn release_user(table: &PageTable) {
    let mut tracked = alloc::vec::Vec::new();
    table.user_pages(|_, _, physical| {
        let physical = PhysAddr::new(physical);
        if mapping_count(physical) == 0 {
            // MMU/设备映射没有走用户映射计数，不能按 HMP 页回收。
            return;
        }
        tracked.push(physical);
        {
            let mut refs = FRAME_REFS.lock();
            if refs.count(physical) > 0 {
                let _ = refs.release(physical);
            }
        }
        mapping_release(physical);
    });
    for frame in table.user_frames() {
        let Some(kind) = FRAME_BANK.kind_of(frame) else {
            continue;
        };
        match kind {
            FrameKind::PageTable => {
                FRAME_BANK.free_for(kind, PhysRange::new(frame, PAGE_SIZE as u64));
            }
            FrameKind::Image | FrameKind::Cow | FrameKind::Restore
                if tracked.contains(&frame) && mapping_count(frame) == 0 =>
            {
                FRAME_BANK.free_for(kind, PhysRange::new(frame, PAGE_SIZE as u64));
            }
            // 类型化能力/匿名页由 OwnedFrame 归还；共享时由其他持有者保留。
            _ => {}
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

#[cfg(test)]
mod mapping_tests {
    use super::*;

    #[test]
    fn mapping_refs_count_aliases_and_sharing() {
        let frame = PhysAddr::new(0x1234_5000);
        assert_eq!(mapping_count(frame), 0);
        assert!(mapping_acquire(frame));
        assert!(!is_shared_frame(frame));
        assert!(mapping_acquire(frame));
        assert!(is_shared_frame(frame));
        assert_eq!(mapping_release(frame), 1);
        assert!(!is_shared_frame(frame));
        assert_eq!(mapping_release(frame), 0);
    }
}
