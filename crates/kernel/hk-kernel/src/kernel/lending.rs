//! caller-pays 借贷。
//!
//! 服务端 TCB 在收到带回复令牌的调用时压入 lending frame；调用期间类别、队列键
//! 与时间片取调用方与服务端中更强的一个，执行时间同时记入调用方 Meter 与预算。
//! `reply`、调用方退出或服务端退出时弹出/清理 frame。

use super::{Kernel, TRACE_LEND, TRACE_RETURN};
use crate::task::{TaskId, TaskState};
use crate::tcb::{LendingFrame, ReplyToken, Tcb, TcbId};
use crate::trace::Category;

impl Kernel {
    /// 服务端收到调用时压入 lending frame。
    pub(crate) fn lend_contract(&self, thread: TcbId, token: ReplyToken) {
        let Some(caller_task_id) = self.task_of(token.caller) else {
            return;
        };
        let Some(caller_task) = self.task(caller_task_id) else {
            return;
        };
        let (class, key, slice, latency, caller_limit) = {
            let caller_task = caller_task.lock();
            (
                caller_task.class(),
                caller_task.queue_key(),
                caller_task.slice_ticks(),
                caller_task.latency_ticks(),
                caller_task.lending_limit(),
            )
        };
        let server_limit = self
            .task_of(thread)
            .and_then(|task| self.task(task))
            .map_or(0, |task| task.lock().lending_limit());
        let depth = caller_limit.min(server_limit);
        if depth == 0 {
            return;
        }
        let frame = LendingFrame {
            caller: token.caller,
            sequence: token.sequence,
            class,
            key,
            slice,
            latency,
        };
        let pushed = self
            .threads
            .lock()
            .get_mut(&thread)
            .is_some_and(|tcb| tcb.push_lending(frame, depth));
        if pushed {
            // 服务端已在运行时，缩短当前 Grant 并立即请求重新调度，使调用方的
            // 时间片/截止期在本次 syscall 返回时生效。
            if let Some(server_task) = self.task_of(thread).and_then(|id| self.task(id)) {
                let now = self.now();
                let last_cpu = {
                    let mut server_task = server_task.lock();
                    if server_task.state() == TaskState::Running {
                        let remaining = server_task.grant_end().saturating_sub(now);
                        let end = now.saturating_add(slice.min(remaining));
                        server_task.set_grant_end(end);
                        server_task.last_cpu()
                    } else {
                        hk_cadence::NO_CPU
                    }
                };
                if last_cpu != hk_cadence::NO_CPU {
                    self.request_resched(last_cpu as usize);
                }
            }
            self.record(
                thread.get(),
                Category::Sched,
                TRACE_LEND,
                caller_task_id.get(),
            );
        }
    }

    /// 服务端回复时弹出对应的 lending frame。
    pub(crate) fn return_lending(&self, thread: TcbId, token: ReplyToken) {
        let caller_task_id = self.task_of(token.caller);
        let removed = self
            .threads
            .lock()
            .get_mut(&thread)
            .is_some_and(|tcb| tcb.pop_lending(token.caller, token.sequence));
        if removed {
            self.record(
                thread.get(),
                Category::Sched,
                TRACE_RETURN,
                caller_task_id.map_or(0, TaskId::get),
            );
        }
    }

    /// 调用方 TCB 退出时，从所有服务端 TCB 清理它的借贷。
    pub(crate) fn release_lending(&self, thread: TcbId) {
        let caller_task_id = self.task_of(thread).map_or(0, TaskId::get);
        let mut threads = self.threads.lock();
        for tcb in threads.values_mut() {
            let removed = tcb.remove_lending_for(thread);
            if removed > 0 {
                self.record(
                    tcb.id().get(),
                    Category::Sched,
                    TRACE_RETURN,
                    caller_task_id,
                );
            }
        }
    }

    /// 返回 TCB 当前的借贷深度。
    pub fn lending_depth(&self, thread: TcbId) -> usize {
        self.threads
            .lock()
            .get(&thread)
            .map_or(0, Tcb::lending_depth)
    }
}
