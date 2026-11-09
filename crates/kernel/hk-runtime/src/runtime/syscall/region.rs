//! Region 映射、查询与解除。

use hk_hal::VirtAddr;
use hk_handle::Rights;
use hk_ipc::Message;
use hk_mem::hmp::{Capability, CapabilityKind, FrameBacking, FrameLease, Region, RegionFlags};

use super::*;

use hk_syscall::PROVIDER_FAULT;

/// 命中的 Region 快照：解析缺页时需要的全部元数据。
#[derive(Clone, Copy)]
struct FaultRegion {
    tcb: TcbId,
    capability: Handle,
    virt: u64,
    offset: u64,
    rights: Rights,
    flags: RegionFlags,
}

/// 判断地址是否落在 Region 内，命中时返回解析缺页所需的快照。
fn fault_region(region: &RegionMap, address: u64) -> Option<FaultRegion> {
    let end = region.virt.checked_add(region.len as u64)?;
    (address >= region.virt && address < end).then_some(FaultRegion {
        tcb: region.owner,
        capability: region.capability,
        virt: region.virt,
        offset: region.offset,
        rights: region.rights,
        flags: region.flags,
    })
}

impl FaultRegion {
    /// 返回地址在能力内的页偏移。
    fn capability_offset(self, address: u64, page: u64) -> u64 {
        let relative = address.saturating_sub(self.virt);
        self.offset + relative / page * page
    }

