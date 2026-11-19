//! Task 创建、契约计算与有效契约选择。

use alloc::sync::Arc;
use core::cmp::Ordering as CmpOrdering;
use core::sync::atomic::Ordering;

use hk_base::Duration;
use hk_cadence::{Cadence, CadenceClass, CadenceId, CadenceSpec, Meter, Weight};
use hk_handle::HandleTable;

use crate::error::KernelError;
use crate::kernel::{Kernel, TRACE_THREAD_SPAWN};
use crate::task::{Task, TaskId};
use crate::tcb::{Tcb, TcbId, ThreadState};
use crate::trace::Category;

/// 取服务端契约与调用方借贷 frame 中更强的一个。
///
/// 比较顺序为调度类别、队列键：类别更小、键更小者更强；两者都相同时取较短
/// 时间片与较小延迟目标，保证抢占与超限响应性。返回 `(类别, 键, 时间片, 延迟)`。
pub(super) fn lending_contract(
    server_class: CadenceClass,
    server_key: u64,
    server_slice: u64,
    server_latency: u64,
    frame: Option<crate::tcb::LendingFrame>,
) -> (CadenceClass, u64, u64, u64) {
    let Some(frame) = frame else {
        return (server_class, server_key, server_slice, server_latency);
    };
    match (frame.class.as_u8(), frame.key).cmp(&(server_class.as_u8(), server_key)) {
        CmpOrdering::Less => (frame.class, frame.key, frame.slice, frame.latency),
        CmpOrdering::Greater => (server_class, server_key, server_slice, server_latency),
        CmpOrdering::Equal => (
            server_class,
            server_key,
            frame.slice.min(server_slice),
            frame.latency.min(server_latency),
        ),
    }
}

impl Kernel {
    /// 创建 Task 与它的主 TCB，并立即激活。
    pub fn spawn_task(
        &self,
        handles: HandleTable,
        spec: CadenceSpec,
    ) -> Result<TcbId, KernelError> {
        let thread = self.spawn_task_parked(handles, spec)?;
        self.activate(thread)?;
        Ok(thread)
    }

    /// 创建 Task 与它的主 TCB，但不进入调度队列。
    pub fn spawn_task_parked(
        &self,
        handles: HandleTable,
        spec: CadenceSpec,
    ) -> Result<TcbId, KernelError> {
        self.spawn_task_parked_with_parent(handles, spec, None)
    }

    /// 创建 Task 与主 TCB 并恢复内核父链；服务装载与检查点恢复使用。
    pub fn spawn_task_parked_with_parent(
        &self,
        handles: HandleTable,
        spec: CadenceSpec,
        parent: Option<TaskId>,
    ) -> Result<TcbId, KernelError> {
        let id = TaskId::new(self.next_task.fetch_add(1, Ordering::Relaxed));
        let cadence = Cadence::from_spec(CadenceId::new(id.get()), spec)?;
        let task = Arc::new(spin::Mutex::new(Task::new(
            id,
            parent,
            cadence,
            Meter::new(),
        )));
        // 契约从创建时刻开始预算周期，Realtime 绝对截止期不会落到过去。
        task.lock().reset_period(self.now());
        let thread = self.allocate_thread(id, handles);
        task.lock().attach(thread);
        if let Err(error) = self.register_task(id, task, true, false) {
            self.threads.lock().remove(&thread);
            return Err(error);
        }
        Ok(thread)
    }

    /// 以父 Task 的契约创建收窄子 Task，但不进入调度队列。
    pub fn spawn_task_child_parked(
        &self,
        handles: HandleTable,
        parent: TaskId,
    ) -> Result<TcbId, KernelError> {
        let parent_task = self.task(parent).ok_or(KernelError::UnknownTask(parent))?;
        let (weight, budget) = {
            let cadence = parent_task.lock().cadence();
            let cadence = cadence.lock();
            (
                (cadence.weight().get() / 2).max(1),
                cadence.budget().ticks() / 2,
            )
        };
        let child = parent_task.lock().cadence().lock().split(
            CadenceId::next(),
            Weight::new(weight)?,
            Duration::from_ticks(budget),
        )?;
        let id = TaskId::new(self.next_task.fetch_add(1, Ordering::Relaxed));
        let task = Arc::new(spin::Mutex::new(Task::new(
            id,
            Some(parent),
            child,
            Meter::new(),
        )));
        task.lock().reset_period(self.now());
        let thread = self.allocate_thread(id, handles);
        task.lock().attach(thread);
        if let Err(error) = self.register_task(id, task, false, false) {
            self.threads.lock().remove(&thread);
            return Err(error);
        }
        Ok(thread)
    }

