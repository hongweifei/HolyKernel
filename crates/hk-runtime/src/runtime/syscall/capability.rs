//! 类型化能力的创建与物理区间查询。

use hk_hal::PAGE_SIZE;
use hk_handle::Rights;
use hk_mem::Frame as PhysicalFrame;
use hk_mem::hmp::{Capability, CapabilityKind, CapabilityOp};

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    pub(crate) fn capability_create(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let Some(kind) = capability_kind(frame.arg0() as u8) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let length = frame.arg1() as usize;
        if length == 0 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let aligned = (length as u64).div_ceil(PAGE_SIZE as u64) * PAGE_SIZE as u64;
        let Ok(charge) = self.tasks[index].ledger.charge(aligned) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some(range) = self.platform.dma_window(aligned as usize) else {
            let _ = self.tasks[index].ledger.release(charge);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(capability) = Capability::new(kind, aligned, PAGE_SIZE, None) else {
            self.platform.dma_free(range);
            let _ = self.tasks[index].ledger.release(charge);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let backing = PhysicalFrame::new(range.start(), aligned as usize);
        let handle = match self.kernel.create_capability(
            tcb,
            capability,
            Some(backing),
            Rights::READ | Rights::WRITE | Rights::TRANSFER | Rights::ADMIN,
        ) {
            Ok(handle) => handle,
            Err(_) => {
                let _ = self.tasks[index].ledger.release(charge);
                frame.set_result(u64::MAX);
                return Switch::Continue;
            }
        };
        frame.set_result(self.install_capability(index, handle, charge));
        Switch::Continue
    }

    pub(crate) fn capability_pin(
        &mut self,
        index: usize,
        tcb: TcbId,
        frame: &mut C,
        scanout: bool,
    ) -> Switch {
        let number = frame.arg0() as usize;
        let pinned = frame.arg1() != 0;
        let Some((handle, capability, _)) = self.capability_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let operation = if scanout {
            CapabilityOp::Scanout
        } else {
            CapabilityOp::Pin
        };
        if capability.require(operation).is_err() {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        if self.kernel.pin_capability(tcb, handle, pinned).is_err() {
            frame.set_result(u64::MAX);
        } else {
            frame.set_result(0);
        }
        Switch::Continue
    }

    pub(crate) fn capability_commit(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let Some((handle, capability, _)) = self.capability_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capability.require(CapabilityOp::Commit).is_err() {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        match self.kernel.commit_capability(tcb, handle) {
            Ok(version) => frame.set_result(version),
            Err(_) => frame.set_result(u64::MAX),
        }
        Switch::Continue
    }

    pub(crate) fn capability_export(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let Some((handle, capability, _)) = self.capability_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capability.require(CapabilityOp::Export).is_err() {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let rights = Rights::READ | Rights::WRITE | Rights::TRANSFER;
        match self.kernel.duplicate_handle(tcb, handle, rights) {
            Ok(copy) => {
                let number = self.install_extra_handle(index, copy);
                frame.set_result(number);
            }
            Err(_) => frame.set_result(u64::MAX),
        }
        Switch::Continue
    }

    pub(crate) fn capability_phys(&mut self, index: usize, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let out = frame.arg1();
        let capacity = frame.arg2() as usize;
        let Some((_, _, Some(range))) = self.capability_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capacity < 16 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let mut bytes = [0_u8; 16];
        bytes[..8].copy_from_slice(&range.start().get().to_le_bytes());
        bytes[8..].copy_from_slice(&range.len().to_le_bytes());
        if self.platform.write_user(out, &bytes) {
            frame.set_result(16);
        } else {
            frame.set_result(u64::MAX);
        }
        Switch::Continue
    }
}

fn capability_kind(value: u8) -> Option<CapabilityKind> {
    match value {
        0 => Some(CapabilityKind::Revision),
        1 => Some(CapabilityKind::Surface),
        2 => Some(CapabilityKind::TaskImage),
        3 => Some(CapabilityKind::DeviceAllocation),
        4 => Some(CapabilityKind::Anonymous),
        5 => Some(CapabilityKind::Frame),
        _ => None,
    }
}
