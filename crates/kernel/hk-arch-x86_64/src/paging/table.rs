//! 页表机制：创建、遍历、映射与解除映射。
//!
//! 句柄与页表创建在此；映射与解除映射在 `map`，遍历、翻译与内核高半区登记在
//! `walk`；页表项操作的单元测试在 `tests`。

use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};

#[cfg(not(test))]
use x86_64::instructions::tlb;
use x86_64::registers::control::Cr4;

use hk_hal::{PAGE_SIZE, PhysAddr};
use hk_mem::{FrameBank, FrameKind};

use super::{FRAME_BANK, read_cr3};

mod map;
mod walk;

#[cfg(test)]
mod tests;

#[cfg(not(test))]
unsafe extern "C" {
    fn hk_invpcid_single(pcid: u64);
}

pub(crate) const ADDRESS_MASK: u64 = 0x000F_FFFF_FFFF_F000;
pub(crate) const ENTRIES: usize = 512;
pub(crate) const KERNEL_PML4_START: usize = 256;

/// PCID 已启用且 INVPCID 可用。
static PCID_ENABLED: AtomicBool = AtomicBool::new(false);
/// 下一个可用的 PCID；0 保留给内核地址空间。
static NEXT_PCID: AtomicU16 = AtomicU16::new(1);
/// CPU 是否支持 1 GiB 大页。
static GB_PAGES: AtomicBool = AtomicBool::new(false);
/// PCID 分配溢出计数，用于在回绕时刷新全部 TLB。
static PCID_WRAPS: AtomicU32 = AtomicU32::new(0);

/// 启用当前 CPU 的 PCID，并记录大页能力。
pub fn init_cpu() {
    let features = crate::cpu::Features::detect();
    GB_PAGES.store(features.gb_pages, Ordering::Release);
    let invpcid = core::arch::x86_64::__cpuid_count(7, 0).ebx & (1 << 10) != 0;
    if features.pcid && invpcid {
        PCID_ENABLED.store(true, Ordering::Release);
        // SAFETY: CPUID 已确认 PCID；CR3 当前 PCID 为 0，可安全置位。
        unsafe {
            Cr4::write_raw(Cr4::read_raw() | (1 << 17));
        }
    }
}

/// 是否支持 1 GiB 大页。
pub fn has_1g_pages() -> bool {
    GB_PAGES.load(Ordering::Acquire)
}

pub(super) fn allocate_pcid() -> u16 {
    if !PCID_ENABLED.load(Ordering::Acquire) {
        return 0;
    }
    let next = NEXT_PCID.fetch_add(1, Ordering::Relaxed);
    let pcid = next & 0x0FFF;
    if pcid == 0 {
        // 回绕会复用旧 PCID；先刷新全部 TLB 再继续。
        PCID_WRAPS.fetch_add(1, Ordering::Relaxed);
        flush_all_pcids();
        1
    } else {
        pcid
    }
}

fn flush_all_pcids() {
    if !PCID_ENABLED.load(Ordering::Acquire) {
        return;
    }
    // 清 PCIDE 会刷新全部 TLB；清前先把 CR3 的 PCID 归零。
    let root = read_cr3().get();
    // SAFETY: 写 CR3/CR4 只影响当前 CPU，且 CPUID 已确认 PCID。
    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) root, options(nostack, preserves_flags));
        let cr4 = Cr4::read_raw();
        Cr4::write_raw(cr4 & !(1 << 17));
        Cr4::write_raw(cr4);
    }
}

fn current_pcid() -> u16 {
    if !PCID_ENABLED.load(Ordering::Acquire) {
        return 0;
    }
    let raw: u64;
    // SAFETY: 读取 CR3 不改变机器状态。
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) raw, options(nostack, preserves_flags)) };
    (raw & 0x0FFF) as u16
}

/// 页表操作错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PagingError {
    /// 没有可用的物理内存区域。
    NoUsableRegion,
    /// 物理帧耗尽。
    OutOfFrames,
    /// 地址未按页对齐。
    Unaligned,
    /// 命中了更大的映射，需要先拆分。
    HugePageConflict {
        /// 页表级别。
        level: u32,
        /// 冲突的虚拟地址。
        address: u64,
    },
    /// 平台或调用方式不支持该操作。
    Unsupported,
}

/// 页表操作句柄。
pub struct PageTable {
    pub(crate) direct_map: u64,
    pub(crate) root: PhysAddr,
    pub(crate) pcid: u16,
}

impl PageTable {
    /// 读取 CR3 建立句柄。
    pub fn new(direct_map: u64) -> Self {
        Self {
            direct_map,
            root: read_cr3(),
            pcid: current_pcid(),
        }
    }

    /// 返回页表根物理地址。
    pub fn root(&self) -> PhysAddr {
        self.root
    }

