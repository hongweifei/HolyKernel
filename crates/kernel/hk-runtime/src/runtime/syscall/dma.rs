//! DMA 与 IOMMU 映射。

use super::*;

/// 设备授权对应的 IOMMU 域（BDF）。
pub(crate) fn domain_of(grant: &DeviceGrant) -> hk_hal::IommuDomain {
    hk_hal::IommuDomain::new(
        (u32::from(grant.address.bus) << 8)
            | (u32::from(grant.address.device) << 3)
            | u32::from(grant.address.function),
    )
}

/// 在授权的 DMA 窗口内为一次映射分配 IOVA。
fn next_iova(
    mappings: &[(Handle, hk_hal::IommuDomain, hk_hal::Iova, usize)],
    grant: Handle,
    window: hk_hal::PhysRange,
    len: usize,
) -> Option<hk_hal::Iova> {
    let page = hk_hal::PAGE_SIZE as u64;
    let mut offset = 0_u64;
    for (mapped_grant, _, _, mapped_len) in mappings {
        if *mapped_grant == grant {
            offset = offset.checked_add((*mapped_len as u64).div_ceil(page) * page)?;
        }
    }
    let start = window.start().get().checked_add(offset)?;
    let end = start.checked_add((len as u64).div_ceil(page) * page)?;
    (end <= window.end().get()).then(|| hk_hal::Iova::new(start))
}

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    pub(crate) fn dma_map_frame(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let grant_number = frame.arg0() as usize;
        let frame_number = frame.arg1() as usize;
        let Some(grant_handle) = self.grant_at(index, grant_number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(grant) = self.kernel.device_grant(tcb, grant_handle) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some(frame_handle) = self.handle_at(index, frame_number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok((range, _, _, _, _)) = self.kernel.frame_info(tcb, frame_handle) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some(iommu) = self.platform.iommu() else {
            frame.set_result(0);
            return Switch::Continue;
        };
        let domain = domain_of(&grant);
        let translating = iommu.translates_device_addresses();
        let iova = if translating {
            let Some(window) = grant.dma else {
                frame.set_result(u64::MAX);
                return Switch::Continue;
            };
            let Some(iova) = next_iova(
                &self.tasks[index].dma_mappings,
                grant_handle,
                window,
                range.len() as usize,
            ) else {
                frame.set_result(u64::MAX);
                return Switch::Continue;
            };
            iova
        } else {
            hk_hal::Iova::new(range.start().get())
        };
        if self.tasks[index]
            .dma_mappings
            .iter()
            .any(|(_, mapped_domain, mapped_iova, _)| {
                *mapped_domain == domain && *mapped_iova == iova
            })
        {
            frame.set_result(if translating { iova.get() } else { 0 });
            return Switch::Continue;
        }
        if iommu
            .map(
                domain,
                iova,
                range,
                hk_hal::IommuFlags::READ | hk_hal::IommuFlags::WRITE,
            )
            .is_err()
        {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        self.tasks[index]
            .dma_mappings
            .push((grant_handle, domain, iova, range.len() as usize));
        frame.set_result(if translating { iova.get() } else { 0 });
        Switch::Continue
    }

    pub(crate) fn dma_map_capability(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let grant_number = frame.arg0() as usize;
        let capability_number = frame.arg1() as usize;
        let Some(grant_handle) = self.grant_at(index, grant_number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(grant) = self.kernel.device_grant(tcb, grant_handle) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some((_, _, Some(range))) = self.capability_at(index, capability_number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some(iommu) = self.platform.iommu() else {
            frame.set_result(0);
            return Switch::Continue;
        };
        let domain = domain_of(&grant);
        let translating = iommu.translates_device_addresses();
        let iova = if translating {
            let Some(window) = grant.dma else {
                frame.set_result(u64::MAX);
                return Switch::Continue;
            };
            let Some(iova) = next_iova(
                &self.tasks[index].dma_mappings,
                grant_handle,
                window,
                range.len() as usize,
            ) else {
                frame.set_result(u64::MAX);
                return Switch::Continue;
            };
            iova
        } else {
            hk_hal::Iova::new(range.start().get())
        };
        if self.tasks[index]
            .dma_mappings
            .iter()
            .any(|(_, mapped_domain, mapped_iova, _)| {
                *mapped_domain == domain && *mapped_iova == iova
            })
        {
            frame.set_result(if translating { iova.get() } else { 0 });
            return Switch::Continue;
        }
        if iommu
            .map(
                domain,
                iova,
                range,
                hk_hal::IommuFlags::READ | hk_hal::IommuFlags::WRITE,
            )
            .is_err()
        {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        self.tasks[index]
            .dma_mappings
            .push((grant_handle, domain, iova, range.len() as usize));
        frame.set_result(if translating { iova.get() } else { 0 });
        Switch::Continue
    }
}
