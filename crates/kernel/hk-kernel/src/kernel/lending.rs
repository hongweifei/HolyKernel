//! caller-pays 借贷。
//!
//! 服务端 TCB 在收到带回复令牌的调用时压入 lending frame；调用期间类别、队列键
//! 与时间片取调用方与服务端中更强的一个，执行时间同时记入调用方 Meter 与预算。
//! `reply`、调用方退出或服务端退出时弹出/清理 frame。

use super::*;
use crate::tcb::LendingFrame;

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
            caller: caller_task_id,
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
        let Some(caller_task_id) = self.task_of(token.caller) else {
            return;
        };
        let removed = self
            .threads
            .lock()
            .get_mut(&thread)
            .is_some_and(|tcb| tcb.pop_lending(caller_task_id, token.sequence));
        if removed {
            self.record(
                thread.get(),
                Category::Sched,
                TRACE_RETURN,
                caller_task_id.get(),
            );
        }
    }

    /// 调用方退出时，从所有服务端 TCB 清理它的借贷。
    pub(crate) fn release_lending(&self, thread: TcbId) {
        let Some(caller_task_id) = self.task_of(thread) else {
            return;
        };
        let mut threads = self.threads.lock();
        for tcb in threads.values_mut() {
            let removed = tcb.remove_lending_for(caller_task_id);
            if removed > 0 {
                self.record(
                    tcb.id().get(),
                    Category::Sched,
                    TRACE_RETURN,
                    caller_task_id.get(),
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
