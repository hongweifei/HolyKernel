//! `PhysicalBackend` 实现：窗口归因、按容量分配与回收。

use super::*;
use crate::hmp::{HmpError, PhysicalBackend};

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
        if let (Some(owner), Some(owner_charge)) = (allocation.owner, allocation.owner_charge)
            && let Some(ledger) = inner.ledgers.get_mut(&owner)
        {
            let _ = ledger.release(owner_charge);
        }
        let slot = &mut inner.by_kind[allocation.kind.index()];
        *slot = slot.saturating_sub(range.len());
        // 归属已校验、分配记录已移除；在把区间交回空闲表前清零。此刻持锁，
        // 其他分配无法拿到这段物理区间。
        crate::hmp::capacity::scrub(range.start(), range.len() as usize);
        inner.coalesce_free(range);
        true
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
