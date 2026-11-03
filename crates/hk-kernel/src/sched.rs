//! 调度器：固定优先级 + 同优先级轮转。

use alloc::collections::VecDeque;

use crate::tcb::{Priority, TcbId};

/// 每 CPU 就绪队列。
///
/// 32 个优先级各一条 FIFO 队列，`pick` 取最高优先级的队首，实现固定优先级与
/// 同优先级轮转。
pub struct Scheduler {
    queues: [VecDeque<TcbId>; Priority::LEVELS],
    current: Option<TcbId>,
}

impl Scheduler {
    /// 构造空调度器。
    pub fn new() -> Self {
        Self {
            queues: core::array::from_fn(|_| VecDeque::new()),
            current: None,
        }
    }

    /// 把线程加入就绪队列。
    pub fn enqueue(&mut self, id: TcbId, priority: Priority) {
        self.queues[priority.level() as usize].push_back(id);
    }

    /// 从就绪队列移除线程。
    pub fn remove(&mut self, id: TcbId) -> bool {
        for queue in &mut self.queues {
            if let Some(index) = queue.iter().position(|queued| *queued == id) {
                queue.remove(index);
                return true;
            }
        }
        false
    }

    /// 取下一个可运行线程并记录为当前线程。
    pub fn pick(&mut self) -> Option<TcbId> {
        for queue in &mut self.queues {
            if let Some(id) = queue.pop_front() {
                self.current = Some(id);
                return Some(id);
            }
        }
        None
    }

    /// 返回当前线程。
    pub fn current(&self) -> Option<TcbId> {
        self.current
    }

    /// 取走当前线程记录。
    pub fn take_current(&mut self) -> Option<TcbId> {
        self.current.take()
    }

    /// 当前线程是 `id` 时清除记录。
    pub fn clear_current_if(&mut self, id: TcbId) {
        if self.current == Some(id) {
            self.current = None;
        }
    }

    /// 返回就绪线程数。
    pub fn runnable(&self) -> usize {
        self.queues.iter().map(VecDeque::len).sum()
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: u64) -> TcbId {
        TcbId::new(value)
    }

    #[test]
    fn picks_highest_priority_first() {
        let mut scheduler = Scheduler::new();
        scheduler.enqueue(id(1), Priority::LOWEST);
        scheduler.enqueue(id(2), Priority::NORMAL);
        scheduler.enqueue(id(3), Priority::HIGHEST);

        assert_eq!(scheduler.pick(), Some(id(3)));
        assert_eq!(scheduler.pick(), Some(id(2)));
        assert_eq!(scheduler.pick(), Some(id(1)));
        assert_eq!(scheduler.pick(), None);
    }

    #[test]
    fn rotates_within_priority() {
        let mut scheduler = Scheduler::new();
        scheduler.enqueue(id(1), Priority::NORMAL);
        scheduler.enqueue(id(2), Priority::NORMAL);

        assert_eq!(scheduler.pick(), Some(id(1)));
        assert_eq!(scheduler.pick(), Some(id(2)));
    }

    #[test]
    fn removes_blocked_thread() {
        let mut scheduler = Scheduler::new();
        scheduler.enqueue(id(1), Priority::NORMAL);
        scheduler.enqueue(id(2), Priority::NORMAL);

        assert!(scheduler.remove(id(1)));
        assert!(!scheduler.remove(id(1)));
        assert_eq!(scheduler.runnable(), 1);
        assert_eq!(scheduler.pick(), Some(id(2)));
    }
}
