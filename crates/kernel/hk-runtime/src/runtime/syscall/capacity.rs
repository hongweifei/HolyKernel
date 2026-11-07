//! Capacity 与 Frame 对象操作。

use hk_mem::hmp::CapacityKind;
use hk_syscall::{
    CAPACITY_KIND_DMA, CAPACITY_KIND_GPU, CAPACITY_KIND_NUMA, CAPACITY_KIND_SYSTEM,
    CAPACITY_KIND_USER,
};

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    pub(crate) fn capacity_info(&mut self, index: usize, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let out = frame.arg1();
        let capacity = frame.arg2() as usize;
        let Some((_, handle)) = self.capacity_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok((range, kind, page_size, limit, used)) =
            self.kernel.capacity_info(self.tasks[index].tcb, handle)
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capacity < 48 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let mut bytes = [0_u8; 48];
        bytes[..8].copy_from_slice(&range.start().get().to_le_bytes());
        bytes[8..16].copy_from_slice(&range.len().to_le_bytes());
        bytes[16..24].copy_from_slice(&(capacity_kind_code(kind) as u64).to_le_bytes());
        bytes[24..32].copy_from_slice(&(page_size as u64).to_le_bytes());
        bytes[32..40].copy_from_slice(&limit.to_le_bytes());
        bytes[40..].copy_from_slice(&used.to_le_bytes());
        frame.set_result(if self.platform.write_user(out, &bytes) {
            48
        } else {
            u64::MAX
        });
        Switch::Continue
    }

    pub(crate) fn capacity_split(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let length = frame.arg1() as usize;
        let Some(kind) = capacity_kind_from_code(frame.arg2() as u8) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some((_, handle)) = self.capacity_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(child) = self.kernel.split_capacity(tcb, handle, length, kind) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        frame.set_result(self.install_capacity(index, child));
        Switch::Continue
    }

    pub(crate) fn capacity_alloc(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let length = frame.arg1() as usize;
        let Some((_, handle)) = self.capacity_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(frame_handle) = self.kernel.allocate_frame(tcb, handle, length) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        frame.set_result(self.install_frame(index, frame_handle));
        Switch::Continue
    }

    pub(crate) fn capacity_free(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let capacity = frame.arg0() as usize;
        let frame_number = frame.arg1() as usize;
        frame.set_result(self.free_frame_at(index, tcb, capacity, frame_number));
        Switch::Continue
    }

    pub(crate) fn frame_free(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let frame_number = frame.arg0() as usize;
        let capacity = frame.arg1() as usize;
        frame.set_result(self.free_frame_at(index, tcb, capacity, frame_number));
        Switch::Continue
    }

    pub(crate) fn frame_info(&mut self, index: usize, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let out = frame.arg1();
        let capacity = frame.arg2() as usize;
        let Some((_, handle)) = self.frame_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok((range, page_size, capacity_id, refs, pinned)) =
            self.kernel.frame_info(self.tasks[index].tcb, handle)
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capacity < 48 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let mut bytes = [0_u8; 48];
        bytes[..8].copy_from_slice(&range.start().get().to_le_bytes());
        bytes[8..16].copy_from_slice(&range.len().to_le_bytes());
        bytes[16..24].copy_from_slice(&(page_size as u64).to_le_bytes());
        bytes[24..32].copy_from_slice(&capacity_id.get().to_le_bytes());
        bytes[32..40].copy_from_slice(&(refs as u64).to_le_bytes());
        bytes[40..].copy_from_slice(&u64::from(pinned).to_le_bytes());
        frame.set_result(if self.platform.write_user(out, &bytes) {
            48
        } else {
            u64::MAX
        });
        Switch::Continue
    }

    pub(crate) fn frame_pin(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let pinned = frame.arg1() != 0;
        let Some((_, handle)) = self.frame_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        match self.kernel.pin_frame(tcb, handle, pinned) {
            Ok(()) => frame.set_result(0),
            Err(_) => frame.set_result(u64::MAX),
        }
        Switch::Continue
    }

    pub(crate) fn frame_share(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let Some((_, handle)) = self.frame_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        match self.kernel.share_frame(tcb, handle) {
            Ok(refs) => frame.set_result(refs as u64),
            Err(_) => frame.set_result(u64::MAX),
        }
        Switch::Continue
    }

    pub(crate) fn frame_release(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let Some((_, handle)) = self.frame_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        match self.kernel.release_frame(tcb, handle) {
            Ok(refs) => frame.set_result(refs as u64),
            Err(_) => frame.set_result(u64::MAX),
        }
        Switch::Continue
    }

    fn free_frame_at(
        &mut self,
        index: usize,
        tcb: TcbId,
        capacity: usize,
        frame_number: usize,
    ) -> u64 {
        let Some((_, capacity)) = self.capacity_at(index, capacity) else {
            return u64::MAX;
        };
        let Some((slot, frame)) = self.frame_at(index, frame_number) else {
            return u64::MAX;
        };
        if self.kernel.free_frame(tcb, capacity, frame).is_err() {
            return u64::MAX;
        }
        let _ = self.kernel.close_handle(tcb, frame);
        self.tasks[index].frames[slot] = None;
        1
    }
}

fn capacity_kind_code(kind: CapacityKind) -> u8 {
    match kind {
        CapacityKind::System => CAPACITY_KIND_SYSTEM,
        CapacityKind::User => CAPACITY_KIND_USER,
        CapacityKind::Dma => CAPACITY_KIND_DMA,
        CapacityKind::Gpu => CAPACITY_KIND_GPU,
        CapacityKind::Numa(_) => CAPACITY_KIND_NUMA,
    }
}

fn capacity_kind_from_code(code: u8) -> Option<CapacityKind> {
    Some(match code {
        CAPACITY_KIND_SYSTEM => CapacityKind::System,
        CAPACITY_KIND_USER => CapacityKind::User,
        CAPACITY_KIND_DMA => CapacityKind::Dma,
        CAPACITY_KIND_GPU => CapacityKind::Gpu,
        CAPACITY_KIND_NUMA => CapacityKind::Numa(0),
        _ => return None,
    })
}
