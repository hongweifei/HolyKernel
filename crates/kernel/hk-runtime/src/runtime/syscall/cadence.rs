//! Cadence/Meter 系统调用。

use hk_cadence::{CadenceClass, CadenceSnapshot, MeterSnapshot};

use crate::runtime::{Switch, UserPlatform, UserRuntime};
use alloc::string::String;
use alloc::vec::Vec;
use hk_hal::{SyscallFrame, UserContext};
use hk_handle::Rights;
use hk_kernel::TcbId;
use hk_syscall::CADENCE_DEMAND_RECORD_BYTES;

/// 按名称解析任务时的名字上限。
const MAX_TASK_NAME: usize = 64;

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

    /// `SYS_METER_RESET(handle)`；只有持有 Meter ADMIN 的调用方可以重置。
    pub(crate) fn meter_reset(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let result = self
            .handle_at(index, frame.arg0() as usize)
            .and_then(|handle| self.kernel.reset_meter(tcb, handle).ok())
            .map_or(u64::MAX, |()| 0);
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
                match self.kernel.split_cadence(tcb, parent, snapshot.spec()) {
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

    /// `SYS_CADENCE_NAMED(name, len, out, out_len)`；仅策略授权任务可用。
    pub(crate) fn cadence_named(&mut self, _index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let result = if !self.kernel.is_policy_authority(tcb) {
            u64::MAX
        } else {
            self.read_task_name(frame.arg0(), frame.arg1() as usize)
                .and_then(|name| {
                    let index = self
                        .tasks
                        .iter()
                        .position(|task| !task.exited && task.name == name)?;
                    self.kernel.task_cadence_snapshot(self.tasks[index].tcb)
                })
                .and_then(|snapshot| {
                    let mut bytes = [0_u8; CadenceSnapshot::ENCODED_LEN];
                    snapshot.encode(&mut bytes);
                    let written = self.write_bytes(frame.arg2(), frame.arg3() as usize, &bytes);
                    (written != u64::MAX).then_some(written)
                })
                .unwrap_or(u64::MAX)
        };
        frame.set_result(result);
        Switch::Continue
    }

    /// `SYS_TASK_ATTACH_NAMED(name, len, cadence)`；仅策略授权任务可用。
    pub(crate) fn task_attach_named(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let result = self
            .read_task_name(frame.arg0(), frame.arg1() as usize)
            .zip(self.handle_at(index, frame.arg2() as usize))
            .and_then(|(name, cadence)| {
                let target = self.named_target(tcb, &name)?;
                self.kernel
                    .attach_cadence_to_task(tcb, target, cadence)
                    .ok()
            });
        frame.set_result(result.map_or(u64::MAX, |()| 0));
        Switch::Continue
    }

    /// `SYS_TASK_REGRANT_NAMED(name, len, cadence)`；仅策略授权任务可用。
    pub(crate) fn task_regrant_named(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let result = self
            .read_task_name(frame.arg0(), frame.arg1() as usize)
            .zip(self.handle_at(index, frame.arg2() as usize))
            .and_then(|(name, cadence)| {
                let target = self.named_target(tcb, &name)?;
                self.kernel
                    .regrant_cadence_to_task(tcb, target, cadence)
                    .ok()
            });
        frame.set_result(result.map_or(u64::MAX, u64::from));
        Switch::Continue
    }

    /// `SYS_CADENCE_DEMAND(exclude, out, len)`；返回写入的 Realtime 契约数。
    ///
    /// 只统计存活 RealTime Task（每个 Task 一次），跳过调用者自身与 `exclude`
    /// 指定的 Cadence。Tempo 用它在真实系统状态上做 EDF 需求界判定，不再维护
    /// 一份可能失真的服务内账本。
    pub(crate) fn cadence_demand(&mut self, tcb: TcbId, frame: &mut C) -> Switch {
        let exclude = frame.arg0();
        let output = frame.arg1();
        let capacity = frame.arg2() as usize;
        let result = if !self.kernel.is_policy_authority(tcb) {
            u64::MAX
        } else {
            let caller = self.kernel.task_of(tcb);
            let mut records = Vec::new();
            for (slot, task) in self.tasks.iter().enumerate() {
                if task.exited || task.leader != slot {
                    continue;
                }
                if self.kernel.task_of(task.tcb) == caller {
                    continue;
                }
                let Some(snapshot) = self.kernel.task_cadence_snapshot(task.tcb) else {
                    continue;
                };
                if snapshot.class != CadenceClass::Realtime
                    || snapshot.budget == 0
                    || snapshot.period == 0
                    || (exclude != 0 && snapshot.id == exclude)
                {
                    continue;
                }
                records.push((snapshot.budget, snapshot.period, snapshot.deadline));
            }
            let length = records.len() * CADENCE_DEMAND_RECORD_BYTES;
            if length > capacity {
                u64::MAX
            } else {
                let mut bytes = Vec::with_capacity(length);
                for (budget, period, deadline) in &records {
                    bytes.extend_from_slice(&budget.to_le_bytes());
                    bytes.extend_from_slice(&period.to_le_bytes());
                    bytes.extend_from_slice(&deadline.to_le_bytes());
                }
                if length > 0 && !self.platform.write_user(output, &bytes) {
                    u64::MAX
                } else {
                    records.len() as u64
                }
            }
        };
        frame.set_result(result);
        Switch::Continue
    }

    /// `SYS_OBJECT_GRANT_NAMED(name, len, kind, rights)`；返回句柄编号。
    ///
    /// 策略任务按名取得 Task/Cadence/Meter 句柄，使句柄式调度 ABI 在生产中可达。
    pub(crate) fn object_grant_named(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let rights = u32::try_from(frame.arg3()).ok().and_then(Rights::from_bits);
        let result = self
            .read_task_name(frame.arg0(), frame.arg1() as usize)
            .zip(rights)
            .and_then(|(name, rights)| {
                let target = self.named_target(tcb, &name)?;
                let handle = match frame.arg2() {
                    hk_syscall::OBJECT_KIND_TASK => self.kernel.grant_task(tcb, target, rights),
                    hk_syscall::OBJECT_KIND_CADENCE => {
                        self.kernel.grant_cadence(tcb, target, rights)
                    }
                    hk_syscall::OBJECT_KIND_METER => self.kernel.grant_meter(tcb, target, rights),
                    _ => return None,
                }
                .ok()?;
                Some(self.install_extra_handle(index, handle))
            })
            .unwrap_or(u64::MAX);
        frame.set_result(result);
        Switch::Continue
    }

    fn named_target(&self, caller: TcbId, name: &str) -> Option<hk_kernel::TaskId> {
        if !self.kernel.is_policy_authority(caller) {
            return None;
        }
        let index = self
            .tasks
            .iter()
            .position(|task| task.name == name && !task.exited)?;
        let target = &self.tasks[index];
        if target.tcb == caller || self.kernel.is_policy_authority(target.tcb) {
            return None;
        }
        self.kernel.task_of(target.tcb)
    }

    fn read_task_name(&self, pointer: u64, length: usize) -> Option<String> {
        if length == 0 || length > MAX_TASK_NAME {
            return None;
        }
        let mut bytes = [0_u8; MAX_TASK_NAME];
        if !self.platform.read_user(pointer, &mut bytes[..length]) {
            return None;
        }
        core::str::from_utf8(&bytes[..length])
            .ok()
            .map(String::from)
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
