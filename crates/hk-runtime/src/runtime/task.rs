//! 任务生命周期与调度。

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 装载新任务，返回分配的端点编号。
    pub fn start(
        &mut self,
        name: &str,
        device_authority: bool,
        priority: u8,
        mut context: C,
        parent: Option<u64>,
    ) -> Result<u64, KernelError> {
        let index = self.tasks.len();
        let endpoint = index as u64 + 1;
        context.set_start_argument(endpoint);

        let kernel = &self.kernel;
        let tcb = kernel.spawn_parked(HandleTable::with_default_quota(), Priority::new(priority));
        let own = kernel.create_endpoint(tcb, Rights::READ | Rights::WRITE | Rights::TRANSFER)?;
        let own_id = kernel.handle_id(tcb, own)?;
        self.endpoints.push(Some(own_id));

        let own_notification =
            kernel.create_notification(tcb, Rights::READ | Rights::WRITE | Rights::TRANSFER)?;
        let own_notification_id = kernel.handle_id(tcb, own_notification)?;
        self.notifications.push(Some(own_notification_id));
        let device_factory = if device_authority {
            Some(kernel.create_device_grant_factory(tcb, Rights::READ | Rights::TRANSFER)?)
        } else {
            None
        };

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
        let mut task = UserTask::new(
            tcb,
            name,
            parent,
            handles,
            notifications,
            context,
            arena_base,
            arena_size,
            self.platform.memory_limit(index),
        );
        task.device_factory = device_factory;
        task.leader = index;
        self.tasks.push(task);
        let _ = self.kernel.activate(tcb);
        Ok(endpoint)
    }

    /// 捕获任务、序列化现场并终止任务。
    pub fn checkpoint(&mut self, name: &str) -> Result<usize, String> {
        let capture = self.capture_checkpoint(name)?;
        let mut context = alloc::vec![0_u8; C::SAVED_BYTES];
        capture.context.save(&mut context);
        let image = hk_checkpoint::encode_parts(
            capture.slot as u32,
            name,
            &context,
            capture.parent,
            capture.arena_used,
            &capture.pages,
        );
        let size = image.len();
        self.commit_checkpoint(name, capture, image)?;
        Ok(size)
    }

    /// 捕获任务现场但暂不终止任务。
    pub fn capture_checkpoint(&mut self, name: &str) -> Result<CheckpointCapture<C>, String> {
        let Some(index) = self
            .tasks
            .iter()
            .position(|task| task.name == name && !task.exited)
        else {
            return Err(String::from("任务不存在或已退出"));
        };
        if self
            .current
            .iter()
            .flatten()
            .any(|current| *current == index)
        {
            return Err(String::from("任务正在 CPU 上执行"));
        }
        let task = &self.tasks[index];
        if task.device_factory.is_some()
            || task.capabilities.iter().flatten().next().is_some()
            || task.grants.iter().flatten().next().is_some()
        {
            return Err(String::from("任务持有设备或共享能力"));
        }
        if let Some((_, entry)) = self
            .tasks
            .iter()
            .enumerate()
            .find(|(other, entry)| *other != index && !entry.exited && entry.leader == index)
        {
            let mut message = String::from("任务仍有线程：");
            message.push_str(&entry.name);
            return Err(message);
        }
        let context = task.context;
        let parent = task.parent;
        let arena_used = task.arena.used();
        let pages = self
            .platform
            .capture_pages(index)
            .ok_or_else(|| String::from("无法捕获地址空间"))?;
        Ok(CheckpointCapture {
            slot: index,
            context,
            parent,
            arena_used,
            pages,
        })
    }

    /// 终止任务、释放原 VSpace，并保存序列化镜像。
    pub fn commit_checkpoint(
        &mut self,
        name: &str,
        capture: CheckpointCapture<C>,
        image: Vec<u8>,
    ) -> Result<usize, String> {
        if !self.terminate(name, 0) {
            return Err(String::from("无法暂停任务"));
        }
        self.platform.release_pages(capture.slot);
        self.checkpoints.retain(|(saved, _)| saved != name);
        self.checkpoints.push((
            String::from(name),
            Checkpoint {
                slot: capture.slot,
                context: capture.context,
                parent: capture.parent,
                arena_used: capture.arena_used,
                image,
            },
        ));
        Ok(capture.slot)
    }

    /// 用保存的现场在同一任务槽恢复 TCB 与地址空间。
    pub fn restore_checkpoint(&mut self, name: &str) -> Result<usize, String> {
        let Some(position) = self.checkpoints.iter().position(|(saved, _)| saved == name) else {
            return Err(String::from("检查点不存在"));
        };
        let (saved, checkpoint) = self.checkpoints.remove(position);
        let Some(snapshot) = hk_checkpoint::Snapshot::decode(&checkpoint.image) else {
            self.checkpoints.push((saved, checkpoint));
            return Err(String::from("快照损坏"));
        };
        let result = self.restore_parts(
            name,
            checkpoint.slot,
            checkpoint.context,
            checkpoint.parent,
            checkpoint.arena_used,
            &snapshot.pages,
        );
        if result.is_err() {
            self.checkpoints.push((saved, checkpoint));
        }
        result
    }

    /// 按快照重建任务槽中的地址空间与 TCB。
    pub fn restore_parts(
        &mut self,
        name: &str,
        slot: usize,
        context: C,
        parent: Option<u64>,
        arena_used: u64,
        pages: &[Page],
    ) -> Result<usize, String> {
        if self
            .current
            .iter()
            .flatten()
            .any(|current| *current == slot)
        {
            return Err(String::from("任务正在 CPU 上执行"));
        }
        let (task_name, exited) = {
            let Some(task) = self.tasks.get(slot) else {
                return Err(String::from("任务槽不存在"));
            };
            (task.name.clone(), task.exited)
        };
        if task_name != name {
            return Err(String::from("任务槽已被其他任务占用"));
        }
        if !exited && !self.terminate(&task_name, 0) {
            return Err(String::from("无法暂停任务"));
        }
        if !self.platform.restore_pages(slot, pages) {
            return Err(String::from("无法重建地址空间"));
        }

        let kernel = &self.kernel;
        let rights = Rights::READ | Rights::WRITE | Rights::TRANSFER;
        let tcb = kernel.spawn_parked(HandleTable::with_default_quota(), Priority::NORMAL);
        let own = kernel
            .create_endpoint(tcb, rights)
            .map_err(|error| format!("{error}"))?;
        let own_id = kernel
            .handle_id(tcb, own)
            .map_err(|error| format!("{error}"))?;
        let own_notification = kernel
            .create_notification(tcb, rights)
            .map_err(|error| format!("{error}"))?;
        let own_notification_id = kernel
            .handle_id(tcb, own_notification)
            .map_err(|error| format!("{error}"))?;
        self.endpoints[slot] = Some(own_id);
        self.notifications[slot] = Some(own_notification_id);

        for (index, task) in self.tasks.iter_mut().enumerate() {
            if index == slot || task.exited {
                continue;
            }
            let endpoint = kernel.grant(task.tcb, own_id, rights).ok();
            if let Some(handle) = task.handles.get_mut(slot) {
                *handle = endpoint;
            } else {
                task.handles.push(endpoint);
            }
            let notification = kernel.grant(task.tcb, own_notification_id, rights).ok();
            if let Some(handle) = task.notifications.get_mut(slot) {
                *handle = notification;
            } else {
                task.notifications.push(notification);
            }
        }

        let mut handles = Vec::with_capacity(self.endpoints.len());
        for (index, object) in self.endpoints.iter().enumerate() {
            let handle = if index == slot {
                Some(own)
            } else {
                object.and_then(|id| kernel.grant(tcb, id, rights).ok())
            };
            handles.push(handle);
        }
        let mut notifications = Vec::with_capacity(self.notifications.len());
        for (index, object) in self.notifications.iter().enumerate() {
            let handle = if index == slot {
                Some(own_notification)
            } else {
                object.and_then(|id| kernel.grant(tcb, id, rights).ok())
            };
            notifications.push(handle);
        }

        let (arena_base, arena_size) = self.platform.arena_layout(slot);
        let mut task = UserTask::new(
            tcb,
            name,
            parent,
            handles,
            notifications,
            context,
            arena_base,
            arena_size,
            self.platform.memory_limit(slot),
        );
        task.arena.set_used(arena_used);
        task.leader = slot;
        self.tasks[slot] = task;
        let _ = self.kernel.activate(tcb);
        Ok(slot)
    }

    /// 从序列化镜像重建任务；返回任务名与槽位。
    pub fn restore_image(&mut self, image: &[u8]) -> Result<(String, usize), String> {
        let Some(snapshot) = hk_checkpoint::Snapshot::decode(image) else {
            return Err(String::from("快照损坏"));
        };
        let Some(context) = C::load(&snapshot.context) else {
            return Err(String::from("寄存器现场损坏"));
        };
        let slot = self.restore_parts(
            &snapshot.name,
            snapshot.slot as usize,
            context,
            snapshot.parent,
            snapshot.arena_used,
            &snapshot.pages,
        )?;
        Ok((snapshot.name, slot))
    }

    /// 读取已保存快照的一段镜像。
    pub fn checkpoint_chunk(&self, name: &str, offset: u64, length: usize) -> Option<Vec<u8>> {
        let (_, checkpoint) = self.checkpoints.iter().find(|(saved, _)| saved == name)?;
        let start = usize::try_from(offset).ok()?.min(checkpoint.image.len());
        let end = start.saturating_add(length).min(checkpoint.image.len());
        Some(checkpoint.image[start..end].to_vec())
    }

    /// 复制当前任务：页表写时复制、句柄与通知映射；子任务返回 0。
    pub(crate) fn fork_task(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        if !self.populate_lazy_regions(index) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
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
        let ledger_limit = self.tasks[index].ledger.limit();
        let mut task = UserTask::new(
            child,
            &name,
            Some(tcb.get()),
            handles,
            notifications,
            context,
            arena.base(),
            arena.size(),
            ledger_limit,
        );
        task.arena = arena;
        task.leader = child_index;
        self.tasks.push(task);
        self.fork_regions(index, child_index);
        let _ = self.kernel.activate(child);

        frame.set_result(child_index as u64 + 1);
        Switch::Continue
    }

    fn drain_irqs(&mut self) {
        let mut pending = [0_u64; 4];
        self.platform.take_irqs(&mut pending);
        for (word, bits) in pending.iter().enumerate() {
            if *bits == 0 {
                continue;
            }
            for bit in 0..64 {
                if bits & (1_u64 << bit) != 0 {
                    self.signal_irq((word * 64 + bit) as u8);
                }
            }
        }
    }

    pub(crate) fn release_grants(&mut self, index: usize) {
        let tcb = self.tasks[index].tcb;
        self.release_all_dma_mappings(index);
        let charges = core::mem::take(&mut self.tasks[index].grant_charges);
        for (handle, charge) in core::mem::take(&mut self.tasks[index].grants)
            .into_iter()
            .zip(charges)
        {
            if let Some(handle) = handle {
                self.release_bindings(index, handle);
                if let Ok(grant) = self.kernel.device_grant(tcb, handle)
                    && let Some(range) = grant.dma
                {
                    self.platform.dma_free(range);
                }
            }
            if let Some(charge) = charge {
                let _ = self.tasks[index].ledger.release(charge);
            }
        }
    }

    pub(crate) fn signal_irq(&self, irq: u8) {
        let bindings: Vec<_> = self
            .tasks
            .iter()
            .flat_map(|task| task.bound_irqs.iter())
            .filter(|(_, line, _, _)| *line == irq)
            .map(|(_, _, notification, bits)| (*notification, *bits))
            .collect();
        for (notification, bits) in bindings {
            let _ = self
                .kernel
                .signal_id(ObjectId::new(notification.get()), bits);
        }
    }

    pub(crate) fn release_dma_mappings(&mut self, index: usize, grant: Handle) {
        let mappings = core::mem::take(&mut self.tasks[index].dma_mappings);
        let mut kept = Vec::new();
        for (handle, domain, iova, len) in mappings {
            if handle == grant {
                if let Some(iommu) = self.platform.iommu() {
                    let _ = iommu.unmap(domain, iova, len);
                }
            } else {
                kept.push((handle, domain, iova, len));
            }
        }
        self.tasks[index].dma_mappings = kept;
    }

    fn release_all_dma_mappings(&mut self, index: usize) {
        let mappings = core::mem::take(&mut self.tasks[index].dma_mappings);
        if let Some(iommu) = self.platform.iommu() {
            for (_, domain, iova, len) in mappings {
                let _ = iommu.unmap(domain, iova, len);
            }
        }
    }

    pub(crate) fn release_bindings(&mut self, index: usize, grant: Handle) {
        let bindings = core::mem::take(&mut self.tasks[index].bound_irqs);
        for (handle, irq, notification, bits) in bindings {
            if handle == grant {
                self.release_irq(irq);
            } else {
                self.tasks[index]
                    .bound_irqs
                    .push((handle, irq, notification, bits));
            }
        }
    }

    pub(crate) fn release_irq(&mut self, irq: u8) {
        let count = &mut self.irq_routes[irq as usize];
        if *count > 0 {
            *count -= 1;
            if *count == 0
                && let Some(controller) = self.platform.interrupt_controller()
            {
                let _ = controller.unbind(hk_hal::Irq::new(u32::from(irq)));
            }
        }
    }
    /// 调度一步；调用方在锁外恢复 `context`，再调用 [`Self::finish`]。
    pub fn step(&mut self, cpu: usize) -> Step<C> {
        self.drain_irqs();
        if self.platform.take_console_input()
            && let Some(index) = self.console_subscriber
            && let Some(task) = self.tasks.get(index)
            && !task.exited
            && let Some(handle) = task.notifications.get(index).copied().flatten()
        {
            let _ = self.kernel.signal(task.tcb, handle, CONSOLE_NOTIFY_INPUT);
        }
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
        self.platform.set_thread_pointer(self.tasks[index].tls);
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

        let exited_tcb = self.tasks[index].tcb;
        self.wake_joiners(exited_tcb);
        if self.tasks[index].leader != index {
            return None;
        }

        let exited_endpoint = index as u64 + 1;
        for other in 0..self.tasks.len() {
            if other == index || self.tasks[other].exited {
                continue;
            }
            let tcb = self.tasks[other].tcb;
            match self.tasks[other].pending {
                Pending::RecvData {
                    handle,
                    expected: Some(expected),
                    ..
                } if expected == exited_tcb => {
                    let _ = self.kernel.cancel_recv(tcb, handle);
                    let mut context = self.tasks[other].context;
                    context.set_result(0);
                    context.set_handle(0);
                    context.set_sender(0);
                    self.tasks[other].context = context;
                    self.tasks[other].pending = Pending::None;
                    self.kernel.wake_thread(tcb);
                }
                Pending::Send { endpoint, handle } if endpoint == exited_endpoint => {
                    let _ = self.kernel.cancel_send(tcb, handle);
                    let mut context = self.tasks[other].context;
                    context.set_result(u64::MAX);
                    self.tasks[other].context = context;
                    self.tasks[other].pending = Pending::None;
                    self.kernel.wake_thread(tcb);
                }
                _ => {}
            }
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

    /// 终止一个未在任何 CPU 上执行的任务；成功返回 `true`。
    pub fn terminate(&mut self, name: &str, code: u64) -> bool {
        let Some(index) = self
            .tasks
            .iter()
            .position(|task| task.name == name && !task.exited)
        else {
            return false;
        };
        if self
            .current
            .iter()
            .flatten()
            .any(|current| *current == index)
        {
            return false;
        }

        let tcb = self.tasks[index].tcb;
        self.release_grants(index);
        let _ = self.kernel.exit(tcb);
        self.tasks[index].exited = true;
        self.tasks[index].exit_code = code;
        self.tasks[index].pending = Pending::None;
        self.wake_joiners(tcb);
        if self.tasks[index].leader == index {
            self.exit_threads(index);
        }
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
        self.release_grants(index);
        let _ = self.kernel.exit(tcb);
        self.tasks[index].exited = true;
        self.tasks[index].exit_code = code;
        self.tasks[index].pending = Pending::None;
        self.current[cpu] = None;
        self.wake_joiners(tcb);
        if self.tasks[index].leader == index {
            self.exit_threads(index);
        }
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
                expected,
                handle_out,
            } => {
                let tcb = self.tasks[index].tcb;
                match self.kernel.recv_from(tcb, handle, expected) {
                    Ok(Some(received)) => {
                        let mut frame = self.tasks[index].context;
                        if handle_out {
                            let number = self.install_handle(index, &received.message);
                            let _ = self.deliver_data_handle(
                                index, &mut frame, &received, buffer, capacity, number,
                            );
                        } else {
                            let _ =
                                self.deliver_data(index, &mut frame, &received, buffer, capacity);
                        }
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
            Pending::ProviderFault {
                capability,
                index: page_index,
                page,
            } => {
                let tcb = self.tasks[index].tcb;
                if let Some(message) = self.kernel.take_reply(tcb) {
                    let phys = message.words[0];
                    if phys != 0 && phys != u64::MAX {
                        let lease = hk_mem::hmp::FrameLease::new(
                            hk_hal::PhysAddr::new(phys),
                            hk_hal::PAGE_SIZE,
                        );
                        if self
                            .kernel
                            .set_capability_page(tcb, capability, page_index, lease)
                            .unwrap_or(false)
                            && self.platform.map_dma(index, page, phys, hk_hal::PAGE_SIZE)
                        {
                            self.platform.zero(page, hk_hal::PAGE_SIZE);
                        } else {
                            blocked_again = true;
                        }
                    } else {
                        blocked_again = true;
                    }
                } else {
                    blocked_again = true;
                }
            }
            Pending::None | Pending::Send { .. } => {}
        }

        if blocked_again {
            self.tasks[index].pending = pending;
            self.current[cpu] = None;
        }

        blocked_again
    }
}
