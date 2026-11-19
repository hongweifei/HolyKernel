//! DMA 与 IOMMU 映射。

use crate::runtime::{Switch, UserPlatform, UserRuntime};
use hk_device::DeviceGrant;
use hk_hal::{SyscallFrame, UserContext};
use hk_handle::{Handle, Rights};
use hk_kernel::{KernelError, TcbId};

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
    mappings: &[(
        Handle,
        Handle,
        hk_hal::IommuDomain,
        hk_hal::Iova,
        usize,
        hk_handle::ObjectKind,
    )],
    grant: Handle,
    window: hk_hal::PhysRange,
    len: usize,
) -> Option<hk_hal::Iova> {
    let page = hk_hal::PAGE_SIZE as u64;
    let mut offset = 0_u64;
    for (mapped_grant, _, _, _, mapped_len, _) in mappings {
        if *mapped_grant == grant {
            offset = offset.checked_add((*mapped_len as u64).div_ceil(page) * page)?;
        }
    }
    let start = window.start().get().checked_add(offset)?;
    let end = start.checked_add((len as u64).div_ceil(page) * page)?;
    (end <= window.end().get()).then(|| hk_hal::Iova::new(start))
}

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 把 Frame 或类型化能力绑定到 DeviceGrant 的 IOMMU 域。
    ///
    /// 两者的物理区间来源不同，但 IOMMU 记账、IOVA 分配与去重逻辑一致。
    pub(crate) fn dma_map(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let grant_number = frame.arg0() as usize;
        let object_number = frame.arg1() as usize;
        let Some(grant_handle) = self.grant_at(index, grant_number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(grant) = self.kernel.device_grant(tcb, grant_handle) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some(object) = self.handle_at(index, object_number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let pages: alloc::vec::Vec<hk_hal::PhysRange> = if let Ok((range, _, _, _, _)) =
            self.kernel.frame_info(tcb, object)
        {
            alloc::vec![range]
        } else if let Ok((capability, Some(range))) = self.kernel.capability(tcb, object) {
            let _ = capability;
            alloc::vec![range]
        } else if let Ok((capability, None)) = self.kernel.capability(tcb, object) {
            // 非连续能力按页建立 IOMMU 映射，设备侧看到连续 IOVA。
            let count = capability.size().div_ceil(hk_hal::PAGE_SIZE as u64) as usize;
            let mut pages = alloc::vec::Vec::with_capacity(count);
            for page_index in 0..count {
                let Ok(Some(range)) = self.kernel.capability_page(tcb, object, page_index) else {
                    frame.set_result(u64::MAX);
                    return Switch::Continue;
                };
                pages.push(range);
            }
            pages
        } else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let total_len: usize = pages.iter().map(|range| range.len() as usize).sum();
        if total_len == 0 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        // 设备映射的权限来自句柄：READ 才能导出，WRITE 才能让设备写；缺少对应
        // 权利时不能把只读对象以可写方式交给设备。
        let Some(kind) = self.kernel.handle_kind(tcb, object) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(rights) = self.kernel.handle_rights(tcb, object, kind) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if !rights.contains(Rights::READ) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let mut iommu_flags = hk_hal::IommuFlags::READ;
        if rights.contains(Rights::WRITE) {
            iommu_flags |= hk_hal::IommuFlags::WRITE;
        }
        let Some(iommu) = self.platform.iommu() else {
            frame.set_result(0);
            return Switch::Continue;
        };
        let domain = domain_of(&grant);
        let translating = iommu.translates_device_addresses();
        if !translating {
            // 没有 IOMMU 地址转换时设备直接使用物理地址，scatter 无法表达。
            let mut expected = pages[0].start();
            for page in &pages {
                if page.start() != expected {
                    frame.set_result(u64::MAX);
                    return Switch::Continue;
                }
                expected = hk_hal::PhysAddr::new(expected.get() + page.len());
            }
        }
        let iova = if translating {
            let Some(window) = grant.dma else {
                frame.set_result(u64::MAX);
                return Switch::Continue;
            };
            let Some(iova) = next_iova(
                &self.tasks[index].dma_mappings,
                grant_handle,
                window,
                total_len,
            ) else {
                frame.set_result(u64::MAX);
                return Switch::Continue;
            };
            iova
        } else {
            hk_hal::Iova::new(pages[0].start().get())
        };
        if self.tasks[index]
            .dma_mappings
            .iter()
            .any(|(_, _, mapped_domain, mapped_iova, _, _)| {
                *mapped_domain == domain && *mapped_iova == iova
            })
        {
            frame.set_result(if translating { iova.get() } else { 0 });
            return Switch::Continue;
        }
        // DMA 期间固定对象，避免回收/复用正在被设备访问的物理页。
        let pinned = match kind {
            hk_handle::ObjectKind::Frame => {
                self.kernel.pin_frame_internal(tcb, object, true).is_ok()
            }
            hk_handle::ObjectKind::Capability => {
                self.kernel.pin_capability_dma(tcb, object, true).is_ok()
            }
            _ => false,
        };
        if !pinned {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let mut offset = 0_u64;
        let mut mapped: alloc::vec::Vec<(u64, hk_hal::PhysRange)> = alloc::vec::Vec::new();
        for page in &pages {
            let address = hk_hal::Iova::new(iova.get() + offset);
            if iommu.map(domain, address, *page, iommu_flags).is_err() {
                for (mapped_offset, previous) in &mapped {
                    let _ = iommu.unmap(
                        domain,
                        hk_hal::Iova::new(iova.get() + mapped_offset),
                        previous.len() as usize,
                    );
                }
                let _ = self.unpin_dma_object(tcb, object, kind);
                frame.set_result(u64::MAX);
                return Switch::Continue;
            }
            mapped.push((offset, *page));
            offset = offset.saturating_add(page.len());
        }
        self.tasks[index]
            .dma_mappings
            .push((grant_handle, object, domain, iova, total_len, kind));
        frame.set_result(if translating { iova.get() } else { 0 });
        Switch::Continue
    }

    /// 解除 DMA 映射期间对对象的固定。
    pub(crate) fn unpin_dma_object(
        &self,
        tcb: TcbId,
        object: Handle,
        kind: hk_handle::ObjectKind,
    ) -> Result<(), KernelError> {
        match kind {
            hk_handle::ObjectKind::Frame => self.kernel.pin_frame_internal(tcb, object, false),
            hk_handle::ObjectKind::Capability => self.kernel.pin_capability_dma(tcb, object, false),
            _ => Ok(()),
        }
    }
}
