//! DMA 与 IOMMU 映射。

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
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
        let domain = hk_hal::IommuDomain::new(
            (u32::from(grant.address.bus) << 8)
                | (u32::from(grant.address.device) << 3)
                | u32::from(grant.address.function),
        );
        let iova = hk_hal::Iova::new(range.start().get());
        if self.tasks[index]
            .dma_mappings
            .iter()
            .any(|(_, mapped_domain, mapped_iova, _)| {
                *mapped_domain == domain && *mapped_iova == iova
            })
        {
            frame.set_result(0);
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
        frame.set_result(0);
        Switch::Continue
    }
}
