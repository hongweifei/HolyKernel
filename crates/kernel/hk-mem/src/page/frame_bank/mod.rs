//! 系统物理帧银行：从全部可用内存区域分配与回收连续物理页。
//!
//! 这是全系统唯一的物理空闲表；HMP `Capacity` 只是它的配额视图，所有
//! 物理分配都记在这里，并按 `CapacityId` 归因。

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use hk_hal::{MemoryRegion, PAGE_SIZE, PhysAddr, PhysRange};
use spin::Mutex;

use crate::hmp::{CapacityId, CapacityKind, Charge, Ledger, LedgerId};

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
    /// 归属容量自己的 Ledger 记账；内核自用容量同样有一本账。
    owner_charge: Option<Charge>,
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
    /// 全系统物理用量。
    ledger: Ledger,
    /// 每个容量自己的 Ledger。
    ledgers: BTreeMap<CapacityId, Ledger>,
    allocations: BTreeMap<u64, Allocation>,
    by_kind: [u64; FrameKind::COUNT],
    /// 已登记给容量的窗口，按起始地址索引。
    owned: BTreeMap<u64, OwnedWindow>,
    /// 内核自用分配的归属容量；`None` 表示尚未建立 System Capacity。
    kernel_owner: Option<CapacityId>,
    /// 各容量的峰值用量。
    peaks: BTreeMap<CapacityId, u64>,
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

    /// 返回指定容量名下的已用字节数；优先读该容量的 Ledger。
    fn used_for(&self, owner: CapacityId) -> u64 {
        if let Some(ledger) = self.ledgers.get(&owner) {
            return ledger.charged();
        }
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
                ledgers: BTreeMap::new(),
                allocations: BTreeMap::new(),
                by_kind: [0; FrameKind::COUNT],
                owned: BTreeMap::new(),
                kernel_owner: None,
                peaks: BTreeMap::new(),
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
        inner.peaks.clear();
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
        // 未显式指定归属时落到内核自用容量（System Capacity），内核堆、页表与
        // 镜像页因此计入同一份归因账。
        let owner = owner.or(inner.kernel_owner);
        let start = match owner {
            Some(owner) => inner.find_free_in_owned(owner, length)?,
            None => inner.find_free_unowned(length)?,
        };
        let charge = inner.ledger.charge(length).ok()?;
        let total = inner.total;
        let owner_charge = match owner {
            Some(owner) => {
                let ledger = inner
                    .ledgers
                    .entry(owner)
                    .or_insert_with(|| Ledger::create(total));
                match ledger.charge(length) {
                    Ok(charge) => Some(charge),
                    Err(_) => {
                        let _ = inner.ledger.release(charge);
                        return None;
                    }
                }
            }
            None => None,
        };
        let Some(allocated) = inner.carve_free(start, length) else {
            let _ = inner.ledger.release(charge);
            if let (Some(owner), Some(owner_charge)) = (owner, owner_charge)
                && let Some(ledger) = inner.ledgers.get_mut(&owner)
            {
                let _ = ledger.release(owner_charge);
            }
            return None;
        };
        if let Some(owner) = owner {
            let used = inner.used_for(owner);
            let peak = inner.peaks.entry(owner).or_insert(0);
            *peak = (*peak).max(used);
        }
        inner.allocations.insert(
            start,
            Allocation {
                range: allocated,
                kind,
                charge,
                owner_charge,
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
            if let (Some(owner), Some(owner_charge)) = (allocation.owner, allocation.owner_charge)
                && let Some(ledger) = inner.ledgers.get_mut(&owner)
            {
                let _ = ledger.release(owner_charge);
            }
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

    /// 把当前全部未归属的空闲区间登记给 `owner`，返回这些区间的外接范围。
    ///
    /// 用于启动期建立覆盖全部可用物理内存的 System Capacity：已经预留的窗口
    /// （如 DMA 池）保持原归属，其余空闲区间成为 `owner` 的窗口。一个容量因此
    /// 可以持有多个窗口，分配与清理都按窗口逐个进行。
    pub fn reserve_remaining(&self, owner: CapacityId) -> Option<PhysRange> {
        let mut inner = self.inner.lock();
        if inner.owned.values().any(|window| window.capacity == owner) {
            return None;
        }
        let covered: Vec<PhysRange> = inner.owned.values().map(|window| window.range).collect();
        let entries: Vec<PhysRange> = inner.free_by_start.values().copied().collect();
        let mut windows: Vec<PhysRange> = Vec::new();
        for entry in entries {
            let end = entry.end().get();
            let mut cursor = entry.start().get();
            for window in &covered {
                let start = window.start().get();
                let finish = window.end().get();
                if finish <= cursor || start >= end {
                    continue;
                }
                if start > cursor {
                    windows.push(PhysRange::new(PhysAddr::new(cursor), start - cursor));
                }
                cursor = cursor.max(finish);
            }
            if cursor < end {
                windows.push(PhysRange::new(PhysAddr::new(cursor), end - cursor));
            }
        }
        let first = windows.first()?.start();
        let last = windows.last()?.end();
        for window in &windows {
            inner.owned.insert(
                window.start().get(),
                OwnedWindow {
                    capacity: owner,
                    range: *window,
                },
            );
        }
        Some(PhysRange::new(first, last.get() - first.get()))
    }

    /// 把一段已分配区间改归 `owner` 名下；区间必须与现有分配记录一致。
    ///
    /// 启动期内核堆在 System Capacity 建立前就已分配，这里把它并入系统归因，
    /// 不改变已记入 Ledger 的用量。
    pub fn adopt_owner(&self, owner: CapacityId, range: PhysRange) -> bool {
        let mut inner = self.inner.lock();
        let (length, charged) = match inner.allocations.get(&range.start().get()) {
            Some(allocation) if allocation.range == range => {
                (allocation.range.len(), allocation.owner_charge)
            }
            _ => return false,
        };
        let owner_charge = if charged.is_some() {
            charged
        } else {
            let total = inner.total;
            let ledger = inner
                .ledgers
                .entry(owner)
                .or_insert_with(|| Ledger::create(total));
            match ledger.charge(length) {
                Ok(charge) => Some(charge),
                Err(_) => return false,
            }
        };
        let Some(allocation) = inner.allocations.get_mut(&range.start().get()) else {
            return false;
        };
        allocation.owner = Some(owner);
        allocation.owner_charge = owner_charge;
        let used = inner.used_for(owner);
        let peak = inner.peaks.entry(owner).or_insert(0);
        *peak = (*peak).max(used);
        true
    }

    /// 把启动期所有未归属的分配一次性采纳到 `owner` 名下。
    ///
    /// 启动早期（System Capacity 建立前）的内核堆、缓存与页表分配只有全局
    /// 记账；这里统一补上容量归属，保证 System 账本能覆盖全部内核自用。
    /// 任一区间超出账本配额时回滚全部记账并返回 `false`。
    pub fn adopt_all_unowned(&self, owner: CapacityId) -> bool {
        let mut inner = self.inner.lock();
        let ranges: Vec<(u64, u64)> = inner
            .allocations
            .iter()
            .filter(|(_, allocation)| allocation.owner.is_none())
            .map(|(start, allocation)| (*start, allocation.range.len()))
            .collect();
        let total = inner.total;
        let mut charges: Vec<(u64, Charge)> = Vec::with_capacity(ranges.len());
        {
            let ledger = inner
                .ledgers
                .entry(owner)
                .or_insert_with(|| Ledger::create(total));
            for (start, length) in ranges {
                match ledger.charge(length) {
                    Ok(charge) => charges.push((start, charge)),
                    Err(_) => {
                        for (_, charge) in charges.drain(..) {
                            let _ = ledger.release(charge);
                        }
                        return false;
                    }
                }
            }
        }
        for (start, charge) in charges {
            if let Some(allocation) = inner.allocations.get_mut(&start) {
                allocation.owner = Some(owner);
                allocation.owner_charge = Some(charge);
            }
        }
        let used = inner.used_for(owner);
        let peak = inner.peaks.entry(owner).or_insert(0);
        *peak = (*peak).max(used);
        true
    }

    /// 注册一个容量自己的 Ledger；已存在时保留原上限。
    ///
    /// 上限的后续变化通过 `set_ledger_limit` 显式同步。System 账本由启动期
    /// 以物理总量注册，容量视图的用户配额（`Capacity::limit`）不覆盖它，
    /// 这样内核自用不会因为用户 split 掉配额而失败。
    pub fn register_ledger(&self, owner: CapacityId, limit: u64) {
        self.inner
            .lock()
            .ledgers
            .entry(owner)
            .or_insert_with(|| Ledger::create(limit));
    }

    /// 更新容量 Ledger 的硬上限。
    pub fn set_ledger_limit(&self, owner: CapacityId, limit: u64) {
        if let Some(ledger) = self.inner.lock().ledgers.get_mut(&owner) {
            ledger.set_limit(limit);
        }
    }

    /// 更新容量 Ledger 的软上限。
    pub fn set_ledger_soft_limit(&self, owner: CapacityId, limit: u64) {
        if let Some(ledger) = self.inner.lock().ledgers.get_mut(&owner) {
            ledger.set_soft_limit(limit);
        }
    }

    /// 删除容量 Ledger。
    pub fn remove_ledger(&self, owner: CapacityId) {
        let mut inner = self.inner.lock();
        inner.ledgers.remove(&owner);
        inner.peaks.remove(&owner);
    }

    /// 设置内核自用分配的归属容量。
    ///
    /// 设置后 `allocate` / `allocate_for` / `allocate_contiguous_for` 不再从
    /// 未归属空闲区间取页，而是从该容量的窗口取页并计入其用量。
    pub fn set_kernel_owner(&self, owner: Option<CapacityId>) {
        self.inner.lock().kernel_owner = owner;
    }

    /// 返回总字节数。
    pub fn total(&self) -> u64 {
        self.inner.lock().total
    }

    /// 返回一段物理页的分配用途；未登记时返回 `None`。
    pub fn kind_of(&self, phys: hk_hal::PhysAddr) -> Option<FrameKind> {
        self.inner
            .lock()
            .allocations
            .get(&phys.get())
            .map(|allocation| allocation.kind)
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

mod backend;

#[cfg(test)]
mod tests;
