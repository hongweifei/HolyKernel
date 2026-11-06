//! 用户映射与句柄复制的系统调用。

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    pub(crate) fn map_create(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let length = frame.arg0() as usize;
        let out = frame.arg1();
        let page = hk_hal::PAGE_SIZE as u64;
        if length == 0 || out == 0 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let aligned = (length as u64).div_ceil(page) * page;
        let Some(virt) = self.tasks[index].user_window.allocate(aligned as usize) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if !self.platform.write_user(out, &virt.to_le_bytes()) {
            self.tasks[index].user_window.release(virt);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Some(range) = self.platform.dma_window(aligned as usize) else {
            self.tasks[index].user_window.release(virt);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let handle = match self.kernel.create_frame(
            tcb,
            range,
            Rights::READ | Rights::WRITE | Rights::TRANSFER | Rights::ADMIN,
        ) {
            Ok(handle) => handle,
            Err(_) => {
                self.platform.dma_free(range);
                self.tasks[index].user_window.release(virt);
                frame.set_result(u64::MAX);
                return Switch::Continue;
            }
        };
        if !self
            .platform
            .map_dma(index, virt, range.start().get(), aligned as usize)
        {
            let _ = self.kernel.close_handle(tcb, handle);
            self.platform.dma_free(range);
            self.tasks[index].user_window.release(virt);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let number = self.install_frame(index, handle);
        frame.set_result(number);
        Switch::Continue
    }

    pub(crate) fn map_attach(&mut self, index: usize, _tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let out = frame.arg1();
        let Some((_, range)) = self.frame_at(index, number) else {
            frame.set_result(0);
            return Switch::Continue;
        };
        if out == 0 {
            frame.set_result(0);
            return Switch::Continue;
        }
        let Some(virt) = self.tasks[index].user_window.allocate(range.len() as usize) else {
            frame.set_result(0);
            return Switch::Continue;
        };
        if !self.platform.write_user(out, &virt.to_le_bytes()) {
            self.tasks[index].user_window.release(virt);
            frame.set_result(0);
            return Switch::Continue;
        }
        if !self
            .platform
            .map_dma(index, virt, range.start().get(), range.len() as usize)
        {
            self.tasks[index].user_window.release(virt);
            frame.set_result(0);
        } else {
            frame.set_result(range.len());
        }
        Switch::Continue
    }

    pub(crate) fn map_unmap(&mut self, index: usize, frame: &mut C) -> Switch {
        let virt = frame.arg0();
        let length = frame.arg1() as usize;
        let result = u64::from(self.platform.unmap_dma(index, virt, length));
        if result != 0 {
            self.tasks[index].user_window.release(virt);
        }
        frame.set_result(result);
        Switch::Continue
    }

    pub(crate) fn map_phys(&mut self, index: usize, _tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let Some((_, range)) = self.frame_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let mut bytes = [0_u8; 16];
        bytes[..8].copy_from_slice(&range.start().get().to_le_bytes());
        bytes[8..].copy_from_slice(&range.len().to_le_bytes());
        if frame.arg2() < 16 || !self.platform.write_user(frame.arg1(), &bytes) {
            frame.set_result(u64::MAX);
        } else {
            frame.set_result(16);
        }
        Switch::Continue
    }

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
        let Some((_, range)) = self.frame_at(index, frame_number) else {
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

    pub(crate) fn duplicate_handle(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let Some(handle) = self.handle_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let rights = Rights::from_bits_truncate(frame.arg1() as u32);
        match self.kernel.duplicate_handle(tcb, handle, rights) {
            Ok(copy) => {
                let number = self.install_extra_handle(index, copy);
                frame.set_result(number);
            }
            Err(_) => frame.set_result(u64::MAX),
        }
        Switch::Continue
    }
}
