//! 写时复制：fork 时共享可写页，缺页时按帧引用计数复制或恢复写权限。
//!
//! 共享状态只由 `hk-paging::FrameRefs` 维护，不使用架构专用 PTE 位。

use hk_hal::{PAGE_SIZE, PhysAddr};
use hk_mem::page::PageAllocator;
use hk_paging::FrameRefs;

use super::FRAME_BANK;
use super::flags::{HUGE, PCD, PRESENT, PWT, WRITABLE};
use super::table::{ADDRESS_MASK, ENTRIES, KERNEL_PML4_START, PageTable};

fn take_cow_page() -> Option<PhysAddr> {
    PageAllocator::new(&FRAME_BANK).cow()
}

impl PageTable {
    /// 复制用户低半区并标记写时复制，返回子页表。
    pub fn fork_user(&self) -> Option<Self> {
        let mut refs = super::frame_refs().lock();
        let child_root = take_cow_page()?;
        self.zero_page(child_root);

        for index in KERNEL_PML4_START..ENTRIES {
            // SAFETY: 两侧 PML4 均经 HHDM 映射，索引在 0..512 范围内。
            let value = unsafe { self.entry(self.root, index).read_volatile() };
            // SAFETY: 目标 PML4 已清零且经 HHDM 可写，索引在 0..512 范围内。
            unsafe {
                self.entry(child_root, index).write_volatile(value);
            }
        }

        for index in 0..KERNEL_PML4_START {
            // SAFETY: 读取模板 PML4 项，索引在 0..512 范围内。
            let value = unsafe { self.entry(self.root, index).read_volatile() };
            if value & PRESENT == 0 {
                continue;
            }
            if value & HUGE != 0 {
                return None;
            }

            let child = take_cow_page()?;
            self.zero_page(child);
            // SAFETY: 父页表处于活动状态，条目地址经 HHDM 映射。
            let source = PhysAddr::new(value & ADDRESS_MASK);
            self.clone_level(source, child, 3, (index as u64) << 39, &mut refs)?;
            // SAFETY: 子 PML4 项指向新分配的 PDPT，并保留原有权限位。
            unsafe {
                self.entry(child_root, index)
                    .write_volatile(child.get() | (value & !ADDRESS_MASK));
            }
        }

        if !crate::cpu::shootdown_tlb(None) {
            return None;
        }
        // 父地址空间的页表项已改为只读；PCID 模式下写 CR3 不会刷新。
        self.flush(0);
        self.activate();
        Some(Self {
            direct_map: self.direct_map,
            root: child_root,
            pcid: super::table::allocate_pcid(),
        })
    }

    fn clone_level(
        &self,
        source: PhysAddr,
        target: PhysAddr,
        level: u32,
        base: u64,
        refs: &mut FrameRefs,
    ) -> Option<()> {
        let shift = match level {
            3 => 30,
            2 => 21,
            _ => 12,
        };
        for index in 0..ENTRIES {
            // SAFETY: 源页表经 HHDM 映射，索引在 0..512 范围内。
            let mut value = unsafe { self.entry(source, index).read_volatile() };
            if value & PRESENT == 0 {
                continue;
            }
            let virt = base | ((index as u64) << shift);
            if value & HUGE != 0 {
                self.split_huge_at(virt);
                // SAFETY: 拆分后项指向普通页表。
                value = unsafe { self.entry(source, index).read_volatile() };
                if value & HUGE != 0 || value & PRESENT == 0 {
                    return None;
                }
            }

            let physical = value & ADDRESS_MASK;
            let mut flags = value & !ADDRESS_MASK;
            if level == 1 {
                let frame = PhysAddr::new(physical);
                let tracked = refs.count(frame);
                let writable = flags & WRITABLE != 0;
                let mmio = flags & (PWT | PCD) != 0;
                if !mmio && !super::mapping_acquire(frame) {
                    return None;
                }
                if !mmio && (writable || tracked > 0) {
                    // 可写页在 fork 时新增父、子两个共享者；已经共享的只读页
                    // 只新增当前子地址空间这一份引用。
                    if !refs.acquire(frame) {
                        return None;
                    }
                    if tracked == 0 && writable && !refs.acquire(frame) {
                        refs.release(frame);
                        return None;
                    }
                    flags &= !WRITABLE;
                }
                // 清掉旧快照可能留下的软件 COW 位。
                flags &= !super::flags::COW;
                // SAFETY: 父页表叶项经 HHDM 可写，保持不变共享页帧。
                unsafe {
                    self.entry(source, index).write_volatile(physical | flags);
                }
                // SAFETY: 子页表叶项指向共享页帧。
                unsafe {
                    self.entry(target, index).write_volatile(physical | flags);
                }
            } else {
                let child = take_cow_page()?;
                self.zero_page(child);
                self.clone_level(PhysAddr::new(physical), child, level - 1, virt, refs)?;
                // SAFETY: 子中间级项指向新页表，并保留原有权限位。
                unsafe {
                    self.entry(target, index)
                        .write_volatile(child.get() | (flags & !super::flags::COW));
                }
            }
        }
        Some(())
    }

    /// 判断物理帧是否与其他地址空间共享。
    pub fn is_shared_frame(&self, phys: PhysAddr) -> bool {
        super::mapping_count(phys) > 1
    }

    /// 处理写时复制页错误：复制页帧并恢复可写映射。
    pub fn resolve_cow(&self, address: u64) -> Option<super::CowResolution> {
        if !self.split_huge_at(address) {
            return None;
        }
        let mut table = self.root;
        for level in [39_u32, 30, 21] {
            let index = ((address >> level) & 0x1FF) as usize;
            // SAFETY: 页表项经 HHDM 映射，索引在 0..512 范围内。
            let value = unsafe { self.entry(table, index).read_volatile() };
            if value & PRESENT == 0 || value & HUGE != 0 {
                return None;
            }
            table = PhysAddr::new(value & ADDRESS_MASK);
        }

        let index = ((address >> 12) & 0x1FF) as usize;
        // SAFETY: 末级页表项经 HHDM 映射，索引在 0..512 范围内。
        let value = unsafe { self.entry(table, index).read_volatile() };
        if value & PRESENT == 0 || value & WRITABLE != 0 {
            return None;
        }

        let physical = PhysAddr::new(value & ADDRESS_MASK);
        let flags = value & !ADDRESS_MASK & !super::flags::COW;
        let mut refs = super::frame_refs().lock();
        let count = refs.count(physical);
        if count == 0 {
            return None;
        }

        if count == 1 {
            refs.release(physical);
            // SAFETY: 末级页表项经 HHDM 可写，索引在 0..512 范围内。
            unsafe {
                self.entry(table, index)
                    .write_volatile(physical.get() | flags | WRITABLE);
            }
            self.flush(address);
            return Some(super::CowResolution::Writable);
        }

        let frame = take_cow_page()?;
        if !super::mapping_acquire(frame) {
            return None;
        }
        let source = (self.direct_map + physical.get()) as *const u8;
        let target = (self.direct_map + frame.get()) as *mut u8;
        // SAFETY: 源页帧与目标页帧均经 HHDM 映射，长度为整页。
        unsafe {
            core::ptr::copy_nonoverlapping(source, target, PAGE_SIZE);
        }

        super::mapping_release(physical);
        refs.release(physical);
        let updated = frame.get() | flags | WRITABLE;
        // SAFETY: 末级页表项经 HHDM 可写，索引在 0..512 范围内。
        unsafe { self.entry(table, index).write_volatile(updated) };
        self.flush(address);
        Some(super::CowResolution::Copied(frame))
    }
}
