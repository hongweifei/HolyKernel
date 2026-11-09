//! Task 生命周期与 Beat 调度决策。

use hk_cadence::{Cadence, CadenceClass, CadenceId, CadenceSpec, Meter};
use hk_hal::MAX_CPUS;

use super::*;

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
        let id = TaskId::new(self.next_task.fetch_add(1, Ordering::Relaxed));
        let cadence = Cadence::from_spec(CadenceId::new(id.get()), spec)?;
        let task = Arc::new(spin::Mutex::new(Task::new(id, None, cadence, Meter::new())));
        let thread = self.allocate_thread(id, handles);
        task.lock().attach(thread);
        self.tasks.lock().insert(id, task);
        Ok(thread)
    }

    /// 在已有 Task 内创建 TCB，并立即激活。
    pub fn spawn_thread(&self, task: TaskId, handles: HandleTable) -> Result<TcbId, KernelError> {
        let thread = self.spawn_thread_parked(task, handles)?;
        self.activate(thread)?;
        Ok(thread)
    }

    /// 在已有 Task 内创建 TCB，但不进入调度队列。
    pub fn spawn_thread_parked(
        &self,
        task: TaskId,
        handles: HandleTable,
    ) -> Result<TcbId, KernelError> {
        let task_ref = self.task(task).ok_or(KernelError::UnknownTask(task))?;
        let thread = self.allocate_thread(task, handles);
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
        if !self.task_has_runnable(task_id, thread)
            && let Some(task) = self.task(task_id)
        {
            task.lock().set_state(TaskState::Suspended);
        }
        Ok(())
    }

    /// 判断是否有排队等待调度的 TCB。
    pub fn has_runnable(&self) -> bool {
        self.scheduler.lock().runnable() > 0
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
                let eligible = self.task(task_id).is_some_and(|task| {
                    let mut task = task.lock();
                    task.replenish(now);
                    if task.budget_exhausted() {
                        task.throttle();
                        false
                    } else {
                        if task.state() == TaskState::Running {
                            task.set_state(TaskState::Eligible);
                        }
                        true
                    }
                });
                if eligible {
                    self.enqueue_thread(current);
                }
            }
        }

        loop {
            let (next, _) = self
                .scheduler
                .lock()
                .pick_with(cpu, |thread| self.thread_cpu_allowed(thread, cpu))?;
            let task_id = self.threads.lock().get(&next).map(Tcb::task)?;
            let slice = self.slice_for_thread(next);
            let valid = self.task(task_id).is_some_and(|task| {
                let mut task = task.lock();
                task.replenish(now);
                if task.budget_exhausted() {
                    task.throttle();
                    false
                } else if matches!(task.state(), TaskState::Exited | TaskState::Suspended) {
                    false
                } else {
                    task.set_state(TaskState::Running);
                    task.set_grant_end(now.saturating_add(slice.unwrap_or(0)));
                    task.record_cpu(cpu);
                    true
                }
            });
            if !valid {
                continue;
            }
            {
                let mut threads = self.threads.lock();
                let Some(tcb) = threads.get_mut(&next) else {
                    continue;
                };
                tcb.set_state(ThreadState::Running);
            }
            self.record(next.get(), Category::Sched, TRACE_DISPATCH, 0);
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
            task.lock().charge_borrowed(elapsed);
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

    /// 按亲和性与负载把 TCB 入队到某个 CPU。
    pub(crate) fn enqueue_thread(&self, thread: TcbId) -> bool {
        let Some((class, key)) = self.queue_entry(thread) else {
            return false;
        };
        let Some(task) = self.task_of(thread).and_then(|id| self.task(id)) else {
            return false;
        };
        let (affinity, migratable, weight, last_cpu) = {
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
            (affinity, migratable, weight, task.last_cpu())
        };
        self.scheduler.lock().enqueue_best(
            thread,
            class,
            key,
            u64::from(weight),
            affinity,
            last_cpu,
            migratable,
        )
    }

    /// 返回 TCB 的入队类别与键；借贷 frame 更强时使用调用方的值。
    pub(crate) fn queue_entry(&self, thread: TcbId) -> Option<(CadenceClass, u64)> {
        let lending = self
            .threads
            .lock()
            .get(&thread)
            .and_then(Tcb::strongest_lending);
        let task = self.task(self.task_of(thread)?)?;
        let (class, key) = {
            let task = task.lock();
            (task.class(), task.queue_key())
        };
        match lending {
            Some(frame) if frame.class < class => Some((frame.class, frame.key)),
            _ => Some((class, key)),
        }
    }

    /// 返回 TCB 的 Grant 时间片；借贷时取调用方与服务端中较短的一个。
    fn slice_for_thread(&self, thread: TcbId) -> Option<u64> {
        let lending = self
            .threads
            .lock()
            .get(&thread)
            .and_then(Tcb::strongest_lending);
        let task = self.task(self.task_of(thread)?)?;
        let slice = task.lock().slice_ticks();
        Some(lending.map_or(slice, |frame| slice.min(frame.slice)))
    }

    /// 返回下一个需要唤醒的调度时刻：预算补充或当前 Grant 结束。
    pub fn next_deadline(&self) -> Option<u64> {
        let mut earliest = None;
        let tasks: Vec<_> = self.tasks.lock().values().cloned().collect();
        for task in tasks {
            if let Some(at) = task.lock().refill_at() {
                earliest = Some(earliest.map_or(at, |value: u64| value.min(at)));
            }
        }
        let currents: Vec<Option<TcbId>> = {
            let scheduler = self.scheduler.lock();
            (0..MAX_CPUS).map(|cpu| scheduler.current(cpu)).collect()
        };
        for thread in currents.into_iter().flatten() {
            let Some(task_id) = self.task_of(thread) else {
                continue;
            };
            let Some(task) = self.task(task_id) else {
                continue;
            };
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
        let tcb = self
            .threads
            .lock()
            .remove(&thread)
            .ok_or(KernelError::UnknownThread(thread))?;
        let live = tcb.handles().live();
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
            self.tasks.lock().remove(&task_id);
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
        if !self.task_has_runnable(task_id, thread)
            && let Some(task) = self.task(task_id)
        {
            task.lock().set_state(TaskState::Blocked);
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
            if matches!(
                task.state(),
                TaskState::Blocked | TaskState::Created | TaskState::Suspended
            ) {
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

    fn refill_throttled(&self, now: u64) {
        let tasks: Vec<_> = self.tasks.lock().values().cloned().collect();
        for task in tasks {
            let threads = {
                let mut task = task.lock();
                if task.state() != TaskState::Throttled || !task.replenish(now) {
                    continue;
                }
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
}
