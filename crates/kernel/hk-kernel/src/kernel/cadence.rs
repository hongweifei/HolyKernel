//! Cadence/Meter 对象句柄操作。
//!
//! `SYS_CADENCE_*` 与 `SYS_METER_INFO` 的内核侧实现：查询、更新、拆分、撤销与
//! 重新附加契约。策略由 Tempo 决定，内核只校验 Rights 与契约不变量。

use hk_base::Duration;
use hk_cadence::{Cadence, CadenceId, CadenceSnapshot, CadenceSpec, Meter, MeterSnapshot, Weight};

use super::*;

impl Kernel {
    fn cadence_object(
        &self,
        thread: TcbId,
        handle: Handle,
        rights: Rights,
    ) -> Result<Arc<spin::Mutex<Cadence>>, KernelError> {
        let (object, _) = self.resolve(thread, handle, rights, ObjectKind::Cadence)?;
        object.cadence().cloned().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Cadence,
        })
    }

    /// 按句柄读取 Cadence 快照。
    pub fn cadence_snapshot(
        &self,
        thread: TcbId,
        handle: Handle,
    ) -> Result<CadenceSnapshot, KernelError> {
        let cadence = self.cadence_object(thread, handle, Rights::READ)?;
        Ok(CadenceSnapshot::from_cadence(&cadence.lock()))
    }

    /// 读取 TCB 所属 Task 的 Cadence 快照。
    pub fn task_cadence_snapshot(&self, thread: TcbId) -> Option<CadenceSnapshot> {
        let task = self.task(self.task_of(thread)?)?;
        let cadence = task.lock().cadence();
        Some(CadenceSnapshot::from_cadence(&cadence.lock()))
    }

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

    /// 授予或收回调度策略授权；由 Root 在启动时设置。
    pub fn set_policy_authority(&self, task: TaskId, policy: bool) -> Result<(), KernelError> {
        let task = self.task(task).ok_or(KernelError::UnknownTask(task))?;
        task.lock().set_policy_authority(policy);
        Ok(())
    }

    /// 取得当前 Task 的 Cadence 句柄；策略 Task 额外获得 ADMIN。
    pub fn self_cadence_handle(&self, thread: TcbId) -> Result<Handle, KernelError> {
        let task_id = self
            .task_of(thread)
            .ok_or(KernelError::UnknownThread(thread))?;
        let task = self
            .task(task_id)
            .ok_or(KernelError::UnknownTask(task_id))?;
        let (policy, cadence) = {
            let task = task.lock();
            (task.policy_authority(), task.cadence())
        };
        let rights = if policy {
            Rights::READ | Rights::WRITE | Rights::ADMIN
        } else {
            Rights::READ | Rights::WRITE
        };
        self.register(
            thread,
            Arc::new(KernelObject::new(ObjectPayload::Cadence(cadence))),
            rights,
        )
    }

    /// 把调用方已持有的 Cadence 附加到自己；`task = 0` 的自助路径使用。
    pub fn attach_own_cadence(
        &self,
        thread: TcbId,
        cadence_handle: Handle,
    ) -> Result<(), KernelError> {
        let cadence = self.cadence_object(thread, cadence_handle, Rights::READ)?;
        let task_id = self
            .task_of(thread)
            .ok_or(KernelError::UnknownThread(thread))?;
        let task = self
            .task(task_id)
            .ok_or(KernelError::UnknownTask(task_id))?;
        {
            let mut task = task.lock();
            task.set_cadence(cadence);
        }
        self.enqueue_task(task_id);
        Ok(())
    }

    /// 在父契约范围内更新 Cadence。
    pub fn update_cadence(
        &self,
        thread: TcbId,
        handle: Handle,
        spec: CadenceSpec,
    ) -> Result<(), KernelError> {
        let cadence = self.cadence_object(thread, handle, Rights::WRITE)?;
        let mut guard = cadence.lock();
        let replacement = Cadence::restore(guard.id(), guard.parent(), guard.epoch(), spec)?;
        *guard = replacement;
        Ok(())
    }

    /// 派生一个子契约并在调用方句柄表中返回新 Handle。
    pub fn split_cadence(
        &self,
        thread: TcbId,
        handle: Handle,
        weight: u32,
        budget: u64,
    ) -> Result<Handle, KernelError> {
        let cadence = self.cadence_object(thread, handle, Rights::ADMIN)?;
        let child = cadence.lock().split(
            CadenceId::next(),
            Weight::new(weight)?,
            Duration::from_ticks(budget),
        )?;
        let child = Arc::new(spin::Mutex::new(child));
        self.register(
            thread,
            Arc::new(KernelObject::new(ObjectPayload::Cadence(child))),
            Rights::READ | Rights::WRITE | Rights::ADMIN | Rights::TRANSFER,
        )
    }

    /// 撤销 Cadence；返回新 epoch，并挂起使用该契约的 Task。
    pub fn revoke_cadence(&self, thread: TcbId, handle: Handle) -> Result<u32, KernelError> {
        let cadence = self.cadence_object(thread, handle, Rights::ADMIN)?;
        let epoch = cadence.lock().revoke();
        self.suspend_using(&cadence);
        Ok(epoch)
    }

    /// 为 Task 重新附加 Cadence，并把它重新入队。
    pub fn attach_cadence(
        &self,
        thread: TcbId,
        task_handle: Handle,
        cadence_handle: Handle,
    ) -> Result<(), KernelError> {
        let (object, _) = self.resolve(thread, task_handle, Rights::ADMIN, ObjectKind::Task)?;
        let task = object.task().cloned().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Task,
        })?;
        let cadence = self.cadence_object(thread, cadence_handle, Rights::READ)?;
        let task_id = {
            let mut task = task.lock();
            task.set_cadence(Arc::clone(&cadence));
            task.id()
        };
        self.enqueue_task(task_id);
        Ok(())
    }

    fn suspend_using(&self, cadence: &Arc<spin::Mutex<Cadence>>) {
        let tasks: Vec<_> = self.tasks.lock().values().cloned().collect();
        for task in tasks {
            let matches = {
                let task = task.lock();
                Arc::ptr_eq(&task.cadence(), cadence)
            };
            if !matches {
                continue;
            }
            let threads = {
                let mut task = task.lock();
                task.set_state(TaskState::Suspended);
                task.threads().to_vec()
            };
            let mut scheduler = self.scheduler.lock();
            for thread in threads {
                scheduler.remove(thread);
                scheduler.clear_current_if(thread);
            }
        }
    }

    fn enqueue_task(&self, task: TaskId) {
        let Some(task) = self.task(task) else {
            return;
        };
        let threads = {
            let task = task.lock();
            task.threads().to_vec()
        };
        let runnable: Vec<TcbId> = {
            let thread_map = self.threads.lock();
            threads
                .into_iter()
                .filter(|id| {
                    thread_map
                        .get(id)
                        .is_some_and(|tcb| tcb.state() == ThreadState::Runnable)
                })
                .collect()
        };
        for thread in runnable {
            self.enqueue_thread(thread);
        }
    }
}
