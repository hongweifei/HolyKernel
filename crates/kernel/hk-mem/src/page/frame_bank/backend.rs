//! `PhysicalBackend` 实现：窗口归因、按容量分配与回收。

use ::alloc::vec::Vec;

use hk_hal::{PAGE_SIZE, PhysAddr, PhysRange};

use super::{FrameBank, FrameKind, OwnedWindow};
use crate::hmp::{CapacityId, CapacityKind, HmpError, PhysicalBackend};

impl FrameBank {
    /// 回收容量窗口与 Ledger；`require_empty` 为 false 时用于析构兜底。
    fn reclaim_capacity_inner(
        &self,
        owner: CapacityId,
        parent: Option<CapacityId>,
        limit: u64,
        require_empty: bool,
    ) -> Result<(), HmpError> {
        let mut inner = self.inner.lock();
        if require_empty && inner.used_for(owner) != 0 {
            return Err(HmpError::PermissionDenied);
        }
        let windows: Vec<PhysRange> = inner
            .owned
            .iter()
            .filter(|(_, window)| window.capacity == owner)
            .map(|(_, window)| window.range)
            .collect();
        for range in &windows {
            inner.owned.remove(&range.start().get());
        }
        let mut parent = parent;
        let mut used_owner_link = false;
        // 父容量可能已被先 merge 掉；沿 split 记录向上找仍然存活的祖先。
        while let Some(pid) = parent {
            if inner.ledgers.contains_key(&pid) {
                break;
            }
            let next = inner.parents.get(&pid).copied();
            parent = match next {
                Some(parent) => Some(parent),
                None if !used_owner_link => {
                    used_owner_link = true;
                    inner.parents.get(&owner).copied()
                }
                None => None,
            };
        }
        if let Some(parent) = parent {
            for range in windows {
                inner.attach_window(parent, range);
            }
            if inner.ledger_kinds.get(&parent) != Some(&CapacityKind::System)
                && let Some(ledger) = inner.ledgers.get_mut(&parent)
            {
                ledger.set_limit(ledger.limit().saturating_add(limit));
            }
            // 孙容量的直接父可能已回收；把它们的父链改接到仍然存活的祖先。
            for link in inner.parents.values_mut() {
                if *link == owner {
                    *link = parent;
                }
            }
        } else {
            inner.parents.retain(|_, link| *link != owner);
        }
        inner.parents.remove(&owner);
        inner.ledgers.remove(&owner);
        inner.peaks.remove(&owner);
        // 强制回收（Capacity::drop）可能仍有 allocation 挂在旧 owner 上。
        // 保留 kind 让后续 release 记到正确统计桶；正常清空回收才删除。
        if require_empty {
            inner.ledger_kinds.remove(&owner);
        }
        Ok(())
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
            // 子区间按节点边界重新插入，避免一个空闲条目跨节点。
            for piece in inner.node_pieces(range) {
                inner.insert_free(piece);
            }
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
            inner.parents.insert(child, parent);
            return Some(range);
        }
        None
    }

    fn split_free_to(
        &self,
        parent: CapacityId,
        child: CapacityId,
        limit: u64,
    ) -> Option<(PhysRange, u64)> {
        if limit == 0 {
            return None;
        }
        let mut inner = self.inner.lock();
        let windows: Vec<PhysRange> = inner
            .owned
            .values()
            .filter(|window| window.capacity == parent)
            .map(|window| window.range)
            .collect();
        if windows.is_empty() {
            return None;
        }
        let mut candidates: Vec<PhysRange> = inner
            .free_by_start
            .values()
            .filter_map(|free| {
                windows.iter().find_map(|window| {
                    let start = free.start().get().max(window.start().get());
                    let end = free.end().get().min(window.end().get());
                    (end > start).then(|| PhysRange::new(PhysAddr::new(start), end - start))
                })
            })
            .collect();
        candidates.sort_unstable_by_key(|range| range.start().get());
        let mut taken: Vec<PhysRange> = Vec::new();
        let mut remaining = limit;
        for candidate in candidates {
            if remaining == 0 {
                break;
            }
            let length = candidate.len().min(remaining);
            let length = length / PAGE_SIZE as u64 * PAGE_SIZE as u64;
            if length == 0 {
                continue;
            }
            taken.push(PhysRange::new(candidate.start(), length));
            remaining -= length;
        }
        if taken.is_empty() {
            return None;
        }
        let moved: u64 = taken.iter().map(|range| range.len()).sum();
        let start = taken.first()?.start();
        let end = taken.last()?.end();
        for range in &taken {
            let carved = inner.carve_free(range.start().get(), range.len())?;
            for piece in inner.node_pieces(carved) {
                inner.insert_free(piece);
                inner.attach_window(child, piece);
            }
        }
        // 父窗口按实际转出的区间切分，剩余部分仍归父容量。
        inner.owned.retain(|_, window| window.capacity != parent);
        for window in windows {
            let mut segments = ::alloc::vec![window];
            for range in &taken {
                let mut next = Vec::new();
                for segment in segments {
                    if range.end().get() <= segment.start().get()
                        || range.start().get() >= segment.end().get()
                    {
                        next.push(segment);
                        continue;
                    }
                    let before = range.start().get().saturating_sub(segment.start().get());
                    if before > 0 {
                        next.push(PhysRange::new(segment.start(), before));
                    }
                    let after_start = range.end().get().max(segment.start().get());
                    if after_start < segment.end().get() {
                        next.push(PhysRange::new(
                            PhysAddr::new(after_start),
                            segment.end().get() - after_start,
                        ));
                    }
                }
                segments = next;
            }
            for segment in segments {
                if !segment.is_empty() {
                    inner.attach_window(parent, segment);
                }
            }
        }
        inner.parents.insert(child, parent);
        Some((PhysRange::new(start, end.get() - start.get()), moved))
    }
    fn split_owned_node(
        &self,
        parent: CapacityId,
        child: CapacityId,
        len: u64,
        node: u8,
    ) -> Option<PhysRange> {
        let mut inner = self.inner.lock();
        // 候选必须是父窗口内、指定节点、整段空闲且页对齐的区间。
        let candidates: Vec<u64> = inner
            .free_by_start
            .values()
            .filter(|range| range.len() >= len && inner.node_for(range.start().get()) == node)
            .filter_map(|range| {
                let start = range.start().get();
                inner
                    .owned
                    .values()
                    .any(|window| {
                        window.capacity == parent
                            && window.range.start().get() <= start
                            && start + len <= window.range.end().get()
                    })
                    .then_some(start)
            })
            .collect();
        for start in candidates {
            let Some(range) = inner.carve_free(start, len) else {
                continue;
            };
            // 父窗口可能被切成前后两段，按多窗口模型分别挂回父容量。
            let parent_start = inner
                .owned
                .iter()
                .find(|(_, window)| {
                    window.capacity == parent
                        && window.range.start().get() <= start
                        && start + len <= window.range.end().get()
                })
                .map(|(&start, window)| (start, window.range))?;
            inner.owned.remove(&parent_start.0);
            if parent_start.1.start().get() < start {
                inner.attach_window(
                    parent,
                    PhysRange::new(parent_start.1.start(), start - parent_start.1.start().get()),
                );
            }
            // 子窗口内的区间仍按页空闲，只是归属子容量。
            for piece in inner.node_pieces(range) {
                inner.insert_free(piece);
            }
            let after = start + len;
            if after < parent_start.1.end().get() {
                inner.attach_window(
                    parent,
                    PhysRange::new(PhysAddr::new(after), parent_start.1.end().get() - after),
                );
            }
            inner.attach_window(child, range);
            inner.parents.insert(child, parent);
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
        self.allocate_contiguous_owned(
            FrameKind::from_capacity(kind),
            len as usize,
            Some(owner),
            PAGE_SIZE as u64,
            None,
        )
    }

    fn allocate_owned_aligned(
        &self,
        owner: CapacityId,
        kind: CapacityKind,
        len: u64,
        align: u64,
        node: Option<u8>,
    ) -> Option<PhysRange> {
        self.allocate_contiguous_owned(
            FrameKind::from_capacity(kind),
            len as usize,
            Some(owner),
            align,
            node,
        )
    }

    fn mark_object_owned(&self, range: PhysRange) {
        FrameBank::mark_object_owned(self, range);
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
        let capacity_kind = inner
            .ledger_kinds
            .get(&owner)
            .copied()
            .unwrap_or(CapacityKind::System);
        let _ = inner.ledger.release(allocation.charge);
        if let (Some(owner), Some(owner_charge)) = (allocation.owner, allocation.owner_charge)
            && let Some(ledger) = inner.ledgers.get_mut(&owner)
        {
            let _ = ledger.release(owner_charge);
        }
        let slot = &mut inner.by_capacity[capacity_kind.index()];
        *slot = slot.saturating_sub(range.len());
        let node_slot = &mut inner.by_node[usize::from(allocation.node)];
        *node_slot = node_slot.saturating_sub(range.len());
        // 分配记录已经移除，区间尚未进入空闲表，其他分配不可能拿到它；这时
        // 放锁清零，避免在 spin 锁内逐字节访问物理内存（与 free_for 一致）。
        drop(inner);
        crate::hmp::capacity::scrub(range.start(), range.len() as usize);
        let mut inner = self.inner.lock();
        inner.coalesce_free(range);
        true
    }

    fn reclaim_capacity(
        &self,
        owner: CapacityId,
        parent: Option<CapacityId>,
        limit: u64,
    ) -> Result<(), HmpError> {
        self.reclaim_capacity_inner(owner, parent, limit, true)
    }

    fn reclaim_capacity_force(
        &self,
        owner: CapacityId,
        parent: Option<CapacityId>,
        limit: u64,
    ) -> Result<(), HmpError> {
        self.reclaim_capacity_inner(owner, parent, limit, false)
    }

    fn windows_owned(&self, owner: CapacityId) -> Vec<PhysRange> {
        self.inner
            .lock()
            .owned
            .iter()
            .filter(|(_, window)| window.capacity == owner)
            .map(|(_, window)| window.range)
            .collect()
    }

    fn used_owned(&self, owner: CapacityId) -> u64 {
        self.inner.lock().used_for(owner)
    }

    fn peak_owned(&self, owner: CapacityId) -> u64 {
        let inner = self.inner.lock();
        inner
            .ledgers
            .get(&owner)
            .map(|ledger| ledger.peak())
            .unwrap_or_else(|| inner.peaks.get(&owner).copied().unwrap_or(0))
    }

    fn register_ledger(&self, owner: CapacityId, limit: u64) {
        FrameBank::register_ledger(self, owner, limit);
    }

    fn set_ledger_kind(&self, owner: CapacityId, kind: CapacityKind) {
        FrameBank::set_ledger_kind(self, owner, kind);
    }

    fn set_ledger_limit(&self, owner: CapacityId, limit: u64) {
        FrameBank::set_ledger_limit(self, owner, limit);
    }

    fn set_ledger_soft_limit(&self, owner: CapacityId, limit: u64) {
        FrameBank::set_ledger_soft_limit(self, owner, limit);
    }

    fn remove_ledger(&self, owner: CapacityId) {
        FrameBank::remove_ledger(self, owner);
    }
}