    /// 返回同一页表根的共享句柄；用于同地址空间的新线程。
    pub fn share(&self) -> Self {
        Self {
            direct_map: self.direct_map,
            root: self.root,
            pcid: self.pcid,
        }
    }

    /// 以当前页表为模板创建用户页表：复制内核高半区映射，低半区留空。
    pub fn create_user(&self, allocator: &FrameBank) -> Result<Self, PagingError> {
        self.create_user_with(|| allocator.allocate_for(FrameKind::PageTable))
    }

    /// 分配一页页表并登记进 System Arena。
    ///
    /// 页表帧无论服务于内核高半区还是用户低半区，都属于内核自用物理内存；
    /// 登记后 `SYS_SYSTEM_ARENA_*` 能看到它们，归还时由 `release_user` 移除。
    fn kernel_page_table(&self, _virt: u64) -> Option<PhysAddr> {
        let frame = FRAME_BANK.allocate_for(FrameKind::PageTable)?;
        crate::system_arena::register_kernel_page(frame, self.direct_map);
        Some(frame)
    }

    /// 以指定分配器创建用户页表；中间页表也由 `allocate` 提供。
    pub fn create_user_with<F>(&self, mut allocate: F) -> Result<Self, PagingError>
    where
        F: FnMut() -> Option<PhysAddr>,
    {
        let root = allocate().ok_or(PagingError::OutOfFrames)?;
        self.zero_page(root);
        // 用户页表根同样由内核分配并维护，登记进 System Arena。
        crate::system_arena::register_kernel_page(root, self.direct_map);

        for index in KERNEL_PML4_START..ENTRIES {
            // SAFETY: 两侧 PML4 均经 HHDM 映射，索引在 0..512 范围内。
            let value = unsafe { self.entry(self.root, index).read_volatile() };
            // SAFETY: 目标 PML4 已清零且经 HHDM 可写，索引在 0..512 范围内。
            unsafe {
                self.entry(root, index).write_volatile(value);
            }
        }

        Ok(Self {
            direct_map: self.direct_map,
            root,
            pcid: allocate_pcid(),
        })
    }

    /// 从物理帧读取一段字节；帧经 direct map 访问。
    pub fn read_phys(&self, phys: PhysAddr, out: &mut [u8]) {
        let source = (self.direct_map + phys.get()) as *const u8;
        // SAFETY: 物理帧位于 direct map 内，调用方保证长度不超过一页。
        unsafe {
            core::ptr::copy_nonoverlapping(source, out.as_mut_ptr(), out.len());
        }
    }

    /// 向物理帧写入一段字节；帧经 direct map 访问。
    pub fn write_phys(&self, phys: PhysAddr, data: &[u8]) {
        let target = (self.direct_map + phys.get()) as *mut u8;
        // SAFETY: 物理帧位于 direct map 内，调用方保证长度不超过一页。
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), target, data.len());
        }
    }

    /// 清空一个物理帧。
    pub fn zero_phys(&self, phys: PhysAddr) {
        let target = (self.direct_map + phys.get()) as *mut u8;
        // SAFETY: 物理帧位于 direct map 内，长度为整页。
        unsafe {
            core::ptr::write_bytes(target, 0, PAGE_SIZE);
        }
    }

    /// 切换到该页表。
    pub fn activate(&self) {
        let pcid = if PCID_ENABLED.load(Ordering::Acquire) {
            u64::from(self.pcid & 0x0FFF)
        } else {
            0
        };
        let value = self.root.get() | pcid;
        // SAFETY: 根物理地址来自页表或帧分配器；PCID 仅在 CPUID 支持时非零。
        // 写 CR3 只影响当前 CPU。
        unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) value, options(nostack, preserves_flags));
        }
    }

    /// 刷新该地址空间中的地址；PCID 模式下用 INVPCID 定向失效。
    pub(crate) fn flush(&self, address: u64) {
        // 宿主测试进程不能执行 invlpg/invpcid 等特权指令。
        #[cfg(test)]
        {
            let _ = (address, self.pcid);
        }
        #[cfg(not(test))]
        if PCID_ENABLED.load(Ordering::Acquire) && self.pcid != 0 {
            // SAFETY: CPUID 已确认 PCID 与 INVPCID；单上下文失效只影响该 PCID。
            unsafe { hk_invpcid_single(u64::from(self.pcid)) };
        } else {
            tlb::flush(x86_64::VirtAddr::new(address));
        }
    }

    pub(crate) fn entry(&self, table: PhysAddr, index: usize) -> *mut u64 {
        debug_assert!(index < ENTRIES);
        ((self.direct_map + table.get()) as *mut u64).wrapping_add(index)
    }

    pub(crate) fn zero_page(&self, frame: PhysAddr) {
        let pointer = (self.direct_map + frame.get()) as *mut u8;
        // SAFETY: 新分配的帧来自可用内存区域，经 HHDM 可写。
        unsafe {
            core::ptr::write_bytes(pointer, 0, PAGE_SIZE);
        }
    }
}
