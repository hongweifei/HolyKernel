//! 系统物理帧银行：从全部可用内存区域分配与回收连续物理页。
//!
//! 这是全系统唯一的物理空闲表；HMP `Capacity` 只是它的配额视图，所有
//! 物理分配都记在这里，并按 `CapacityId` 归因。

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use hk_hal::{MemoryRegion, PAGE_SIZE, PhysAddr, PhysRange};
use spin::Mutex;

use crate::hmp::{CapacityId, CapacityKind, Charge, HmpError, Ledger, LedgerId, PhysicalBackend};

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

    /// 把 Capacity 用途映射到 FrameBank 用途分类，用于统计。
    pub const fn from_capacity(kind: CapacityKind) -> Self {
        match kind {
            CapacityKind::System => Self::Other,
            CapacityKind::User => Self::Image,
            CapacityKind::Dma => Self::Dma,
            CapacityKind::Gpu => Self::Other,
            CapacityKind::Numa(_) => Self::Other,
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
    /// 归属容量；`None` 表示内核自用。
    owner: Option<CapacityId>,
}

/// 登记给某个容量的窗口。
struct OwnedWindow {
    capacity: CapacityId,
    range: PhysRange,
}

struct Inner {
    free_by_start: BTreeMap<u64, PhysRange>,
    free_by_len: BTreeMap<u64, BTreeSet<u64>>,
    total: u64,
    ledger: Ledger,
    allocations: BTreeMap<u64, Allocation>,
    by_kind: [u64; FrameKind::COUNT],
    /// 已登记给容量的窗口，按起始地址索引。
    owned: BTreeMap<u64, OwnedWindow>,
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

    /// 在指定容量登记的窗口内寻找一段足够长的空闲区间。
    ///
    /// 空闲条目可能与相邻容量合并成大块，也可能被窗口边界切成多段，因此不能
    /// 只看单个条目：这里把落在窗口内的空闲段收集起来、按起点排序并合并相邻
    /// 段，再找足够长的连续区间。
    fn find_free_in_owned(&self, owner: CapacityId, length: u64) -> Option<u64> {
        let windows: Vec<(u64, u64)> = self
            .owned
            .values()
            .filter(|window| window.capacity == owner)
            .map(|window| (window.range.start().get(), window.range.end().get()))
            .collect();
        for (ws, we) in windows {
            let mut pieces: Vec<(u64, u64)> = self
                .free_by_start
                .values()
                .filter_map(|range| {
                    let start = range.start().get().max(ws);
                    let end = range.end().get().min(we);
                    (end > start).then_some((start, end))
                })
                .collect();
            pieces.sort_unstable();
            let mut cursor = None;
            for (start, end) in pieces {
                let run_start = match cursor {
                    Some((run_start, run_end)) if run_end == start => run_start,
                    Some(_) => {
                        cursor = Some((start, end));
                        continue;
                    }
                    None => start,
                };
                if end - run_start >= length {
                    return Some(run_start);
                }
                cursor = Some((run_start, end));
            }
        }
        None
    }

    /// 从空闲表中切出 `[start, start+len)`，必要时拆分条目。
    ///
    /// 区间可能横跨多个相邻空闲条目；先把这些条目整体取出，再按前后残段
    /// 分别插回。
    fn carve_free(&mut self, start: u64, len: u64) -> Option<PhysRange> {
        let end = start.checked_add(len)?;
        // 收集所有与区间相交的空闲条目。
        let covered: Vec<(u64, PhysRange)> = self
            .free_by_start
            .iter()
            .filter(|(_, range)| range.start().get() < end && start < range.end().get())
            .map(|(key, range)| (*key, *range))
            .collect();
        let (first_start, first_end) = {
            let mut begin = u64::MAX;
            let mut finish = 0;
            for (_, range) in &covered {
                begin = begin.min(range.start().get());
                finish = finish.max(range.end().get());
            }
            (begin, finish)
        };
        if first_start > start || first_end < end {
            return None;
        }
        // 覆盖范围必须连续无洞，否则切出的区间里混着别人的页。
        let mut sorted: Vec<(u64, u64)> = covered
            .iter()
            .map(|(_, range)| (range.start().get(), range.end().get()))
            .collect();
        sorted.sort_unstable();
        let mut reach = first_start;
        for (piece_start, piece_end) in &sorted {
            if *piece_start > reach {
                return None;
            }
            reach = reach.max(*piece_end);
        }
        if reach < end {
            return None;
        }
        for (key, _) in &covered {
            self.take_free(*key)?;
        }
        if start > first_start {
            self.insert_free(PhysRange::new(
                PhysAddr::new(first_start),
                start - first_start,
            ));
        }
        let tail = first_end.saturating_sub(end);
        if tail > 0 {
            self.insert_free(PhysRange::new(PhysAddr::new(end), tail));
        }
        Some(PhysRange::new(PhysAddr::new(start), len))
    }

    /// 在未登记给任何容量的空闲区间内寻找一段足够长的区间。
    ///
    /// 先把相邻空闲段合并成连续运行段，再跳过与已登记窗口相交的部分，避免
    /// 整块大区间因含有一小段归属区而被整体弃用。
    fn find_free_unowned(&self, length: u64) -> Option<u64> {
        let mut spans: Vec<(u64, u64)> = self
            .free_by_start
            .values()
            .map(|range| (range.start().get(), range.end().get()))
            .collect();
        spans.sort_unstable();
        let mut runs: Vec<(u64, u64)> = Vec::new();
        for (start, end) in spans {
            match runs.last_mut() {
                Some(last) if last.1 == start => last.1 = end,
                _ => runs.push((start, end)),
            }
        }
        for (start, end) in runs {
            let mut cursor = start;
            while cursor + length <= end {
                let blocked = self
                    .owned
                    .values()
                    .filter_map(|window| {
                        let ws = window.range.start().get();
                        let we = window.range.end().get();
                        (ws < cursor + length && cursor < we).then_some(we)
                    })
                    .max();
                match blocked {
                    Some(next) if next <= end => cursor = next,
                    Some(_) => break,
                    None => return Some(cursor),
                }
            }
        }
        None
    }

    /// 返回指定容量名下的已用字节数。
    fn used_for(&self, owner: CapacityId) -> u64 {
        self.allocations
            .values()
            .filter(|allocation| allocation.owner == Some(owner))
            .fold(0, |sum, allocation| {
                sum.saturating_add(allocation.range.len())
            })
    }

    /// 把一段区间放回空闲表并与相邻区间合并。
    fn coalesce_free(&mut self, range: PhysRange) {
        let start = range.start().get();
        let mut merged = range;
        if let Some(previous) = self
            .free_by_start
            .range(..start)
            .next_back()
            .map(|(_, range)| *range)
            && previous.end().get() == merged.start().get()
        {
            self.take_free(previous.start().get());
            merged = PhysRange::new(
                previous.start(),
                previous.len().saturating_add(merged.len()),
            );
        }
        let next_start = merged.end().get();
        if let Some(next) = self
            .free_by_start
            .range(next_start..)
            .next()
            .map(|(_, range)| *range)
            && next.start().get() == next_start
        {
            self.take_free(next.start().get());
            merged = PhysRange::new(merged.start(), merged.len().saturating_add(next.len()));
        }
        self.insert_free(merged);
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
                owned: BTreeMap::new(),
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
        inner.owned.clear();
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
        self.allocate_contiguous_owned(kind, len, None)
    }

    /// 按用途与归属容量分配一段连续物理内存。
    ///
    /// `owner` 为 `Some` 时只从该容量登记的窗口内取页；为 `None` 时从全部
    /// 未归属的空闲区间取页（内核自用）。
    fn allocate_contiguous_owned(
        &self,
        kind: FrameKind,
        len: usize,
        owner: Option<CapacityId>,
    ) -> Option<PhysRange> {
        let length = (len as u64).div_ceil(PAGE_SIZE as u64) * PAGE_SIZE as u64;
        if length == 0 {
            return None;
        }
        let mut inner = self.inner.lock();
        let start = match owner {
            Some(owner) => inner.find_free_in_owned(owner, length)?,
            None => inner.find_free_unowned(length)?,
        };
        let charge = inner.ledger.charge(length).ok()?;
        let Some(allocated) = inner.carve_free(start, length) else {
            let _ = inner.ledger.release(charge);
            return None;
        };
        inner.allocations.insert(
            start,
            Allocation {
                range: allocated,
                kind,
                charge,
                owner,
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
    ///
    /// 归还在释放内部锁后清零区间，避免在持锁期间访问物理内存。
    pub fn free_for(&self, _kind: FrameKind, range: PhysRange) {
        if range.is_empty() {
            return;
        }
        {
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
            inner.coalesce_free(range);
        }
        crate::hmp::capacity::scrub(range.start(), range.len() as usize);
    }

    /// 归还一段用途为 `Other` 的物理区间。
    pub fn free(&self, range: PhysRange) {
        self.free_for(FrameKind::Other, range);
    }

    /// 从空闲空间预留一段区间并登记给指定容量，返回区间。
    ///
    /// 预留会从空闲表中切出该区间，再把它记为容量归属；区间内仍按页空闲，
    /// 但只有该容量能分配。启动期为 DMA 池预留连续区域时使用。
    ///
    /// 每个容量只允许预留一次；重复预留会返回 `None`，避免同一 owner 名下
    /// 出现多个窗口导致归因歧义。
    pub fn reserve_for(&self, owner: CapacityId, len: usize) -> Option<PhysRange> {
        let length = (len as u64).div_ceil(PAGE_SIZE as u64) * PAGE_SIZE as u64;
        if length == 0 {
            return None;
        }
        let mut inner = self.inner.lock();
        if inner.owned.values().any(|window| window.capacity == owner) {
            return None;
        }
        let start = inner.find_free_unowned(length)?;
        let range = inner.take_free(start)?;
        let start = range.start();
        let remaining = range.len() - length;
        if remaining > 0 {
            inner.insert_free(PhysRange::new(
                PhysAddr::new(start.get() + length),
                remaining,
            ));
        }
        let reserved = PhysRange::new(start, length);
        // 区间仍页空闲，但归属该容量：重新按页登记为可分配但仍属该容量。
        inner.insert_free(reserved);
        inner.owned.insert(
            start.get(),
            OwnedWindow {
                capacity: owner,
                range: reserved,
            },
        );
        Some(reserved)
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

impl PhysicalBackend for FrameBank {
    fn split_owned(&self, parent: CapacityId, child: CapacityId, len: u64) -> Option<PhysRange> {
        let mut inner = self.inner.lock();
        // 一个容量可能因非相邻合并而持有多个窗口；逐个尝试从窗口尾部切出，
        // 只有尾部整段空闲的窗口才能成为子窗口。
        let windows: Vec<(u64, PhysRange)> = inner
            .owned
            .iter()
            .filter(|(_, window)| window.capacity == parent)
            .map(|(&start, window)| (start, window.range))
            .collect();
        for (parent_start, parent_range) in windows {
            let parent_end = parent_range.end().get();
            let Some(start) = parent_end.checked_sub(len) else {
                continue;
            };
            if start < parent_start {
                continue;
            }
            // 子窗口必须整段空闲；必要时从更大的空闲条目中切出。
            let Some(range) = inner.carve_free(start, len) else {
                continue;
            };
            // 子区间作为独立空闲条目重新插入，与它的窗口对齐。
            inner.insert_free(range);
            if start == parent_start {
                // 整个窗口都切成子窗口：父窗口不再保留零长度条目。
                inner.owned.remove(&parent_start);
            } else if let Some(window) = inner.owned.get_mut(&parent_start) {
                window.range = PhysRange::new(PhysAddr::new(parent_start), start - parent_start);
            }
            inner.owned.insert(
                start,
                OwnedWindow {
                    capacity: child,
                    range,
                },
            );
            return Some(range);
        }
        None
    }

    fn merge_owned(
        &self,
        parent: CapacityId,
        child: CapacityId,
        range: PhysRange,
    ) -> Result<(), HmpError> {
        let mut inner = self.inner.lock();
        if inner.used_for(child) != 0 {
            return Err(HmpError::PermissionDenied);
        }
        let start = range.start().get();
        let Some(window) = inner.owned.remove(&start) else {
            // 后端没有该区间的窗口：视为已经并回，保持幂等。
            return Ok(());
        };
        if window.capacity != child {
            inner.owned.insert(start, window);
            return Err(HmpError::WrongCapability);
        }
        // 只在与父窗口相邻时并回，绝不把中间的其它区间吞进父窗口。
        let adjacent = inner
            .owned
            .iter()
            .find(|(_, window)| window.capacity == parent)
            .map(|(&parent_start, window)| (parent_start, window.range));
        let Some((parent_start, parent_range)) = adjacent else {
            // 没有父窗口可并：区间保留在父容量名下。
            inner.owned.insert(
                start,
                OwnedWindow {
                    capacity: parent,
                    range,
                },
            );
            return Ok(());
        };
        if parent_range.end().get() == start || range.end().get() == parent_range.start().get() {
            let begin = parent_range.start().get().min(start);
            let end = parent_range.end().get().max(range.end().get());
            if let Some(parent_window) = inner.owned.get_mut(&parent_start) {
                parent_window.range = PhysRange::new(PhysAddr::new(begin), end - begin);
            }
        } else {
            // 不相邻：作为父容量的另一个窗口保留，不扩张已有窗口。
            inner.owned.insert(
                start,
                OwnedWindow {
                    capacity: parent,
                    range,
                },
            );
        }
        Ok(())
    }

    fn allocate_owned(&self, owner: CapacityId, kind: CapacityKind, len: u64) -> Option<PhysRange> {
        self.allocate_contiguous_owned(FrameKind::from_capacity(kind), len as usize, Some(owner))
    }

    fn release_owned(&self, owner: CapacityId, range: PhysRange) -> bool {
        let mut inner = self.inner.lock();
        let start = range.start().get();
        let Some(allocation) = inner.allocations.get(&start) else {
            return false;
        };
        if allocation.owner != Some(owner) || allocation.range != range {
            return false;
        }
        let Some(allocation) = inner.allocations.remove(&start) else {
            return false;
        };
        let _ = inner.ledger.release(allocation.charge);
        let slot = &mut inner.by_kind[allocation.kind.index()];
        *slot = slot.saturating_sub(range.len());
        inner.coalesce_free(range);
        true
    }

    fn used_owned(&self, owner: CapacityId) -> u64 {
        self.inner.lock().used_for(owner)
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

    #[test]
    fn reserved_window_only_serves_its_owner() {
        let bank = FrameBank::from_range(0x10_0000, 0x10_4000);
        let owner = CapacityId::next();
        let reserved = bank.reserve_for(owner, 0x2000).expect("预留");
        assert_eq!(reserved.start(), PhysAddr::new(0x10_0000));

        // 未归属的分配跳过预留窗口，从其后取页。
        let unowned = bank.allocate().expect("内核页");
        assert_eq!(unowned, PhysAddr::new(0x10_2000));

        // 归属分配只能从预留窗口内取页。
        let owned = bank
            .allocate_owned(owner, CapacityKind::User, PAGE_SIZE as u64)
            .expect("归属页");
        assert_eq!(owned.start(), PhysAddr::new(0x10_0000));
        assert_eq!(bank.used_owned(owner), PAGE_SIZE as u64);

        // 再次归属分配取窗口内下一页。
        let second = bank
            .allocate_owned(owner, CapacityKind::User, PAGE_SIZE as u64)
            .expect("归属第二页");
        assert_eq!(second.start(), PhysAddr::new(0x10_1000));
        assert_eq!(bank.used_owned(owner), 2 * PAGE_SIZE as u64);

        // 窗口耗尽后拒绝。
        assert!(
            bank.allocate_owned(owner, CapacityKind::User, PAGE_SIZE as u64)
                .is_none()
        );
    }

    #[test]
    fn released_owned_range_returns_to_window() {
        let bank = FrameBank::from_range(0x10_0000, 0x10_2000);
        let owner = CapacityId::next();
        bank.reserve_for(owner, 0x2000).expect("预留");
        let range = bank
            .allocate_owned(owner, CapacityKind::Dma, PAGE_SIZE as u64)
            .expect("归属页");
        assert_eq!(bank.used_owned(owner), PAGE_SIZE as u64);

        assert!(bank.release_owned(owner, range));
        assert_eq!(bank.used_owned(owner), 0);

        let reused = bank
            .allocate_owned(owner, CapacityKind::Dma, PAGE_SIZE as u64)
            .expect("复用");
        assert_eq!(reused.start(), range.start());
    }

    #[test]
    fn split_carves_child_window_from_parent() {
        let bank = FrameBank::from_range(0x10_0000, 0x10_4000);
        let parent = CapacityId::next();
        let child = CapacityId::next();
        bank.reserve_for(parent, 0x4000).expect("预留父窗口");

        let child_range = bank.split_owned(parent, child, 0x2000).expect("切出子窗口");
        assert_eq!(child_range.start(), PhysAddr::new(0x10_2000));

        let child_page = bank
            .allocate_owned(child, CapacityKind::User, PAGE_SIZE as u64)
            .expect("子分配");
        assert_eq!(child_page.start(), PhysAddr::new(0x10_2000));
        assert_eq!(bank.used_owned(child), PAGE_SIZE as u64);
        assert_eq!(bank.used_owned(parent), 0);

        let parent_page = bank
            .allocate_owned(parent, CapacityKind::User, PAGE_SIZE as u64)
            .expect("父分配");
        assert_eq!(parent_page.start(), PhysAddr::new(0x10_0000));
    }

    #[test]
    fn reserve_rejects_second_window_for_same_owner() {
        let bank = FrameBank::from_range(0x10_0000, 0x10_4000);
        let owner = CapacityId::next();
        bank.reserve_for(owner, 0x2000).expect("首次预留");
        assert!(
            bank.reserve_for(owner, 0x2000).is_none(),
            "同一容量不允许预留第二个窗口"
        );
        // 另一个容量仍可预留。
        let other = CapacityId::next();
        assert!(bank.reserve_for(other, 0x2000).is_some());
    }

    #[test]
    fn merge_rejoins_adjacent_child_window() {
        let bank = FrameBank::from_range(0x10_0000, 0x10_4000);
        let parent = CapacityId::next();
        let child = CapacityId::next();
        bank.reserve_for(parent, 0x4000).expect("预留父窗口");
        let child_range = bank.split_owned(parent, child, 0x2000).expect("切出子窗口");

        bank.merge_owned(parent, child, child_range)
            .expect("并回子窗口");
        // 子窗口并回父窗口后，父容量可以分配整段 0x4000。
        let whole = bank
            .allocate_owned(parent, CapacityKind::User, 0x4000)
            .expect("父容量应能取回整段");
        assert_eq!(whole.start(), PhysAddr::new(0x10_0000));
        assert_eq!(whole.len(), 0x4000);
    }

    #[test]
    fn merge_keeps_non_adjacent_window_separate() {
        let bank = FrameBank::from_range(0x10_0000, 0x10_8000);
        let parent = CapacityId::next();
        let first = CapacityId::next();
        let second = CapacityId::next();
        bank.reserve_for(parent, 0x8000).expect("预留父窗口");

        // 先切出后段给 second，再切出中段给 first，使它们不相邻。
        let second_range = bank.split_owned(parent, second, 0x2000).expect("切后段");
        let first_range = bank.split_owned(parent, first, 0x2000).expect("切中段");
        assert_eq!(second_range.start(), PhysAddr::new(0x10_6000));
        assert_eq!(first_range.start(), PhysAddr::new(0x10_4000));

        // 父窗口是 0x10_0000..0x10_4000，first 紧邻其后 → 相邻合并。
        bank.merge_owned(parent, first, first_range)
            .expect("并回相邻子窗口");
        let page = bank
            .allocate_owned(parent, CapacityKind::User, 0x4000)
            .expect("相邻合并后父容量可取 0x4000");
        assert_eq!(page.start(), PhysAddr::new(0x10_0000));

        // second 之后并入父容量，不再归 second 所有。
        bank.merge_owned(parent, second, second_range)
            .expect("并回 second");
        assert!(
            bank.allocate_owned(second, CapacityKind::User, PAGE_SIZE as u64)
                .is_none(),
            "并回后不再归 second 所有"
        );
    }

    #[test]
    fn merge_owned_rejects_child_with_live_allocations() {
        let bank = FrameBank::from_range(0x10_0000, 0x10_4000);
        let parent = CapacityId::next();
        let child = CapacityId::next();
        bank.reserve_for(parent, 0x4000).expect("预留父窗口");
        let child_range = bank.split_owned(parent, child, 0x2000).expect("切出子窗口");
        let page = bank
            .allocate_owned(child, CapacityKind::User, PAGE_SIZE as u64)
            .expect("子页");

        assert_eq!(
            bank.merge_owned(parent, child, child_range),
            Err(HmpError::PermissionDenied),
            "子容量仍有分配时后端拒绝合并"
        );
        // 拒绝后窗口与分配都保留在子容量名下。
        assert_eq!(bank.used_owned(child), PAGE_SIZE as u64);
        assert!(bank.release_owned(child, page));
        assert_eq!(bank.used_owned(child), 0);
    }

    #[test]
    fn merge_owned_rejects_window_of_other_capacity() {
        let bank = FrameBank::from_range(0x10_0000, 0x10_4000);
        let parent = CapacityId::next();
        let child = CapacityId::next();
        let other = CapacityId::next();
        bank.reserve_for(parent, 0x4000).expect("预留父窗口");
        let child_range = bank.split_owned(parent, child, 0x2000).expect("切出子窗口");

        assert_eq!(
            bank.merge_owned(parent, other, child_range),
            Err(HmpError::WrongCapability),
            "窗口归属与传入容量不一致时拒绝"
        );
        // 窗口仍在 child 名下。
        assert!(
            bank.allocate_owned(child, CapacityKind::User, PAGE_SIZE as u64)
                .is_some()
        );
    }

    #[test]
    fn merge_owned_is_idempotent_without_window() {
        let bank = FrameBank::from_range(0x10_0000, 0x10_2000);
        let range = PhysRange::new(PhysAddr::new(0x10_0000), 0x1000);
        assert_eq!(
            bank.merge_owned(CapacityId::next(), CapacityId::next(), range),
            Ok(()),
            "没有归属窗口时合并没有副作用"
        );
    }

    #[test]
    fn split_owned_uses_window_whose_tail_is_free() {
        let bank = FrameBank::from_range(0x10_0000, 0x11_0000);
        let parent = CapacityId::next();
        let first = CapacityId::next();
        let second = CapacityId::next();

        bank.reserve_for(parent, 0x1_0000).expect("预留父窗口");
        let second_range = bank.split_owned(parent, second, 0x2000).expect("切后段");
        let first_range = bank.split_owned(parent, first, 0x2000).expect("切中段");
        assert_eq!(second_range.start(), PhysAddr::new(0x10_E000));
        assert_eq!(first_range.start(), PhysAddr::new(0x10_C000));

        // 占满父容量第一个窗口（0x10_0000..0x10_C000）。
        let whole = bank
            .allocate_owned(parent, CapacityKind::User, 0xC000)
            .expect("占满第一个窗口");
        assert_eq!(whole.start(), PhysAddr::new(0x10_0000));

        // second 与父窗口隔着 first 的区间，并回后成为父容量的第二个窗口。
        bank.merge_owned(parent, second, second_range)
            .expect("并回不相邻子窗口");

        // 第一个窗口已无空闲尾部；切分应落到第二个窗口而不是误报失败。
        let child = CapacityId::next();
        let carved = bank
            .split_owned(parent, child, 0x2000)
            .expect("从第二个窗口切出");
        assert_eq!(carved.start(), PhysAddr::new(0x10_E000));
        assert_eq!(bank.used_owned(child), 0);
        assert_eq!(bank.used_owned(parent), 0xC000);
    }
}
