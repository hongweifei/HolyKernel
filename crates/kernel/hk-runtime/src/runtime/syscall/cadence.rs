//! Cadence/Meter 系统调用。

use hk_cadence::{CadenceSnapshot, MeterSnapshot};

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// `SYS_CADENCE_INFO(handle, out, len)`；`handle = 0` 查询当前 Task。
    pub(crate) fn cadence_info(&mut self, index: usize, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let snapshot = if number == 0 {
            self.kernel.task_cadence_snapshot(self.tasks[index].tcb)
        } else {
            self.handle_at(index, number).and_then(|handle| {
                self.kernel
                    .cadence_snapshot(self.tasks[index].tcb, handle)
                    .ok()
            })
        };
        let result = match snapshot {
            Some(snapshot) => {
                let mut bytes = [0_u8; CadenceSnapshot::ENCODED_LEN];
                snapshot.encode(&mut bytes);
                self.write_bytes(frame.arg1(), frame.arg2() as usize, &bytes)
            }
            None => u64::MAX,
        };
        frame.set_result(result);
        Switch::Continue
    }

    /// `SYS_METER_INFO(handle, out, len)`；`handle = 0` 查询当前 Task。
    pub(crate) fn meter_info(&mut self, index: usize, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let snapshot = if number == 0 {
            self.kernel.task_meter_snapshot(self.tasks[index].tcb)
        } else {
            self.handle_at(index, number).and_then(|handle| {
                self.kernel
                    .meter_snapshot(self.tasks[index].tcb, handle)
                    .ok()
            })
        };
        let result = match snapshot {
            Some(snapshot) => {
                let mut bytes = [0_u8; MeterSnapshot::ENCODED_LEN];
                snapshot.encode(&mut bytes);
                self.write_bytes(frame.arg1(), frame.arg2() as usize, &bytes)
            }
            None => u64::MAX,
        };
        frame.set_result(result);
        Switch::Continue
    }

    /// `SYS_CADENCE_UPDATE(handle, spec, len)`。
    pub(crate) fn cadence_update(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let handle = self
            .handle_at(index, frame.arg0() as usize)
            .zip(self.read_snapshot(frame.arg1(), frame.arg2() as usize));
        let result = match handle {
            Some((handle, snapshot)) => self
                .kernel
                .update_cadence(tcb, handle, snapshot.spec())
                .map_or(u64::MAX, |()| 0),
            None => u64::MAX,
        };
        frame.set_result(result);
        Switch::Continue
    }

    /// `SYS_CADENCE_SPLIT(parent, spec, len)`；返回新句柄编号。
    pub(crate) fn cadence_split(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let parent = self
            .handle_at(index, frame.arg0() as usize)
            .zip(self.read_snapshot(frame.arg1(), frame.arg2() as usize));
        let result = match parent {
            Some((parent, snapshot)) => {
                let spec = snapshot.spec();
                match self
                    .kernel
                    .split_cadence(tcb, parent, spec.weight, spec.budget.ticks())
                {
                    Ok(child) => self.install_extra_handle(index, child),
                    Err(_) => u64::MAX,
                }
            }
            None => u64::MAX,
        };
        frame.set_result(result);
        Switch::Continue
    }

    /// `SYS_CADENCE_REVOKE(handle)`；返回新 epoch。
    pub(crate) fn cadence_revoke(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let handle = self.handle_at(index, frame.arg0() as usize);
        let result = match handle {
            Some(handle) => self
                .kernel
                .revoke_cadence(tcb, handle)
                .map_or(u64::MAX, u64::from),
            None => u64::MAX,
        };
        frame.set_result(result);
        Switch::Continue
    }

    /// `SYS_TASK_ATTACH_CADENCE(task, cadence)`。
    pub(crate) fn task_attach_cadence(
        &mut self,
        index: usize,
        tcb: TcbId,
        frame: &mut C,
    ) -> Switch {
        let pair = self
            .handle_at(index, frame.arg0() as usize)
            .zip(self.handle_at(index, frame.arg1() as usize));
        let result = if frame.arg0() == 0 {
            match self.handle_at(index, frame.arg1() as usize) {
                Some(cadence) => self
                    .kernel
                    .attach_own_cadence(tcb, cadence)
                    .map_or(u64::MAX, |()| 0),
                None => u64::MAX,
            }
        } else {
            match pair {
                Some((task, cadence)) => self
                    .kernel
                    .attach_cadence(tcb, task, cadence)
                    .map_or(u64::MAX, |()| 0),
                None => u64::MAX,
            }
        };
        frame.set_result(result);
        Switch::Continue
    }

    fn read_snapshot(&self, pointer: u64, length: usize) -> Option<CadenceSnapshot> {
        if length < CadenceSnapshot::ENCODED_LEN {
            return None;
        }
        let mut bytes = [0_u8; CadenceSnapshot::ENCODED_LEN];
        if !self.platform.read_user(pointer, &mut bytes) {
            return None;
        }
        CadenceSnapshot::decode(&bytes).ok()
    }

    fn write_bytes(&self, pointer: u64, length: usize, bytes: &[u8]) -> u64 {
        if length < bytes.len() || !self.platform.write_user(pointer, bytes) {
            return u64::MAX;
        }
        bytes.len() as u64
    }
}
