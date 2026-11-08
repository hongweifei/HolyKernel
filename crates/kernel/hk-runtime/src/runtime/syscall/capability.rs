//! 类型化能力的创建与物理区间查询。

use hk_hal::PAGE_SIZE;
use hk_handle::Rights;
use hk_mem::hmp::{Capability, CapabilityKind, CapabilityOp, FrameBacking, OwnedFrame};

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    pub(super) fn allocate_owned(&mut self, index: usize, len: u64) -> Option<FrameBacking> {
        let capacity = self.system_capacity.clone()?;
        let allocated = { capacity.lock().allocate(len as usize) };
        let frame = match allocated {
            Ok(frame) => frame,
            Err(_) => {
                if self.reclaim_zero_pages(index, len) == 0 {
                    return None;
                }
                capacity.lock().allocate(len as usize).ok()?
            }
        };
        Some(FrameBacking::Owned(OwnedFrame::new(frame, capacity)))
    }

    pub(crate) fn capability_create(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let Some(kind) = CapabilityKind::from_u8(frame.arg0() as u8) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let length = frame.arg1() as usize;
        if length == 0 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let aligned = (length as u64).div_ceil(PAGE_SIZE as u64) * PAGE_SIZE as u64;
        let Ok(charge) = self.process_mut(index).ledger.charge(aligned) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some(backing) = self.allocate_owned(index, aligned) else {
            let _ = self.process_mut(index).ledger.release(charge);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(capability) = Capability::new(kind, aligned, PAGE_SIZE, None) else {
            let _ = self.process_mut(index).ledger.release(charge);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let handle = match self.kernel.create_capability(
            tcb,
            capability,
            Some(backing),
            Rights::READ | Rights::WRITE | Rights::TRANSFER | Rights::ADMIN,
        ) {
            Ok(handle) => handle,
            Err(_) => {
                let _ = self.process_mut(index).ledger.release(charge);
                frame.set_result(u64::MAX);
                return Switch::Continue;
            }
        };
        frame.set_result(self.install_capability(index, handle, charge));
        Switch::Continue
    }

    pub(crate) fn capability_pin(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let pinned = frame.arg1() != 0;
        let Some((handle, capability, _)) = self.capability_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capability.require(CapabilityOp::Pin).is_err() {
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

    /// 设置 Surface 的 scanout 驻留；开启时同时固定，驻留期间不能解除固定。
    pub(crate) fn capability_scanout(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let enabled = frame.arg1() != 0;
        let Some((handle, capability, _)) = self.capability_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capability.require(CapabilityOp::Scanout).is_err() {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        if self
            .kernel
            .scanout_capability(tcb, handle, enabled)
            .is_err()
        {
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
            Ok(copy) => match self.install_extra_capability(index, copy) {
                Some(number) => frame.set_result(number),
                None => frame.set_result(u64::MAX),
            },
            Err(_) => frame.set_result(u64::MAX),
        }
        Switch::Continue
    }

    pub(crate) fn capability_create_lazy(
        &mut self,
        index: usize,
        tcb: TcbId,
        frame: &mut C,
    ) -> Switch {
        let length = frame.arg1() as usize;
        if length == 0
            || CapabilityKind::from_u8(frame.arg0() as u8) != Some(CapabilityKind::Anonymous)
        {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let aligned = (length as u64).div_ceil(PAGE_SIZE as u64) * PAGE_SIZE as u64;
        let Ok(charge) = self.process_mut(index).ledger.charge(aligned) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(capability) = Capability::new(CapabilityKind::Anonymous, aligned, PAGE_SIZE, None)
        else {
            let _ = self.process_mut(index).ledger.release(charge);
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let handle = match self.kernel.create_capability(
            tcb,
            capability,
            None,
            Rights::READ | Rights::WRITE | Rights::TRANSFER,
        ) {
            Ok(handle) => handle,
            Err(_) => {
                let _ = self.process_mut(index).ledger.release(charge);
                frame.set_result(u64::MAX);
                return Switch::Continue;
            }
        };
        frame.set_result(self.install_capability(index, handle, charge));
        Switch::Continue
    }

    pub(crate) fn capability_bind_provider(
        &mut self,
        index: usize,
        tcb: TcbId,
        frame: &mut C,
    ) -> Switch {
        let number = frame.arg0() as usize;
        let endpoint = frame.arg1() as usize;
        let Some((capability, _, _)) = self.capability_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some(endpoint) = self.handle_at(index, endpoint) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        match self
            .kernel
            .bind_capability_provider(tcb, capability, endpoint)
        {
            Ok(()) => frame.set_result(0),
            Err(_) => frame.set_result(u64::MAX),
        }
        Switch::Continue
    }

    pub(crate) fn ledger_info(&mut self, index: usize, frame: &mut C) -> Switch {
        let out = frame.arg0();
        let capacity = frame.arg1() as usize;
        if out == 0 || capacity < 48 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let resident = self.process_resident(index);
        let (charged, limit, peak, pressure, resident, soft_limit) = {
            let ledger = &mut self.process_mut(index).ledger;
            ledger.set_resident(resident);
            (
                ledger.charged(),
                ledger.limit(),
                ledger.peak(),
                ledger.pressure(),
                ledger.resident(),
                ledger.soft_limit(),
            )
        };
        let mut bytes = [0_u8; 48];
        bytes[..8].copy_from_slice(&charged.to_le_bytes());
        bytes[8..16].copy_from_slice(&limit.to_le_bytes());
        bytes[16..24].copy_from_slice(&peak.to_le_bytes());
        bytes[24..32].copy_from_slice(&pressure.to_le_bytes());
        bytes[32..40].copy_from_slice(&resident.to_le_bytes());
        bytes[40..].copy_from_slice(&soft_limit.to_le_bytes());
        frame.set_result(if self.platform.write_user(out, &bytes) {
            48
        } else {
            u64::MAX
        });
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