    /// 以现成 Cadence 创建 Task；checkpoint 恢复使用。
    pub fn spawn_task_with_cadence(
        &self,
        handles: HandleTable,
        cadence: Cadence,
    ) -> Result<TcbId, KernelError> {
        self.spawn_task_with_cadence_and_parent(handles, cadence, None)
    }

    /// 以现成 Cadence 创建 Task 并恢复内核父链；checkpoint 恢复使用。
    pub fn spawn_task_with_cadence_and_parent(
        &self,
        handles: HandleTable,
        cadence: Cadence,
        parent: Option<TaskId>,
    ) -> Result<TcbId, KernelError> {
        let id = TaskId::new(self.next_task.fetch_add(1, Ordering::Relaxed));
        let task = Arc::new(spin::Mutex::new(Task::new(
            id,
            parent,
            cadence,
            Meter::new(),
        )));
        task.lock().reset_period(self.now());
        let thread = self.allocate_thread(id, handles);
        task.lock().attach(thread);
        if let Err(error) = self.register_task(id, task, true, true) {
            self.threads.lock().remove(&thread);
            return Err(error);
        }
        Ok(thread)
    }

    /// 从恢复时刻重新开始预算周期。
    pub fn reset_budget_period(&self, task: TaskId, now: u64) {
        if let Some(task) = self.task(task) {
            task.lock().reset_period(now);
        }
    }

    /// 在已有 Task 内创建 TCB，并立即激活。
    pub fn spawn_thread(&self, task: TaskId) -> Result<TcbId, KernelError> {
        let thread = self.spawn_thread_parked(task)?;
        self.activate(thread)?;
        Ok(thread)
    }

    /// 在已有 Task 内创建 TCB，但不进入调度队列。
    ///
    /// 新 TCB 继承该 Task 已有 TCB 的句柄表；同一 Task 的线程共享句柄。
    pub fn spawn_thread_parked(&self, task: TaskId) -> Result<TcbId, KernelError> {
        let task_ref = self.task(task).ok_or(KernelError::UnknownTask(task))?;
        let first = task_ref
            .lock()
            .threads()
            .first()
            .copied()
            .ok_or(KernelError::UnknownTask(task))?;
        let table = self
            .threads
            .lock()
            .get(&first)
            .map(Tcb::handles)
            .ok_or(KernelError::UnknownThread(first))?;
        let thread = self.allocate_thread_with(task, table);
        task_ref.lock().attach(thread);
        Ok(thread)
    }

    fn allocate_thread(&self, task: TaskId, handles: HandleTable) -> TcbId {
        let id = TcbId::new(self.next_tcb.fetch_add(1, Ordering::Relaxed));
        let mut tcb = Tcb::new(id, task, handles);
        tcb.set_state(ThreadState::Inactive);
        self.threads.lock().insert(id, tcb);
        self.record(id.get(), Category::Sched, TRACE_THREAD_SPAWN, 0);
        id
    }

    fn allocate_thread_with(&self, task: TaskId, table: Arc<spin::Mutex<HandleTable>>) -> TcbId {
        let id = TcbId::new(self.next_tcb.fetch_add(1, Ordering::Relaxed));
        let mut tcb = Tcb::new_shared(id, task, table);
        tcb.set_state(ThreadState::Inactive);
        self.threads.lock().insert(id, tcb);
        self.record(id.get(), Category::Sched, TRACE_THREAD_SPAWN, 0);
        id
    }

    /// 返回 TCB 的有效契约 `(类别, 键, 时间片, 延迟)`。
    ///
    /// 借贷 frame 更强时整体采用调用方的值；两者同级时取较短时间片与较小延迟。
    pub(crate) fn effective_contract(
        &self,
        thread: TcbId,
    ) -> Option<(CadenceClass, u64, u64, u64)> {
        let lending = self
            .threads
            .lock()
            .get(&thread)
            .and_then(Tcb::strongest_lending);
        let task = self.task(self.task_of(thread)?)?;
        let (class, key, slice, latency) = {
            let task = task.lock();
            (
                task.class(),
                task.queue_key(),
                task.slice_ticks(),
                task.latency_ticks(),
            )
        };
        Some(lending_contract(class, key, slice, latency, lending))
    }

    /// 返回 TCB 的有效入队类别与键。
    pub(crate) fn queue_entry(&self, thread: TcbId) -> Option<(CadenceClass, u64)> {
        self.effective_contract(thread)
            .map(|(class, key, _, _)| (class, key))
    }

    /// 返回 TCB 的有效 Grant 时间片；借贷时取更强契约的时间片。测试辅助。
    #[cfg(test)]
    pub(crate) fn slice_for_thread(&self, thread: TcbId) -> Option<u64> {
        self.effective_contract(thread)
            .map(|(_, _, slice, _)| slice)
    }
}
