//! Task 生命周期与 Beat 调度决策。

use core::cmp::Ordering as CmpOrdering;

use hk_base::Duration;
use hk_cadence::{Cadence, CadenceClass, CadenceId, CadenceSpec, Meter, Weight};

use super::*;

/// 取服务端契约与调用方借贷 frame 中更强的一个。
///
/// 比较顺序为调度类别、队列键：类别更小、键更小者更强；两者都相同时取较短
/// 时间片与较小延迟目标，保证抢占与超限响应性。返回 `(类别, 键, 时间片, 延迟)`。
fn lending_contract(
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

    /// 把停泊 TCB 置为可运行并按所属 Task 的契约入队。
    pub fn activate(&self, thread: TcbId) -> Result<(), KernelError> {
        let task_id = {
            let mut threads = self.threads.lock();
            let tcb = threads
                .get_mut(&thread)
                .ok_or(KernelError::UnknownThread(thread))?;
            if tcb.state() != ThreadState::Inactive {
                return Ok(());
            }
            tcb.set_state(ThreadState::Runnable);
            tcb.task()
        };
        let Some(task) = self.task(task_id) else {
            return Err(KernelError::UnknownTask(task_id));
        };
        {
            let mut task = task.lock();
            let now = self.now();
            if matches!(
                task.state(),
                TaskState::Created | TaskState::Suspended | TaskState::Blocked
            ) {
                task.set_state(TaskState::Eligible);
            }
            task.replenish(now);
        }
        self.enqueue_thread(thread);
        Ok(())
    }

    /// 把 TCB 停泊并移出调度队列；Task 无其他可运行 TCB 时进入挂起。
    pub fn suspend(&self, thread: TcbId) -> Result<(), KernelError> {
        let task_id = {
            let mut threads = self.threads.lock();
            let tcb = threads
                .get_mut(&thread)
                .ok_or(KernelError::UnknownThread(thread))?;
            tcb.set_state(ThreadState::Inactive);
            tcb.task()
        };
        {
            let mut scheduler = self.scheduler.lock();
            scheduler.remove(thread);
            scheduler.clear_current_if(thread);
        }
        self.clear_active_if(thread);
        if let Some(task) = self.task(task_id) {
            task.lock().clear_queued();
        }
        if self.task_has_runnable(task_id, thread) {
            if let Some(task) = self.task(task_id) {
                let mut task = task.lock();
                if task.state() == TaskState::Running {
                    task.set_state(TaskState::Eligible);
                }
            }
            self.enqueue_next_thread(task_id, Some(thread));
        } else if let Some(task) = self.task(task_id) {
            task.lock().set_state(TaskState::Suspended);
        }
        Ok(())
    }

    /// 判断是否有排队等待调度的 TCB。
    pub fn has_runnable(&self) -> bool {
        self.scheduler.lock().runnable() > 0
    }

    /// 判断是否有等待预算补充的任务。
    pub fn pending_refills(&self) -> bool {
        !self.refills.lock().is_empty()
    }

    /// 取 `cpu` 的下一个 TCB；当前 TCB 按 Task 虚拟时间重新入队。
    pub fn schedule(&self, cpu: usize) -> Option<TcbId> {
        self.ensure_idle(cpu);
        let now = self.now();
        self.refill_throttled(now);

        let current = { self.scheduler.lock().take_current(cpu) };
        if let Some(current) = current {
            let task_id = {
                let mut threads = self.threads.lock();
                match threads.get_mut(&current) {
                    Some(tcb) if tcb.state() == ThreadState::Running => {
                        tcb.set_state(ThreadState::Runnable);
                        Some(tcb.task())
                    }
                    _ => None,
                }
            };
            if let Some(task_id) = task_id {
                if !self.is_idle(current) {
                    self.record(current.get(), Category::Sched, TRACE_PREEMPT, 0);
                }
                let (eligible, throttled) = self.task(task_id).map_or((false, false), |task| {
                    let mut task = task.lock();
                    task.replenish(now);
                    if task.budget_exhausted() {
                        task.throttle();
                        (false, true)
                    } else {
                        if task.state() == TaskState::Running {
                            task.set_state(TaskState::Eligible);
                        }
                        (true, false)
                    }
                });
                if eligible {
                    self.enqueue_next_thread(task_id, Some(current));
                } else {
                    self.clear_active_if(current);
                    if throttled {
                        if let Some(task) = self.task(task_id) {
                            task.lock().clear_queued();
                        }
                        self.schedule_refill(task_id);
                        self.record(current.get(), Category::Sched, TRACE_THROTTLE, 0);
                    }
                }
            }
        }

        loop {
            let (next, _) = self
                .scheduler
                .lock()
                .pick_with(cpu, |thread| self.thread_cpu_allowed(thread, cpu))?;
            let task_id = self.threads.lock().get(&next).map(Tcb::task)?;
            let idle = self.is_idle(next);
            let lending = self
                .threads
                .lock()
                .get(&next)
                .and_then(Tcb::strongest_lending);
            let mut throttled = false;
            let mut overrun = false;
            let mut migrated = false;
            let valid = self.task(task_id).is_some_and(|task| {
                let mut task = task.lock();
                task.replenish(now);
                if task.budget_exhausted() {
                    task.throttle();
                    throttled = true;
                    false
                } else if matches!(task.state(), TaskState::Exited | TaskState::Suspended)
                    || task.active().is_some_and(|active| active != next)
                {
                    false
                } else {
                    let (class, key, slice, latency) = lending_contract(
                        task.class(),
                        task.queue_key(),
                        task.slice_ticks(),
                        task.latency_ticks(),
                        lending,
                    );
                    if !idle {
                        let (_, late) = task.dispatch_with(now, class, key, latency);
                        overrun = late;
                        migrated = task.record_cpu(cpu);
                    }
                    task.set_active(Some(next));
                    task.set_state(TaskState::Running);
                    task.set_grant_end(now.saturating_add(slice));
                    true
                }
            });
            if !valid {
                self.clear_active_if(next);
                if throttled {
                    if let Some(task) = self.task(task_id) {
                        task.lock().clear_queued();
                    }
                    self.schedule_refill(task_id);
                    if !idle {
                        self.record(next.get(), Category::Sched, TRACE_THROTTLE, 0);
                    }
                }
                continue;
            }
            if !idle {
                self.record(next.get(), Category::Sched, TRACE_DEQUEUE, 0);
                if overrun {
                    self.record(next.get(), Category::Sched, TRACE_OVERRUN, 0);
                }
                if migrated {
                    self.record(next.get(), Category::Sched, TRACE_MIGRATE, cpu as u64);
                }
            }
            {
                let mut threads = self.threads.lock();
                let Some(tcb) = threads.get_mut(&next) else {
                    continue;
                };
                tcb.set_state(ThreadState::Running);
            }
            if !idle {
                self.record(next.get(), Category::Sched, TRACE_DISPATCH, 0);
            }
            return Some(next);
        }
    }

    /// 记入 TCB 的实际执行时间；借贷期间同时记入调用方 Meter 与预算。
    pub fn charge(&self, thread: TcbId, elapsed: u64) {
        let caller = self
            .threads
            .lock()
            .get(&thread)
            .and_then(Tcb::strongest_lending)
            .map(|frame| frame.caller);
        if let Some(caller) = caller
            && let Some(task) = self.task(caller)
        {
            let throttled = {
                let mut task = task.lock();
                task.charge_borrowed(elapsed);
                task.state() == TaskState::Throttled
            };
            if throttled {
                self.schedule_refill(caller);
                if let Some(caller_thread) = self
                    .task(caller)
                    .and_then(|task| task.lock().threads().first().copied())
                {
                    self.record(caller_thread.get(), Category::Sched, TRACE_THROTTLE, 0);
                }
            }
        }
        let Some(task_id) = self.task_of(thread) else {
            return;
        };
        if let Some(task) = self.task(task_id) {
            task.lock().charge(elapsed);
        }
    }

    /// 判断 TCB 是否允许在 `cpu` 上运行。
    fn thread_cpu_allowed(&self, thread: TcbId, cpu: usize) -> bool {
        let Some(task_id) = self.task_of(thread) else {
            return false;
        };
        let Some(task) = self.task(task_id) else {
            return false;
        };
        task.lock().cpu_allowed(cpu)
    }

    /// 为 Task 保留唯一 Grant；已有 active TCB 时不再入队。
    fn reserve_thread(&self, thread: TcbId) -> bool {
        let Some(task) = self.task_of(thread).and_then(|id| self.task(id)) else {
            return false;
        };
        let mut task = task.lock();
        if matches!(
            task.state(),
            TaskState::Throttled | TaskState::Suspended | TaskState::Exited
        ) {
            return false;
        }
        match task.active() {
            None => {
                task.set_active(Some(thread));
                true
            }
            Some(active) => active == thread,
        }
    }

    /// 清除 TCB 的 active 标记。
    pub(crate) fn clear_active_if(&self, thread: TcbId) {
        if let Some(task) = self.task_of(thread).and_then(|id| self.task(id)) {
            let mut task = task.lock();
            if task.active() == Some(thread) {
                task.set_active(None);
            }
        }
    }

    /// 选择 Task 内下一个可运行 TCB 入队；`exclude` 优先轮转到它之后。
    pub(crate) fn enqueue_next_thread(&self, task_id: TaskId, exclude: Option<TcbId>) -> bool {
        let Some(task_ref) = self.task(task_id) else {
            return false;
        };
        let (threads, state) = {
            let task = task_ref.lock();
            (task.threads().to_vec(), task.state())
        };
        if matches!(
            state,
            TaskState::Throttled | TaskState::Suspended | TaskState::Exited
        ) {
            task_ref.lock().set_active(None);
            return false;
        }
        let chosen = {
            let thread_map = self.threads.lock();
            let count = threads.len();
            let start = exclude
                .and_then(|id| threads.iter().position(|thread| *thread == id))
                .map_or(0, |index| index + 1);
            let mut fallback = None;
            let mut chosen = None;
            for offset in 0..count {
                let id = threads[(start + offset) % count];
                if !thread_map
                    .get(&id)
                    .is_some_and(|tcb| tcb.state() == ThreadState::Runnable)
                {
                    continue;
                }
                if Some(id) == exclude {
                    fallback = Some(id);
                } else {
                    chosen = Some(id);
                    break;
                }
            }
            chosen.or(fallback)
        };
        match chosen {
            Some(thread) => {
                {
                    let mut task = task_ref.lock();
                    task.set_active(Some(thread));
                    if matches!(task.state(), TaskState::Blocked | TaskState::Created) {
                        task.set_state(TaskState::Eligible);
                    }
                }
                self.enqueue_thread(thread)
            }
            None => {
                task_ref.lock().set_active(None);
                false
            }
        }
    }

    /// 按亲和性与负载把 TCB 入队到某个 CPU。
    pub(crate) fn enqueue_thread(&self, thread: TcbId) -> bool {
        if !self.reserve_thread(thread) {
            return false;
        }
        let Some((class, key)) = self.queue_entry(thread) else {
            self.clear_active_if(thread);
            return false;
        };
        let Some(task) = self.task_of(thread).and_then(|id| self.task(id)) else {
            self.clear_active_if(thread);
            return false;
        };
        let (affinity, migratable, weight, last_cpu, heat) = {
            let task = task.lock();
            let (affinity, migratable, weight) = {
                let cadence = task.cadence();
                let cadence = cadence.lock();
                (
                    cadence.affinity().bits(),
                    cadence.migratable(),
                    cadence.weight().get(),
                )
            };
            (
                affinity,
                migratable,
                weight,
                task.last_cpu(),
                task.cache_heat(),
            )
        };
        let now = self.now();
        // 硬亲和：亲和集合内没有在线 CPU 时不入队，任务保持 Eligible 等待亲和变化。
        let Some(cpu) = self
            .scheduler
            .lock()
            .choose_cpu(affinity, last_cpu, migratable, heat)
        else {
            self.clear_active_if(thread);
            return false;
        };
        let enqueued =
            self.scheduler
                .lock()
                .enqueue(thread, class, key, cpu, u64::from(weight), migratable);
        if enqueued {
            if let Some(task) = self.task_of(thread).and_then(|id| self.task(id)) {
                task.lock().mark_queued(now);
            }
            self.record(
                thread.get(),
                Category::Sched,
                TRACE_ENQUEUE,
                u64::from(class.as_u8()),
            );
            self.request_resched_if_needed(thread, class, key, cpu);
        }
        enqueued
    }

    /// 契约亲和性变更后，把已排队但 CPU 不再允许的线程移出队列。
    pub(crate) fn remove_disallowed_home(&self, task_id: TaskId) {
        let Some(task) = self.task(task_id) else {
            return;
        };
        let threads = task.lock().threads().to_vec();
        for thread in threads {
            let Some(task) = self.task_of(thread).and_then(|id| self.task(id)) else {
                continue;
            };
            let affinity = task.lock().affinity_bits();
            self.scheduler.lock().remove_if_disallowed(thread, affinity);
        }
    }

    /// 目标 CPU 正在运行的 Task 更弱时，请求该 CPU 立即重新调度。
    fn request_resched_if_needed(&self, thread: TcbId, class: CadenceClass, key: u64, cpu: usize) {
        let Some(current) = self.scheduler.lock().current(cpu) else {
            return;
        };
        if current == thread {
            return;
        }
        // 比较运行中 TCB 的有效契约（含借贷 frame），而非其 Task 自身契约。
        let Some((current_class, current_key)) = self.queue_entry(current) else {
            return;
        };
        if (class.as_u8(), key) < (current_class.as_u8(), current_key) {
            self.request_resched(cpu);
        }
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

    /// 返回 `cpu` 下一个需要唤醒的调度时刻：全局预算补充或本地 Grant 结束。
    pub fn next_deadline(&self, cpu: usize) -> Option<u64> {
        let mut earliest = self.refills.lock().keys().next().copied();
        let current = self.scheduler.lock().current(cpu);
        if let Some(thread) = current
            && let Some(task_id) = self.task_of(thread)
            && let Some(task) = self.task(task_id)
        {
            let end = task.lock().grant_end();
            if end > 0 {
                earliest = Some(earliest.map_or(end, |value: u64| value.min(end)));
            }
        }
        earliest
    }

    /// 终止 TCB；Task 的最后一个 TCB 退出时回收 Task。
    pub fn exit(&self, thread: TcbId) -> Result<(), KernelError> {
        self.release_lending(thread);
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

    /// 唤醒被阻塞的 TCB。
    pub fn wake_thread(&self, thread: TcbId) {
        self.wake(thread);
    }

    /// 挂起 TCB，等待用户态运行库唤醒。
    pub fn park(&self, thread: TcbId) -> Result<(), KernelError> {
        self.block(thread, BlockReason::Park)
    }

    pub(crate) fn block(&self, thread: TcbId, reason: BlockReason) -> Result<(), KernelError> {
        let task_id = {
            let mut threads = self.threads.lock();
            let tcb = threads
                .get_mut(&thread)
                .ok_or(KernelError::UnknownThread(thread))?;
            tcb.set_state(ThreadState::Blocked);
            tcb.set_blocked(Some(reason));
            tcb.task()
        };
        {
            let mut scheduler = self.scheduler.lock();
            scheduler.remove(thread);
            scheduler.clear_current_if(thread);
        }
        self.clear_active_if(thread);
        if self.task_has_runnable(task_id, thread) {
            if let Some(task) = self.task(task_id) {
                let mut task = task.lock();
                if matches!(
                    task.state(),
                    TaskState::Running | TaskState::Created | TaskState::Blocked
                ) {
                    task.set_state(TaskState::Eligible);
                }
            }
            self.enqueue_next_thread(task_id, Some(thread));
        } else if let Some(task) = self.task(task_id) {
            let mut task = task.lock();
            if !matches!(
                task.state(),
                TaskState::Throttled | TaskState::Suspended | TaskState::Exited
            ) {
                task.set_state(TaskState::Blocked);
            }
            task.set_active(None);
        }
        self.record(thread.get(), Category::Sched, TRACE_BLOCK, 0);
        Ok(())
    }

    pub(crate) fn wake(&self, thread: TcbId) {
        let task_id = {
            let mut threads = self.threads.lock();
            let Some(tcb) = threads.get_mut(&thread) else {
                return;
            };
            if tcb.state() != ThreadState::Blocked {
                return;
            }
            tcb.set_state(ThreadState::Runnable);
            tcb.set_blocked(None);
            tcb.task()
        };
        let Some(task) = self.task(task_id) else {
            return;
        };
        let throttled = {
            let mut task = task.lock();
            // Suspended 只能由 attach/resume 恢复；常规唤醒不得越过撤销。
            if matches!(task.state(), TaskState::Blocked | TaskState::Created) {
                task.set_state(TaskState::Eligible);
            }
            task.state() == TaskState::Throttled
        };
        if !throttled {
            self.enqueue_thread(thread);
        }
        self.record(thread.get(), Category::Sched, TRACE_WAKE, 0);
    }

    fn task_has_runnable(&self, task: TaskId, excluding: TcbId) -> bool {
        let Some(task) = self.task(task) else {
            return false;
        };
        let threads = task.lock().threads().to_vec();
        let thread_map = self.threads.lock();
        threads.iter().any(|id| {
            *id != excluding
                && thread_map
                    .get(id)
                    .is_some_and(|tcb| tcb.state() == ThreadState::Runnable)
        })
    }

    /// 把节流任务放入全局补充队列。
    fn schedule_refill(&self, task: TaskId) {
        let Some(task_ref) = self.task(task) else {
            return;
        };
        let refill = task_ref.lock().refill_at();
        if let Some(at) = refill {
            let mut refills = self.refills.lock();
            let entries = refills.entry(at).or_default();
            if !entries.contains(&task) {
                entries.push(task);
            }
        }
    }

    fn refill_throttled(&self, now: u64) {
        loop {
            let Some((&deadline, _)) = self.refills.lock().keys().next().map(|key| (key, ()))
            else {
                break;
            };
            if deadline > now {
                break;
            }
            let tasks = {
                let mut refills = self.refills.lock();
                refills.remove(&deadline).unwrap_or_default()
            };
            for task_id in tasks {
                let Some(task_ref) = self.task(task_id) else {
                    continue;
                };
                let (thread, refill) = {
                    let mut task = task_ref.lock();
                    if task.state() != TaskState::Throttled {
                        continue;
                    }
                    if !task.replenish(now) {
                        (None, task.refill_at())
                    } else {
                        (task.threads().first().copied(), None)
                    }
                };
                if let Some(at) = refill {
                    let mut refills = self.refills.lock();
                    let entries = refills.entry(at).or_default();
                    if !entries.contains(&task_id) {
                        entries.push(task_id);
                    }
                    continue;
                }
                if let Some(thread) = thread {
                    self.record(thread.get(), Category::Sched, TRACE_REFILL, task_id.get());
                }
                self.enqueue_next_thread(task_id, None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskId;
    use crate::tcb::LendingFrame;

    fn frame(class: CadenceClass, key: u64, slice: u64, latency: u64) -> LendingFrame {
        LendingFrame {
            caller: TaskId::new(1),
            sequence: 1,
            class,
            key,
            slice,
            latency,
        }
    }

    #[test]
    fn uses_caller_contract_when_class_is_stronger() {
        let frame = frame(CadenceClass::Realtime, 8_000, 2_000, 1_000);
        assert_eq!(
            lending_contract(CadenceClass::Latency, 4_000, 8_000, 4_000, Some(frame)),
            (CadenceClass::Realtime, 8_000, 2_000, 1_000)
        );
    }

    #[test]
    fn uses_caller_key_within_same_class() {
        let frame = frame(CadenceClass::Latency, 1_000, 1_000, 500);
        assert_eq!(
            lending_contract(CadenceClass::Latency, 4_000, 8_000, 4_000, Some(frame)),
            (CadenceClass::Latency, 1_000, 1_000, 500)
        );
    }

    #[test]
    fn keeps_server_when_it_is_stronger() {
        let frame = frame(CadenceClass::Throughput, 0, 8_000, 0);
        assert_eq!(
            lending_contract(CadenceClass::Realtime, 0, 2_000, 1_000, Some(frame)),
            (CadenceClass::Realtime, 0, 2_000, 1_000)
        );
    }

    #[test]
    fn ties_take_shorter_slice_and_tighter_latency() {
        let frame = frame(CadenceClass::Latency, 5_000, 1_000, 500);
        assert_eq!(
            lending_contract(CadenceClass::Latency, 5_000, 2_000, 2_000, Some(frame)),
            (CadenceClass::Latency, 5_000, 1_000, 500)
        );
    }
}
