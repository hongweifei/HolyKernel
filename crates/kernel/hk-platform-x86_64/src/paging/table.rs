//! 页表机制：创建、遍历、映射与解除映射。

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};

#[cfg(not(test))]
use x86_64::instructions::tlb;
use x86_64::registers::control::Cr4;

use hk_hal::{PAGE_SIZE, PhysAddr, VirtAddr};
use hk_mem::FrameKind;
use hk_mem::page::PagePool;

use super::flags::{FLAGS_MASK, HUGE, PCD, PRESENT, PWT, PageFlags, USER, WRITABLE};
use super::{FRAME_BANK, PageAllocator, read_cr3};

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
    pub fn create_user(&self, allocator: &PageAllocator) -> Result<Self, PagingError> {
        self.create_user_with(|| allocator.allocate_for(FrameKind::PageTable))
    }

    /// 以指定分配器创建用户页表；中间页表也由 `allocate` 提供。
    pub fn create_user_with<F>(&self, mut allocate: F) -> Result<Self, PagingError>
    where
        F: FnMut() -> Option<PhysAddr>,
    {
        let root = allocate().ok_or(PagingError::OutOfFrames)?;
        self.zero_page(root);

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

    /// 遍历低半区的 4 KiB 用户页；大页展开为 4 KiB，设备（PWT/PCD）页跳过。
    pub fn user_pages<F>(&self, mut visit: F)
    where
        F: FnMut(u64, u64, u64),
    {
        for pml4 in 0..KERNEL_PML4_START {
            // SAFETY: PML4 经 HHDM 映射，索引在 0..512 范围内。
            let pml4e = unsafe { self.entry(self.root, pml4).read_volatile() };
            if pml4e & PRESENT == 0 || pml4e & USER == 0 {
                continue;
            }
            let pdpt = PhysAddr::new(pml4e & ADDRESS_MASK);
            for pdpt_index in 0..ENTRIES {
                // SAFETY: PDPT 是已存在的页表帧，经 HHDM 映射。
                let pdp = unsafe { self.entry(pdpt, pdpt_index).read_volatile() };
                if pdp & PRESENT == 0 || pdp & USER == 0 {
                    continue;
                }
                let pdpt_virt = ((pml4 as u64) << 39) | ((pdpt_index as u64) << 30);
                if pdp & HUGE != 0 {
                    if pdp & (PWT | PCD) != 0 {
                        continue;
                    }
                    let base = pdp & ADDRESS_MASK;
                    for pd_index in 0..ENTRIES {
                        for pt_index in 0..ENTRIES {
                            let virt =
                                pdpt_virt | ((pd_index as u64) << 21) | ((pt_index as u64) << 12);
                            let phys = base + ((pd_index as u64) << 21) + ((pt_index as u64) << 12);
                            visit(virt, pdp & FLAGS_MASK, phys);
                        }
                    }
                    continue;
                }
                let pd = PhysAddr::new(pdp & ADDRESS_MASK);
                for pd_index in 0..ENTRIES {
                    // SAFETY: PD 是已存在的页表帧，经 HHDM 映射。
                    let pde = unsafe { self.entry(pd, pd_index).read_volatile() };
                    if pde & PRESENT == 0 || pde & USER == 0 {
                        continue;
                    }
                    let pd_virt = pdpt_virt | ((pd_index as u64) << 21);
                    if pde & HUGE != 0 {
                        if pde & (PWT | PCD) != 0 {
                            continue;
                        }
                        let base = pde & ADDRESS_MASK;
                        for pt_index in 0..ENTRIES {
                            let virt = pd_virt | ((pt_index as u64) << 12);
                            let phys = base + ((pt_index as u64) << 12);
                            visit(virt, pde & FLAGS_MASK, phys);
                        }
                        continue;
                    }
                    let pt = PhysAddr::new(pde & ADDRESS_MASK);
                    for pt_index in 0..ENTRIES {
                        // SAFETY: PT 是已存在的页表帧，经 HHDM 映射。
                        let pte = unsafe { self.entry(pt, pt_index).read_volatile() };
                        if pte & PRESENT == 0 || pte & USER == 0 {
                            continue;
                        }
                        if pte & (PWT | PCD) != 0 {
                            continue;
                        }
                        let virt = pd_virt | ((pt_index as u64) << 12);
                        visit(virt, pte & FLAGS_MASK, pte & ADDRESS_MASK);
                    }
                }
            }
        }
    }

    /// 收集低半区用户页表与数据帧，供检查点释放或复用。
    pub fn user_frames(&self) -> Vec<PhysAddr> {
        let mut frames = Vec::new();
        frames.push(self.root);
        for pml4 in 0..KERNEL_PML4_START {
            // SAFETY: PML4 经 HHDM 映射，索引在 0..512 范围内。
            let pml4e = unsafe { self.entry(self.root, pml4).read_volatile() };
            if pml4e & PRESENT == 0 {
                continue;
            }
            let pdpt = PhysAddr::new(pml4e & ADDRESS_MASK);
            frames.push(pdpt);
            for pdpt_index in 0..ENTRIES {
                // SAFETY: PDPT 是已存在的页表帧，经 HHDM 映射。
                let pdp = unsafe { self.entry(pdpt, pdpt_index).read_volatile() };
                if pdp & PRESENT == 0 {
                    continue;
                }
                let pd = PhysAddr::new(pdp & ADDRESS_MASK);
                frames.push(pd);
                for pd_index in 0..ENTRIES {
                    // SAFETY: PD 是已存在的页表帧，经 HHDM 映射。
                    let pde = unsafe { self.entry(pd, pd_index).read_volatile() };
                    if pde & PRESENT == 0 || pde & HUGE != 0 {
                        continue;
                    }
                    let pt = PhysAddr::new(pde & ADDRESS_MASK);
                    frames.push(pt);
                    for pt_index in 0..ENTRIES {
                        // SAFETY: PT 是已存在的页表帧，经 HHDM 映射。
                        let pte = unsafe { self.entry(pt, pt_index).read_volatile() };
                        if pte & PRESENT == 0 || pte & (PWT | PCD) != 0 {
                            continue;
                        }
                        frames.push(PhysAddr::new(pte & ADDRESS_MASK));
                    }
                }
            }
        }
        frames.sort_unstable();
        frames.dedup();
        frames
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

    pub(super) fn split_huge_at(&self, address: u64) -> bool {
        let mut allocate = || PagePool::new(&FRAME_BANK).page_table();
        self.split_huge_at_with(address, &mut allocate).is_ok()
    }

    /// 判断地址是否映射为用户可访问页；大页直接视为已映射。
    pub fn is_user_mapped(&self, address: u64) -> bool {
        let mut table = self.root;
        for level in [39_u32, 30, 21] {
            let index = ((address >> level) & 0x1FF) as usize;
            // SAFETY: 页表项经 HHDM 映射，索引在 0..512 范围内。
            let value = unsafe { self.entry(table, index).read_volatile() };
            if value & PRESENT == 0 || value & USER == 0 {
                return false;
            }
            if value & HUGE != 0 {
                return true;
            }
            table = PhysAddr::new(value & ADDRESS_MASK);
        }
        let index = ((address >> 12) & 0x1FF) as usize;
        // SAFETY: 末级页表项经 HHDM 映射，索引在 0..512 范围内。
        let value = unsafe { self.entry(table, index).read_volatile() };
        value & PRESENT != 0 && value & USER != 0
    }

    /// 查询虚拟地址对应的物理地址与原始页表属性。
    pub fn translate(&self, virt: VirtAddr) -> Option<(PhysAddr, u64)> {
        let mut table = self.root;
        for level in [39_u32, 30, 21] {
            let index = ((virt.get() >> level) & 0x1FF) as usize;
            // SAFETY: 页表项经 HHDM 映射，索引在 0..512 范围内。
            let value = unsafe { self.entry(table, index).read_volatile() };
            if value & PRESENT == 0 {
                return None;
            }
            if value & HUGE != 0 {
                let size = match level {
                    30 => 1_u64 << 30,
                    21 => 1_u64 << 21,
                    _ => return None,
                };
                let phys = (value & ADDRESS_MASK) + (virt.get() & (size - 1));
                return Some((PhysAddr::new(phys), value & FLAGS_MASK));
            }
            table = PhysAddr::new(value & ADDRESS_MASK);
        }
        let index = ((virt.get() >> 12) & 0x1FF) as usize;
        // SAFETY: 末级页表项经 HHDM 映射，索引在 0..512 范围内。
        let value = unsafe { self.entry(table, index).read_volatile() };
        if value & PRESENT == 0 {
            return None;
        }
        Some((PhysAddr::new(value & ADDRESS_MASK), value & FLAGS_MASK))
    }

    /// 修改一段 4 KiB 用户映射的属性；遇到大页或缺失页时返回 `false`。
    pub fn protect_user_range(&self, virt: VirtAddr, len: usize, flags: PageFlags) -> bool {
        if len == 0
            || !len.is_multiple_of(PAGE_SIZE)
            || !virt.get().is_multiple_of(PAGE_SIZE as u64)
        {
            return false;
        }
        let mut offset = 0;
        while offset < len {
            let address = virt.get() + offset as u64;
            if !self.split_huge_at(address) {
                return false;
            }
            let mut table = self.root;
            for level in [39_u32, 30, 21] {
                let index = ((address >> level) & 0x1FF) as usize;
                // SAFETY: 页表项位于 HHDM 映射的物理内存。
                let value = unsafe { self.entry(table, index).read_volatile() };
                if value & PRESENT == 0 || value & HUGE != 0 {
                    return false;
                }
                table = PhysAddr::new(value & ADDRESS_MASK);
            }
            let index = ((address >> 12) & 0x1FF) as usize;
            let entry = self.entry(table, index);
            // SAFETY: 末级页表项位于 HHDM 映射的物理内存。
            let value = unsafe { entry.read_volatile() };
            if value & PRESENT == 0 {
                return false;
            }
            // SAFETY: 保留物理地址，仅替换属性位。
            unsafe { entry.write_volatile((value & ADDRESS_MASK) | flags.bits()) };
            self.flush(address);
            offset += PAGE_SIZE;
        }
        true
    }

    /// 建立 4 KiB 映射。
    pub fn map(
        &self,
        virt: VirtAddr,
        phys: PhysAddr,
        flags: PageFlags,
        allocator: &PageAllocator,
    ) -> Result<(), PagingError> {
        self.map_with(virt, phys, flags, || {
            allocator.allocate_for(FrameKind::PageTable)
        })
    }

    /// 建立映射，并用调用方提供的分配器分配中间页表。
    pub fn map_with<F>(
        &self,
        virt: VirtAddr,
        phys: PhysAddr,
        flags: PageFlags,
        mut allocate: F,
    ) -> Result<(), PagingError>
    where
        F: FnMut() -> Option<PhysAddr>,
    {
        if !virt.is_page_aligned() || !phys.is_page_aligned() {
            return Err(PagingError::Unaligned);
        }

        let mut table = self.root;
        for level in [39_u32, 30, 21] {
            let index = ((virt.get() >> level) & 0x1FF) as usize;
            let entry = self.entry(table, index);
            // SAFETY: 页表项经 HHDM 映射，索引在 0..512 范围内。
            let value = unsafe { entry.read_volatile() };

            if value & PRESENT == 0 {
                let frame = allocate().ok_or(PagingError::OutOfFrames)?;
                self.zero_page(frame);
                // SAFETY: 同上；新页表已清零，写入指向它的当前级表项。
                // 中间级必须带 USER，否则 ring 3 访问会被拒绝。
                unsafe {
                    entry.write_volatile(frame.get() | PRESENT | WRITABLE | USER);
                }
                table = frame;
            } else if value & HUGE != 0 {
                self.split_huge_at_with(virt.get(), &mut allocate)?;
                // SAFETY: 拆分后当前级项指向普通页表。
                let value = unsafe { entry.read_volatile() };
                table = PhysAddr::new(value & ADDRESS_MASK);
            } else {
                table = PhysAddr::new(value & ADDRESS_MASK);
            }
        }

        let index = ((virt.get() >> 12) & 0x1FF) as usize;
        let entry = self.entry(table, index);
        // SAFETY: 末级页表项同样位于 HHDM 映射的物理内存。
        unsafe {
            entry.write_volatile(phys.get() | flags.bits() | PRESENT);
        }
        // 刷新已修改的页对应的一级 TLB 项。
        self.flush(virt.get());
        Ok(())
    }

    /// 建立 2 MiB 或 1 GiB 大页映射。
    pub fn map_huge(
        &self,
        virt: VirtAddr,
        phys: PhysAddr,
        flags: PageFlags,
        size: u64,
        allocator: &PageAllocator,
    ) -> Result<(), PagingError> {
        self.map_huge_with(virt, phys, flags, size, || {
            allocator.allocate_for(FrameKind::PageTable)
        })
    }

    /// 建立大页映射，并用调用方提供的分配器分配中间页表。
    pub fn map_huge_with<F>(
        &self,
        virt: VirtAddr,
        phys: PhysAddr,
        flags: PageFlags,
        size: u64,
        mut allocate: F,
    ) -> Result<(), PagingError>
    where
        F: FnMut() -> Option<PhysAddr>,
    {
        let leaf_level = if size == (1 << 30) {
            if !has_1g_pages() {
                return Err(PagingError::Unsupported);
            }
            30
        } else if size == (1 << 21) {
            21
        } else {
            return Err(PagingError::Unsupported);
        };
        if !virt.get().is_multiple_of(size) || !phys.get().is_multiple_of(size) {
            return Err(PagingError::Unaligned);
        }

        let mut table = self.root;
        for level in [39_u32, 30, 21] {
            if level < leaf_level {
                break;
            }
            let index = ((virt.get() >> level) & 0x1FF) as usize;
            let entry = self.entry(table, index);
            // SAFETY: 页表项经 HHDM 映射，索引在 0..512 范围内。
            let value = unsafe { entry.read_volatile() };
            if level == leaf_level {
                if value & PRESENT != 0 {
                    return Err(PagingError::HugePageConflict {
                        level,
                        address: virt.get(),
                    });
                }
                // SAFETY: 叶项直接指向物理大页，低地址位已由对齐保证为零。
                unsafe {
                    entry.write_volatile(
                        (phys.get() & ADDRESS_MASK) | flags.bits() | PRESENT | HUGE,
                    );
                }
                self.flush(virt.get());
                return Ok(());
            }
            if value & PRESENT == 0 {
                let frame = allocate().ok_or(PagingError::OutOfFrames)?;
                self.zero_page(frame);
                // SAFETY: 新页表已清零，中间级带 USER 以允许 ring 3 穿透。
                unsafe {
                    entry.write_volatile(frame.get() | PRESENT | WRITABLE | USER);
                }
                table = frame;
            } else if value & HUGE != 0 {
                return Err(PagingError::HugePageConflict {
                    level,
                    address: virt.get(),
                });
            } else {
                table = PhysAddr::new(value & ADDRESS_MASK);
            }
        }
        Err(PagingError::Unsupported)
    }

    /// 把覆盖 `address` 的大页逐级拆到 4 KiB。
    fn split_huge_at_with<F>(&self, address: u64, allocate: &mut F) -> Result<(), PagingError>
    where
        F: FnMut() -> Option<PhysAddr>,
    {
        let mut table = self.root;
        for level in [39_u32, 30, 21] {
            let index = ((address >> level) & 0x1FF) as usize;
            let entry = self.entry(table, index);
            // SAFETY: 页表项经 HHDM 映射，索引在 0..512 范围内。
            let value = unsafe { entry.read_volatile() };
            if value & PRESENT == 0 {
                return Ok(());
            }
            if value & HUGE == 0 {
                table = PhysAddr::new(value & ADDRESS_MASK);
                continue;
            }
            let size = match level {
                30 => 1_u64 << 30,
                21 => 1_u64 << 21,
                _ => return Err(PagingError::Unsupported),
            };
            let child = allocate().ok_or(PagingError::OutOfFrames)?;
            self.zero_page(child);
            let base = value & ADDRESS_MASK;
            let leaf = value & !ADDRESS_MASK & !HUGE;
            for index in 0..ENTRIES {
                let phys = base + index as u64 * size;
                let bits = if level == 30 {
                    phys | leaf | PRESENT | HUGE
                } else {
                    phys | leaf | PRESENT
                };
                // SAFETY: 新页表经 HHDM 可写，索引在 0..512 范围内。
                unsafe { self.entry(child, index).write_volatile(bits) };
            }
            // SAFETY: 父项改为指向新页表；中间级不保留 NX/PWT/PCD。
            unsafe {
                entry.write_volatile(child.get() | PRESENT | WRITABLE | USER);
            }
            table = child;
        }
        self.flush(address);
        Ok(())
    }

    /// 解除一段用户映射；已不存在的页按已解除处理。
    pub fn unmap_user_range(&self, virt: VirtAddr, len: usize) -> bool {
        if !virt.is_page_aligned() {
            return false;
        }
        let mut offset = 0;
        while offset < len {
            let address = virt.get() + offset as u64;
            if !self.split_huge_at(address) {
                return false;
            }
            let mut table = self.root;
            let mut present = true;
            for level in [39_u32, 30, 21] {
                let index = ((address >> level) & 0x1FF) as usize;
                let entry = self.entry(table, index);
                // SAFETY: 页表项位于 HHDM 映射的物理内存。
                let value = unsafe { entry.read_volatile() };
                if value & PRESENT == 0 {
                    present = false;
                    break;
                }
                if value & HUGE != 0 {
                    return false;
                }
                table = PhysAddr::new(value & ADDRESS_MASK);
            }
            if !present {
                offset += PAGE_SIZE;
                continue;
            }
            let index = ((address >> 12) & 0x1FF) as usize;
            let entry = self.entry(table, index);
            // SAFETY: 末级页表项位于 HHDM 映射的物理内存。
            unsafe { entry.write_volatile(0) };
            self.flush(address);
            offset += PAGE_SIZE;
        }
        true
    }

    /// 使用页表帧池建立映射。
    pub fn map_mmio(
        &self,
        virt: VirtAddr,
        phys: PhysAddr,
        flags: PageFlags,
    ) -> Result<(), PagingError> {
        self.map_with(virt, phys, flags, || {
            PagePool::new(&FRAME_BANK).page_table()
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmaps_absent_range_idempotently() {
        let mut memory = alloc::vec![0_u8; PAGE_SIZE];
        let base = memory.as_mut_ptr() as u64;
        let table = PageTable {
            direct_map: base,
            root: PhysAddr::new(0),
            pcid: 0,
        };
        assert!(table.unmap_user_range(VirtAddr::new(0x1000), PAGE_SIZE));
        assert!(!table.unmap_user_range(VirtAddr::new(0x1001), PAGE_SIZE));
    }

    #[test]
    fn maps_2m_huge_page() {
        let mut memory = alloc::vec![0_u8; 3 * PAGE_SIZE];
        let base = memory.as_mut_ptr() as u64;
        let table = PageTable {
            direct_map: base,
            root: PhysAddr::new(0),
            pcid: 0,
        };
        let allocator = PageAllocator::from_range(PAGE_SIZE as u64, 3 * PAGE_SIZE as u64);
        table
            .map_huge_with(
                VirtAddr::new(0x20_0000),
                PhysAddr::new(0x40_0000),
                PageFlags::from_raw(PRESENT | WRITABLE | USER),
                1 << 21,
                || allocator.allocate_for(FrameKind::PageTable),
            )
            .expect("建立 2 MiB 大页");
        let (phys, flags) = table
            .translate(VirtAddr::new(0x20_0000 + 0x1234))
            .expect("翻译大页");
        assert_eq!(phys, PhysAddr::new(0x40_0000 + 0x1234));
        assert_ne!(flags & HUGE, 0);
    }

    #[test]
    fn creates_user_table_with_kernel_entries() {
        let mut memory = alloc::vec![0_u8; 3 * PAGE_SIZE];
        let base = memory.as_mut_ptr() as u64;
        // SAFETY: 缓冲区覆盖模板与用户页表各一页。
        unsafe {
            let root = base as *mut u64;
            root.add(0).write_volatile(0x9ABC);
            root.add(256).write_volatile(0x1234);
            root.add(511).write_volatile(0x5678);
        }

        let table = PageTable {
            direct_map: base,
            root: PhysAddr::new(0),
            pcid: 0,
        };
        let allocator = PageAllocator::from_range(PAGE_SIZE as u64, 2 * PAGE_SIZE as u64);
        let user = table.create_user(&allocator).expect("创建用户页表");
        assert_eq!(user.root, PhysAddr::new(PAGE_SIZE as u64));

        // SAFETY: 用户页表位于缓冲区第二页，已清零并复制内核入口。
        unsafe {
            let root = (base + PAGE_SIZE as u64) as *const u64;
            assert_eq!(root.add(0).read_volatile(), 0);
            assert_eq!(root.add(256).read_volatile(), 0x1234);
            assert_eq!(root.add(511).read_volatile(), 0x5678);
        }
    }
}
