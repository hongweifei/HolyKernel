//! 用户态运行时装配：按服务清单创建 TCB、端点、句柄与任务条目。
//!
//! 装配逻辑与服务表一致，与具体平台无关；平台只提供用户现场与 Arena 布局。

use alloc::string::String;
use alloc::vec::Vec;

use hk_hal::{SyscallFrame, UserContext};
use hk_handle::Rights;
use hk_kernel::{Kernel, KernelError};

use crate::runtime::{Process, TaskInit, UserPlatform, UserRuntime, UserTask};
use crate::{CapacityGrant, RootServer, ServiceSpec};

/// 用服务清单装配用户态运行时。
///
/// `make_context` 为每个服务生成初始用户现场；服务入口的首个参数是内核分配的
/// 端点编号。
pub fn user_runtime<C, P, F>(
    specs: &[ServiceSpec],
    platform: P,
    mut make_context: F,
) -> Result<UserRuntime<C, P>, KernelError>
where
    C: UserContext + SyscallFrame,
    P: UserPlatform,
    F: FnMut(usize, u64) -> C,
{
    let kernel = Kernel::new();
    let server = RootServer::boot_parked(&kernel, specs)?;
    let root = server.root().get();

    let mut endpoints = Vec::with_capacity(specs.len());
    let mut notifications = Vec::with_capacity(specs.len());
    let mut own_notifications = Vec::with_capacity(specs.len());
    // 预留给桌面高峰（线程/重启）留足槽位：任务表在运行期不再扩容，避免
    // 扩容搬移期间旧缓冲区被其它路径继续引用。
    let mut tasks = Vec::with_capacity((specs.len() * 4).max(64));
    let mut processes = Vec::with_capacity(specs.len());

    for service in server.services() {
        let handle = kernel
            .create_notification(service.tcb, Rights::READ | Rights::WRITE | Rights::TRANSFER)?;
        notifications.push(Some(kernel.handle_id(service.tcb, handle)?));
        own_notifications.push(handle);
    }

    for (index, service) in server.services().iter().enumerate() {
        endpoints.push(Some(service.endpoint));
        let mut handles = Vec::with_capacity(specs.len());
        for (slot, endpoint) in server.services().iter().enumerate() {
            let handle = if slot == index {
                service.handle
            } else {
                kernel.grant(
                    service.tcb,
                    endpoint.endpoint,
                    Rights::READ | Rights::WRITE | Rights::TRANSFER,
                )?
            };
            handles.push(Some(handle));
        }

        let mut notification_handles = Vec::with_capacity(specs.len());
        for (slot, notification) in notifications.iter().enumerate() {
            let handle = match notification {
                Some(id) if slot != index => kernel.grant(
                    service.tcb,
                    *id,
                    Rights::READ | Rights::WRITE | Rights::TRANSFER,
                )?,
                _ => own_notifications[index],
            };
            notification_handles.push(Some(handle));
        }

        let (arena_base, arena_size) = platform.arena_layout(index);
        let context = make_context(index, index as u64 + 1);
        let factory = if specs[index].device_authority {
            Some(kernel.create_device_grant_factory(service.tcb, Rights::READ | Rights::TRANSFER)?)
        } else {
            None
        };
        // 共享的顶层 User Capacity 只授予分配所需权利：ADMIN 会允许任一服务
        // 拆分/合并这同一份容量，永久削减其他服务与 Memory Server 的配额。
        let shared_rights = Rights::READ | Rights::WRITE;
        // 服务自己的子容量（设备 Dma 等）由它独占，保留拆分与转移能力。
        let owned_rights = Rights::READ | Rights::WRITE | Rights::TRANSFER | Rights::ADMIN;
        let handle = match specs[index].capacity {
            CapacityGrant::Split(kind, bytes) => {
                let capacity = platform
                    .reserve_capacity(bytes, kind)
                    .ok_or(KernelError::Hmp(hk_mem::hmp::HmpError::OutOfCapacity))?;
                kernel.create_capacity(service.tcb, capacity, owned_rights)?
            }
            CapacityGrant::User => {
                let capacity = platform
                    .user_capacity()
                    .ok_or(KernelError::Hmp(hk_mem::hmp::HmpError::OutOfCapacity))?;
                kernel.create_capacity_shared(service.tcb, capacity, shared_rights)?
            }
        };
        let capacity_ref = kernel.capacity_ref(service.tcb, handle).ok();
        if let (Some(limit), Some(capacity)) = (specs[index].soft_limit, capacity_ref.as_ref()) {
            capacity.lock().set_soft_limit(limit);
        }
        let ledger_limit = capacity_ref
            .as_ref()
            .map(|capacity| capacity.lock().limit())
            .unwrap_or_else(|| platform.memory_limit(index));
        let mut process = Process::new(arena_base, arena_size, ledger_limit, handle);
        process.device_factory = factory;
        // 把同一本 Task Ledger 挂到内核 Task：句柄转移在内核侧事务里记账。
        if let Some(task) = kernel.task_of(service.tcb) {
            kernel.set_task_ledger(task, process.ledger.clone())?;
        }
        let mut task = UserTask::new(TaskInit {
            tcb: service.tcb,
            process: index,
            name: String::from(specs[index].name),
            parent: Some(root),
            handles,
            notifications: notification_handles,
            context,
        });
        task.capacities.push(Some(handle));
        task.context.set_start_capacity(
            crate::runtime::CAPACITY_HANDLE_BASE as u64 + task.capacities.len() as u64,
        );
        tasks.push(task);
        processes.push(Some(process));
    }

    kernel.exit(server.root())?;
    Ok(UserRuntime::new(
        kernel,
        root,
        endpoints,
        notifications,
        tasks,
        processes,
        platform,
    ))
}
