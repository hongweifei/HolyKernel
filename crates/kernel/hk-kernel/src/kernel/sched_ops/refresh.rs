//! Cadence 变更后的调度 placement 刷新。
//!
//! Running Task 只重算当前 Grant 并请求重新调度；其它状态先移除旧队列条目，
//! 再按新契约重新入队。Throttled Task 获得新预算后立即回到 Eligible。

use hk_cadence::{CadenceClass, NO_CPU};

use crate::kernel::Kernel;
use crate::task::{TaskId, TaskState};

impl Kernel {
    /// 契约变更后刷新 Task 的调度 placement。
    pub(crate) fn refresh_task(&self, task_id: TaskId) {
        let Some(task_ref) = self.task(task_id) else {
            return;
        };
        let (state, threads) = {
            let task = task_ref.lock();
            (task.state(), task.threads().to_vec())
        };
        self.clear_refill(task_id);
        match state {
            TaskState::Running => {
                self.retune_running(task_id);
                return;
            }
            TaskState::Suspended | TaskState::Exited => return,
            TaskState::Created
            | TaskState::Eligible
            | TaskState::Blocked
            | TaskState::Throttled => {}
        }
        {
            let mut task = task_ref.lock();
            task.set_active(None);
            if matches!(task.state(), TaskState::Created | TaskState::Throttled) {
                task.set_state(TaskState::Eligible);
            }
            let cadence = task.cadence();
            let cadence = cadence.lock();
            if !cadence.migratable() {
                let last = task.last_cpu();
                if last != NO_CPU && !cadence.affinity().contains(last) {
                    task.clear_last_cpu();
                }
            }
        }
        {
            let mut scheduler = self.scheduler.lock();
            for thread in threads {
                scheduler.remove(thread);
                scheduler.clear_current_if(thread);
            }
        }
        self.enqueue_next_thread(task_id, None);
    }

    /// 重算 Running Task 的当前 Grant；契约可能来自新的 Cadence 或借贷 frame。
    pub(crate) fn retune_running(&self, task_id: TaskId) {
        let Some(task_ref) = self.task(task_id) else {
            return;
        };
        let (active, last_cpu) = {
            let task = task_ref.lock();
            (task.active(), task.last_cpu())
        };
        let Some(active) = active else {
            return;
        };
        let Some((class, key, slice, _)) = self.effective_contract(active) else {
            return;
        };
        let now = self.now();
        let mut task = task_ref.lock();
        if task.state() != TaskState::Running {
            return;
        }
        let slice = if task.has_budget() {
            slice.min(task.budget_left()).max(1)
        } else {
            slice.max(1)
        };
        let mut end = now.saturating_add(slice);
        if class == CadenceClass::Realtime && key > 0 {
            end = end.min(key.max(now.saturating_add(1)));
        }
        task.set_grant_end(end);
        let cadence = task.cadence();
        let cadence = cadence.lock();
        if !cadence.migratable() && last_cpu != NO_CPU && !cadence.affinity().contains(last_cpu) {
            task.clear_last_cpu();
        }
        drop(cadence);
        drop(task);
        if last_cpu != NO_CPU {
            self.request_resched(last_cpu as usize);
        }
    }

    /// 从全局补充队列移除 Task。
    pub(crate) fn clear_refill(&self, task_id: TaskId) {
        let mut refills = self.refills.lock();
        for entries in refills.values_mut() {
            entries.retain(|id| *id != task_id);
        }
        refills.retain(|_, entries| !entries.is_empty());
    }
}
