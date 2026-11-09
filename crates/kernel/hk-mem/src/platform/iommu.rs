//! 软件 IOMMU 域表：在无硬件 IOMMU 时提供映射状态与重叠检查。

use alloc::collections::BTreeMap;

use hk_hal::{Iommu, IommuDomain, IommuError, IommuFlags, Iova, PAGE_SIZE, PhysAddr, PhysRange};
use spin::Mutex;

#[derive(Clone, Copy)]
struct Entry {
    phys: u64,
    len: u64,
    flags: IommuFlags,
}

/// 软件 IOMMU 后端。
pub struct SoftwareIommu {
    mappings: Mutex<BTreeMap<(u32, u64), Entry>>,
}

impl SoftwareIommu {
    /// 构造空 IOMMU。
    pub const fn new() -> Self {
        Self {
            mappings: Mutex::new(BTreeMap::new()),
        }
    }

    /// 查询 IOVA 对应的物理区间与权限。
    pub fn lookup(&self, domain: IommuDomain, iova: Iova) -> Option<(PhysRange, IommuFlags)> {
        self.mappings
            .lock()
            .get(&(domain.get(), iova.get()))
            .map(|entry| {
                (
                    PhysRange::new(PhysAddr::new(entry.phys), entry.len),
                    entry.flags,
                )
            })
    }
}

impl Default for SoftwareIommu {
    fn default() -> Self {
        Self::new()
    }
}

impl Iommu for SoftwareIommu {
    fn map(
        &self,
        domain: IommuDomain,
        iova: Iova,
        phys: PhysRange,
        flags: IommuFlags,
    ) -> Result<(), IommuError> {
        let page = PAGE_SIZE as u64;
        let start = iova.get();
        let len = phys.len();
        if len == 0
            || !start.is_multiple_of(page)
            || !phys.start().get().is_multiple_of(page)
            || !len.is_multiple_of(page)
        {
            return Err(IommuError::Unaligned);
        }
        let end = start.checked_add(len).ok_or(IommuError::OutOfMemory)?;
        let mut mappings = self.mappings.lock();
        for ((slot_domain, slot_start), entry) in mappings.iter() {
            if *slot_domain != domain.get() {
                continue;
            }
            let slot_end = slot_start.saturating_add(entry.len);
            if *slot_start < end && start < slot_end {
                return Err(IommuError::AlreadyMapped);
            }
        }
        mappings.insert(
            (domain.get(), start),
            Entry {
                phys: phys.start().get(),
                len,
                flags,
            },
        );
        Ok(())
    }

    fn unmap(&self, domain: IommuDomain, iova: Iova, len: usize) -> Result<(), IommuError> {
        if len == 0 || !iova.get().is_multiple_of(PAGE_SIZE as u64) {
            return Err(IommuError::Unaligned);
        }
        let mut mappings = self.mappings.lock();
        let Some(entry) = mappings.remove(&(domain.get(), iova.get())) else {
            return Err(IommuError::NotMapped);
        };
        if entry.len != len as u64 {
            mappings.insert((domain.get(), iova.get()), entry);
            return Err(IommuError::Unaligned);
        }
        Ok(())
    }

    fn flush(&self, _domain: IommuDomain) -> Result<(), IommuError> {
        Ok(())
    }

    fn translate(&self, domain: IommuDomain, iova: Iova) -> Option<(PhysRange, IommuFlags)> {
        self.lookup(domain, iova)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hk_hal::PhysAddr;

    #[test]
    fn maps_and_rejects_overlap() {
        let iommu = SoftwareIommu::new();
        let domain = IommuDomain::new(1);
        let range = PhysRange::new(PhysAddr::new(0x20_0000), 0x2000);
        iommu
            .map(
                domain,
                Iova::new(0x20_0000),
                range,
                IommuFlags::READ | IommuFlags::WRITE,
            )
            .expect("映射");
        assert_eq!(
            iommu.map(
                domain,
                Iova::new(0x20_1000),
                range,
                IommuFlags::READ | IommuFlags::WRITE
            ),
            Err(IommuError::AlreadyMapped)
        );
        assert_eq!(iommu.unmap(domain, Iova::new(0x20_0000), 0x2000), Ok(()));
        assert_eq!(
            iommu.unmap(domain, Iova::new(0x20_0000), 0x2000),
            Err(IommuError::NotMapped)
        );
    }
}
