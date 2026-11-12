//! 写时复制：fork 时共享可写页，缺页时按帧引用计数复制或恢复写权限。
//!
//! 共享状态只由 `hk-paging::FrameRefs` 维护，不使用架构专用 PTE 位。

use hk_hal::{PAGE_SIZE, PhysAddr, PhysRange};
use hk_mem::FrameKind;
use hk_mem::page::PageAllocator;
use hk_paging::FrameRefs;

use super::FRAME_BANK;
use super::flags::{HUGE, PCD, PRESENT, PWT, WRITABLE};
use super::table::{ADDRESS_MASK, ENTRIES, KERNEL_PML4_START, PageTable};

/// 取一页 COW 私有数据页；从 User Capacity 分配，物理归因不再落到 System。
fn take_cow_page() -> Option<PhysAddr> {
    crate::system::user_capacity()?
        .lock()
        .allocate(PAGE_SIZE)
        .ok()
        .map(|frame| frame.phys())
}

/// 取一页页表；页表页始终由 System 承载。
fn take_table_page() -> Option<PhysAddr> {
    PageAllocator::new(&FRAME_BANK).page_table()
}

/// fork 期间的共享状态：COW 区间、帧引用表与私有复制字节数。
struct ForkState<'a> {
    cow_ranges: &'a [(u64, u64)],
    refs: &'a mut FrameRefs,
    copied: u64,
}

impl PageTable {
    /// 复制用户低半区；`cow_ranges` 内的可写页标 COW，其余可写页私有复制。
    /// 返回副本与私有复制页的字节数，供 Task Ledger 记账。
    pub fn fork_user(&self, cow_ranges: &[(u64, u64)]) -> Option<(Self, u64)> {
        let mut refs = super::frame_refs().lock();
        let mut state = ForkState {
            cow_ranges,
            refs: &mut refs,
            copied: 0,
        };
        let child_root = take_table_page()?;
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

            let child = take_table_page()?;
            self.zero_page(child);
            // SAFETY: 父页表处于活动状态，条目地址经 HHDM 映射。
            let source = PhysAddr::new(value & ADDRESS_MASK);
            self.clone_level(source, child, 3, (index as u64) << 39, &mut state)?;
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
        Some((
            Self {
                direct_map: self.direct_map,
                root: child_root,
                pcid: super::table::allocate_pcid(),
            },
            state.copied,
        ))
    }

    fn clone_level(
        &self,
        source: PhysAddr,
        target: PhysAddr,
        level: u32,
        base: u64,
        state: &mut ForkState<'_>,
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
                let tracked = state.refs.count(frame);
                let writable = flags & WRITABLE != 0;
                let mmio = flags & (PWT | PCD) != 0;
                let cow = state
                    .cow_ranges
                    .iter()
                    .any(|(start, len)| virt >= *start && virt < start.saturating_add(*len));
                if !mmio && writable && !cow {
                    // 未登记 Region 的可写页不能隐式共享；为子任务复制私有页，
                    // 父页表保持原映射与权限。
                    let child_frame = self.copy_private_page(frame, &mut state.copied)?;
                    flags &= !super::flags::COW;
                    // SAFETY: 子页表叶项指向新分配的私有页帧。
                    unsafe {
                        self.entry(target, index)
                            .write_volatile(child_frame.get() | flags);
                    }
                    continue;
                }
                if !mmio && !super::mapping_acquire(frame) {
                    return None;
                }
                if !mmio && (writable || tracked > 0) {
                    // Region 内的可写页在 fork 时新增父、子两个共享者；
                    // 已经共享的只读页只新增当前子地址空间这一份引用。
                    if !state.refs.acquire(frame) {
                        return None;
                    }
                    if tracked == 0 && writable && !state.refs.acquire(frame) {
                        state.refs.release(frame);
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
                let child = take_table_page()?;
                self.zero_page(child);
                self.clone_level(PhysAddr::new(physical), child, level - 1, virt, state)?;
                // SAFETY: 子中间级项指向新页表，并保留原有权限位。
                unsafe {
                    self.entry(target, index)
                        .write_volatile(child.get() | (flags & !super::flags::COW));
                }
            }
        }
        Some(())
    }

    /// 为子地址空间复制一份私有页，并累计复制字节数。
    fn copy_private_page(&self, source: PhysAddr, copied: &mut u64) -> Option<PhysAddr> {
        let frame = take_cow_page()?;
        let source = (self.direct_map + source.get()) as *const u8;
        let target = (self.direct_map + frame.get()) as *mut u8;
        // SAFETY: 源、目标页帧均经 HHDM 映射，长度为整页。
        unsafe {
            core::ptr::copy_nonoverlapping(source, target, PAGE_SIZE);
        }
        if !super::mapping_acquire(frame) {
            FRAME_BANK.free_for(
                FrameKind::Image,
                PhysRange::new(frame, hk_hal::PAGE_SIZE as u64),
            );
            return None;
        }
        *copied = copied.saturating_add(PAGE_SIZE as u64);
        Some(frame)
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
