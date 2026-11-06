//! Region 映射、查询与解除。

use hk_hal::VirtAddr;
use hk_handle::Rights;
use hk_mem::hmp::{Region, RegionFlags};

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    pub(crate) fn region_map(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let offset = frame.arg1();
        let length = frame.arg2() as usize;
        let packed = frame.arg3();
        let Some((_, capability, backing)) = self.capability_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some(backing) = backing else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
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
        let phys = backing.start().get().saturating_add(offset);
        if !self.platform.map_dma(index, virt, phys, length) {
            self.tasks[index].user_window.release(virt);
            frame.set_result(u64::MAX);
            return Switch::Continue;
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
        frame.set_result(self.install_region(index, handle, virt, length));
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

    pub(crate) fn region_unmap(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let Some((slot, handle, virt, length)) = self.region_at(index, number) else {
            frame.set_result(0);
            return Switch::Continue;
        };
        if !self.platform.unmap_dma(index, virt, length) {
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
