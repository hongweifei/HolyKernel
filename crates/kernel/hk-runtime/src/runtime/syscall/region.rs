//! Region 映射、查询与解除。

use hk_hal::VirtAddr;
use hk_handle::Rights;
use hk_ipc::Message;
use hk_mem::hmp::{
    Capability, CapabilityKind, CapabilityOp, FrameBacking, FrameLease, Region, RegionFlags,
};

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
        if backing.is_none()
            && capability.kind() != CapabilityKind::Anonymous
            && self
                .kernel
                .capability_provider(tcb, capability_handle)
                .ok()
                .flatten()
                .is_none()
        {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let rights = Rights::from_bits_truncate(packed as u32);
        let flags = RegionFlags::from_bits_truncate((packed >> 32) as u32);
        let Ok(granted) =
            self.kernel
                .handle_rights(tcb, capability_handle, hk_handle::ObjectKind::Capability)
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if !granted.covers(rights)
            || (flags.contains(RegionFlags::PINNED) && !granted.contains(Rights::ADMIN))
        {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
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
        if flags.contains(RegionFlags::PINNED)
            && self
                .kernel
                .pin_capability(tcb, capability_handle, true)
                .is_err()
        {
            let _ = self.kernel.close_handle(tcb, handle);
            let _ = self.process_mut(index).hmp_arena.remove(region_id);
            let _ = self.platform.unmap_user(index, virt, length);
            self.process_mut(index).user_window.release(virt);
            let _ = self.kernel.close_handle(tcb, capability_handle);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        frame.set_result(self.install_region(
            index,
            handle,
            capability_handle,
            region_id,
            false,
            virt,
            length,
            offset,
            rights,
            flags,
            None,
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
        let Some((slot, handle, virt, length)) = self.region_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        // 权限只能收窄或被映射能力句柄覆盖：protect 不能把只读能力提升为可写。
        let Some((owner, capability)) = self.process(index).regions[slot]
            .as_ref()
            .map(|region| (region.owner, region.capability))
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(granted) =
            self.kernel
                .handle_rights(owner, capability, hk_handle::ObjectKind::Capability)
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if !granted.covers(rights)
            || (flags.contains(RegionFlags::PINNED) && !granted.contains(Rights::ADMIN))
        {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let was_pinned = self.process(index).regions[slot]
            .as_ref()
            .is_some_and(|region| region.flags.contains(RegionFlags::PINNED));
        if was_pinned != flags.contains(RegionFlags::PINNED)
            && self
                .kernel
                .pin_capability(owner, capability, flags.contains(RegionFlags::PINNED))
                .is_err()
        {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
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
        let capability_id = self
            .kernel
            .handle_id(owner, capability)
            .ok()
            .map(|id| id.get());
        let has_overrides = capability_id.is_some_and(|id| {
            self.process(index)
                .cow_pages
                .keys()
                .any(|(owner, _)| *owner == id)
        });
        if let Some(backing) = backing
            && !has_overrides
        {
            let phys = backing.start().get().saturating_add(offset);
            if !self
                .platform
                .map_user(index, virt, phys, length, rights, flags)
            {
                frame.set_result(u64::MAX);
                return Switch::Continue;
            }
        } else {
            // 惰性 Region 只更新已装入页的 PTE；未装入的页在下次缺页时按
            // RegionMap 中的新 rights/flags 建立映射。
            let page = hk_hal::PAGE_SIZE as u64;
            let mut relative = 0_u64;
            while relative < length as u64 {
                let page_index = (offset.saturating_add(relative) / page) as usize;
                let page_range = capability_id
                    .and_then(|id| {
                        self.process(index)
                            .cow_pages
                            .get(&(id, page_index))
                            .map(|page| page.range)
                    })
                    .or_else(|| {
                        self.kernel
                            .capability_page(owner, capability, page_index)
                            .ok()
                            .flatten()
                            .or_else(|| {
                                backing.map(|backing| {
                                    hk_hal::PhysRange::new(
                                        hk_hal::PhysAddr::new(
                                            backing
                                                .start()
                                                .get()
                                                .saturating_add(offset)
                                                .saturating_add(relative),
                                        ),
                                        page,
                                    )
                                })
                            })
                    });
                if let Some(range) = page_range
                    && !self.platform.map_user(
                        index,
                        virt + relative,
                        range.start().get(),
                        page as usize,
                        rights,
                        flags,
                    )
                {
                    frame.set_result(u64::MAX);
                    return Switch::Continue;
                }
                relative += page;
            }
        }
        frame.set_result(0);
        Switch::Continue
    }

    /// 按 `GROW` 标志扩展 Region，并同步 Arena 表、用户窗口与已有物理映射。
    pub(crate) fn region_grow(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let new_len = frame.arg1() as usize;
        let Some((slot, handle, virt, length)) = self.region_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if new_len <= length {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Some(region) = self.process(index).regions[slot].as_ref() else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if !region.flags.contains(RegionFlags::GROW) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let owner = region.owner;
        let capability = region.capability;
        let offset = region.offset;
        let region_id = region.region;
        let rights = region.rights;
        let flags = region.flags;
        let Ok((capability_info, backing)) = self.kernel.capability(owner, capability) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capability_info.require(CapabilityOp::Grow).is_err() {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Some(end) = offset.checked_add(new_len as u64) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if end > capability_info.size() || !new_len.is_multiple_of(hk_hal::PAGE_SIZE) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        if !self
            .process_mut(index)
            .user_window
            .grow(virt, length, new_len)
        {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        if self
            .process_mut(index)
            .hmp_arena
            .resize(region_id, new_len)
            .is_err()
        {
            let _ = self
                .process_mut(index)
                .user_window
                .shrink(virt, new_len, length);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let delta = new_len - length;
        if let Some(backing) = backing {
            let phys = backing
                .start()
                .get()
                .saturating_add(offset)
                .saturating_add(length as u64);
            if !self
                .platform
                .map_user(index, virt + length as u64, phys, delta, rights, flags)
            {
                let _ = self.process_mut(index).hmp_arena.resize(region_id, length);
                let _ = self
                    .process_mut(index)
                    .user_window
                    .shrink(virt, new_len, length);
                frame.set_result(u64::MAX);
                return Switch::Continue;
            }
        }
        if self.kernel.grow_region(tcb, handle, new_len).is_err() {
            if backing.is_some() {
                let _ = self.platform.unmap_user(index, virt + length as u64, delta);
            }
            let _ = self.process_mut(index).hmp_arena.resize(region_id, length);
            let _ = self
                .process_mut(index)
                .user_window
                .shrink(virt, new_len, length);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        if let Some(map) = self.process_mut(index).regions[slot].as_mut() {
            map.len = new_len;
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
        // 设备映射按实际窗口计入任务配额；解除映射时归还。
        let Ok(charge) = self.process_mut(index).ledger.charge(len as u64) else {
            return false;
        };
        let Ok(capability) = Capability::new(
            CapabilityKind::DeviceAllocation,
            len as u64,
            hk_hal::PAGE_SIZE,
            None,
        ) else {
            let _ = self.process_mut(index).ledger.release(charge);
            return false;
        };
        let lease = FrameBacking::Imported(FrameLease::import(hk_hal::PhysAddr::new(phys), len));
        let Ok(capability_handle) = self.kernel.create_capability(
            tcb,
            capability.clone(),
            Some(lease),
            Rights::READ | Rights::WRITE,
        ) else {
            let _ = self.process_mut(index).ledger.release(charge);
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
            let _ = self.process_mut(index).ledger.release(charge);
            return false;
        };
        if region.map(VirtAddr::new(virt)).is_err() {
            let _ = self.kernel.close_handle(tcb, capability_handle);
            let _ = self.process_mut(index).ledger.release(charge);
            return false;
        }
        let region_id = region.id();
        if self.process_mut(index).hmp_arena.insert(&region).is_err() {
            let _ = self.kernel.close_handle(tcb, capability_handle);
            let _ = self.process_mut(index).ledger.release(charge);
            return false;
        }
        let Ok(region_handle) =
            self.kernel
                .create_region(tcb, region, Rights::READ | Rights::WRITE | Rights::ADMIN)
        else {
            let _ = self.kernel.close_handle(tcb, capability_handle);
            let _ = self.process_mut(index).hmp_arena.remove(region_id);
            let _ = self.process_mut(index).ledger.release(charge);
            return false;
        };
        self.install_region(
            index,
            region_handle,
            capability_handle,
            region_id,
            true,
            virt,
            len,
            0,
            Rights::READ | Rights::WRITE,
            RegionFlags::empty(),
            Some(charge),
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
        if region.flags.contains(RegionFlags::GUARD) {
            return false;
        }
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
        if self.provider_fault_region(index, region, address, &capability, provider, false) {
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
        write: bool,
    ) -> bool {
        let page = hk_hal::PAGE_SIZE as u64;
        let offset = region.capability_offset(address, page);
        if offset >= capability.size() {
            return false;
        }
        let Ok(capability_id) = self.kernel.handle_id(region.tcb, region.capability) else {
            return false;
        };
        let words = hk_mem::hmp::provider::FaultMessage {
            offset,
            page,
            address,
            access: if write {
                hk_mem::hmp::provider::ProviderAccess::Write
            } else {
                hk_mem::hmp::provider::ProviderAccess::Read
            },
            capability: capability_id.get(),
            rights: region.rights.bits(),
            flags: region.flags.bits(),
        }
        .encode();
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
            private: write,
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
        // Guard 命中即故障：不装零页，也不转发 Provider。
        if region.flags.contains(RegionFlags::GUARD) {
            return FaultOutcome::Unhandled;
        }
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
            if self.provider_fault_region(index, region, address, &capability, provider, false) {
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

    /// 在 fork 前把惰性匿名 Region 的页装入父地址空间。
    ///
    /// 只处理 `Anonymous` 且未绑定 Provider 的 Region：装载器映射、TaskImage
    /// 与 Provider 管理的页已经存在或由对应服务提供，内核不能代为装入。已经
    /// 建立 PTE 的页保持原映射，避免把镜像或栈替换成零页。
    pub(crate) fn populate_lazy_regions(&mut self, index: usize) -> bool {
        let ranges: alloc::vec::Vec<(u64, usize)> = self
            .process(index)
            .regions
            .iter()
            .flatten()
            .filter_map(|region| {
                let (capability, backing) = self
                    .kernel
                    .capability(region.owner, region.capability)
                    .ok()?;
                if backing.is_some() || capability.kind() != CapabilityKind::Anonymous {
                    return None;
                }
                let provider = self
                    .kernel
                    .capability_provider(region.owner, region.capability)
                    .ok()
                    .flatten();
                provider.is_none().then_some((region.virt, region.len))
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
                let address = virt + offset;
                if !self.platform.user_mapped(address) && !self.anonymous_fault(cpu, address) {
                    return false;
                }
                offset += page;
            }
        }
        true
    }

    /// 解析用户 COW 缺页；复制出新页时登记为当前进程的私有覆盖。
    pub fn resolve_cow(&mut self, cpu: usize, address: u64) -> bool {
        let Some(index) = self.current[cpu] else {
            return false;
        };
        let Some(region) = self.find_fault_region(index, address) else {
            return false;
        };
        let page = hk_hal::PAGE_SIZE as u64;
        let page_index = (region.capability_offset(address, page) / page) as usize;
        let Ok((capability, _)) = self.kernel.capability(region.tcb, region.capability) else {
            return false;
        };
        // 绑定 Provider 的能力把写时复制交给 Provider；匿名与已常驻能力由内核
        // 复制并记入 Task Ledger。
        let provider = self
            .kernel
            .capability_provider(region.tcb, region.capability)
            .ok()
            .flatten();
        if let Some(provider) = provider {
            return self.provider_fault_region(index, region, address, &capability, provider, true);
        }
        match self.platform.resolve_cow(address) {
            Some(crate::runtime::CowOutcome::Copied(physical)) => {
                let Ok(charge) = self.process_mut(index).ledger.charge(page) else {
                    return false;
                };
                let range = hk_hal::PhysRange::new(hk_hal::PhysAddr::new(physical), page);
                let Ok(id) = self.kernel.handle_id(region.tcb, region.capability) else {
                    let _ = self.process_mut(index).ledger.release(charge);
                    return false;
                };
                self.process_mut(index).cow_pages.insert(
                    (id.get(), page_index),
                    crate::runtime::CowPage {
                        range,
                        charge: Some(charge),
                        backing: None,
                    },
                );
                true
            }
            Some(crate::runtime::CowOutcome::Writable) => true,
            None => false,
        }
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
        // Guard 页命中即故障，不装入零页，也不转发 Provider。
        if region.flags.contains(RegionFlags::GUARD) {
            return false;
        }
        let page = hk_hal::PAGE_SIZE as u64;
        let offset = region.capability_offset(address, page);
        if offset >= capability.size() {
            return false;
        }
        let page_address = region.page_address(offset);
        let page_index = (offset / page) as usize;
        // COW 私有页覆盖优先于共享 capability 页。
        let capability_id = self
            .kernel
            .handle_id(region.tcb, region.capability)
            .ok()
            .map(|id| id.get());
        let overridden = capability_id.and_then(|id| {
            self.process(index)
                .cow_pages
                .get(&(id, page_index))
                .map(|page| page.range)
        });
        if let Some(range) = overridden {
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
        if !self
            .kernel
            .handle_rights(owner, handle, hk_handle::ObjectKind::Region)
            .is_ok_and(|rights| rights.contains(Rights::ADMIN))
        {
            // 只有通过 Region 句柄的 ADMIN 才能解除映射。
            frame.set_result(0);
            return Switch::Continue;
        }
        let capability = region.capability;
        let charge = region.charge;
        let pinned = region.flags.contains(RegionFlags::PINNED);
        if pinned
            && self
                .kernel
                .pin_capability(owner, capability, false)
                .is_err()
        {
            // 仍在 scanout 驻留的映射不能解除。
            frame.set_result(0);
            return Switch::Continue;
        }
        let capability_id = self
            .kernel
            .handle_id(owner, capability)
            .ok()
            .map(|id| id.get());
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
        let internal_capability = self.process(index).regions[slot]
            .as_ref()
            .is_some_and(|region| region.internal_capability);
        if let Some(region_id) = region_id {
            let _ = self.process_mut(index).hmp_arena.remove(region_id);
        }
        if internal_capability {
            // `SYS_FRAME_MAP` / 设备映射的 capability 由内核代建，只被这个
            // Region 持有；解除映射时必须一并关闭，否则 capability 及其
            // OwnedFrame/Imported lease 会泄漏。
            let _ = self.kernel.close_handle(owner, capability);
        }
        let _ = self.kernel.close_handle(owner, handle);
        self.process_mut(index).regions[slot] = None;
        self.process_mut(index).region_free.push(slot);
        if let Some(charge) = charge {
            let _ = self.process_mut(index).ledger.release(charge);
        }
        if let Some(capability_id) = capability_id {
            self.prune_cow_pages(index, capability_id);
        }
        frame.set_result(1);
        Switch::Continue
    }
}
