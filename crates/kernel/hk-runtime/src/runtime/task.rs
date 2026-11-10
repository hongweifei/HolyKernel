//! 任务生命周期与调度。

use hk_cadence::{Cadence, CadenceClass, CadenceId, CadenceSpec, DEFAULT_SPEC};

use super::*;

/// Provider 回复无效时的退出码（ASCII "PROV"）。
const PROVIDER_FAULT_EXIT: u64 = 0x5052_4f56;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 装载新任务，返回分配的端点编号。
    pub fn start(
        &mut self,
        name: &str,
        device_authority: bool,
        cadence: CadenceSpec,
        mut context: C,
        parent: Option<u64>,
    ) -> Result<u64, KernelError> {
        let index = self.tasks.len();
        let endpoint = index as u64 + 1;
        context.set_start_argument(endpoint);

        let kernel = &self.kernel;
        let tcb = kernel.spawn_task_parked(HandleTable::with_default_quota(), cadence)?;
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

        let (handles, notifications) =
            self.endpoint_views(tcb, index, Some(own), Some(own_notification));
        self.append_endpoint(own_id, own_notification_id);

        let (arena_base, arena_size) = self.platform.arena_layout(index);
        let mut process = Process::new(arena_base, arena_size, self.platform.memory_limit(index));
        process.map_arena(&self.kernel, tcb)?;
        process.device_factory = device_factory;
        let mut task = UserTask::new(TaskInit {
            tcb,
            process: index,
            name: String::from(name),
            parent,
            handles,
            notifications,
            context,
        });
        task.leader = index;
        self.tasks.push(task);
        self.processes.push(Some(process));
        let _ = self.kernel.activate(tcb);
        Ok(endpoint)
    }

    /// 为新任务建立端点与通知句柄视图。
    pub(crate) fn endpoint_views(
        &self,
        tcb: TcbId,
        own_slot: usize,
        own_endpoint: Option<Handle>,
        own_notification: Option<Handle>,
    ) -> (Vec<Option<Handle>>, Vec<Option<Handle>>) {
        let rights = Rights::READ | Rights::WRITE | Rights::TRANSFER;
        let handles = self
            .endpoints
            .iter()
            .enumerate()
            .map(|(slot, object)| {
                if slot == own_slot {
                    own_endpoint
                } else {
                    object.and_then(|id| self.kernel.grant(tcb, id, rights).ok())
                }
            })
            .collect();
        let notifications = self
            .notifications
            .iter()
            .enumerate()
            .map(|(slot, object)| {
                if slot == own_slot {
                    own_notification
                } else {
                    object.and_then(|id| self.kernel.grant(tcb, id, rights).ok())
                }
            })
            .collect();
        (handles, notifications)
    }

    /// 把新端点追加到已有任务的端点视图。
    fn append_endpoint(&mut self, endpoint: ObjectId, notification: ObjectId) {
        let kernel = &self.kernel;
        let rights = Rights::READ | Rights::WRITE | Rights::TRANSFER;
        for task in &mut self.tasks {
            if task.exited {
                continue;
            }
            task.handles
                .push(kernel.grant(task.tcb, endpoint, rights).ok());
            task.notifications
                .push(kernel.grant(task.tcb, notification, rights).ok());
        }
    }

    /// 在已有槽位替换端点，并同步其他任务的端点视图。
    fn replace_endpoint(&mut self, slot: usize, endpoint: ObjectId, notification: ObjectId) {
        let kernel = &self.kernel;
        let rights = Rights::READ | Rights::WRITE | Rights::TRANSFER;
        for (index, task) in self.tasks.iter_mut().enumerate() {
            if index == slot || task.exited {
                continue;
            }
            let endpoint_handle = kernel.grant(task.tcb, endpoint, rights).ok();
            if let Some(handle) = task.handles.get_mut(slot) {
                *handle = endpoint_handle;
            } else {
                task.handles.push(endpoint_handle);
            }
            let notification_handle = kernel.grant(task.tcb, notification, rights).ok();
            if let Some(handle) = task.notifications.get_mut(slot) {
                *handle = notification_handle;
            } else {
                task.notifications.push(notification_handle);
            }
        }
    }

    /// 为没有独立端点的线程槽位补齐空视图。
    pub(crate) fn append_empty_endpoint(&mut self) {
        for task in &mut self.tasks {
            task.handles.push(None);
            task.notifications.push(None);
        }
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
            capture.cadence,
            capture.meter,
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
        if self.process(index).device_factory.is_some()
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
        let arena_used = self.process(index).arena.used();
        let cadence = self
            .kernel
            .task_cadence_snapshot(task.tcb)
            .unwrap_or_default();
        let meter = self
            .kernel
            .task_meter_snapshot(task.tcb)
            .unwrap_or_default();
        let pages = self
            .platform
            .capture_pages(index)
            .ok_or_else(|| String::from("无法捕获地址空间"))?;
        Ok(CheckpointCapture {
            slot: index,
            context,
            parent,
            arena_used,
            cadence,
            meter,
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
            snapshot.cadence,
            snapshot.meter,
            &snapshot.pages,
        );
        if result.is_err() {
            self.checkpoints.push((saved, checkpoint));
        }
        result
    }

    /// 按快照重建任务槽中的地址空间与 TCB。
    #[allow(clippy::too_many_arguments)]
    pub fn restore_parts(
        &mut self,
        name: &str,
        slot: usize,
        context: C,
        parent: Option<u64>,
        arena_used: u64,
        cadence: hk_cadence::CadenceSnapshot,
        meter: hk_cadence::MeterSnapshot,
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
        let mut restored = Cadence::restore(
            CadenceId::new(cadence.id),
            cadence.parent(),
            cadence.epoch,
            cadence.spec(),
        )
        .map_err(|error| format!("{error}"))?;
        // 父契约已不存在时提升为独立契约，避免悬挂父链。
        if let Some(parent) = restored.parent()
            && !self
                .tasks
                .iter()
                .filter(|task| !task.exited)
                .filter_map(|task| self.kernel.task_cadence_snapshot(task.tcb))
                .any(|snapshot| snapshot.id == parent.get())
        {
            restored.promote();
        }
        // 只有 Realtime 契约需要 Tempo 准入；其他类别保持恢复的契约不降级。
        if restored.class() == CadenceClass::Realtime {
            let tempo = self
                .tasks
                .iter()
                .find(|task| !task.exited && task.name == "hk-server-tempo")
                .and_then(|task| self.kernel.task_of(task.tcb))
                .and_then(|task| self.kernel.cadence_spec(task).map(|pool| (task, pool)));
            let request = restored.spec();
            let mut tasks: Vec<(u64, u64, u64)> = self
                .tasks
                .iter()
                .filter(|task| !task.exited)
                .filter_map(|task| self.kernel.task_cadence_snapshot(task.tcb))
                .filter(|snapshot| snapshot.class == CadenceClass::Realtime)
                .map(|snapshot| (snapshot.budget, snapshot.period, snapshot.deadline))
                .collect();
            tasks.push((
                request.budget.ticks(),
                request.period.ticks(),
                request.deadline.ticks(),
            ));
            let admissible = hk_cadence::edf_admissible(&tasks);
            let mut granted = false;
            if let Some((tempo_task, pool)) = tempo {
                if request.weight <= pool.weight
                    && request.budget.ticks() <= pool.budget.ticks()
                    && admissible
                {
                    granted = true;
                } else {
                    let weight = request.weight.min(pool.weight).max(1);
                    let budget = request.budget.ticks().min(pool.budget.ticks());
                    if let Ok(child) = self.kernel.admit_task_cadence(tempo_task, weight, budget) {
                        restored = child;
                        granted = true;
                    }
                }
            }
            // 准入失败：优先回退到父契约；父契约缺失时用默认 Throughput（§8）。
            if !granted {
                let parent = restored.parent();
                let parent_spec = parent.and_then(|id| self.kernel.cadence_spec_by_id(id));
                restored = match parent_spec {
                    Some(spec) => {
                        Cadence::restore(CadenceId::next(), parent, restored.epoch(), spec)
                            .map_err(|error| format!("{error}"))?
                    }
                    None => Cadence::from_spec(
                        CadenceId::next(),
                        CadenceSpec::new(CadenceClass::Throughput, 1024),
                    )
                    .map_err(|error| format!("{error}"))?,
                };
            }
        }
        let base = self.average_virtual_pass(restored.class());
        let tcb = kernel
            .spawn_task_with_cadence(HandleTable::with_default_quota(), restored)
            .map_err(|error| format!("{error}"))?;
        if let Some(task) = kernel.task_of(tcb) {
            let mut meter = meter;
            meter.virtual_pass = base;
            let _ = kernel.restore_meter(task, meter);
            kernel.reset_budget_period(task, self.contract_now());
        }
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

        self.replace_endpoint(slot, own_id, own_notification_id);
        let (handles, notifications) =
            self.endpoint_views(tcb, slot, Some(own), Some(own_notification));

        let (arena_base, arena_size) = self.platform.arena_layout(slot);
        let mut process = Process::new(arena_base, arena_size, self.platform.memory_limit(slot));
        process.arena.set_used(arena_used);
        process
            .ledger
            .charge(arena_used)
            .map_err(|error| format!("{error}"))?;
        process
            .map_arena(&self.kernel, tcb)
            .map_err(|error| format!("{error}"))?;
        let mut task = UserTask::new(TaskInit {
            tcb,
            process: slot,
            name: String::from(name),
            parent,
            handles,
            notifications,
            context,
        });
        task.leader = slot;
        self.processes[slot] = Some(process);
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
            snapshot.cadence,
            snapshot.meter,
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
        let child = match kernel.task_of(tcb) {
            Some(parent) => kernel
                .spawn_task_child_parked(HandleTable::with_default_quota(), parent)
                .expect("子任务创建"),
            None => kernel
                .spawn_task_parked(HandleTable::with_default_quota(), DEFAULT_SPEC)
                .expect("子任务创建"),
        };

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

        let (handles, notifications) = self.endpoint_views(
            child,
            child_index,
            Some(own_endpoint),
            Some(own_notification),
        );
        self.append_endpoint(own_endpoint_id, own_notification_id);

        let mut context = *frame;
        context.set_result(0);
        let name = format!("{}-fork", self.tasks[index].name);
        let arena = self.process(index).arena;
        let ledger_limit = self.process(index).ledger.limit();
        let mut process = Process::new(0, 0, ledger_limit);
        process.arena = arena;
        if process.ledger.charge(arena.used()).is_err()
            || process.map_arena(&self.kernel, child).is_err()
        {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        if let Some(parent_region) = self.process(index).arena_region.as_ref()
            && let Some(child_region) = process.arena_region.as_mut()
        {
            child_region.pages = parent_region.pages.clone();
        }
        process.cow_pages = self.process(index).cow_pages.clone();
        let mut task = UserTask::new(TaskInit {
            tcb: child,
            process: child_index,
            name,
            parent: Some(tcb.get()),
            handles,
            notifications,
            context,
        });
        task.leader = child_index;
        self.tasks.push(task);
        self.processes.push(Some(process));
        self.fork_regions(index, child_index);
        self.prune_all_cow_pages(child_index);
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
        for entry in core::mem::take(&mut self.tasks[index].grants)
            .into_iter()
            .flatten()
        {
            let handle = entry.handle;
            self.release_bindings(index, handle);
            if let Ok(grant) = self.kernel.device_grant(tcb, handle)
                && let Some(range) = grant.dma
            {
                self.platform.dma_free(range);
            }
            if let Some(charge) = entry.charge {
                let _ = self.process_mut(index).ledger.release(charge);
            }
        }
    }

    pub(crate) fn signal_irq(&self, irq: u8) {
        for task in &self.tasks {
            for (_, line, notification, bits) in &task.bound_irqs {
                if *line == irq {
                    let _ = self
                        .kernel
                        .signal_id(ObjectId::new(notification.get()), *bits);
                }
            }
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
        self.kernel.set_clock(self.contract_now());
        self.drain_irqs();
        self.wake_sleepers();
        self.wake_timeouts();
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

        self.platform.set_thread_pointer(self.tasks[index].tls);
        self.resumes[cpu] = self.resumes[cpu].saturating_add(1);
        self.run_start[cpu] = self.platform.timestamp();
        Step::Resume {
            index,
            context: self.tasks[index].context,
        }
    }

    /// 恢复执行返回后收尾；任务退出时关闭其他任务持有的该端点句柄，并返回名称与退出码。
    pub fn finish(&mut self, cpu: usize, index: usize) -> Option<(String, u64)> {
        self.current[cpu] = None;
        let now = self.platform.timestamp();
        let started = self.run_start[cpu];
        if now >= started {
            let cycles = now - started;
            self.tasks[index].cpu_cycles = self.tasks[index].cpu_cycles.saturating_add(cycles);
            self.cpu_busy[cpu] = self.cpu_busy[cpu].saturating_add(cycles);
            let elapsed = self.contract_ticks(cycles);
            self.kernel.charge(self.tasks[index].tcb, elapsed);
        }
        self.last_finish = Some(now);
        if !self.tasks[index].exited {
            return None;
        }

        self.remove_sleep(index);
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
        if self.tasks[index].leader == index {
            self.teardown_process(index);
        }
        let _ = self.kernel.exit(tcb);
        self.tasks[index].exited = true;
        self.tasks[index].exit_code = code;
        self.tasks[index].pending = Pending::None;
        self.remove_sleep(index);
        self.wake_joiners(tcb);
        if self.tasks[index].leader == index {
            self.exit_threads(index);
        }
        self.platform.release_pages(index);
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
        if self.tasks[index].leader == index {
            self.teardown_process(index);
        }
        let _ = self.kernel.exit(tcb);
        self.tasks[index].exited = true;
        self.tasks[index].exit_code = code;
        self.tasks[index].pending = Pending::None;
        self.current[cpu] = None;
        self.remove_sleep(index);
        self.wake_joiners(tcb);
        if self.tasks[index].leader == index {
            self.exit_threads(index);
        }
        self.platform.release_pages(index);
        true
    }

    /// 唤醒到期的休眠任务。
    /// 登记休眠；同一任务只保留一个 deadline。
    pub(crate) fn add_sleep(&mut self, index: usize, wake: u64) {
        let entries = self.sleeping.entry(wake).or_default();
        if !entries.contains(&index) {
            entries.push(index);
        }
    }

    /// 从 deadline 表移除任务。
    pub(crate) fn remove_sleep(&mut self, index: usize) {
        self.sleeping.retain(|_, entries| {
            entries.retain(|slot| *slot != index);
            !entries.is_empty()
        });
        self.timeouts.retain(|_, entries| {
            entries.retain(|slot| *slot != index);
            !entries.is_empty()
        });
    }

    fn wake_sleepers(&mut self) {
        let now = self.contract_now();
        while let Some((&deadline, _)) = self.sleeping.first_key_value() {
            if deadline > now {
                break;
            }
            let Some(entries) = self.sleeping.remove(&deadline) else {
                break;
            };
            for index in entries {
                let Some(task) = self.tasks.get(index) else {
                    continue;
                };
                if task.exited {
                    continue;
                }
                match task.pending {
                    Pending::Sleep { wake } if now >= wake => {
                        self.kernel.wake_thread(task.tcb);
                    }
                    Pending::Sleep { wake } => self.add_sleep(index, wake),
                    _ => {}
                }
            }
        }
    }

    fn wake_timeouts(&mut self) {
        let now = self.contract_now();
        while let Some((&deadline, _)) = self.timeouts.first_key_value() {
            if deadline > now {
                break;
            }
            let Some(entries) = self.timeouts.remove(&deadline) else {
                break;
            };
            for index in entries {
                let Some((tcb, pending, exited)) = self
                    .tasks
                    .get(index)
                    .map(|task| (task.tcb, task.pending, task.exited))
                else {
                    continue;
                };
                if exited {
                    continue;
                }
                match pending {
                    Pending::RecvData {
                        handle,
                        deadline: Some(at),
                        ..
                    } if at <= now => {
                        // `Ok(false)` 表示发送方已认领该接收等待（消息正在交付），
                        // 或消息已进入待取队列：不按超时处理，交给 prepare 消费。
                        match self.kernel.cancel_recv(tcb, handle) {
                            Ok(true) | Err(_) => {
                                self.tasks[index].context.set_result(u64::MAX);
                                self.tasks[index].pending = Pending::None;
                            }
                            Ok(false) => {}
                        }
                        self.kernel.wake_thread(tcb);
                    }
                    Pending::RecvData {
                        deadline: Some(at), ..
                    } => {
                        self.timeouts.entry(at).or_default().push(index);
                    }
                    Pending::Call {
                        deadline: Some(at), ..
                    } if at <= now => {
                        // 回复已到达时不要丢弃：交给 prepare 取回结果。
                        if self.kernel.cancel_call(tcb) {
                            self.tasks[index].context.set_result(u64::MAX);
                            self.tasks[index].pending = Pending::None;
                        }
                        self.kernel.wake_thread(tcb);
                    }
                    Pending::Call {
                        deadline: Some(at), ..
                    } => {
                        self.timeouts.entry(at).or_default().push(index);
                    }
                    _ => {}
                }
            }
        }
    }

    fn next_task(&mut self, cpu: usize) -> Option<usize> {
        let tcb = self.kernel.schedule(cpu)?;
        self.tasks.iter().position(|task| task.tcb == tcb)
    }

    /// 返回当前契约时钟（微秒）。平台没有 TSC 频率时按调度 tick 的 10 ms 折算。
    pub(crate) fn contract_now(&self) -> u64 {
        let frequency = self.platform.tsc_frequency();
        if frequency == 0 {
            return self.platform.timer_ticks().saturating_mul(10_000);
        }
        self.contract_ticks(self.platform.timestamp())
    }

    /// 计算同类 Task 的平均虚拟进度，用于恢复时重定基。
    fn average_virtual_pass(&self, class: hk_cadence::CadenceClass) -> u64 {
        let mut total = 0_u64;
        let mut count = 0_u64;
        for task in &self.tasks {
            if task.exited {
                continue;
            }
            let Some(cadence) = self.kernel.task_cadence_snapshot(task.tcb) else {
                continue;
            };
            if cadence.class != class {
                continue;
            }
            if let Some(meter) = self.kernel.task_meter_snapshot(task.tcb) {
                total = total.saturating_add(meter.virtual_pass);
                count += 1;
            }
        }
        total.checked_div(count).unwrap_or(0)
    }

    /// 返回 `cpu` 下一次需要唤醒的契约时刻：本地 Grant 结束、预算补充或 sleep 到期。
    pub fn next_wakeup(&self, cpu: usize) -> Option<u64> {
        let mut earliest = self.kernel.next_deadline(cpu);
        if let Some((&deadline, _)) = self.sleeping.first_key_value() {
            earliest = Some(earliest.map_or(deadline, |value: u64| value.min(deadline)));
        }
        if let Some((&deadline, _)) = self.timeouts.first_key_value() {
            earliest = Some(earliest.map_or(deadline, |value: u64| value.min(deadline)));
        }
        earliest
    }

    /// 把 `cpu` 的下一次唤醒写给平台；没有待办时只保留 watchdog。
    pub fn arm_wakeup(&self, cpu: usize) {
        self.platform.arm_wakeup(self.next_wakeup(cpu));
    }

    /// 把平台周期数换算为契约 tick。
    fn contract_ticks(&self, cycles: u64) -> u64 {
        let frequency = self.platform.tsc_frequency();
        if frequency == 0 {
            return 0;
        }
        let micros =
            (u128::from(cycles) * 1_000_000 / u128::from(frequency)).min(u128::from(u64::MAX));
        micros as u64
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
                deadline,
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
                        self.remove_sleep(index);
                    }
                    Ok(None) => {
                        if deadline.is_some_and(|at| self.contract_now() >= at) {
                            // 超时兜底：撤销本次登记，避免悬挂接收者。
                            let _ = self.kernel.cancel_recv(tcb, handle);
                            self.tasks[index].context.set_result(u64::MAX);
                        } else {
                            blocked_again = true;
                        }
                    }
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
            Pending::Call {
                endpoint, started, ..
            } => {
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
                        self.remove_sleep(index);
                    }
                    None => blocked_again = true,
                }
            }
            Pending::ProviderFault {
                capability,
                index: page_index,
                page,
                rights,
                flags,
            } => {
                let caller = self.tasks[index].tcb;
                let owner = self.leader_of(index);
                let owner_tcb = self.tasks[owner].tcb;
                if let Some(message) = self.kernel.take_reply(caller) {
                    let frame_handle = message.handles[0];
                    let frame = frame_handle.and_then(|handle| {
                        let frame = self.kernel.take_owned_frame(caller, handle).ok();
                        let _ = self.kernel.close_handle(caller, handle);
                        frame
                    });
                    if let Some(frame) = frame {
                        let range = frame.range();
                        let stored = self
                            .kernel
                            .set_capability_page(
                                owner_tcb,
                                capability,
                                page_index,
                                hk_mem::hmp::FrameBacking::Owned(frame),
                            )
                            .unwrap_or(false);
                        let mapped = stored
                            && self.platform.map_user(
                                index,
                                page,
                                range.start().get(),
                                range.len() as usize,
                                rights,
                                flags,
                            );
                        if !mapped {
                            return self.fail_provider(cpu, index, pending);
                        }
                        // Provider 提供页内容；内核只安装 PTE，不能再次清零。
                    } else {
                        return self.fail_provider(cpu, index, pending);
                    }
                } else {
                    return self.fail_provider(cpu, index, pending);
                }
            }
            Pending::None | Pending::Send { .. } | Pending::Sleep { .. } => {}
        }

        if blocked_again {
            self.tasks[index].pending = pending;
            self.current[cpu] = None;
        }

        blocked_again
    }

    /// Provider 回复缺失或安装失败：按缺页故障终止，而不是消费回复后永久阻塞。
    fn fail_provider(&mut self, cpu: usize, index: usize, pending: Pending) -> bool {
        if !self.fault(cpu, PROVIDER_FAULT_EXIT) {
            self.tasks[index].pending = pending;
            self.current[cpu] = None;
        }
        true
    }
}
