//! 调度切换：入队、派发、阻塞唤醒与预算补充。

use hk_cadence::CadenceClass;

use crate::error::KernelError;
use crate::kernel::{
    Kernel, TRACE_BLOCK, TRACE_DEQUEUE, TRACE_DISPATCH, TRACE_ENQUEUE, TRACE_MIGRATE,
    TRACE_OVERRUN, TRACE_PREEMPT, TRACE_REFILL, TRACE_THROTTLE, TRACE_WAKE,
};
use crate::task::{TaskId, TaskState};
use crate::tcb::{BlockReason, Tcb, TcbId, ThreadState};
use crate::trace::Category;

use super::task::lending_contract;

impl Kernel {
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
                    // 硬预算：Grant 不得越过本周期剩余预算，否则低权重任务会在
                    // 节流前多跑一个 slice，破坏 Tempo 的 EDF 准入结论。
                    let slice = if task.has_budget() {
                        slice.min(task.budget_left()).max(1)
                    } else {
                        slice
                    };
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
            // 亲和集合内没有在线 CPU：Task 保持 Eligible 但尚未入队，
            // 等待时间从这一刻开始累计，亲和变化后由 dispatch 计入 Meter。
            if let Some(task) = self.task_of(thread).and_then(|id| self.task(id)) {
                task.lock().mark_queued(now);
            }
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

    /// 返回 `cpu` 下一个需要唤醒的调度时刻：全局预算补充或本地 Grant 结束。
    pub fn next_deadline(&self, cpu: usize) -> Option<u64> {
        let now = self.now();
        let mut earliest = self.refills.lock().keys().next().copied();
        if let Some(deadline) = self.scheduler.lock().next_realtime_deadline(cpu)
            && deadline > now
        {
            earliest = Some(earliest.map_or(deadline, |value: u64| value.min(deadline)));
        }
        let current = self.scheduler.lock().current(cpu);
        if let Some(thread) = current
            && let Some(task_id) = self.task_of(thread)
            && let Some(task) = self.task(task_id)
        {
            let task = task.lock();
            let end = task.grant_end();
            if end > 0 {
                earliest = Some(earliest.map_or(end, |value: u64| value.min(end)));
            }
            // 当前正在运行的 Realtime Task 也要把绝对截止期纳入一次性定时器。
            if task.class() == CadenceClass::Realtime {
                let deadline = task.queue_key();
                if deadline > now {
                    earliest = Some(earliest.map_or(deadline, |value: u64| value.min(deadline)));
                }
            }
        }
        earliest
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
                // 阻塞期间不再等待 CPU；唤醒时重新记录等待起点。
                task.clear_queued();
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
