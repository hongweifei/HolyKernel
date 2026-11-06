//! 线性物理帧分配器：从启动信息的可用区域中做顺序分配。

use hk_hal::{MemoryRegion, PAGE_SIZE, PhysAddr};

/// 线性物理帧分配器。
pub struct LinearPages {
    next: u64,
    end: u64,
}

impl LinearPages {
    /// 以物理区间 `[next, end)` 构造分配器。
    pub const fn from_range(next: u64, end: u64) -> Self {
        Self { next, end }
    }

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

    /// 分配一段连续物理内存，长度按页向上取整。
    pub fn allocate_contiguous(&mut self, len: usize) -> Option<hk_hal::PhysRange> {
        let length = align_up(len as u64, PAGE_SIZE as u64);
        if length == 0 || self.next.checked_add(length)? > self.end {
            return None;
        }
        let start = PhysAddr::new(self.next);
        self.next += length;
        Some(hk_hal::PhysRange::new(start, length))
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

#[cfg(test)]
mod tests {
    use super::*;
    use hk_hal::RegionKind;

    #[test]
    fn picks_largest_usable_region() {
        let regions = [
            MemoryRegion::new(PhysAddr::new(0x1_0000), 0x2_0000, RegionKind::Usable),
            MemoryRegion::new(PhysAddr::new(0x10_0000), 0x40_0000, RegionKind::Usable),
        ];
        let kernel = MemoryRegion::new(PhysAddr::new(0), 0x1000, RegionKind::Kernel);

        let allocator = LinearPages::new(&regions, kernel).expect("可用区域");
        assert_eq!(allocator.next, 0x10_0000);
        assert_eq!(allocator.end, 0x50_0000);
    }

    #[test]
    fn avoids_kernel_image() {
        let regions = [MemoryRegion::new(
            PhysAddr::new(0x1_0000),
            0x10_0000,
            RegionKind::Usable,
        )];
        let kernel = MemoryRegion::new(PhysAddr::new(0x1_0000), 0x2000, RegionKind::Kernel);

        let allocator = LinearPages::new(&regions, kernel).expect("可用区域");
        assert_eq!(allocator.next, 0x1_2000);
    }
    #[test]
    fn allocates_contiguous_ranges() {
        let mut frames = LinearPages::from_range(0x10_0000, 0x10_3000);
        let range = frames.allocate_contiguous(0x1800).expect("连续分配");
        assert_eq!(range.start(), PhysAddr::new(0x10_0000));
        assert_eq!(range.len(), 0x2000);
        assert_eq!(frames.allocate(), Some(PhysAddr::new(0x10_2000)));
        assert_eq!(frames.allocate_contiguous(0x2000), None);
    }
}
