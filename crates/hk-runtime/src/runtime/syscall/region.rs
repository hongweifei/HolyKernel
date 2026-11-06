//! Region 映射、查询与解除。

use hk_hal::VirtAddr;
use hk_handle::Rights;
use hk_mem::hmp::{Capability, CapabilityKind, FrameLease, Region, RegionFlags};

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    pub(crate) fn region_map(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let offset = frame.arg1();
        let length = frame.arg2() as usize;
        let packed = frame.arg3();
        let Some((capability_handle, capability, backing)) = self.capability_at(index, number)
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if backing.is_none() && capability.kind() != CapabilityKind::Anonymous {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let rights = Rights::from_bits_truncate(packed as u32);
        let flags = RegionFlags::from_bits_truncate((packed >> 32) as u32);
        let Ok(mut region) = Region::new(&capability, offset, length, rights, flags) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some(virt) = self.tasks[index].user_window.allocate(length) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if let Some(backing) = backing {
            let phys = backing.start().get().saturating_add(offset);
            if !self.platform.map_dma(index, virt, phys, length) {
                self.tasks[index].user_window.release(virt);
                frame.set_result(u64::MAX);
                return Switch::Continue;
            }
        }
        if region.map(VirtAddr::new(virt)).is_err() {
            let _ = self.platform.unmap_dma(index, virt, length);
            self.tasks[index].user_window.release(virt);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let handle = match self.kernel.create_region(
            tcb,
            region,
            Rights::READ | Rights::WRITE | Rights::ADMIN,
        ) {
            Ok(handle) => handle,
            Err(_) => {
                let _ = self.platform.unmap_dma(index, virt, length);
                self.tasks[index].user_window.release(virt);
                frame.set_result(u64::MAX);
                return Switch::Continue;
            }
        };
        frame.set_result(self.install_region(index, handle, capability_handle, virt, length));
        Switch::Continue
    }

    pub(crate) fn region_info(&mut self, index: usize, _tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let out = frame.arg1();
        let capacity = frame.arg2() as usize;
        let Some((_, _, virt, length)) = self.region_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capacity < 16 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let mut bytes = [0_u8; 16];
        bytes[..8].copy_from_slice(&virt.to_le_bytes());
        bytes[8..].copy_from_slice(&(length as u64).to_le_bytes());
        if self.platform.write_user(out, &bytes) {
            frame.set_result(16);
        } else {
            frame.set_result(u64::MAX);
        }
        Switch::Continue
    }

    /// 为已映射的设备物理区间建立 DeviceAllocation 能力与 Region。
    pub(crate) fn install_device_mapping(
        &mut self,
        index: usize,
        tcb: TcbId,
        phys: u64,
        len: usize,
        virt: u64,
    ) -> bool {
        let Ok(capability) = Capability::new(
            CapabilityKind::DeviceAllocation,
            len as u64,
            hk_hal::PAGE_SIZE,
            None,
        ) else {
            return false;
        };
        let lease = FrameLease::import(hk_hal::PhysAddr::new(phys), len);
        let Ok(capability_handle) = self.kernel.create_capability(
            tcb,
            capability.clone(),
            Some(lease),
            Rights::READ | Rights::WRITE,
        ) else {
            return false;
        };
        let Ok(mut region) = Region::new(
            &capability,
            0,
            len,
            Rights::READ | Rights::WRITE,
            RegionFlags::empty(),
        ) else {
            let _ = self.kernel.close_handle(tcb, capability_handle);
            return false;
        };
        if region.map(VirtAddr::new(virt)).is_err() {
            let _ = self.kernel.close_handle(tcb, capability_handle);
            return false;
        }
        let Ok(region_handle) =
            self.kernel
                .create_region(tcb, region, Rights::READ | Rights::WRITE | Rights::ADMIN)
        else {
            let _ = self.kernel.close_handle(tcb, capability_handle);
            return false;
        };
        self.install_region(index, region_handle, capability_handle, virt, len);
        true
    }

    /// 处理匿名能力的按需零页缺页。
    pub fn anonymous_fault(&mut self, cpu: usize, address: u64) -> bool {
        let Some(index) = self.current[cpu] else {
            return false;
        };
        let owner = self.leader_of(index);
        let tcb = self.tasks[owner].tcb;
        let Some((slot, virt, _length)) =
            self.tasks[owner]
                .regions
                .iter()
                .enumerate()
                .find_map(|(slot, region)| {
                    let region = region.as_ref()?;
                    let end = region.virt.checked_add(region.len as u64)?;
                    (address >= region.virt && address < end).then_some((
                        slot,
                        region.virt,
                        region.len,
                    ))
                })
        else {
            return false;
        };
        let Some(region) = self.tasks[owner].regions[slot].as_ref() else {
            return false;
        };
        let capability_handle = region.capability;
        let Ok((capability, backing)) = self.kernel.capability(tcb, capability_handle) else {
            return false;
        };
        if backing.is_some() || capability.kind() != CapabilityKind::Anonymous {
            return false;
        }
        let page = hk_hal::PAGE_SIZE as u64;
        let offset = address.saturating_sub(virt) / page;
        if offset >= capability.size() / page {
            return false;
        }
        let page_address = virt + offset * page;
        if let Some(range) = self
            .kernel
            .capability_page(tcb, capability_handle, offset as usize)
            .ok()
            .flatten()
        {
            if !self.platform.user_mapped(page_address) {
                let _ =
                    self.platform
                        .map_dma(index, page_address, range.start().get(), page as usize);
            }
            return self.platform.user_mapped(page_address);
        }
        let Some(range) = self.platform.dma_window(page as usize) else {
            return false;
        };
        let lease = FrameLease::new(range.start(), page as usize);
        if !self
            .platform
            .map_dma(index, page_address, range.start().get(), page as usize)
        {
            return false;
        }
        if !self
            .kernel
            .set_capability_page(tcb, capability_handle, offset as usize, lease)
            .unwrap_or(false)
        {
            let _ = self.platform.unmap_dma(index, page_address, page as usize);
            return false;
        }
        self.platform.zero(page_address, page as usize);
        true
    }

    pub(crate) fn region_unmap(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let Some((slot, handle, virt, length)) = self.region_at(index, number) else {
            frame.set_result(0);
            return Switch::Continue;
        };
        let lazy = self.tasks[index].regions[slot]
            .as_ref()
            .and_then(|region| {
                self.kernel
                    .capability(self.tasks[index].tcb, region.capability)
                    .ok()
            })
            .is_some_and(|(_, backing)| backing.is_none());
        if lazy {
            let page = hk_hal::PAGE_SIZE as u64;
            let mut offset = 0;
            while offset < length as u64 {
                let _ = self.platform.unmap_dma(index, virt + offset, page as usize);
                offset += page;
            }
        } else if !self.platform.unmap_dma(index, virt, length) {
            frame.set_result(0);
            return Switch::Continue;
        }
        self.tasks[index].user_window.release(virt);
        let _ = self.kernel.close_handle(tcb, handle);
        self.tasks[index].regions[slot] = None;
        frame.set_result(1);
        Switch::Continue
    }
}
