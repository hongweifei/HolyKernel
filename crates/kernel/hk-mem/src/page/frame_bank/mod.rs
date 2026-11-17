//! 系统物理帧银行：从全部可用内存区域分配与回收连续物理页。
//!
//! 这是全系统唯一的物理空闲表；HMP `Capacity` 只是它的配额视图，所有
//! 物理分配都记在这里，并按 `CapacityId` 归因。

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use hk_hal::{MemoryRegion, PAGE_SIZE, PhysAddr, PhysRange};
use spin::Mutex;

use crate::hmp::{CapacityId, CapacityKind, Charge, Ledger, LedgerId};

mod inner;

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
    /// 把 Capacity 用途映射到 FrameBank 分配用途。
    pub const fn from_capacity(kind: CapacityKind) -> Self {
        match kind {
            CapacityKind::System => Self::Other,
            CapacityKind::User => Self::Image,
            CapacityKind::Dma => Self::Dma,
            CapacityKind::Gpu => Self::Other,
            CapacityKind::Numa(_) => Self::Other,
        }
    }

    /// 判断该用途的页是否属于用户地址空间。
    ///
    /// 用户镜像、写时复制与检查点恢复页都映射在用户低半区；内核堆、
    /// 页表与内核自用页位于内核高半区。System Arena 按此区分用户页表
    /// 页与内核页表页。
    pub const fn is_user_space(self) -> bool {
        matches!(self, Self::Image | Self::Cow | Self::Restore)
    }
}

/// 支持的 NUMA 节点上限。
pub const MAX_NODES: usize = hk_syscall::MAX_NUMA_NODES;

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
    /// 各 Capacity 用途已用字节数：System、User、Dma、Gpu、Numa。
    pub by_capacity: [u64; CapacityKind::COUNT],
    /// 各 NUMA 节点已用字节数；未配置拓扑时只有节点 0。
    pub by_node: [u64; MAX_NODES],
}

/// 单个 Capacity 的 Ledger 快照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedgerStats {
    /// Capacity 标识。
    pub capacity: CapacityId,
    /// Capacity 用途。
    pub kind: CapacityKind,
    /// 当前已记账字节数。
    pub used: u64,
    /// 硬上限。
    pub limit: u64,
    /// 峰值。
    pub peak: u64,
    /// 常驻字节数快照。
    pub resident: u64,
    /// 软上限；0 表示未设置。
    pub soft_limit: u64,
    /// 压力百分比（0–100）。
    pub pressure: u64,
}

struct Allocation {
    range: PhysRange,
    kind: FrameKind,
    charge: Charge,
    /// 归属容量自己的 Ledger 记账；内核自用容量同样有一本账。
    owner_charge: Option<Charge>,
    /// 归属容量；`None` 表示内核自用。
    owner: Option<CapacityId>,
    /// 物理页归 HMP 对象持有（`OwnedFrame`），页表拆除时不得代为归还。
    object_owned: bool,
    /// 分配所在的 NUMA 节点。
    node: u8,
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
    /// 容量 Ledger 的用途标签，用于枚举。
    ledger_kinds: BTreeMap<CapacityId, CapacityKind>,
    allocations: BTreeMap<u64, Allocation>,
    by_capacity: [u64; CapacityKind::COUNT],
    /// 物理邻近域拓扑；空表示单节点 0。
    nodes: Vec<(PhysRange, u8)>,
    by_node: [u64; MAX_NODES],
    /// 已登记给容量的窗口，按起始地址索引。
    owned: BTreeMap<u64, OwnedWindow>,
    /// 内核自用分配的归属容量；`None` 表示尚未建立 System Capacity。
    kernel_owner: Option<CapacityId>,
    /// 各容量的峰值用量。
    peaks: BTreeMap<CapacityId, u64>,
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
                ledger_kinds: BTreeMap::new(),
                allocations: BTreeMap::new(),
                by_capacity: [0; CapacityKind::COUNT],
                nodes: Vec::new(),
                by_node: [0; MAX_NODES],
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

    /// 以 NUMA 拓扑初始化：空列表等价于单节点 0。
    pub fn init_with_nodes(
        &self,
        regions: &[MemoryRegion],
        kernel: MemoryRegion,
        nodes: &[(PhysRange, u8)],
    ) {
        self.init(regions, kernel);
        let mut inner = self.inner.lock();
        inner.nodes = nodes
            .iter()
            .filter(|(range, _)| !range.is_empty())
            .copied()
            .collect();
        inner.split_free_by_nodes();
    }

