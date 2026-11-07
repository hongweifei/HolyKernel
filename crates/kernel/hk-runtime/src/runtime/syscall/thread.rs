//! 同地址空间线程：创建、加入、TLS 与共享内存等待。

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 在进程地址空间内创建线程；返回线程编号。
    pub(crate) fn thread_create(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let entry = frame.arg0();
        let stack_top = frame.arg1();
        let argument = frame.arg2();
        let tls = frame.arg3();
        let leader = self.leader_of(index);
        if leader != index || entry == 0 || stack_top == 0 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }

        let child_index = self.tasks.len();
        if !self.platform.share_vspace(leader, child_index) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }

        let kernel = &self.kernel;
        let rights = Rights::READ | Rights::WRITE | Rights::TRANSFER;
        let child = kernel.spawn_parked(HandleTable::with_default_quota(), Priority::NORMAL);

        self.endpoints.push(None);
        self.notifications.push(None);

        let mut handles = Vec::with_capacity(self.endpoints.len());
        for (slot, object) in self.endpoints.iter().enumerate() {
            let handle = if slot == child_index {
                None
            } else {
                object.and_then(|id| kernel.grant(child, id, rights).ok())
            };
            handles.push(handle);
        }
        let mut notifications = Vec::with_capacity(self.notifications.len());
        for (slot, object) in self.notifications.iter().enumerate() {
            let handle = if slot == child_index {
                None
            } else {
                object.and_then(|id| kernel.grant(child, id, rights).ok())
            };
            notifications.push(handle);
        }

        // 线程槽位没有自己的端点；其他任务按槽位下标补齐空洞。
        for task in &mut self.tasks {
            task.handles.push(None);
            task.notifications.push(None);
        }

        let arena = self.tasks[leader].arena;
        let ledger_limit = self.tasks[leader].ledger.limit();
        let name = format!("{}-thread{}", self.tasks[leader].name, child_index - leader);
        let mut task = UserTask::new(
            child,
            &name,
            Some(tcb.get()),
            handles,
            notifications,
            C::for_thread(entry, stack_top, argument),
            0,
            0,
            ledger_limit,
        );
        task.arena = arena;
        task.leader = leader;
        task.tls = tls;
        self.tasks.push(task);
        let _ = kernel.activate(child);

        frame.set_result(child.get());
        Switch::Continue
    }

    /// 等待线程退出。
    pub(crate) fn thread_join(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let target = frame.arg0();
        if target == 0 || target == tcb.get() {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Some(entry) = self.tasks.iter().find(|task| task.tcb.get() == target) else {
            frame.set_result(0);
            return Switch::Continue;
        };
        let target_tcb = entry.tcb;
        if entry.exited {
            frame.set_result(0);
            return Switch::Continue;
        }

        self.joiners.push((target_tcb, tcb));
        let mut context = *frame;
        context.set_result(0);
        self.tasks[index].context = context;
        if self.kernel.park(tcb).is_err() {
            self.joiners
                .retain(|(exited, joiner)| !(*exited == target_tcb && *joiner == tcb));
            self.tasks[index].context.set_result(u64::MAX);
            return Switch::Continue;
        }
        Switch::Block
    }

    /// 设置当前线程的 TLS 基址。
    pub(crate) fn thread_set_tls(&mut self, index: usize, frame: &mut C) -> Switch {
        let tls = frame.arg0();
        self.tasks[index].tls = tls;
        self.platform.set_thread_pointer(tls);
        frame.set_result(0);
        Switch::Continue
    }

    /// 休眠指定调度 tick 数；到期由 [`Self::wake_sleepers`] 唤醒。
    pub(crate) fn sleep(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let ticks = frame.arg0();
        frame.set_result(0);
        self.tasks[index].context = *frame;
        if ticks == 0 {
            self.tasks[index].pending = Pending::None;
            return Switch::Continue;
        }

        let wake = self.platform.timer_ticks().saturating_add(ticks);
        self.tasks[index].pending = Pending::Sleep { wake };
        if !self.sleeping.contains(&index) {
            self.sleeping.push(index);
        }
        if self.kernel.park(tcb).is_err() {
            self.sleeping.retain(|slot| *slot != index);
            self.tasks[index].pending = Pending::None;
            self.tasks[index].context.set_result(u64::MAX);
            return Switch::Continue;
        }
        Switch::Block
    }

    /// 在共享内存字上等待；值已变化时立即返回。
    pub(crate) fn futex_wait(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let address = frame.arg0();
        let expected = frame.arg1() as u32;
        let mut word = [0_u8; 4];
        if !self.platform.read_user(address, &mut word) || u32::from_le_bytes(word) != expected {
            frame.set_result(FUTEX_CHANGED);
            return Switch::Continue;
        }

        let leader = self.leader_of(index);
        self.futex_waiters.push((leader, address, tcb));
        let mut context = *frame;
        context.set_result(0);
        self.tasks[index].context = context;
        if self.kernel.park(tcb).is_err() {
            self.futex_waiters.retain(|(owner, addr, waiter)| {
                !(*owner == leader && *addr == address && *waiter == tcb)
            });
            self.tasks[index].context.set_result(u64::MAX);
            return Switch::Continue;
        }
        Switch::Block
    }

    /// 唤醒共享内存字上的等待者；返回唤醒数量。
    pub(crate) fn futex_wake(&mut self, index: usize, frame: &mut C) -> Switch {
        let address = frame.arg0();
        let count = (frame.arg1().max(1)) as usize;
        let leader = self.leader_of(index);
        let mut woken = 0;
        let mut kept = Vec::new();
        for (owner, addr, waiter) in core::mem::take(&mut self.futex_waiters) {
            if woken < count && owner == leader && addr == address {
                self.kernel.wake_thread(waiter);
                woken += 1;
            } else {
                kept.push((owner, addr, waiter));
            }
        }
        self.futex_waiters = kept;
        frame.set_result(woken as u64);
        Switch::Continue
    }
}
