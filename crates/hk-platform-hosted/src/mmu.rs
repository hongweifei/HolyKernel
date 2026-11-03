//! 虚拟地址空间。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::sync::MutexGuard;

use hk_hal::{MapFlags, Mmu, MmuError, PAGE_SIZE, PhysAddr, VSpaceId, VirtAddr};

/// Hosted 平台的地址空间与映射表。
#[derive(Debug)]
pub struct HostedMmu {
    state: Mutex<MmuState>,
}

#[derive(Debug)]
struct MmuState {
    mappings: BTreeMap<(u64, u64), Mapping>,
    vspaces: BTreeSet<u64>,
    next_vspace: u64,
}

#[derive(Clone, Copy, Debug)]
struct Mapping {
    phys: PhysAddr,
    flags: MapFlags,
}

impl HostedMmu {
    /// 构造空地址空间表。
    pub fn new() -> Self {
        Self {
            state: Mutex::new(MmuState {
                mappings: BTreeMap::new(),
                vspaces: BTreeSet::new(),
                next_vspace: 1,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, MmuState> {
        self.state.lock().expect("地址空间锁中毒")
    }

    /// 返回地址空间当前的映射页数。
    pub fn mapped_pages(&self, vspace: VSpaceId) -> usize {
        let state = self.lock();
        state
            .mappings
            .keys()
            .filter(|(owner, _)| *owner == vspace.get())
            .count()
    }
}

impl Default for HostedMmu {
    fn default() -> Self {
        Self::new()
    }
}

impl Mmu for HostedMmu {
    fn page_size(&self) -> usize {
        PAGE_SIZE
    }

    fn create_vspace(&self) -> VSpaceId {
        let mut state = self.lock();
        let id = state.next_vspace;
        state.next_vspace += 1;
        state.vspaces.insert(id);
        VSpaceId::new(id)
    }

    fn destroy_vspace(&self, vspace: VSpaceId) {
        let mut state = self.lock();
        state.vspaces.remove(&vspace.get());
        state
            .mappings
            .retain(|(owner, _), _| *owner != vspace.get());
    }

    fn map(
        &self,
        vspace: VSpaceId,
        frame: PhysAddr,
        virt: VirtAddr,
        len: usize,
        flags: MapFlags,
    ) -> Result<(), MmuError> {
        if len == 0
            || !len.is_multiple_of(PAGE_SIZE)
            || !frame.is_page_aligned()
            || !virt.is_page_aligned()
        {
            return Err(MmuError::Unaligned);
        }
        if flags.violates_wx() {
            return Err(MmuError::WxConflict);
        }

        let mut state = self.lock();
        if !state.vspaces.contains(&vspace.get()) {
            return Err(MmuError::InvalidAddressSpace);
        }

        let pages = (len / PAGE_SIZE) as u64;
        let first_page = virt.page_index();
        for offset in 0..pages {
            if state
                .mappings
                .contains_key(&(vspace.get(), first_page + offset))
            {
                return Err(MmuError::AlreadyMapped);
            }
        }

        for offset in 0..pages {
            state.mappings.insert(
                (vspace.get(), first_page + offset),
                Mapping {
                    phys: PhysAddr::new(frame.get() + offset * PAGE_SIZE as u64),
                    flags,
                },
            );
        }
        Ok(())
    }

    fn unmap(&self, vspace: VSpaceId, virt: VirtAddr, len: usize) -> Result<PhysAddr, MmuError> {
        if len == 0 || !len.is_multiple_of(PAGE_SIZE) || !virt.is_page_aligned() {
            return Err(MmuError::Unaligned);
        }

        let mut state = self.lock();
        if !state.vspaces.contains(&vspace.get()) {
            return Err(MmuError::InvalidAddressSpace);
        }

        let pages = (len / PAGE_SIZE) as u64;
        let first_page = virt.page_index();
        let mut physical = None;
        for offset in 0..pages {
            match state.mappings.get(&(vspace.get(), first_page + offset)) {
                Some(mapping) => {
                    physical.get_or_insert(mapping.phys);
                }
                None => return Err(MmuError::NotMapped),
            }
        }

        for offset in 0..pages {
            state.mappings.remove(&(vspace.get(), first_page + offset));
        }
        Ok(physical.expect("已确认映射存在"))
    }

    fn protect(
        &self,
        vspace: VSpaceId,
        virt: VirtAddr,
        len: usize,
        flags: MapFlags,
    ) -> Result<(), MmuError> {
        if len == 0 || !len.is_multiple_of(PAGE_SIZE) || !virt.is_page_aligned() {
            return Err(MmuError::Unaligned);
        }
        if flags.violates_wx() {
            return Err(MmuError::WxConflict);
        }

        let mut state = self.lock();
        if !state.vspaces.contains(&vspace.get()) {
            return Err(MmuError::InvalidAddressSpace);
        }

        let pages = (len / PAGE_SIZE) as u64;
        let first_page = virt.page_index();
        for offset in 0..pages {
            if !state
                .mappings
                .contains_key(&(vspace.get(), first_page + offset))
            {
                return Err(MmuError::NotMapped);
            }
        }

        for offset in 0..pages {
            if let Some(mapping) = state.mappings.get_mut(&(vspace.get(), first_page + offset)) {
                mapping.flags = flags;
            }
        }
        Ok(())
    }

    fn translate(&self, vspace: VSpaceId, virt: VirtAddr) -> Option<(PhysAddr, MapFlags)> {
        let state = self.lock();
        let mapping = state.mappings.get(&(vspace.get(), virt.page_index()))?;
        let offset = virt.get() & (PAGE_SIZE as u64 - 1);
        Some((mapping.phys.offset(offset), mapping.flags))
    }

    fn shootdown(&self, _vspace: VSpaceId) {
        // Hosted 平台在单一进程地址空间中模拟页表，映射变更立即可见。
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_and_translates_pages() {
        let mmu = HostedMmu::new();
        let vspace = mmu.create_vspace();
        let frame = PhysAddr::new(0x10_0000);
        let virt = VirtAddr::new(0x4000);

        assert_eq!(
            mmu.map(vspace, frame, virt, PAGE_SIZE, MapFlags::READ),
            Ok(())
        );
        assert_eq!(
            mmu.translate(vspace, VirtAddr::new(0x4018)),
            Some((PhysAddr::new(0x10_0018), MapFlags::READ))
        );
        assert_eq!(mmu.mapped_pages(vspace), 1);

        assert_eq!(mmu.unmap(vspace, virt, PAGE_SIZE), Ok(frame));
        assert_eq!(mmu.translate(vspace, virt), None);
    }

    #[test]
    fn maps_multiple_pages() {
        let mmu = HostedMmu::new();
        let vspace = mmu.create_vspace();
        let flags = MapFlags::READ | MapFlags::WRITE;

        assert_eq!(
            mmu.map(
                vspace,
                PhysAddr::new(0x20_0000),
                VirtAddr::new(0x8000),
                3 * PAGE_SIZE,
                flags
            ),
            Ok(())
        );
        assert_eq!(mmu.mapped_pages(vspace), 3);
        assert_eq!(
            mmu.translate(vspace, VirtAddr::new(0x8010))
                .map(|(phys, _)| phys),
            Some(PhysAddr::new(0x20_0010))
        );
        assert_eq!(
            mmu.translate(vspace, VirtAddr::new(0xA000))
                .map(|(phys, _)| phys),
            Some(PhysAddr::new(0x20_2000))
        );
    }

    #[test]
    fn rejects_wx_mapping() {
        let mmu = HostedMmu::new();
        let vspace = mmu.create_vspace();
        let flags = MapFlags::READ | MapFlags::WRITE | MapFlags::EXECUTE;

        assert_eq!(
            mmu.map(
                vspace,
                PhysAddr::new(0x10_0000),
                VirtAddr::new(0x4000),
                PAGE_SIZE,
                flags
            ),
            Err(MmuError::WxConflict)
        );
    }

    #[test]
    fn rejects_double_mapping() {
        let mmu = HostedMmu::new();
        let vspace = mmu.create_vspace();
        let virt = VirtAddr::new(0x4000);

        mmu.map(
            vspace,
            PhysAddr::new(0x10_0000),
            virt,
            PAGE_SIZE,
            MapFlags::READ,
        )
        .expect("首次映射成功");
        assert_eq!(
            mmu.map(
                vspace,
                PhysAddr::new(0x20_0000),
                virt,
                PAGE_SIZE,
                MapFlags::READ
            ),
            Err(MmuError::AlreadyMapped)
        );
    }

    #[test]
    fn rejects_unaligned_mapping() {
        let mmu = HostedMmu::new();
        let vspace = mmu.create_vspace();

        assert_eq!(
            mmu.map(
                vspace,
                PhysAddr::new(0x10_0000),
                VirtAddr::new(0x4010),
                PAGE_SIZE,
                MapFlags::READ
            ),
            Err(MmuError::Unaligned)
        );
    }

    #[test]
    fn rejects_unknown_address_space() {
        let mmu = HostedMmu::new();

        assert_eq!(
            mmu.map(
                VSpaceId::new(42),
                PhysAddr::new(0x10_0000),
                VirtAddr::new(0x4000),
                PAGE_SIZE,
                MapFlags::READ
            ),
            Err(MmuError::InvalidAddressSpace)
        );
    }

    #[test]
    fn protects_mapping() {
        let mmu = HostedMmu::new();
        let vspace = mmu.create_vspace();
        let virt = VirtAddr::new(0x4000);

        mmu.map(
            vspace,
            PhysAddr::new(0x10_0000),
            virt,
            PAGE_SIZE,
            MapFlags::READ | MapFlags::WRITE,
        )
        .expect("映射成功");
        assert_eq!(mmu.protect(vspace, virt, PAGE_SIZE, MapFlags::READ), Ok(()));
        assert_eq!(
            mmu.translate(vspace, virt),
            Some((PhysAddr::new(0x10_0000), MapFlags::READ))
        );
    }

    #[test]
    fn destroys_address_space() {
        let mmu = HostedMmu::new();
        let vspace = mmu.create_vspace();

        mmu.map(
            vspace,
            PhysAddr::new(0x10_0000),
            VirtAddr::new(0x4000),
            PAGE_SIZE,
            MapFlags::READ,
        )
        .expect("映射成功");
        mmu.destroy_vspace(vspace);

        assert_eq!(mmu.mapped_pages(vspace), 0);
        assert_eq!(mmu.translate(vspace, VirtAddr::new(0x4000)), None);
    }
}