    /// 在内存接管之后设置 NUMA 拓扑。
    ///
    /// 启动期先按 Limine 内存图建立空闲表，ACPI 表解析完成后再调用这里：空闲
    /// 条目按节点边界切分，已分配区间的节点归属按物理地址重算，统计随之一致。
    pub fn set_nodes(&self, nodes: &[(PhysRange, u8)]) {
        let mut inner = self.inner.lock();
        inner.nodes = nodes
            .iter()
            .filter(|(range, _)| !range.is_empty())
            .copied()
            .collect();
        inner.split_free_by_nodes();
        let updates: Vec<(u64, u64, u8)> = inner
            .allocations
            .values()
            .map(|allocation| {
                (
                    allocation.range.start().get(),
                    allocation.range.len(),
                    inner.node_for(allocation.range.start().get()),
                )
            })
            .collect();
        for (start, _, node) in &updates {
            if let Some(allocation) = inner.allocations.get_mut(start) {
                allocation.node = *node;
            }
        }
        inner.by_node = [0; MAX_NODES];
        for (_, length, node) in updates {
            let slot = &mut inner.by_node[usize::from(node) % MAX_NODES];
            *slot = slot.saturating_add(length);
        }
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
        inner.by_capacity = [0; CapacityKind::COUNT];
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
        self.allocate_contiguous_owned(kind, len, None, PAGE_SIZE as u64, None)
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
        align: u64,
        node: Option<u8>,
    ) -> Option<PhysRange> {
        let align = align.max(PAGE_SIZE as u64);
        let length = (len as u64).div_ceil(PAGE_SIZE as u64) * PAGE_SIZE as u64;
        if length == 0 {
            return None;
        }
        let mut inner = self.inner.lock();
        // 未显式指定归属时落到内核自用容量（System Capacity），内核堆、页表与
        // 镜像页因此计入同一份归因账。
        let owner = owner.or(inner.kernel_owner);
        let capacity_kind = owner
            .and_then(|owner| inner.ledger_kinds.get(&owner).copied())
            .unwrap_or(CapacityKind::System);
        let start = match owner {
            Some(owner) => inner.find_free_in_owned(owner, length, align, node)?,
            None => inner.find_free_unowned(length, align, node)?,
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
        let node = inner.node_for(allocated.start().get());
        inner.allocations.insert(
            start,
            Allocation {
                range: allocated,
                kind,
                charge,
                owner_charge,
                owner,
                object_owned: false,
                node,
            },
        );
        let slot = &mut inner.by_capacity[capacity_kind.index()];
        *slot = slot.saturating_add(length);
        let node_slot = &mut inner.by_node[usize::from(node) % MAX_NODES];
        *node_slot = node_slot.saturating_add(length);
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
            let owned_by_object = inner
                .allocations
                .get(&start)
                .is_some_and(|allocation| allocation.object_owned);
            if owned_by_object {
                // 页表拆除只解除映射；物理页由 OwnedFrame 归还。
                return;
            }
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
            let capacity_kind = allocation
                .owner
                .and_then(|owner| inner.ledger_kinds.get(&owner).copied())
                .unwrap_or(CapacityKind::System);
            let _ = inner.ledger.release(allocation.charge);
            if let (Some(owner), Some(owner_charge)) = (allocation.owner, allocation.owner_charge)
                && let Some(ledger) = inner.ledgers.get_mut(&owner)
            {
                let _ = ledger.release(owner_charge);
            }
            let slot = &mut inner.by_capacity[capacity_kind.index()];
            *slot = slot.saturating_sub(range.len());
            let node_slot = &mut inner.by_node[usize::from(allocation.node) % MAX_NODES];
            *node_slot = node_slot.saturating_sub(range.len());
            inner.coalesce_free(range);
        }
        crate::hmp::capacity::scrub(range.start(), range.len() as usize);
    }

    /// 归还一段用途为 `Other` 的物理区间。
    pub fn free(&self, range: PhysRange) {
        self.free_for(FrameKind::Other, range);
    }

    /// 标记一段已分配区间由 HMP 对象持有；页表拆除不再归还它。
    pub fn mark_object_owned(&self, range: PhysRange) {
        let mut inner = self.inner.lock();
        if let Some(allocation) = inner.allocations.get_mut(&range.start().get())
            && allocation.range == range
        {
            allocation.object_owned = true;
        }
    }

