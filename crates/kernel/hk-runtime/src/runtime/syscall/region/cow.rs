//! 写时复制与匿名按需缺页。

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 解析用户 COW 缺页；复制出新页时登记为当前进程的私有覆盖。
    pub fn resolve_cow(&mut self, cpu: usize, address: u64) -> bool {
        let Some(index) = self.current[cpu] else {
            return false;
        };
        let Some(region) = self.find_fault_region(index, address) else {
            return false;
        };
        // 只有可写且标记 COW 的 Region 才允许走写时复制；只读、SEALED 或
        // 未共享的 Region 的写缺页是保护错误，不能升级为可写映射。
        if !region.rights.contains(hk_handle::Rights::WRITE)
            || !region.flags.contains(RegionFlags::COW)
        {
            return false;
        }
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
            return self.provider_fault_region(
                index,
                region,
                address,
                &capability,
                provider,
                hk_mem::hmp::provider::ProviderAccess::Write,
            );
        }
        let capacity = self.task_capacity(index);
        match self.platform.resolve_cow(address, capacity.as_ref()) {
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
                        backing: None,
                    },
                );
                self.record_page_charge_for_capability(
                    index,
                    region.capability,
                    page_index,
                    charge,
                );
                true
            }
            Some(crate::runtime::CowOutcome::Writable) => true,
            None => false,
        }
    }

    /// 把能力里已经装入的页映射进当前任务。
    ///
    /// 同一个能力被多个 Region/任务映射时，第一份缺页由 Provider 装入，后续
    /// 缺页在这里直接共享同一物理页，只按当前任务记账。
    ///
    /// 返回 `None` 表示能力里没有这一页；`Some(false)` 表示有页但本次映射失败
    /// （配额/页表），调用方不得再转发 Provider 覆盖共享页。
    pub(super) fn map_resident_capability_page(
        &mut self,
        index: usize,
        region: FaultRegion,
        address: u64,
        capability: &Capability,
    ) -> Option<bool> {
        let page = hk_hal::PAGE_SIZE as u64;
        let offset = region.capability_offset(address, page);
        if offset >= capability.size() {
            return None;
        }
        let page_index = (offset / page) as usize;
        let page_address = region.page_address(offset);
        if self.platform.user_mapped(page_address) {
            return Some(true);
        }
        let Ok(Some(range)) =
            self.kernel
                .capability_page(region.tcb, region.capability, page_index)
        else {
            return None;
        };
        let Ok(charge) = self.process_mut(index).ledger.charge(page) else {
            return Some(false);
        };
        if !self.platform.map_region(
            index,
            &hk_mem::RegionMapping {
                virt: page_address,
                phys: range.start().get(),
                len: page as usize,
                rights: region.rights,
                flags: region.flags,
                policy: capability.cache(),
            },
        ) {
            let _ = self.process_mut(index).ledger.release(charge);
            return Some(false);
        }
        self.record_page_charge(index, region.region, page_index, charge);
        Some(true)
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

    pub(super) fn anonymous_fault_region(
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
                let _ = self.platform.map_region(
                    index,
                    &hk_mem::RegionMapping {
                        virt: page_address,
                        phys: range.start().get(),
                        len: page as usize,
                        rights: region.rights,
                        flags: region.flags,
                        policy: capability.cache(),
                    },
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
                let _ = self.platform.map_region(
                    index,
                    &hk_mem::RegionMapping {
                        virt: page_address,
                        phys: range.start().get(),
                        len: page as usize,
                        rights: region.rights,
                        flags: region.flags,
                        policy: capability.cache(),
                    },
                );
            }
            return self.platform.user_mapped(page_address);
        }
        let Some(backing) = self.allocate_owned(index, page, None) else {
            return false;
        };
        let Ok(charge) = self.process_mut(index).ledger.charge(page) else {
            return false;
        };
        let range = backing.range();
        if !self.platform.map_region(
            index,
            &hk_mem::RegionMapping {
                virt: page_address,
                phys: range.start().get(),
                len: page as usize,
                rights: region.rights,
                flags: region.flags,
                policy: capability.cache(),
            },
        ) {
            let _ = self.process_mut(index).ledger.release(charge);
            return false;
        }
        if !self
            .kernel
            .set_capability_page(region.tcb, region.capability, page_index, backing)
            .unwrap_or(false)
        {
            let _ = self.platform.unmap_user(index, page_address, page as usize);
            let _ = self.process_mut(index).ledger.release(charge);
            return false;
        }
        self.record_page_charge(index, region.region, page_index, charge);
        self.platform.zero(page_address, page as usize);
        true
    }
}
