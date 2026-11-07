//! 系统物理帧银行：从全部可用内存区域分配与回收连续物理页。

use alloc::collections::{BTreeMap, BTreeSet};

use hk_hal::{MemoryRegion, PAGE_SIZE, PhysAddr, PhysRange};
use spin::Mutex;

use crate::hmp::{Charge, Ledger, LedgerId};

/// FrameBank 分配用途。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// 内核堆。
    KernelHeap,
    /// 页表。
    PageTable,
    /// 写时复制。
    Cow,
    /// 检查点恢复。
    Restore,
    /// 用户镜像。
    Image,
    /// DMA。
    Dma,
    /// 其他。
    Other,
}

impl FrameKind {
    /// 用途数量。
    pub const COUNT: usize = 7;

    const fn index(self) -> usize {
        match self {
            Self::KernelHeap => 0,
            Self::PageTable => 1,
            Self::Cow => 2,
            Self::Restore => 3,
            Self::Image => 4,
            Self::Dma => 5,
            Self::Other => 6,
        }
    }
}

/// FrameBank 记账快照。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameBankStats {
    /// 总字节数。
    pub total: u64,
    /// 已分配字节数。
    pub used: u64,
    /// 峰值已用字节数。
    pub peak: u64,
    /// 空闲字节数。
    pub free: u64,
    /// 各用途已用字节数。
    pub by_kind: [u64; FrameKind::COUNT],
}

struct Allocation {
    range: PhysRange,
    kind: FrameKind,
    charge: Charge,
}

struct Inner {
    free_by_start: BTreeMap<u64, PhysRange>,
    free_by_len: BTreeMap<u64, BTreeSet<u64>>,
    total: u64,
    ledger: Ledger,
    allocations: BTreeMap<u64, Allocation>,
    by_kind: [u64; FrameKind::COUNT],
}

impl Inner {
    fn insert_free(&mut self, range: PhysRange) {
        let start = range.start().get();
        self.free_by_len
            .entry(range.len())
            .or_default()
            .insert(start);
        self.free_by_start.insert(start, range);
    }