    /// 从空闲空间预留一段区间并登记给指定容量，返回区间。
    ///
    /// 预留会从空闲表中切出该区间，再把它记为容量归属；区间内仍按页空闲，
    /// 但只有该容量能分配。启动期为 DMA 分配器预留连续区域时使用。
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
        let start = inner.find_free_unowned(length, PAGE_SIZE as u64, None)?;
        let range = inner.carve_free(start, length)?;
        // 区间仍页空闲，但归属该容量：按节点边界重新登记为可分配但仍属该容量。
        for piece in inner.node_pieces(range) {
            inner.insert_free(piece);
        }
        inner.owned.insert(
            range.start().get(),
            OwnedWindow {
                capacity: owner,
                range,
            },
        );
        Some(range)
    }

    /// 把当前全部未归属的空闲区间登记给 `owner`，返回这些区间的外接范围。
    ///
    /// 用于启动期建立覆盖全部可用物理内存的 System Capacity：已经预留的窗口
    /// （如 DMA 分配器）保持原归属，其余空闲区间成为 `owner` 的窗口。一个容量因此
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
        let old_kind = inner
            .allocations
            .get(&range.start().get())
            .and_then(|allocation| allocation.owner)
            .and_then(|owner| inner.ledger_kinds.get(&owner).copied())
            .unwrap_or(CapacityKind::System);
        let new_kind = inner
            .ledger_kinds
            .get(&owner)
            .copied()
            .unwrap_or(CapacityKind::System);
        if old_kind != new_kind {
            // 采纳改变了物理用量的分类归属，统计必须跟着迁移。
            let old = &mut inner.by_capacity[old_kind.index()];
            *old = old.saturating_sub(length);
            let new = &mut inner.by_capacity[new_kind.index()];
            *new = new.saturating_add(length);
        }
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

    /// 登记容量 Ledger 的用途标签；重复登记以最后一次为准。
    pub fn set_ledger_kind(&self, owner: CapacityId, kind: CapacityKind) {
        self.inner.lock().ledger_kinds.insert(owner, kind);
    }

    /// 返回按 CapacityId 排序的全部容量 Ledger 快照。
    pub fn ledger_stats(&self) -> Vec<LedgerStats> {
        let inner = self.inner.lock();
        inner
            .ledgers
            .iter()
            .filter_map(|(owner, ledger)| {
                let kind = *inner.ledger_kinds.get(owner)?;
                Some(LedgerStats {
                    capacity: *owner,
                    kind,
                    used: ledger.charged(),
                    limit: ledger.limit(),
                    peak: ledger.peak(),
                    resident: ledger.resident(),
                    soft_limit: ledger.soft_limit(),
                    pressure: ledger.pressure(),
                })
            })
            .collect()
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
        inner.ledger_kinds.remove(&owner);
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

    /// 枚举指定归属容量的全部分配记录，按物理起始地址排序。
    ///
    /// 每条记录为 `(物理区间, 分配用途)`；供 System Arena 把内核自用
    /// 内存登记为 Region。`owner` 为 `None` 时返回尚未归属的启动期分配。
    pub fn allocations_of(&self, owner: Option<CapacityId>) -> Vec<(PhysRange, FrameKind)> {
        let inner = self.inner.lock();
        inner
            .allocations
            .values()
            .filter(|allocation| allocation.owner == owner)
            .map(|allocation| (allocation.range, allocation.kind))
            .collect()
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
            by_capacity: inner.by_capacity,
            by_node: inner.by_node,
        }
    }

    /// 返回指定节点当前空闲字节数；未配置拓扑时节点 0 覆盖全部空闲页。
    pub fn free_in_node(&self, node: u8) -> u64 {
        let inner = self.inner.lock();
        inner
            .free_by_start
            .values()
            .filter(|range| inner.node_for(range.start().get()) == node)
            .map(|range| range.len())
            .fold(0, u64::saturating_add)
    }

    /// 返回一段已分配物理区间所属的 NUMA 节点。
    pub fn node_of(&self, range: PhysRange) -> Option<u8> {
        let inner = self.inner.lock();
        let allocation = inner.allocations.get(&range.start().get())?;
        (allocation.range == range).then_some(allocation.node)
    }
}

mod backend;

#[cfg(test)]
mod tests;

/// 64 位向上取整；`align` 为 0 时按 1 处理。
fn align_up_u64(value: u64, align: u64) -> Option<u64> {
    let align = align.max(1);
    value
        .checked_add(align - 1)
        .map(|value| value / align * align)
}
