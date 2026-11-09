//! 页表机制：创建、遍历、映射与解除映射。

use alloc::vec::Vec;

use x86_64::instructions::tlb;
use x86_64::registers::control::{Cr3, Cr3Flags};
use x86_64::structures::paging::PhysFrame;

use hk_hal::{PAGE_SIZE, PhysAddr, VirtAddr};
use hk_mem::FrameKind;
use hk_mem::page::PagePool;

use super::flags::{COW, FLAGS_MASK, HUGE, PCD, PRESENT, PWT, PageFlags, USER, WRITABLE};
use super::{FRAME_BANK, PageAllocator, read_cr3};

pub(crate) const ADDRESS_MASK: u64 = 0x000F_FFFF_FFFF_F000;
pub(crate) const ENTRIES: usize = 512;
pub(crate) const KERNEL_PML4_START: usize = 256;

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
}

/// 页表操作句柄。
pub struct PageTable {
    pub(crate) direct_map: u64,
    pub(crate) root: PhysAddr,
}

impl PageTable {
    /// 读取 CR3 建立句柄。
    pub fn new(direct_map: u64) -> Self {
        Self {
            direct_map,
            root: read_cr3(),
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
        })
    }

    /// 遍历低半区的 4 KiB 用户页；设备（PWT/PCD）页与大页跳过。
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
                let pd = PhysAddr::new(pdp & ADDRESS_MASK);
                for pd_index in 0..ENTRIES {
                    // SAFETY: PD 是已存在的页表帧，经 HHDM 映射。
                    let pde = unsafe { self.entry(pd, pd_index).read_volatile() };
                    if pde & PRESENT == 0 || pde & USER == 0 || pde & HUGE != 0 {
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
                        let virt = ((pml4 as u64) << 39)
                            | ((pdpt_index as u64) << 30)
                            | ((pd_index as u64) << 21)
                            | ((pt_index as u64) << 12);
                        visit(virt, pte & FLAGS_MASK, pte & ADDRESS_MASK);
                    }
                }
            }
        }
    }

    /// 判断低半区是否存在写时复制页；此类任务暂不支持检查点。
    pub fn has_cow_user_page(&self) -> bool {
        for pml4 in 0..KERNEL_PML4_START {
            // SAFETY: PML4 经 HHDM 映射，索引在 0..512 范围内。
            let pml4e = unsafe { self.entry(self.root, pml4).read_volatile() };
            if pml4e & PRESENT == 0 {
                continue;
            }
            let pdpt = PhysAddr::new(pml4e & ADDRESS_MASK);
            for pdpt_index in 0..ENTRIES {
                // SAFETY: PDPT 是已存在的页表帧，经 HHDM 映射。
                let pdp = unsafe { self.entry(pdpt, pdpt_index).read_volatile() };
                if pdp & PRESENT == 0 {
                    continue;
                }
                let pd = PhysAddr::new(pdp & ADDRESS_MASK);
                for pd_index in 0..ENTRIES {
                    // SAFETY: PD 是已存在的页表帧，经 HHDM 映射。
                    let pde = unsafe { self.entry(pd, pd_index).read_volatile() };
                    if pde & PRESENT == 0 || pde & HUGE != 0 {
                        continue;
                    }
                    let pt = PhysAddr::new(pde & ADDRESS_MASK);
                    for pt_index in 0..ENTRIES {
                        // SAFETY: PT 是已存在的页表帧，经 HHDM 映射。
                        let pte = unsafe { self.entry(pt, pt_index).read_volatile() };
                        if pte & PRESENT != 0 && pte & COW != 0 {
                            return true;
                        }
                    }
                }
            }
        }
        false
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
        let frame = PhysFrame::containing_address(x86_64::PhysAddr::new(self.root.get()));
        // SAFETY: 根物理地址来自页表或帧分配器；写入 CR3 只影响当前 CPU。
        unsafe { Cr3::write(frame, Cr3Flags::empty()) }
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
                return Err(PagingError::HugePageConflict {
                    level,
                    address: virt.get(),
                });
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
        tlb::flush(x86_64::VirtAddr::new(virt.get()));
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
            tlb::flush(x86_64::VirtAddr::new(address));
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
        };
        assert!(table.unmap_user_range(VirtAddr::new(0x1000), PAGE_SIZE));
        assert!(!table.unmap_user_range(VirtAddr::new(0x1001), PAGE_SIZE));
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
