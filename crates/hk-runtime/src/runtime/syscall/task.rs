use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    pub(crate) fn allocate(&mut self, index: usize, size: usize, align: usize) -> Option<u64> {
        // 线程共享进程 Arena，避免每个线程各自占用一段未映射的地址空间。
        let slot = self.leader_of(index);
        let start = self.tasks[slot].arena.allocate(size, align)?;
        self.platform.zero(start, size);
        Some(start)
    }

    pub(crate) fn notification_handle(&self, index: usize, number: usize) -> Option<Handle> {
        self.tasks[index]
            .notifications
            .get(number.wrapping_sub(1))
            .copied()
            .flatten()
    }

    /// 把随消息收到的句柄加入任务额外句柄表，返回编号；没有句柄时为 0。
    pub(crate) fn close_handle(&mut self, index: usize, tcb: TcbId, endpoint: usize) -> u64 {
        let Some(handle) = self.handle_at(index, endpoint) else {
            return u64::MAX;
        };

        match self.kernel.close_handle(tcb, handle) {
            Ok(()) => {
                self.clear_handle(index, endpoint);
                0
            }
            Err(_) => u64::MAX,
        }
    }
}
