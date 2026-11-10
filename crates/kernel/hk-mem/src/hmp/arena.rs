//! Arena：任务内存根与 Region 表。

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use hk_hal::{PAGE_SIZE, VirtAddr};

use super::{HmpError, LedgerSnapshot, Region, RegionId};

static NEXT_ARENA_ID: AtomicU64 = AtomicU64::new(1);

/// Arena 标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ArenaId(u64);

impl ArenaId {
    /// 构造标识。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 返回标识数值。
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Arena 中的 Region 表项。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArenaRegion {
    id: RegionId,
    virt: VirtAddr,
    len: usize,
}

impl ArenaRegion {
    /// 返回 Region 标识。
    pub const fn id(self) -> RegionId {
        self.id
    }

    /// 返回虚拟地址。
    pub const fn virt(self) -> VirtAddr {
        self.virt
    }

    /// 返回字节长度。
    pub const fn len(self) -> usize {
        self.len
    }

    /// 判断长度是否为零。
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }
}

/// 任务内存根。
///
/// Arena 拥有地址区间与 Region 表。Region 的页表物化由平台完成；Arena 负责
/// 维护区间、重叠与生命周期。
pub struct Arena {
    id: ArenaId,
    base: VirtAddr,
    size: u64,
    regions: Vec<ArenaRegion>,
    peak: u64,
}

impl Arena {
    /// 创建 Arena；地址与长度按页对齐。
    pub fn new(id: ArenaId, base: VirtAddr, size: u64) -> Result<Self, HmpError> {
        if size == 0 || !size.is_multiple_of(PAGE_SIZE as u64) || !base.is_page_aligned() {
            return Err(HmpError::InvalidSize);
        }
        Ok(Self {
            id,
            base,
            size,
            regions: Vec::new(),
            peak: 0,
        })
    }

    /// 以新标识创建 Arena。
    pub fn create(base: VirtAddr, size: u64) -> Result<Self, HmpError> {
        Self::new(
            ArenaId::new(NEXT_ARENA_ID.fetch_add(1, Ordering::Relaxed)),
            base,
            size,
        )
    }

    /// 把已映射的 Region 登记到 Arena。
    pub fn insert(&mut self, region: &Region) -> Result<RegionId, HmpError> {
        let virt = region.virt().ok_or(HmpError::NotMapped)?;
        let id = region.id();
        let end = virt
            .get()
            .checked_add(region.len() as u64)
            .ok_or(HmpError::OutOfRange)?;
        let arena_end = self
            .base
            .get()
            .checked_add(self.size)
            .ok_or(HmpError::OutOfRange)?;
        if virt.get() < self.base.get() || end > arena_end {
            return Err(HmpError::OutOfRange);
        }
        let overlaps = self.regions.iter().any(|existing| {
            existing.virt.get() < end
                && virt.get() < existing.virt.get().saturating_add(existing.len as u64)
        });
        if overlaps {
            return Err(HmpError::AlreadyMapped);
        }
        let used = self.used().saturating_add(region.len() as u64);
        if used > self.size {
            return Err(HmpError::OutOfRange);
        }
        self.peak = self.peak.max(used);
        self.regions.push(ArenaRegion {
            id,
            virt,
            len: region.len(),
        });
        Ok(id)
    }

    /// 调整已登记 Region 的长度。
    ///
    /// 新范围必须仍在 Arena 内，且不能与其它 Region 重叠。
    pub fn resize(&mut self, id: RegionId, len: usize) -> Result<(), HmpError> {
        let index = self
            .regions
            .iter()
            .position(|region| region.id == id)
            .ok_or(HmpError::NotMapped)?;
        let virt = self.regions[index].virt;
        let end = virt
            .get()
            .checked_add(len as u64)
            .ok_or(HmpError::OutOfRange)?;
        let arena_end = self
            .base
            .get()
            .checked_add(self.size)
            .ok_or(HmpError::OutOfRange)?;
        if len == 0 || virt.get() < self.base.get() || end > arena_end {
            return Err(HmpError::OutOfRange);
        }
        let overlaps = self.regions.iter().enumerate().any(|(other, existing)| {
            other != index
                && existing.virt.get() < end
                && virt.get() < existing.virt.get().saturating_add(existing.len as u64)
        });
        if overlaps {
            return Err(HmpError::AlreadyMapped);
        }
        let old_len = self.regions[index].len;
        let used = self
            .used()
            .saturating_sub(old_len as u64)
            .saturating_add(len as u64);
        if used > self.size {
            return Err(HmpError::OutOfRange);
        }
        self.peak = self.peak.max(used);
        self.regions[index].len = len;
        Ok(())
    }

