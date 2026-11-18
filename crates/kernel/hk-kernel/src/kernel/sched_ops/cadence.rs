//! Task 生命周期、契约计算与退出回收。

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cmp::Ordering as CmpOrdering;
use core::sync::atomic::Ordering;

use hk_base::Duration;
use hk_cadence::{Cadence, CadenceClass, CadenceId, CadenceSpec, Meter, Weight};
use hk_handle::{HandleTable, ObjectId};
use hk_ipc::Message;

use crate::error::KernelError;
use crate::kernel::{
    Kernel, TRACE_EXIT, TRACE_HANDLE_RELEASE, TRACE_PROMOTE, TRACE_REPLY, TRACE_THREAD_SPAWN,
};
use crate::task::{Task, TaskId, TaskState};
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
        self.tasks.lock().insert(id, task);
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
        self.tasks.lock().insert(id, task);
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
        self.tasks.lock().insert(id, task);
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
    fn effective_contract(&self, thread: TcbId) -> Option<(CadenceClass, u64, u64, u64)> {
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

    /// 终止 TCB；Task 的最后一个 TCB 退出时回收 Task。
    pub fn exit(&self, thread: TcbId) -> Result<(), KernelError> {
        self.release_lending(thread);
        self.release_transfer_charges(thread);
        self.clear_active_if(thread);
        let tcb = self
            .threads
            .lock()
            .remove(&thread)
            .ok_or(KernelError::UnknownThread(thread))?;
        let live = tcb.handles().lock().live();
        let task_id = tcb.task();
        let empty = if let Some(task) = self.task(task_id) {
            let mut task = task.lock();
            let empty = task.detach(thread);
            if empty {
                task.set_state(TaskState::Exited);
            }
            empty
        } else {
            false
        };
        drop(tcb);
        {
            let mut scheduler = self.scheduler.lock();
            scheduler.remove(thread);
            scheduler.clear_current_if(thread);
        }
        if empty {
            if let Some(task) = self.task(task_id) {
                let cadence = task.lock().cadence();
                self.release_child_budget(&cadence.lock());
            }
            // 父 Task 退出：把子 Task 的收窄契约提升为独立契约，子任务继续运行；
            // 显式“回收”仍由 Tempo 通过 revoke/regrant 完成。
            self.promote_children(task_id);
            let endpoints = self.take_endpoint_servers(task_id);
            self.fail_calls_to(&endpoints);
            self.tasks.lock().remove(&task_id);
        } else {
            if let Some(task) = self.task(task_id) {
                task.lock().clear_queued();
            }
            self.enqueue_next_thread(task_id, Some(thread));
        }
        self.record(thread.get(), Category::Sched, TRACE_EXIT, 0);
        self.record(
            thread.get(),
            Category::Handle,
            TRACE_HANDLE_RELEASE,
            live as u64,
        );
        Ok(())
    }

    /// 取出并移除绑定到指定 Task 的端点。
    fn take_endpoint_servers(&self, task: TaskId) -> Vec<ObjectId> {
        let mut servers = self.endpoint_servers.lock();
        let endpoints: Vec<ObjectId> = servers
            .iter()
            .filter(|(_, server)| **server == task)
            .map(|(endpoint, _)| *endpoint)
            .collect();
        servers.retain(|_, server| *server != task);
        endpoints
    }

    /// 服务退出时让所有等待这些端点的调用方立即收到错误回复。
    fn fail_calls_to(&self, endpoints: &[ObjectId]) {
        if endpoints.is_empty() {
            return;
        }
        let mut callers = Vec::new();
        {
            let mut threads = self.threads.lock();
            for (id, tcb) in threads.iter_mut() {
                let Some(pending) = tcb.pending_call_mut() else {
                    continue;
                };
                if !pending.answered && endpoints.contains(&pending.endpoint) {
                    pending.reply = Some(Message::new(u64::MAX));
                    pending.answered = true;
                    callers.push(*id);
                }
            }
        }
        for caller in callers {
            self.record(caller.get(), Category::Ipc, TRACE_REPLY, 0);
            self.wake(caller);
        }
    }

    /// 父 Task 退出时提升其子 Task：解除 Task 父链与 Cadence 父链。
    fn promote_children(&self, parent: TaskId) {
        let tasks: Vec<_> = self.tasks.lock().values().cloned().collect();
        for task in tasks {
            let (thread, cadence) = {
                let mut task = task.lock();
                if task.parent() != Some(parent) {
                    continue;
                }
                task.set_parent(None);
                (task.threads().first().copied(), task.cadence())
            };
            cadence.lock().promote();
            if let Some(thread) = thread {
                self.record(thread.get(), Category::Sched, TRACE_PROMOTE, parent.get());
            }
        }
    }
}
