//! Task 退出、端点回收与子契约提升。
//!
//! 与 `task.rs`（创建与契约计算）、`switch.rs`（入队与派发）分开，退出路径
//! 涉及句柄、端点、借贷与父预算，单独成文更容易审计。

use alloc::vec::Vec;
use hk_handle::ObjectId;
use hk_ipc::Message;

use crate::error::KernelError;
use crate::kernel::{Kernel, TRACE_EXIT, TRACE_HANDLE_RELEASE, TRACE_PROMOTE, TRACE_REPLY};
use crate::task::{TaskId, TaskState};
use crate::tcb::TcbId;
use crate::trace::Category;

impl Kernel {
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
                self.release_child_budget(&cadence);
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
            // 解除父链前先归还父契约的预留；父 Task 此刻仍在任务表中。
            self.release_child_budget(&cadence);
            cadence.lock().promote();
            if let Some(thread) = thread {
                self.record(thread.get(), Category::Sched, TRACE_PROMOTE, parent.get());
            }
        }
    }
}
