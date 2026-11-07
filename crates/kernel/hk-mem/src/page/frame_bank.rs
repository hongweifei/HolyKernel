//! 系统物理帧银行：从全部可用内存区域分配与回收连续物理页。

use alloc::vec::Vec;

use hk_hal::{MemoryRegion, PAGE_SIZE, PhysAddr, PhysRange};
use spin::Mutex;

struct Inner {
    free: Vec<PhysRange>,
    total: u64,
    used: u64,
}

/// 系统物理帧银行。
pub struct FrameBank {
    inner: Mutex<Inner>,
}

impl FrameBank {
    /// 构造空帧银行，供全局静态初始化。
    pub const fn empty() -> Self {
        Self {
            inner: Mutex::new(Inner {
                free: Vec::new(),
                total: 0,
                used: 0,
            }),
        }
    }

    /// 以单个物理区间构造帧银行。
    pub fn from_range(start: u64, end: u64) -> Self {
        let bank = Self::empty();
        bank.fill(
            core::slice::from_ref(&MemoryRegion::new(
                PhysAddr::new(start),
                end.saturating_sub(start),
                hk_hal::RegionKind::Usable,
            )),
            None,
        );
        bank
    }

    /// 从启动信息的全部可用区域初始化，并避开内核镜像。
    pub fn init(&self, regions: &[MemoryRegion], kernel: MemoryRegion) {
        self.fill(regions, Some(kernel));
    }

    fn fill(&self, regions: &[MemoryRegion], kernel: Option<MemoryRegion>) {
        let page = PAGE_SIZE as u64;
        let mut inner = self.inner.lock();
        inner.free.clear();
        inner.total = 0;
        inner.used = 0;
        for region in regions {
            if !region.kind.is_usable() {
                continue;
            }
            let start = region.base.get().div_ceil(page) * page;
            let end = region.end().get() & !(page - 1);
            if end <= start {
                continue;
            }
            if let Some(kernel) = kernel
                && start < kernel.end().get()
                && kernel.base.get() < end
            {
                let before_end = kernel.base.get() & !(page - 1);
                if before_end > start {
                    inner
                        .free
                        .push(PhysRange::new(PhysAddr::new(start), before_end - start));
                }
                let after_start = kernel.end().get().div_ceil(page) * page;
                if end > after_start {
                    inner.free.push(PhysRange::new(
                        PhysAddr::new(after_start),
                        end - after_start,
                    ));
                }
                continue;
            }
            inner
                .free
                .push(PhysRange::new(PhysAddr::new(start), end - start));
        }
        inner.free.sort_unstable_by_key(|range| range.start().get());
        inner.total = inner.free.iter().map(|range| range.len()).sum();
    }

    /// 分配一段连续物理内存，长度按页向上取整。
    pub fn allocate_contiguous(&self, len: usize) -> Option<PhysRange> {
        let length = (len as u64).div_ceil(PAGE_SIZE as u64) * PAGE_SIZE as u64;
        if length == 0 {
            return None;
        }
        let mut inner = self.inner.lock();
        let index = inner.free.iter().position(|range| range.len() >= length)?;
        let range = inner.free.swap_remove(index);
        let start = range.start();
        let remaining = range.len() - length;
        if remaining > 0 {
            inner.free.push(PhysRange::new(
                PhysAddr::new(start.get() + length),
                remaining,
            ));
        }
        inner.used = inner.used.saturating_add(length);
        Some(PhysRange::new(start, length))
    }

    /// 分配一个 4 KiB 物理帧。
    pub fn allocate(&self) -> Option<PhysAddr> {
        self.allocate_contiguous(PAGE_SIZE).map(PhysRange::start)
    }

    /// 归还一段物理区间并与相邻空闲区间合并。
    pub fn free(&self, range: PhysRange) {
        if range.is_empty() {
            return;
        }
        let mut inner = self.inner.lock();
        inner.used = inner.used.saturating_sub(range.len());
        inner.free.push(range);
        inner.free.sort_unstable_by_key(|range| range.start().get());
        let mut merged: Vec<PhysRange> = Vec::with_capacity(inner.free.len());
        for range in inner.free.drain(..) {
            if let Some(last) = merged.last_mut()
                && last.end().get() == range.start().get()
            {
                *last = PhysRange::new(last.start(), last.len().saturating_add(range.len()));
                continue;
            }
            merged.push(range);
        }
        inner.free = merged;
    }

    /// 返回总字节数。
    pub fn total(&self) -> u64 {
        self.inner.lock().total
    }

    /// 返回已分配字节数。
    pub fn used(&self) -> u64 {
        self.inner.lock().used
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hk_hal::RegionKind;

    #[test]
    fn covers_all_usable_regions() {
        let regions = [
            MemoryRegion::new(PhysAddr::new(0x1_0000), 0x2_0000, RegionKind::Usable),
            MemoryRegion::new(PhysAddr::new(0x10_0000), 0x40_0000, RegionKind::Usable),
            MemoryRegion::new(PhysAddr::new(0x80_0000), 0x10_0000, RegionKind::Reserved),
        ];
        let kernel = MemoryRegion::new(PhysAddr::new(0), 0x1000, RegionKind::Kernel);
        let bank = FrameBank::empty();
        bank.init(&regions, kernel);
        assert_eq!(bank.allocate(), Some(PhysAddr::new(0x1_0000)));
        let large = bank.allocate_contiguous(0x20_000).expect("连续页");
        assert_eq!(large.start(), PhysAddr::new(0x10_0000));
    }

    #[test]
    fn avoids_kernel_image() {
        let regions = [MemoryRegion::new(
            PhysAddr::new(0x1_0000),
            0x10_0000,
            RegionKind::Usable,
        )];
        let kernel = MemoryRegion::new(PhysAddr::new(0x1_0000), 0x2000, RegionKind::Kernel);
        let bank = FrameBank::empty();
        bank.init(&regions, kernel);
        assert_eq!(bank.allocate(), Some(PhysAddr::new(0x1_2000)));
    }

    #[test]
    fn allocates_and_coalesces_frames() {
        let bank = FrameBank::from_range(0x10_0000, 0x10_3000);
        let range = bank.allocate_contiguous(0x1800).expect("连续分配");
        assert_eq!(range.start(), PhysAddr::new(0x10_0000));
        assert_eq!(range.len(), 0x2000);
        assert_eq!(bank.allocate(), Some(PhysAddr::new(0x10_2000)));
        assert_eq!(bank.allocate_contiguous(0x2000), None);
        bank.free(PhysRange::new(PhysAddr::new(0x10_0000), 0x1000));
        bank.free(PhysRange::new(PhysAddr::new(0x10_1000), 0x1000));
        let merged = bank.allocate_contiguous(0x2000).expect("合并后");
        assert_eq!(merged.start(), PhysAddr::new(0x10_0000));
    }
}
