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

    /// 判断 TCB 所属 Task 是否持有调度策略授权。
    pub fn is_policy_authority(&self, thread: TcbId) -> bool {
        self.task_of(thread)
            .and_then(|task| self.task(task))
            .is_some_and(|task| task.lock().policy_authority())
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
            Rights::READ
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
        let was_suspended = task.lock().state() == TaskState::Suspended;
        let previous = task.lock().cadence();
        if !Arc::ptr_eq(&previous, &cadence) {
            self.release_child_budget(&previous.lock());
        }
        {
            let mut task = task.lock();
            task.set_cadence(cadence, self.now());
        }
        self.remove_disallowed_home(task_id);
        if was_suspended {
            self.resume_task(task_id);
        } else {
            self.enqueue_next_thread(task_id, None);
        }
        Ok(())
    }

    /// 在父契约范围内更新 Cadence。
    ///
    /// 普通 Task 的 WRITE 只允许在父契约范围内调整权重与预算；类别、亲和性、
    /// 迁移、借贷与周期/截止期必须保持不变。策略 Task（Tempo）可放宽这些字段。
    pub fn update_cadence(
        &self,
        thread: TcbId,
        handle: Handle,
        spec: CadenceSpec,
    ) -> Result<(), KernelError> {
        let cadence = self.cadence_object(thread, handle, Rights::WRITE)?;
        let policy = self.is_policy_authority(thread);
        let parent_id = { cadence.lock().parent() };
        let parent = parent_id.and_then(|id| self.cadence_by_id(id));
        let mut guard = cadence.lock();
        let current = guard.spec();
        if !policy
            && (spec.class != current.class
                || spec.affinity != current.affinity
                || spec.migratable != current.migratable
                || spec.lending != current.lending
                || spec.period != current.period
                || spec.deadline != current.deadline)
        {
            return Err(KernelError::CadenceDenied);
        }
        if let Some(parent) = &parent {
            let parent = parent.lock();
            if spec.weight > parent.weight().get() {
                return Err(KernelError::CadenceDenied);
            }
            if parent.has_budget() {
                let available = parent
                    .budget()
                    .ticks()
                    .saturating_sub(parent.children_budget())
                    .saturating_add(current.budget.ticks());
                if spec.budget.ticks() > available {
                    return Err(KernelError::CadenceDenied);
                }
            } else if !spec.budget.is_zero() {
                return Err(KernelError::CadenceDenied);
            }
        }
        let old_budget = current.budget.ticks();
        let replacement = Cadence::restore(guard.id(), guard.parent(), guard.epoch(), spec)?;
        *guard = replacement;
        drop(guard);
        // 新契约从当前时刻开始预算周期，避免 Realtime 绝对截止期落到过去。
        if let Some(owner) = self.task_with_cadence(&cadence) {
            let (owner_id, running) = {
                let mut task = owner.lock();
                task.reset_period(self.now());
                (task.id(), task.state() == TaskState::Running)
            };
            // 亲和性变更后把仍在旧 CPU 队列中的线程重新挂到允许的 CPU。
            self.remove_disallowed_home(owner_id);
            if !running {
                self.enqueue_next_thread(owner_id, None);
            }
        }
        if let Some(parent) = parent {
            let mut parent = parent.lock();
            parent.release_child_budget(old_budget);
            parent.reserve_child_budget(spec.budget.ticks());
        }
        Ok(())
    }

    /// 返回持有指定 Cadence 的 Task。
    fn task_with_cadence(
        &self,
        cadence: &Arc<spin::Mutex<Cadence>>,
    ) -> Option<Arc<spin::Mutex<Task>>> {
        let tasks: Vec<_> = self.tasks.lock().values().cloned().collect();
        tasks
            .into_iter()
            .find(|task| Arc::ptr_eq(&task.lock().cadence(), cadence))
    }

    /// 按契约标识返回契约描述；用于 checkpoint 恢复时回退父契约。
    pub fn cadence_spec_by_id(&self, id: CadenceId) -> Option<CadenceSpec> {
        self.cadence_by_id(id).map(|cadence| cadence.lock().spec())
    }

    /// 按契约标识查找持有的 Cadence；仅用于父契约范围校验。
    fn cadence_by_id(&self, id: CadenceId) -> Option<Arc<spin::Mutex<Cadence>>> {
        let tasks: Vec<_> = self.tasks.lock().values().cloned().collect();
        for task in tasks {
            let cadence = task.lock().cadence();
            if cadence.lock().id() == id {
                return Some(cadence);
            }
        }
        None
    }

    /// 归还子契约占用的父预算。
    pub(crate) fn release_child_budget(&self, child: &Cadence) {
        let Some(parent) = child.parent() else {
            return;
        };
        let tasks: Vec<_> = self.tasks.lock().values().cloned().collect();
        for task in tasks {
            let cadence = task.lock().cadence();
            let mut cadence = cadence.lock();
            if cadence.id() == parent {
                cadence.release_child_budget(child.budget().ticks());
                return;
            }
        }
    }

    /// 以某个 Task 的契约为父，派生收窄子契约；用于恢复时重准入。
    pub fn admit_task_cadence(
        &self,
        task: TaskId,
        weight: u32,
        budget: u64,
    ) -> Result<Cadence, KernelError> {
        let task = self.task(task).ok_or(KernelError::UnknownTask(task))?;
        let cadence = task.lock().cadence();
        cadence
            .lock()
            .split(
                CadenceId::next(),
                Weight::new(weight)?,
                Duration::from_ticks(budget),
            )
            .map_err(KernelError::from)
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

    /// 撤销 Cadence；返回新 epoch，使旧句柄失效，并挂起使用该契约的 Task。
    pub fn revoke_cadence(&self, thread: TcbId, handle: Handle) -> Result<u32, KernelError> {
        let (object, _) = self.resolve(thread, handle, Rights::ADMIN, ObjectKind::Cadence)?;
        let cadence = object.cadence().cloned().ok_or(KernelError::WrongKind {
            expected: ObjectKind::Cadence,
        })?;
        // 对象头 epoch 与契约 epoch 同源：撤销后所有既有句柄在解析时失效。
        let epoch = object.header().revoke_all();
        cadence.lock().set_epoch(epoch);
        self.suspend_using(&cadence);
        Ok(epoch)
    }

    /// 按任务标识附加 Cadence，并把它重新入队。
    pub fn attach_cadence_to_task(
        &self,
        thread: TcbId,
        task: TaskId,
        cadence_handle: Handle,
    ) -> Result<(), KernelError> {
        let cadence = self.cadence_object(thread, cadence_handle, Rights::READ)?;
        let target = self.task(task).ok_or(KernelError::UnknownTask(task))?;
        let was_suspended = target.lock().state() == TaskState::Suspended;
        let previous = target.lock().cadence();
        if !Arc::ptr_eq(&previous, &cadence) {
            self.release_child_budget(&previous.lock());
        }
        {
            let mut target = target.lock();
            target.set_cadence(cadence, self.now());
        }
        self.remove_disallowed_home(task);
        if was_suspended {
            self.resume_task(task);
        } else {
            self.enqueue_next_thread(task, None);
        }
        Ok(())
    }

    /// 撤销 Task 当前 Cadence 的 epoch，再附加新契约；返回旧 epoch。
    pub fn regrant_cadence_to_task(
        &self,
        thread: TcbId,
        task: TaskId,
        cadence_handle: Handle,
    ) -> Result<u32, KernelError> {
        let cadence = self.cadence_object(thread, cadence_handle, Rights::READ)?;
        let target = self.task(task).ok_or(KernelError::UnknownTask(task))?;
        let was_suspended = target.lock().state() == TaskState::Suspended;
        let previous = { target.lock().cadence() };
        let epoch = previous.lock().revoke();
        self.release_child_budget(&previous.lock());
        {
            let mut target = target.lock();
            target.set_cadence(cadence, self.now());
        }
        self.remove_disallowed_home(task);
        if was_suspended {
            self.resume_task(task);
        } else {
            self.enqueue_next_thread(task, None);
        }
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
        let was_suspended = task.lock().state() == TaskState::Suspended;
        let previous = task.lock().cadence();
        if !Arc::ptr_eq(&previous, &cadence) {
            self.release_child_budget(&previous.lock());
        }
        let task_id = {
            let mut task = task.lock();
            task.set_cadence(Arc::clone(&cadence), self.now());
            task.id()
        };
        self.remove_disallowed_home(task_id);
        if was_suspended {
            self.resume_task(task_id);
        } else {
            self.enqueue_next_thread(task_id, None);
        }
        Ok(())
    }

    fn suspend_using(&self, cadence: &Arc<spin::Mutex<Cadence>>) {
        let tasks: Vec<_> = self.tasks.lock().values().cloned().collect();
        for task in tasks {
            let (matches, threads) = {
                let task = task.lock();
                (
                    Arc::ptr_eq(&task.cadence(), cadence),
                    task.threads().to_vec(),
                )
            };
            if !matches {
                continue;
            }
            {
                let mut task = task.lock();
                task.set_state(TaskState::Suspended);
                task.set_active(None);
                task.clear_queued();
            }
            {
                let mut thread_map = self.threads.lock();
                for thread in &threads {
                    if let Some(tcb) = thread_map.get_mut(thread)
                        && tcb.state() == ThreadState::Running
                    {
                        tcb.set_state(ThreadState::Runnable);
                    }
                }
            }
            let mut scheduler = self.scheduler.lock();
            for thread in threads {
                scheduler.remove(thread);
                scheduler.clear_current_if(thread);
            }
        }
    }

    /// 重新选择一个 Grant；阻塞中的 TCB 保持原状态。
    fn resume_task(&self, task: TaskId) {
        let Some(task_ref) = self.task(task) else {
            return;
        };
        {
            let mut task = task_ref.lock();
            task.set_active(None);
            task.clear_queued();
            if task.state() == TaskState::Suspended {
                task.set_state(TaskState::Eligible);
            }
        }
        self.enqueue_next_thread(task, None);
    }
}