    fn take_free(&mut self, start: u64) -> Option<PhysRange> {
        let range = self.free_by_start.remove(&start)?;
        let length = range.len();
        if let Some(starts) = self.free_by_len.get_mut(&length) {
            starts.remove(&start);
            if starts.is_empty() {
                self.free_by_len.remove(&length);
            }
        }
        Some(range)
    }
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
                free_by_start: BTreeMap::new(),
                free_by_len: BTreeMap::new(),
                total: 0,
                ledger: Ledger::new(LedgerId::new(0), 0),
                allocations: BTreeMap::new(),
                by_kind: [0; FrameKind::COUNT],
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
        inner.free_by_start.clear();
        inner.free_by_len.clear();
        inner.total = 0;
        inner.allocations.clear();
        inner.by_kind = [0; FrameKind::COUNT];
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
                    inner.insert_free(PhysRange::new(PhysAddr::new(start), before_end - start));
                }
                let after_start = kernel.end().get().div_ceil(page) * page;
                if end > after_start {
                    inner.insert_free(PhysRange::new(
                        PhysAddr::new(after_start),
                        end - after_start,
                    ));
                }
                continue;
            }
            inner.insert_free(PhysRange::new(PhysAddr::new(start), end - start));
        }
        inner.total = inner.free_by_start.values().map(|range| range.len()).sum();
        inner.ledger = Ledger::new(LedgerId::new(0), inner.total);
    }

    /// 按用途分配一段连续物理内存，长度按页向上取整。
    pub fn allocate_contiguous_for(&self, kind: FrameKind, len: usize) -> Option<PhysRange> {
        let length = (len as u64).div_ceil(PAGE_SIZE as u64) * PAGE_SIZE as u64;
        if length == 0 {
            return None;
        }
        let mut inner = self.inner.lock();
        let start = {
            let (_, starts) = inner.free_by_len.range(length..).next()?;
            *starts.iter().next()?
        };
        let charge = inner.ledger.charge(length).ok()?;
        let range = inner.take_free(start)?;
        let start = range.start();
        let remaining = range.len() - length;
        if remaining > 0 {
            inner.insert_free(PhysRange::new(
                PhysAddr::new(start.get() + length),
                remaining,
            ));
        }
        let allocated = PhysRange::new(start, length);
        inner.allocations.insert(
            start.get(),
            Allocation {
                range: allocated,
                kind,
                charge,
            },
        );
        let slot = &mut inner.by_kind[kind.index()];
        *slot = slot.saturating_add(length);
        Some(allocated)
    }

    /// 分配一段连续物理内存，用途记为 `Other`。
    pub fn allocate_contiguous(&self, len: usize) -> Option<PhysRange> {
        self.allocate_contiguous_for(FrameKind::Other, len)
    }

    /// 按用途分配一个 4 KiB 物理帧。
    pub fn allocate_for(&self, kind: FrameKind) -> Option<PhysAddr> {
        self.allocate_contiguous_for(kind, PAGE_SIZE)
            .map(PhysRange::start)
    }

    /// 分配一个 4 KiB 物理帧，用途记为 `Other`。
    pub fn allocate(&self) -> Option<PhysAddr> {
        self.allocate_for(FrameKind::Other)
    }

    /// 按分配记录归还一段物理区间并与相邻空闲区间合并。
    pub fn free_for(&self, _kind: FrameKind, range: PhysRange) {
        if range.is_empty() {
            return;
        }
        let mut inner = self.inner.lock();
        let start = range.start().get();
        let matches = inner
            .allocations
            .get(&start)
            .is_some_and(|allocation| allocation.range == range);
        if !matches {
            return;
        }
        let Some(allocation) = inner.allocations.remove(&start) else {
            return;
        };
        let _ = inner.ledger.release(allocation.charge);
        let slot = &mut inner.by_kind[allocation.kind.index()];
        *slot = slot.saturating_sub(range.len());
        let mut merged = range;
        if let Some(previous) = inner
            .free_by_start
            .range(..start)
            .next_back()
            .map(|(_, range)| *range)
            && previous.end().get() == merged.start().get()
        {
            inner.take_free(previous.start().get());
            merged = PhysRange::new(
                previous.start(),
                previous.len().saturating_add(merged.len()),
            );
        }
        let next_start = merged.end().get();
        if let Some(next) = inner
            .free_by_start
            .range(next_start..)
            .next()
            .map(|(_, range)| *range)
            && next.start().get() == next_start
        {
            inner.take_free(next.start().get());
            merged = PhysRange::new(merged.start(), merged.len().saturating_add(next.len()));
        }
        inner.insert_free(merged);
    }

    /// 归还一段用途为 `Other` 的物理区间。
    pub fn free(&self, range: PhysRange) {
        self.free_for(FrameKind::Other, range);
    }

    /// 返回总字节数。
    pub fn total(&self) -> u64 {
        self.inner.lock().total
    }

    /// 返回已分配字节数。
    pub fn used(&self) -> u64 {
        self.inner.lock().ledger.charged()
    }

    /// 返回记账快照。
    pub fn stats(&self) -> FrameBankStats {
        let inner = self.inner.lock();
        FrameBankStats {
            total: inner.total,
            used: inner.ledger.charged(),
            peak: inner.ledger.peak(),
            free: inner.total.saturating_sub(inner.ledger.charged()),
            by_kind: inner.by_kind,
        }
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
        bank.free(range);
        bank.free(PhysRange::new(PhysAddr::new(0x10_2000), 0x1000));
        let merged = bank.allocate_contiguous(0x3000).expect("合并后");
        assert_eq!(merged.start(), PhysAddr::new(0x10_0000));
    }
}
