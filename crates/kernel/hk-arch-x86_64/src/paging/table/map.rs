//! 页表映射建立、属性修改与解除映射。

use hk_hal::{PAGE_SIZE, PhysAddr, PhysRange, VirtAddr};
use hk_mem::FrameKind;

use crate::paging::flags::{HUGE, PRESENT, PageFlags, USER, WRITABLE};
use crate::paging::{FRAME_BANK, mapping_count, mapping_release};

use super::{ADDRESS_MASK, ENTRIES, PageTable, PagingError, has_1g_pages};

impl PageTable {
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
    pub fn map(&self, virt: VirtAddr, phys: PhysAddr, flags: PageFlags) -> Result<(), PagingError> {
        self.map_with(virt, phys, flags, || self.kernel_page_table(virt.get()))
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
    ) -> Result<(), PagingError> {
        self.map_huge_with(virt, phys, flags, size, || {
            self.kernel_page_table(virt.get())
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

    pub(crate) fn split_huge_at(&self, address: u64) -> bool {
        let mut allocate = || self.kernel_page_table(address);
        self.split_huge_at_with(address, &mut allocate).is_ok()
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
            let value = unsafe { entry.read_volatile() };
            if value & PRESENT != 0 {
                let physical = PhysAddr::new(value & ADDRESS_MASK);
                if mapping_count(physical) > 0
                    && mapping_release(physical) == 0
                    && let Some(kind) = FRAME_BANK.kind_of(physical)
                {
                    match kind {
                        FrameKind::Image | FrameKind::Cow | FrameKind::Restore => {
                            FRAME_BANK.free_for(kind, PhysRange::new(physical, PAGE_SIZE as u64));
                        }
                        _ => {}
                    }
                }
            }
            // SAFETY: 末级页表项位于 HHDM 映射的物理内存。
            unsafe { entry.write_volatile(0) };
            self.flush(address);
            offset += PAGE_SIZE;
        }
        // 本地失效率由调用方决定是否广播；同进程不在其它 CPU 运行时无需 IPI。
        true
    }

    /// 使用页表帧池建立映射。
    pub fn map_mmio(
        &self,
        virt: VirtAddr,
        phys: PhysAddr,
        flags: PageFlags,
    ) -> Result<(), PagingError> {
        self.map_with(virt, phys, flags, || self.kernel_page_table(virt.get()))
    }
}
