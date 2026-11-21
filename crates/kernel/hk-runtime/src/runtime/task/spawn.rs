//! 任务装载与端点视图。

use alloc::string::String;
use alloc::vec::Vec;

use hk_cadence::CadenceSpec;
use hk_hal::{SyscallFrame, UserContext};
use hk_handle::{Handle, HandleTable, ObjectId, Rights};
use hk_kernel::{KernelError, TcbId};

use crate::runtime::{Process, TaskInit, UserPlatform, UserRegionSpec, UserRuntime, UserTask};

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 装载新任务，返回分配的端点编号。
    pub fn start(
        &mut self,
        name: &str,
        device_authority: bool,
        cadence: CadenceSpec,
        context: C,
        parent: Option<u64>,
    ) -> Result<u64, KernelError> {
        self.start_at(
            self.tasks.len(),
            name,
            device_authority,
            cadence,
            context,
            parent,
        )
    }

    /// 返回可复用的任务槽位：同名且已退出的主线程槽位，否则追加新槽位。
    pub fn reuse_slot(&self, name: &str) -> usize {
        self.tasks
            .iter()
            .enumerate()
            .rev()
            .find(|(index, task)| task.exited && task.leader == *index && task.name == name)
            .map_or(self.tasks.len(), |(index, _)| index)
    }

    /// 在指定槽位装载任务；已退出的槽位会被复用。
    pub fn start_at(
        &mut self,
        index: usize,
        name: &str,
        device_authority: bool,
        cadence: CadenceSpec,
        context: C,
        parent: Option<u64>,
    ) -> Result<u64, KernelError> {
        self.start_at_with_regions(index, name, device_authority, cadence, context, parent, &[])
    }

    /// 在指定槽位装载任务，并登记装载器交付的 Region。
    #[allow(clippy::too_many_arguments)]
    pub fn start_at_with_regions(
        &mut self,
        index: usize,
        name: &str,
        device_authority: bool,
        cadence: CadenceSpec,
        mut context: C,
        parent: Option<u64>,
        regions: &[UserRegionSpec],
    ) -> Result<u64, KernelError> {
        let reuse = index < self.tasks.len();
        if index > self.tasks.len()
            || (reuse && (!self.tasks[index].exited || self.tasks[index].leader != index))
        {
            return Err(KernelError::Hmp(hk_mem::hmp::HmpError::InvalidSize));
        }
        let endpoint = index as u64 + 1;
        context.set_start_argument(endpoint);

        let kernel = &self.kernel;
        let parent_task = parent.and_then(|number| kernel.task_of(TcbId::new(number)));
        let tcb = kernel.spawn_task_parked_with_parent(
            HandleTable::with_default_quota(),
            cadence,
            parent_task,
        )?;
        let own = kernel.create_endpoint(tcb, Rights::READ | Rights::WRITE | Rights::TRANSFER)?;
        let own_id = kernel.handle_id(tcb, own)?;
        let own_notification =
            kernel.create_notification(tcb, Rights::READ | Rights::WRITE | Rights::TRANSFER)?;
        let own_notification_id = kernel.handle_id(tcb, own_notification)?;
        let device_factory = if device_authority {
            Some(kernel.create_device_grant_factory(tcb, Rights::READ | Rights::TRANSFER)?)
        } else {
            None
        };

        if reuse {
            self.endpoints[index] = Some(own_id);
            self.notifications[index] = Some(own_notification_id);
        } else {
            self.endpoints.push(Some(own_id));
            self.notifications.push(Some(own_notification_id));
        }
        let (handles, notifications) =
            self.endpoint_views(tcb, index, Some(own), Some(own_notification));
        if reuse {
            self.replace_endpoint(index, own_id, own_notification_id);
        } else {
            self.append_endpoint(own_id, own_notification_id);
        }

        let (arena_base, arena_size) = self.platform.arena_layout(index);
        // 与 `boot` 同一规则：共享 User Capacity 只授予分配所需权利，ADMIN 会
        // 允许单个服务拆分/合并全局用户容量；独占子容量保留全套权利。
        let shared_rights = Rights::READ | Rights::WRITE;
        let owned_rights = Rights::READ | Rights::WRITE | Rights::TRANSFER | Rights::ADMIN;
        let capacity_handle = match crate::service_capacity(device_authority) {
            crate::CapacityGrant::Split(kind, bytes) => {
                let capacity = self
                    .platform
                    .reserve_capacity(bytes, kind)
                    .ok_or(KernelError::Hmp(hk_mem::hmp::HmpError::OutOfCapacity))?;
                self.kernel.create_capacity(tcb, capacity, owned_rights)?
            }
            crate::CapacityGrant::User => {
                let capacity = self
                    .platform
                    .user_capacity()
                    .ok_or(KernelError::Hmp(hk_mem::hmp::HmpError::OutOfCapacity))?;
                self.kernel
                    .create_capacity_shared(tcb, capacity, shared_rights)?
            }
        };
        let ledger_limit = self
            .kernel
            .capacity_ref(tcb, capacity_handle)
            .map(|capacity| capacity.lock().limit())
            .unwrap_or_else(|_| self.platform.memory_limit(index));
        let mut process = Process::new(arena_base, arena_size, ledger_limit, capacity_handle);
        process.device_factory = device_factory;
        self.attach_task_ledger(tcb, &process.ledger);
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
        // 复用已退出槽位表示一次重启；计数供服务发现与重启测试观察。
        if reuse {
            task.restarts = self.tasks[index].restarts.saturating_add(1);
        }
        task.capacities.push(Some(capacity_handle));
        task.context.set_start_capacity(
            crate::runtime::CAPACITY_HANDLE_BASE as u64 + task.capacities.len() as u64,
        );
        if reuse {
            self.tasks[index] = task;
            self.processes[index] = Some(process);
        } else {
            self.tasks.push(task);
            self.processes.push(Some(process));
        }
        self.debug_task_push(if reuse { "spawn-reuse" } else { "spawn" }, index);
        self.register_user_regions(index, regions)?;
        if reuse {
            self.replay_image_provider(index);
        }
        self.notify_service_started(name);
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
    ///
    /// 同一 Task 的线程共享句柄表：每个 Task（以主线程为代表）只授予一次，
    /// 该 Task 的所有线程视图复用同一组句柄编号。
    pub(super) fn append_endpoint(&mut self, endpoint: ObjectId, notification: ObjectId) {
        let rights = Rights::READ | Rights::WRITE | Rights::TRANSFER;
        let mut leaders = Vec::new();
        for (index, task) in self.tasks.iter().enumerate() {
            if task.exited || task.leader != index {
                continue;
            }
            leaders.push((
                index,
                self.kernel.grant(task.tcb, endpoint, rights).ok(),
                self.kernel.grant(task.tcb, notification, rights).ok(),
            ));
        }
        for task in &mut self.tasks {
            if task.exited {
                continue;
            }
            if let Some((_, endpoint, notification)) =
                leaders.iter().find(|(leader, _, _)| *leader == task.leader)
            {
                task.handles.push(*endpoint);
                task.notifications.push(*notification);
            }
        }
    }

    /// 在已有槽位替换端点，并同步其他任务的端点视图。
    pub(super) fn replace_endpoint(
        &mut self,
        slot: usize,
        endpoint: ObjectId,
        notification: ObjectId,
    ) {
        let rights = Rights::READ | Rights::WRITE | Rights::TRANSFER;
        let mut updates = Vec::new();
        for (index, task) in self.tasks.iter().enumerate() {
            if index == slot || task.exited || task.leader != index {
                continue;
            }
            if let Some(old) = task.handles.get(slot).and_then(|handle| *handle) {
                let _ = self.kernel.close_handle(task.tcb, old);
            }
            if let Some(old) = task.notifications.get(slot).and_then(|handle| *handle) {
                let _ = self.kernel.close_handle(task.tcb, old);
            }
            updates.push((
                index,
                self.kernel.grant(task.tcb, endpoint, rights).ok(),
                self.kernel.grant(task.tcb, notification, rights).ok(),
            ));
        }
        for task in &mut self.tasks {
            if task.exited || task.leader == slot {
                continue;
            }
            let Some((_, endpoint, notification)) =
                updates.iter().find(|(leader, _, _)| *leader == task.leader)
            else {
                continue;
            };
            if let Some(handle) = task.handles.get_mut(slot) {
                *handle = *endpoint;
            } else {
                task.handles.push(*endpoint);
            }
            if let Some(handle) = task.notifications.get_mut(slot) {
                *handle = *notification;
            } else {
                task.notifications.push(*notification);
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
}