    /// 返回能力内页偏移对应的用户虚拟地址。
    fn page_address(self, capability_offset: u64) -> u64 {
        self.virt + (capability_offset - self.offset)
    }
}

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
        let Some(virt) = self.process_mut(index).user_window.allocate(length) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if let Some(backing) = backing {
            let phys = backing.start().get().saturating_add(offset);
            if !self
                .platform
                .map_user(index, virt, phys, length, rights, flags)
            {
                self.process_mut(index).user_window.release(virt);
                frame.set_result(u64::MAX);
                return Switch::Continue;
            }
        }
        if region.map(VirtAddr::new(virt)).is_err() {
            let _ = self.platform.unmap_user(index, virt, length);
            self.process_mut(index).user_window.release(virt);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let region_id = region.id();
        if self.process_mut(index).hmp_arena.insert(&region).is_err() {
            let _ = self.platform.unmap_user(index, virt, length);
            self.process_mut(index).user_window.release(virt);
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
                let _ = self.platform.unmap_user(index, virt, length);
                self.process_mut(index).user_window.release(virt);
                let _ = self.process_mut(index).hmp_arena.remove(region_id);
                frame.set_result(u64::MAX);
                return Switch::Continue;
            }
        };
        frame.set_result(self.install_region(
            index,
            handle,
            capability_handle,
            region_id,
            None,
            virt,
            length,
            offset,
            rights,
            flags,
        ));
        Switch::Continue
    }

    /// 修改已映射 Region 的权利与标志，并同步页表。
    ///
    /// 只改变映射属性，不搬动物理页；能力层校验 W^X 与 sealed 约束。
    pub(crate) fn region_protect(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let packed = frame.arg1();
        let rights = Rights::from_bits_truncate(packed as u32);
        let flags = RegionFlags::from_bits_truncate((packed >> 32) as u32);
        let Some((_, handle, virt, length)) = self.region_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if self
            .kernel
            .protect_region(tcb, handle, rights, flags)
            .is_err()
        {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Some(slot) = self
            .process(index)
            .regions
            .iter()
            .position(|region| region.as_ref().is_some_and(|map| map.handle == handle))
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some((owner, capability, offset)) = self.process(index).regions[slot]
            .as_ref()
            .map(|map| (map.owner, map.capability, map.offset))
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if let Some(map) = self.process_mut(index).regions[slot].as_mut() {
            map.rights = rights;
            map.flags = flags;
        }
        let backing = self
            .kernel
            .capability(owner, capability)
            .ok()
            .and_then(|(_, backing)| backing);
        if let Some(backing) = backing {
            let phys = backing.start().get().saturating_add(offset);
            if !self
                .platform
                .map_user(index, virt, phys, length, rights, flags)
            {
                frame.set_result(u64::MAX);
                return Switch::Continue;
            }
        }
        frame.set_result(0);
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
        let lease = FrameBacking::Imported(FrameLease::import(hk_hal::PhysAddr::new(phys), len));
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
        let region_id = region.id();
        if self.process_mut(index).hmp_arena.insert(&region).is_err() {
            let _ = self.kernel.close_handle(tcb, capability_handle);
            return false;
        }
        let Ok(region_handle) =
            self.kernel
                .create_region(tcb, region, Rights::READ | Rights::WRITE | Rights::ADMIN)
        else {
            let _ = self.kernel.close_handle(tcb, capability_handle);
            let _ = self.process_mut(index).hmp_arena.remove(region_id);
            return false;
        };
        self.install_region(
            index,
            region_handle,
            capability_handle,
            region_id,
            None,
            virt,
            len,
            0,
            Rights::READ | Rights::WRITE,
            RegionFlags::empty(),
        );
        true
    }

    fn find_fault_region(&self, index: usize, address: u64) -> Option<FaultRegion> {
        let owner = self.leader_of(index);
        let process = self.process(owner);
        process
            .regions
            .iter()
            .flatten()
            .find_map(|region| fault_region(region, address))
            .or_else(|| {
                process
                    .arena_region
                    .as_ref()
                    .and_then(|region| fault_region(region, address))
            })
    }

    /// 把缺页请求转给绑定 Provider；成功时任务已阻塞。
    pub fn provider_fault(&mut self, cpu: usize, address: u64, context: C) -> bool {
        let Some(index) = self.current[cpu] else {
            return false;
        };
        self.tasks[index].context = context;
        let Some(region) = self.find_fault_region(index, address) else {
            return false;
        };
        let Ok((capability, backing)) = self.kernel.capability(region.tcb, region.capability)
        else {
            return false;
        };
        if backing.is_some() {
            return false;
        }
        let Ok(Some(provider)) = self
            .kernel
            .capability_provider(region.tcb, region.capability)
        else {
            return false;
        };
        if self.provider_fault_region(index, region, address, &capability, provider) {
            self.current[cpu] = None;
            true
        } else {
            false
        }
    }

    fn provider_fault_region(
        &mut self,
        index: usize,
        region: FaultRegion,
        address: u64,
        capability: &Capability,
        provider: ObjectId,
    ) -> bool {
        let page = hk_hal::PAGE_SIZE as u64;
        let offset = region.capability_offset(address, page);
        if offset >= capability.size() {
            return false;
        }
        let mut words = [0_u64; 8];
        words[0] = offset;
        words[1] = page;
        words[2] = address;
        let message = Message::new(PROVIDER_FAULT).with_words(words);
        if self
            .kernel
            .call_object(self.tasks[index].tcb, provider, message)
            .is_err()
        {
            return false;
        }
        self.tasks[index].pending = Pending::ProviderFault {
            capability: region.capability,
            index: (offset / page) as usize,
            page: region.page_address(offset),
            rights: region.rights,
            flags: region.flags,
        };
        true
    }

    /// 一次解析 Region 后分派 Provider 或匿名缺页。
    pub fn handle_fault(&mut self, cpu: usize, address: u64, context: C) -> FaultOutcome {
        let Some(index) = self.current[cpu] else {
            return FaultOutcome::Unhandled;
        };
        let Some(region) = self.find_fault_region(index, address) else {
            return FaultOutcome::Unhandled;
        };
        let Ok((capability, backing)) = self.kernel.capability(region.tcb, region.capability)
        else {
            return FaultOutcome::Unhandled;
        };
        if backing.is_some() {
            return FaultOutcome::Unhandled;
        }
        let provider = self
            .kernel
            .capability_provider(region.tcb, region.capability)
            .ok()
            .flatten();
        if let Some(provider) = provider {
            self.tasks[index].context = context;
            if self.provider_fault_region(index, region, address, &capability, provider) {
                self.current[cpu] = None;
                FaultOutcome::Blocked
            } else {
                FaultOutcome::Unhandled
            }
        } else if capability.kind() == CapabilityKind::Anonymous
            && self.anonymous_fault_region(index, region, address, &capability)
        {
            FaultOutcome::Resumed
        } else {
            FaultOutcome::Unhandled
        }
    }

    /// 在 fork 前把惰性 Region 的页装入父地址空间。
    pub(crate) fn populate_lazy_regions(&mut self, index: usize) -> bool {
        let ranges: alloc::vec::Vec<(u64, usize)> = self
            .process(index)
            .regions
            .iter()
            .flatten()
            .filter_map(|region| {
                let (_, backing) = self
                    .kernel
                    .capability(region.owner, region.capability)
                    .ok()?;
                backing.is_none().then_some((region.virt, region.len))
            })
            .collect();
        if ranges.is_empty() {
            return true;
        }
        let Some(cpu) = self
            .current
            .iter()
            .position(|current| *current == Some(index))
        else {
            return false;
        };
        let page = hk_hal::PAGE_SIZE as u64;
        for (virt, len) in ranges {
            let mut offset = 0;
            while offset < len as u64 {
                if !self.anonymous_fault(cpu, virt + offset) {
                    return false;
                }
                offset += page;
            }
        }
        true
    }

    /// 处理匿名能力的按需零页缺页。
    pub fn anonymous_fault(&mut self, cpu: usize, address: u64) -> bool {
        let Some(index) = self.current[cpu] else {
            return false;
        };
        let Some(region) = self.find_fault_region(index, address) else {
            return false;
        };
        let Ok((capability, backing)) = self.kernel.capability(region.tcb, region.capability)
        else {
            return false;
        };
        if backing.is_some() || capability.kind() != CapabilityKind::Anonymous {
            return false;
        }
        if self
            .kernel
            .capability_provider(region.tcb, region.capability)
            .ok()
            .flatten()
            .is_some()
        {
            return false;
        }
        self.anonymous_fault_region(index, region, address, &capability)
    }

    fn anonymous_fault_region(
        &mut self,
        index: usize,
        region: FaultRegion,
        address: u64,
        capability: &Capability,
    ) -> bool {
        let page = hk_hal::PAGE_SIZE as u64;
        let offset = region.capability_offset(address, page);
        if offset >= capability.size() {
            return false;
        }
        let page_address = region.page_address(offset);
        let page_index = (offset / page) as usize;
        if let Some(range) = self
            .kernel
            .capability_page(region.tcb, region.capability, page_index)
            .ok()
            .flatten()
        {
            if !self.platform.user_mapped(page_address) {
                let _ = self.platform.map_user(
                    index,
                    page_address,
                    range.start().get(),
                    page as usize,
                    region.rights,
                    region.flags,
                );
            }
            return self.platform.user_mapped(page_address);
        }
        let Some(backing) = self.allocate_owned(index, page) else {
            return false;
        };
        let range = backing.range();
        if !self.platform.map_user(
            index,
            page_address,
            range.start().get(),
            page as usize,
            region.rights,
            region.flags,
        ) {
            return false;
        }
        if !self
            .kernel
            .set_capability_page(region.tcb, region.capability, page_index, backing)
            .unwrap_or(false)
        {
            let _ = self.platform.unmap_user(index, page_address, page as usize);
            return false;
        }
        self.platform.zero(page_address, page as usize);
        true
    }

    pub(crate) fn region_unmap(&mut self, index: usize, _tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let Some((slot, handle, virt, length)) = self.region_at(index, number) else {
            frame.set_result(0);
            return Switch::Continue;
        };
        let Some(region) = self.process(index).regions[slot].as_ref() else {
            frame.set_result(0);
            return Switch::Continue;
        };
        let owner = region.owner;
        let capability = region.capability;
        let lazy = self
            .kernel
            .capability(owner, capability)
            .is_ok_and(|(_, backing)| backing.is_none());
        if lazy {
            let page = hk_hal::PAGE_SIZE as u64;
            let mut offset = 0;
            while offset < length as u64 {
                if !self
                    .platform
                    .unmap_user(index, virt + offset, page as usize)
                {
                    frame.set_result(0);
                    return Switch::Continue;
                }
                offset += page;
            }
        } else if !self.platform.unmap_user(index, virt, length) {
            frame.set_result(0);
            return Switch::Continue;
        }
        self.process_mut(index).user_window.release(virt);
        let region_id = self.process(index).regions[slot]
            .as_ref()
            .map(|region| region.region);
        let frame_handle = self.process(index).regions[slot]
            .as_ref()
            .and_then(|region| region.frame);
        if let Some(region_id) = region_id {
            let _ = self.process_mut(index).hmp_arena.remove(region_id);
        }
        if let Some(frame_handle) = frame_handle {
            let _ = self.kernel.pin_frame(owner, frame_handle, false);
            let _ = self.kernel.close_handle(owner, frame_handle);
            // `SYS_FRAME_MAP` 的 capability 是内核代建的，只被这个 Region 持有；
            // 解除映射时必须一并关闭，否则 capability 与它的 lease 会泄漏。
            let _ = self.kernel.close_handle(owner, capability);
        }
        let _ = self.kernel.close_handle(owner, handle);
        self.process_mut(index).regions[slot] = None;
        self.process_mut(index).region_free.push(slot);
        frame.set_result(1);
        Switch::Continue
    }
}
