//! 任务生命周期与调度。

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 装载新任务，返回分配的端点编号。
    pub fn start(
        &mut self,
        name: &str,
        mut context: C,
        parent: Option<u64>,
    ) -> Result<u64, KernelError> {
        let index = self.tasks.len();
        let endpoint = index as u64 + 1;
        context.set_start_argument(endpoint);

        let kernel = &self.kernel;
        let tcb = kernel.spawn_parked(HandleTable::with_default_quota(), Priority::NORMAL);
        let own = kernel.create_endpoint(tcb, Rights::READ | Rights::WRITE | Rights::TRANSFER)?;
        let own_id = kernel.handle_id(tcb, own)?;
        self.endpoints.push(Some(own_id));

        let own_notification =
            kernel.create_notification(tcb, Rights::READ | Rights::WRITE | Rights::TRANSFER)?;
        let own_notification_id = kernel.handle_id(tcb, own_notification)?;
        self.notifications.push(Some(own_notification_id));

        let mut handles = Vec::with_capacity(self.endpoints.len());
        for (slot, object) in self.endpoints.iter().enumerate() {
            let handle = if slot == index {
                Some(own)
            } else {
                match object {
                    Some(id) => kernel
                        .grant(tcb, *id, Rights::READ | Rights::WRITE | Rights::TRANSFER)
                        .ok(),
                    None => None,
                }
            };
            handles.push(handle);
        }

        let mut notifications = Vec::with_capacity(self.notifications.len());
        for (slot, object) in self.notifications.iter().enumerate() {
            let handle = if slot == index {
                Some(own_notification)
            } else {
                match object {
                    Some(id) => kernel
                        .grant(tcb, *id, Rights::READ | Rights::WRITE | Rights::TRANSFER)
                        .ok(),
                    None => None,
                }
            };
            notifications.push(handle);
        }

        for task in &mut self.tasks {
            if task.exited {
                continue;
            }
            let handle = kernel
                .grant(
                    task.tcb,
                    own_id,
                    Rights::READ | Rights::WRITE | Rights::TRANSFER,
                )
                .ok();
            task.handles.push(handle);
            let handle = kernel
                .grant(
                    task.tcb,
                    own_notification_id,
                    Rights::READ | Rights::WRITE | Rights::TRANSFER,
                )
                .ok();
            task.notifications.push(handle);
        }

        let (arena_base, arena_size) = self.platform.arena_layout(index);
        self.tasks.push(UserTask::new(
            tcb,
            name,
            parent,
            handles,
            notifications,
            context,
            arena_base,
            arena_size,
        ));
        let _ = self.kernel.activate(tcb);
        Ok(endpoint)
    }

    /// 复制当前任务：页表写时复制、句柄与通知映射；子任务返回 0。
    pub(crate) fn fork_task(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let child_index = self.tasks.len();
        if !self.platform.fork(index, child_index) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }

        let kernel = &self.kernel;
        let rights = Rights::READ | Rights::WRITE | Rights::TRANSFER;
        let child = kernel.spawn_parked(HandleTable::with_default_quota(), Priority::NORMAL);

        let Ok(own_endpoint) = kernel.create_endpoint(child, rights) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(own_endpoint_id) = kernel.handle_id(child, own_endpoint) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        self.endpoints.push(Some(own_endpoint_id));

        let Ok(own_notification) = kernel.create_notification(child, rights) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(own_notification_id) = kernel.handle_id(child, own_notification) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        self.notifications.push(Some(own_notification_id));

        let mut handles = Vec::with_capacity(self.endpoints.len());
        for (slot, object) in self.endpoints.iter().enumerate() {
            let handle = if slot == child_index {
                Some(own_endpoint)
            } else {
                match object {
                    Some(id) => kernel.grant(child, *id, rights).ok(),
                    None => None,
                }
            };
            handles.push(handle);
        }

        let mut notifications = Vec::with_capacity(self.notifications.len());
        for (slot, object) in self.notifications.iter().enumerate() {
            let handle = if slot == child_index {
                Some(own_notification)
            } else {
                match object {
                    Some(id) => kernel.grant(child, *id, rights).ok(),
                    None => None,
                }
            };
            notifications.push(handle);
        }

        for task in &mut self.tasks {
            if task.exited {
                continue;
            }
            task.handles
                .push(kernel.grant(task.tcb, own_endpoint_id, rights).ok());
            task.notifications
                .push(kernel.grant(task.tcb, own_notification_id, rights).ok());
        }

        let mut context = *frame;
        context.set_result(0);
        let name = format!("{}-fork", self.tasks[index].name);
        let arena = self.tasks[index].arena;
        let mut task = UserTask::new(
            child,
            &name,
            Some(tcb.get()),
            handles,
            notifications,
            context,
            arena.base(),
            arena.size(),
        );
        task.arena = arena;
        self.tasks.push(task);
        let _ = self.kernel.activate(child);

        frame.set_result(child_index as u64 + 1);
        Switch::Continue
    }

    /// 调度一步；调用方在锁外恢复 `context`，再调用 [`Self::finish`]。
    pub fn step(&mut self, cpu: usize) -> Step<C> {
        let Some(index) = self.next_task(cpu) else {
            self.last_finish = None;
            return Step::Done;
        };

        if self.prepare(cpu, index) {
            return Step::Blocked;
        }

        if let Some(last) = self.last_finish.take() {
            let now = self.platform.timestamp();
            if now >= last {
                let cycles = now - last;
                self.switch_count = self.switch_count.saturating_add(1);
                self.switch_cycles = self.switch_cycles.saturating_add(cycles);
                self.switch_max = self.switch_max.max(cycles);
            }
        }

        self.platform.activate(index);
        self.resumes[cpu] = self.resumes[cpu].saturating_add(1);
        Step::Resume {
            index,
            context: self.tasks[index].context,
        }
    }

    /// 恢复执行返回后收尾；任务退出时关闭其他任务持有的该端点句柄，并返回名称与退出码。
    pub fn finish(&mut self, cpu: usize, index: usize) -> Option<(String, u64)> {
        self.current[cpu] = None;
        self.last_finish = Some(self.platform.timestamp());
        if !self.tasks[index].exited {
            return None;
        }

        for other in 0..self.tasks.len() {
            if other == index || self.tasks[other].exited {
                continue;
            }
            if let Some(handle) = self.tasks[other].handles.get(index).copied().flatten() {
                let _ = self.kernel.close_handle(self.tasks[other].tcb, handle);
                self.tasks[other].handles[index] = None;
            }
            if let Some(handle) = self.tasks[other]
                .notifications
                .get(index)
                .copied()
                .flatten()
            {
                let _ = self.kernel.close_handle(self.tasks[other].tcb, handle);
                self.tasks[other].notifications[index] = None;
            }
        }

        let task = &self.tasks[index];
        Some((task.name.clone(), task.exit_code))
    }

    /// 命中调度 tick 时保存现场；返回是否切换任务。
    pub fn preempt(&mut self, cpu: usize, context: C) -> bool {
        let Some(index) = self.current[cpu] else {
            return false;
        };
        self.tasks[index].context = context;
        self.tasks[index].pending = Pending::None;
        true
    }

    /// 标记当前任务因用户态异常退出；成功返回 `true`。
    pub fn fault(&mut self, cpu: usize, code: u64) -> bool {
        let Some(index) = self.current[cpu] else {
            return false;
        };
        if self.tasks[index].exited {
            return false;
        }

        let tcb = self.tasks[index].tcb;
        let _ = self.kernel.exit(tcb);
        self.tasks[index].exited = true;
        self.tasks[index].exit_code = code;
        self.tasks[index].pending = Pending::None;
        self.current[cpu] = None;
        true
    }

    fn next_task(&mut self, cpu: usize) -> Option<usize> {
        let tcb = self.kernel.schedule(cpu)?;
        self.tasks.iter().position(|task| task.tcb == tcb)
    }

    fn prepare(&mut self, cpu: usize, index: usize) -> bool {
        self.current[cpu] = Some(index);
        // 恢复阻塞接收前先切到目标地址空间，字节载荷才能写入用户缓冲。
        self.platform.activate(index);

        let pending = self.tasks[index].pending;
        self.tasks[index].pending = Pending::None;
        let mut blocked_again = false;

        match pending {
            Pending::Recv { handle } => {
                let tcb = self.tasks[index].tcb;
                match self.kernel.recv(tcb, handle) {
                    Ok(Some(received)) => {
                        self.tasks[index].reply = received.reply;
                        let sender = self.endpoint_of(received.sender);
                        let handle = self.install_handle(index, &received.message);
                        let context = &mut self.tasks[index].context;
                        context.set_result(received.message.label);
                        context.set_word(received.message.words[0]);
                        context.set_sender(sender);
                        context.set_handle(handle);
                    }
                    Ok(None) => blocked_again = true,
                    Err(_) => self.tasks[index].context.set_result(u64::MAX),
                }
            }
            Pending::RecvData {
                handle,
                buffer,
                capacity,
            } => {
                let tcb = self.tasks[index].tcb;
                match self.kernel.recv(tcb, handle) {
                    Ok(Some(received)) => {
                        let mut frame = self.tasks[index].context;
                        let _ = self.deliver_data(index, &mut frame, &received, buffer, capacity);
                        self.tasks[index].context = frame;
                    }
                    Ok(None) => blocked_again = true,
                    Err(_) => self.tasks[index].context.set_result(u64::MAX),
                }
            }
            Pending::Notify { handle, mask } => {
                let tcb = self.tasks[index].tcb;
                match self.kernel.wait(tcb, handle, mask) {
                    Ok(Some(bits)) => self.tasks[index].context.set_result(bits),
                    Ok(None) => blocked_again = true,
                    Err(_) => self.tasks[index].context.set_result(u64::MAX),
                }
            }
            Pending::Call { endpoint, started } => {
                let tcb = self.tasks[index].tcb;
                match self.kernel.take_reply(tcb) {
                    Some(message) => {
                        let now = self.platform.timestamp();
                        if now >= started {
                            let cycles = now - started;
                            self.call_count = self.call_count.saturating_add(1);
                            self.call_cycles = self.call_cycles.saturating_add(cycles);
                            self.call_max = self.call_max.max(cycles);
                        }
                        let context = &mut self.tasks[index].context;
                        context.set_result(message.label);
                        context.set_word(message.words[0]);
                        context.set_sender(endpoint);
                    }
                    None => blocked_again = true,
                }
            }
            Pending::None | Pending::Send => {}
        }

        if blocked_again {
            self.tasks[index].pending = pending;
            self.current[cpu] = None;
        }

        blocked_again
    }
}
