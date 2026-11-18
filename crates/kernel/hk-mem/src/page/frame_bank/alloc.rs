//! 帧银行分配路径：初始化空闲表、切页、预留与释放。

use ::alloc::collections::BTreeMap;
use ::alloc::vec::Vec;

use hk_hal::{MemoryRegion, PAGE_SIZE, PhysAddr, PhysRange};

use super::{Allocation, FrameBank, FrameKind, Inner, MAX_NODES, OwnedWindow};
use crate::hmp::{CapacityId, CapacityKind, Ledger, LedgerId};

impl FrameBank {
    /// 构造空帧银行，供全局静态初始化。
    pub const fn empty() -> Self {
        Self {
            inner: spin::Mutex::new(Inner {
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
                parents: BTreeMap::new(),
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
        inner.parents.clear();
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
    pub(super) fn allocate_contiguous_owned(
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
            // 分配记录已移除，区间尚未进入空闲表；此时不会再有分配者拿到它。
            // 放锁后清零，再将区间并入空闲表，避免清零晚于重新分配。
            drop(inner);
            crate::hmp::capacity::scrub(range.start(), range.len() as usize);
            let mut inner = self.inner.lock();
            inner.coalesce_free(range);
        }
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
}
