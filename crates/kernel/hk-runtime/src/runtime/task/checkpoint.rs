//! 检查点捕获、恢复与镜像读取。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_cadence::{Cadence, CadenceClass, CadenceId, CadenceSpec};
use hk_checkpoint::Page;
use hk_hal::{SyscallFrame, UserContext, VirtAddr};
use hk_handle::{HandleTable, Rights};
use hk_kernel::{KernelError, TcbId};
use hk_mem::hmp::{Capability, CapabilityKind, Region};

use crate::runtime::{
    Checkpoint, CheckpointCapture, Process, TaskInit, UserPlatform, UserRuntime, UserTask,
};

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 捕获任务、序列化现场并终止任务。
    pub fn checkpoint(&mut self, name: &str) -> Result<usize, String> {
        let capture = self.capture_checkpoint(name)?;
        let mut context = alloc::vec![0_u8; C::SAVED_BYTES];
        capture.context.save(&mut context);
        let threads: Vec<hk_checkpoint::ThreadSnapshot> = capture
            .threads
            .iter()
            .map(|(thread, tls)| {
                let mut bytes = alloc::vec![0_u8; C::SAVED_BYTES];
                thread.save(&mut bytes);
                hk_checkpoint::ThreadSnapshot {
                    context: bytes,
                    tls: *tls,
                }
            })
            .collect();
        let image = hk_checkpoint::encode_parts(
            capture.slot as u32,
            name,
            &context,
            capture.parent,
            capture.arena_used,
            capture.cadence,
            capture.meter,
            &capture.pages,
            &threads,
            &capture.regions,
            &capture.objects,
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
        // 设备授权与 DMA 窗口不进入快照；类型化能力按 Region 元数据恢复为
        // 惰性能力，因此不再阻止检查点。
        if self.process(index).device_factory.is_some()
            || task.grants.iter().flatten().next().is_some()
        {
            return Err(String::from("任务持有设备或 DMA 窗口"));
        }
        let context = task.context;
        let parent = task.parent;
        let arena_used = self.process(index).arena_used();
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
        // 同一进程的附加线程按槽位顺序保存现场与 TLS。
        let threads: Vec<(C, u64)> = self
            .tasks
            .iter()
            .enumerate()
            .filter(|(other, entry)| *other != index && !entry.exited && entry.leader == index)
            .map(|(_, entry)| (entry.context, entry.tls))
            .collect();
        let mut regions = Vec::new();
        for region in self.process(index).regions.iter().flatten() {
            let Ok((capability, _)) = self.kernel.capability(region.owner, region.capability)
            else {
                return Err(String::from("无法读取 Region 能力"));
            };
            regions.push(hk_checkpoint::RegionSnapshot {
                virt: region.virt,
                offset: region.offset,
                len: region.len as u64,
                rights: region.rights.bits(),
                flags: region.flags.bits(),
                kind: capability.kind() as u8,
                capability_size: capability.size(),
            });
        }
        // 同一 Task 的线程共享句柄表，对象图只枚举主线程一次。
        let objects = self.object_records(index);
        Ok(CheckpointCapture {
            slot: index,
            context,
            parent,
            arena_used,
            cadence,
            meter,
            pages,
            threads,
            regions,
            objects,
        })
    }

    /// 终止任务、释放原地址空间，并保存序列化镜像。
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
            &snapshot.threads,
            &snapshot.regions,
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
        threads: &[hk_checkpoint::ThreadSnapshot],
        regions: &[hk_checkpoint::RegionSnapshot],
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
        // 恢复内核父链：快照记录的是父任务 TCB 编号。
        let parent_task = parent.and_then(|number| self.kernel.task_of(TcbId::new(number)));
        let tcb = self
            .kernel
            .spawn_task_with_cadence_and_parent(
                HandleTable::with_default_quota(),
                restored,
                parent_task,
            )
            .map_err(|error| format!("{error}"))?;
        if let Some(task) = self.kernel.task_of(tcb) {
            let mut meter = meter;
            meter.virtual_pass = base;
            let _ = self.kernel.restore_meter(task, meter);
            self.kernel.reset_budget_period(task, self.contract_now());
        }
        let own = self
            .kernel
            .create_endpoint(tcb, rights)
            .map_err(|error| format!("{error}"))?;
        let own_id = self
            .kernel
            .handle_id(tcb, own)
            .map_err(|error| format!("{error}"))?;
        let own_notification = self
            .kernel
            .create_notification(tcb, rights)
            .map_err(|error| format!("{error}"))?;
        let own_notification_id = self
            .kernel
            .handle_id(tcb, own_notification)
            .map_err(|error| format!("{error}"))?;
        self.endpoints[slot] = Some(own_id);
        self.notifications[slot] = Some(own_notification_id);

        self.replace_endpoint(slot, own_id, own_notification_id);
        let (handles, notifications) =
            self.endpoint_views(tcb, slot, Some(own), Some(own_notification));

        let (arena_base, arena_size) = self.platform.arena_layout(slot);
        let user_capacity = self
            .platform
            .user_capacity()
            .ok_or_else(|| String::from("缺少 User Capacity"))?;
        let capacity_handle = self
            .kernel
            .create_capacity_shared(
                tcb,
                user_capacity,
                Rights::READ | Rights::WRITE | Rights::TRANSFER | Rights::ADMIN,
            )
            .map_err(|error| format!("{error}"))?;
        let ledger_limit = self
            .kernel
            .capacity_ref(tcb, capacity_handle)
            .map(|capacity| capacity.lock().limit())
            .unwrap_or_else(|_| self.platform.memory_limit(slot));
        let mut process = Process::new(arena_base, arena_size, ledger_limit, capacity_handle);
        process.set_arena_used(arena_used);
        self.attach_task_ledger(tcb, &process.ledger);
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
        task.capacities.push(Some(capacity_handle));
        self.processes[slot] = Some(process);
        self.tasks[slot] = task;
        // 按 Region 元数据重建能力、映射描述与记账；页表已由 `restore_pages`
        // 还原，这里不再次安装 PTE。
        for descriptor in regions {
            self.restore_region(slot, *descriptor)
                .map_err(|error| format!("{error}"))?;
        }
        let mut restored_threads = Vec::new();
        for (offset, thread) in threads.iter().enumerate() {
            let Some(context) = C::load(&thread.context) else {
                self.abort_restore(slot, tcb, &restored_threads);
                return Err(String::from("线程现场损坏"));
            };
            let child = match self.kernel.spawn_thread_parked(
                self.kernel
                    .task_of(tcb)
                    .ok_or_else(|| String::from("缺少 Task"))?,
            ) {
                Ok(child) => child,
                Err(error) => {
                    self.abort_restore(slot, tcb, &restored_threads);
                    return Err(format!("{error}"));
                }
            };
            restored_threads.push(child);
            self.endpoints.push(None);
            self.notifications.push(None);
            // 线程共享主线程的句柄表，视图复用主线程的句柄编号。
            self.append_empty_endpoint();
            let handles = self.tasks[slot].handles.clone();
            let notifications = self.tasks[slot].notifications.clone();
            let mut entry = UserTask::new(TaskInit {
                tcb: child,
                process: slot,
                name: format!("{name}-thread{}", offset + 1),
                parent: Some(tcb.get()),
                handles,
                notifications,
                context,
            });
            entry.leader = slot;
            entry.tls = thread.tls;
            self.tasks.push(entry);
            self.processes.push(None);
            let _ = self.kernel.activate(child);
        }
        let _ = self.kernel.activate(tcb);
        Ok(slot)
    }

    /// 按 Region 元数据恢复一个映射视图。
    fn restore_region(
        &mut self,
        index: usize,
        descriptor: hk_checkpoint::RegionSnapshot,
    ) -> Result<(), KernelError> {
        let tcb = self.tasks[index].tcb;
        let kind = CapabilityKind::from_u8(descriptor.kind)
            .ok_or(KernelError::Hmp(hk_mem::hmp::HmpError::WrongCapability))?;
        let rights = Rights::from_bits_truncate(descriptor.rights);
        let flags = hk_mem::hmp::RegionFlags::from_bits_truncate(descriptor.flags);
        let len = usize::try_from(descriptor.len)
            .map_err(|_| KernelError::Hmp(hk_mem::hmp::HmpError::InvalidSize))?;
        let capability = Capability::new(kind, descriptor.capability_size, hk_hal::PAGE_SIZE)
            .map_err(KernelError::Hmp)?;
        let capability_handle = self.kernel.create_capability(
            tcb,
            capability.clone(),
            None,
            Rights::READ | Rights::WRITE | Rights::ADMIN,
        )?;
        let mut region = Region::new(&capability, descriptor.offset, len, rights, flags)
            .map_err(KernelError::Hmp)?;
        if region.map(VirtAddr::new(descriptor.virt)).is_err() {
            let _ = self.kernel.close_handle(tcb, capability_handle);
            return Err(KernelError::Hmp(hk_mem::hmp::HmpError::AlreadyMapped));
        }
        let region_id = region.id();
        if self.process_mut(index).hmp_arena.insert(&region).is_err() {
            let _ = self.kernel.close_handle(tcb, capability_handle);
            return Err(KernelError::Hmp(hk_mem::hmp::HmpError::AlreadyMapped));
        }
        let region_handle =
            self.kernel
                .create_region(tcb, region, Rights::READ | Rights::WRITE | Rights::ADMIN)?;
        let charge = self
            .charge_frame(index, descriptor.len)
            .map_err(KernelError::Hmp)?;
        self.install_region(
            index,
            region_handle,
            capability_handle,
            region_id,
            true,
            descriptor.virt,
            len,
            descriptor.offset,
            rights,
            flags,
            Some(charge),
        );
        Ok(())
    }

    /// 恢复失败时退出已创建的 TCB，避免留下半初始化任务。
    fn abort_restore(&mut self, slot: usize, tcb: TcbId, threads: &[TcbId]) {
        for thread in threads {
            let _ = self.kernel.exit(*thread);
        }
        let _ = self.kernel.exit(tcb);
        self.tasks[slot].exited = true;
        self.tasks[slot].exit_code = u64::MAX;
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
            &snapshot.threads,
            &snapshot.regions,
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
}