    /// 从 Arena 移除 Region 表项。
    pub fn remove(&mut self, id: RegionId) -> Result<ArenaRegion, HmpError> {
        let index = self
            .regions
            .iter()
            .position(|region| region.id == id)
            .ok_or(HmpError::NotMapped)?;
        Ok(self.regions.remove(index))
    }

    /// 返回指定 Region 表项。
    pub fn region(&self, id: RegionId) -> Option<&ArenaRegion> {
        self.regions.iter().find(|region| region.id == id)
    }

    /// 返回 Region 表。
    pub fn regions(&self) -> &[ArenaRegion] {
        &self.regions
    }

    /// 返回 Arena 标识。
    pub const fn id(&self) -> ArenaId {
        self.id
    }

    /// 返回起始虚拟地址。
    pub const fn base(&self) -> VirtAddr {
        self.base
    }

    /// 返回地址区间长度。
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// 返回 Arena 中已登记的虚拟地址字节数。
    pub fn used(&self) -> u64 {
        self.regions
            .iter()
            .map(|region| region.len as u64)
            .fold(0, u64::saturating_add)
    }

    /// 返回 Arena 地址利用率的只读快照；只用于报告，不执行硬配额。
    pub fn ledger_snapshot(&self) -> LedgerSnapshot {
        LedgerSnapshot::utilization(self.used(), self.size, self.peak, 0)
    }

    /// 判断地址区间是否落在 Arena 内。
    pub fn contains(&self, virt: VirtAddr, len: u64) -> bool {
        let Some(end) = virt.get().checked_add(len) else {
            return false;
        };
        let Some(arena_end) = self.base.get().checked_add(self.size) else {
            return false;
        };
        virt.get() >= self.base.get() && end <= arena_end
    }

    /// 销毁 Arena，返回全部 Region 表项。
    pub fn destroy(self) -> Vec<ArenaRegion> {
        self.regions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hk_handle::{ObjectId, Rights};

    use crate::hmp::{Capability, CapabilityKind, RegionFlags};

    fn region(base: u64) -> Region {
        let capability = Capability::with_id(
            ObjectId::new(1),
            CapabilityKind::Anonymous,
            0x4000,
            PAGE_SIZE,
            None,
        )
        .expect("能力");
        let mut region = Region::new(
            &capability,
            0,
            PAGE_SIZE,
            Rights::READ,
            RegionFlags::empty(),
        )
        .expect("Region");
        region.map(VirtAddr::new(base)).expect("映射");
        region
    }

    fn arena() -> Arena {
        Arena::new(ArenaId::new(1), VirtAddr::new(0x40_0000), 0x4000).expect("Arena")
    }

    #[test]
    fn inserts_and_removes_regions() {
        let mut arena = arena();
        let region = region(0x40_0000);
        let id = arena.insert(&region).expect("加入 Region");
        assert_eq!(arena.used(), PAGE_SIZE as u64);
        assert!(arena.region(id).is_some());
        assert_eq!(arena.regions().len(), 1);

        let removed = arena.remove(id).expect("移除 Region");
        assert_eq!(removed.virt(), VirtAddr::new(0x40_0000));
        assert_eq!(arena.used(), 0);
        assert_eq!(arena.remove(id).err(), Some(HmpError::NotMapped));
    }

    #[test]
    fn rejects_overlap_and_out_of_range_regions() {
        let mut arena = arena();
        arena.insert(&region(0x40_0000)).expect("第一个 Region");

        assert_eq!(
            arena.insert(&region(0x40_0000)).err(),
            Some(HmpError::AlreadyMapped)
        );
        assert_eq!(
            arena.insert(&region(0x41_0000)).err(),
            Some(HmpError::OutOfRange)
        );
    }

    #[test]
    fn rejects_unmapped_region() {
        let capability = Capability::with_id(
            ObjectId::new(2),
            CapabilityKind::Anonymous,
            PAGE_SIZE as u64,
            PAGE_SIZE,
            None,
        )
        .expect("能力");
        let region = Region::new(
            &capability,
            0,
            PAGE_SIZE,
            Rights::READ,
            RegionFlags::empty(),
        )
        .expect("Region");
        assert_eq!(arena().insert(&region).err(), Some(HmpError::NotMapped));
    }

    #[test]
    fn validates_arena_geometry() {
        assert_eq!(
            Arena::new(ArenaId::new(1), VirtAddr::new(0x40_0100), 0x4000).err(),
            Some(HmpError::InvalidSize)
        );
        assert_eq!(
            Arena::new(ArenaId::new(1), VirtAddr::new(0x40_0000), 0x1001).err(),
            Some(HmpError::InvalidSize)
        );
    }
}
