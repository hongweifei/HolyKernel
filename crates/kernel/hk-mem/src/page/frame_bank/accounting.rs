//! 帧银行记账路径：容量 Ledger、用量统计与归属采纳。

use ::alloc::vec::Vec;

use hk_hal::{PhysAddr, PhysRange};

use super::{FrameBank, FrameBankStats, FrameKind, LedgerStats};
use crate::hmp::{CapacityId, CapacityKind, Charge, Ledger};

impl FrameBank {
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
    pub fn kind_of(&self, phys: PhysAddr) -> Option<FrameKind> {
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
