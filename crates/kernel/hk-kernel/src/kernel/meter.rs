//! Meter 对象句柄操作。
//!
//! `SYS_METER_*` 的内核侧实现：查询、重置与 checkpoint 恢复。计量是调度的
//! 唯一事实来源，任务管理器、Debug Server 与检查点读取同一份数据。

use hk_cadence::{Meter, MeterSnapshot};
use hk_handle::{Handle, ObjectKind, Rights};

use super::Kernel;
use crate::error::KernelError;
use crate::task::TaskId;
use crate::tcb::TcbId;

impl Kernel {
    /// 按句柄读取 Meter 快照。
    pub fn meter_snapshot(
        &self,
        thread: TcbId,
        handle: Handle,
    ) -> Result<MeterSnapshot, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::READ, ObjectKind::Meter)?;
        let meter = object.meter().cloned().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Meter,
        })?;
        Ok(meter.lock().snapshot())
    }

    /// 重置 Meter 计量；需要 Meter 的 ADMIN。
    pub fn reset_meter(&self, thread: TcbId, handle: Handle) -> Result<(), KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::ADMIN, ObjectKind::Meter)?;
        let meter = object.meter().cloned().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Meter,
        })?;
        meter.lock().reset();
        Ok(())
    }

    /// 读取 TCB 所属 Task 的 Meter 快照。
    pub fn task_meter_snapshot(&self, thread: TcbId) -> Option<MeterSnapshot> {
        let task = self.task(self.task_of(thread)?)?;
        let meter = task.lock().meter();
        Some(meter.lock().snapshot())
    }

    /// 从 checkpoint 恢复 Meter 水位。
    pub fn restore_meter(&self, task: TaskId, snapshot: MeterSnapshot) -> Result<(), KernelError> {
        let task = self.task(task).ok_or(KernelError::UnknownTask(task))?;
        let meter = task.lock().meter();
        *meter.lock() = Meter::restore(snapshot);
        Ok(())
    }
}
