//! 内核页表操作。
//!
//! 沿用 Limine 建立的页表，按需为 MMIO 建立 4 KiB 映射：缺失的中间级页表从物理
//! 帧分配器取页并清零，页表本身经 HHDM 访问。

use core::arch::asm;

use hk_hal::{MemoryRegion, PAGE_SIZE, PhysAddr, VirtAddr};

const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
const USER: u64 = 1 << 2;
const PWT: u64 = 1 << 3;
const PCD: u64 = 1 << 4;
const HUGE: u64 = 1 << 7;
const ADDRESS_MASK: u64 = 0x000F_FFFF_FFFF_F000;
const ENTRIES: usize = 512;

/// 页表项属性。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageFlags(u64);

impl PageFlags {
    /// 内核可读写页。
    pub const KERNEL_RW: Self = Self(WRITABLE);
    /// 用户可读写页。
    pub const USER_RW: Self = Self(WRITABLE | USER);
    /// 设备 MMIO 页：可写、写直达、禁止缓存。
    pub const MMIO: Self = Self(WRITABLE | PWT | PCD);

    const fn bits(self) -> u64 {
        self.0
    }
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
}

/// 物理帧分配器：在可用内存区域上做线性分配。
pub struct FrameAllocator {
    next: u64,
    end: u64,
}

impl FrameAllocator {
    /// 从启动信息中挑选最大的可用区域并避开内核镜像。
    pub fn new(regions: &[MemoryRegion], kernel: MemoryRegion) -> Option<Self> {
        let page = PAGE_SIZE as u64;
        let mut best: Option<(u64, u64)> = None;

        for region in regions {
            if !region.kind.is_usable() {
                continue;
            }
            let mut start = align_up(region.base.get(), page);
            let end = region.end().get() & !(page - 1);
            if end <= start {
                continue;
            }
            if start < kernel.end().get() && kernel.base.get() < end {
                start = align_up(kernel.end().get(), page);
            }
            if start + page > end {
                continue;
            }

            let capacity = end - start;
            let larger = best.is_none_or(|(best_start, best_end)| capacity > best_end - best_start);
            if larger {
                best = Some((start, end));
            }
        }

        best.map(|(next, end)| Self { next, end })
    }

    /// 分配一个 4 KiB 物理帧。
    pub fn allocate(&mut self) -> Option<PhysAddr> {
        let page = PAGE_SIZE as u64;
        if self.next + page > self.end {
            return None;
        }
        let frame = PhysAddr::new(self.next);
        self.next += page;
        Some(frame)
    }
}

fn align_up(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}

/// 页表操作句柄。
pub struct PageTable {
    direct_map: u64,
    root: PhysAddr,
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

    /// 建立 4 KiB 映射。
    pub fn map(
        &self,
        virt: VirtAddr,
        phys: PhysAddr,
        flags: PageFlags,
        allocator: &mut FrameAllocator,
    ) -> Result<(), PagingError> {
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
                let frame = allocator.allocate().ok_or(PagingError::OutOfFrames)?;
                self.zero_frame(frame);
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
        // SAFETY: 刷新已修改的页对应的一级 TLB 项。
        unsafe {
            asm!("invlpg [{}]", in(reg) virt.get(), options(nostack, preserves_flags));
        }
        Ok(())
    }

    fn entry(&self, table: PhysAddr, index: usize) -> *mut u64 {
        debug_assert!(index < ENTRIES);
        ((self.direct_map + table.get()) as *mut u64).wrapping_add(index)
    }

    fn zero_frame(&self, frame: PhysAddr) {
        let pointer = (self.direct_map + frame.get()) as *mut u8;
        // SAFETY: 新分配的帧来自可用内存区域，经 HHDM 可写。
        unsafe {
            core::ptr::write_bytes(pointer, 0, PAGE_SIZE);
        }
    }
}

fn read_cr3() -> PhysAddr {
    let value: u64;
    // SAFETY: 读取 CR3 不改变机器状态；只在 ring 0 执行。
    unsafe {
        asm!("mov {}, cr3", out(reg) value, options(nomem, nostack));
    }
    PhysAddr::new(value & ADDRESS_MASK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hk_hal::RegionKind;

    #[test]
    fn frame_allocator_picks_largest_usable_region() {
        let regions = [
            MemoryRegion::new(PhysAddr::new(0x1_0000), 0x2_0000, RegionKind::Usable),
            MemoryRegion::new(PhysAddr::new(0x10_0000), 0x40_0000, RegionKind::Usable),
        ];
        let kernel = MemoryRegion::new(PhysAddr::new(0), 0x1000, RegionKind::Kernel);

        let allocator = FrameAllocator::new(&regions, kernel).expect("可用区域");
        assert_eq!(allocator.next, 0x10_0000);
        assert_eq!(allocator.end, 0x50_0000);
    }

    #[test]
    fn frame_allocator_avoids_kernel_image() {
        let regions = [MemoryRegion::new(
            PhysAddr::new(0x1_0000),
            0x10_0000,
            RegionKind::Usable,
        )];
        let kernel = MemoryRegion::new(PhysAddr::new(0x1_0000), 0x2000, RegionKind::Kernel);

        let allocator = FrameAllocator::new(&regions, kernel).expect("可用区域");
        assert_eq!(allocator.next, 0x1_2000);
    }
}
