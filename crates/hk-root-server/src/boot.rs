//! 用户态运行时装配：按服务清单创建 TCB、端点、句柄与任务条目。
//!
//! 装配逻辑与服务表一致，与具体平台无关；平台只提供用户现场与 Arena 布局。

use alloc::vec::Vec;

use hk_hal::{SyscallFrame, UserContext};
use hk_handle::Rights;
use hk_kernel::{Kernel, KernelError};

use crate::runtime::{UserPlatform, UserRuntime, UserTask};
use crate::{RootServer, ServiceSpec};

/// 用服务清单装配用户态运行时。
///
/// `make_context` 为每个服务生成初始用户现场；服务入口的首个参数是内核分配的
/// 端点编号。
pub fn user_runtime<C, P, F>(
    names: &[&'static str],
    platform: P,
    mut make_context: F,
) -> Result<UserRuntime<C, P>, KernelError>
where
    C: UserContext + SyscallFrame,
    P: UserPlatform,
    F: FnMut(usize, u64) -> C,
{
    let kernel = Kernel::new();
    let specs: Vec<ServiceSpec> = names.iter().map(|name| ServiceSpec::new(name)).collect();
    let server = RootServer::boot_parked(&kernel, &specs)?;
    let root = server.root().get();

    let mut endpoints = Vec::with_capacity(names.len());
    let mut notifications = Vec::with_capacity(names.len());
    let mut own_notifications = Vec::with_capacity(names.len());
    let mut tasks = Vec::with_capacity(names.len());

    for service in server.services() {
        let handle = kernel
            .create_notification(service.tcb, Rights::READ | Rights::WRITE | Rights::TRANSFER)?;
        notifications.push(Some(kernel.handle_id(service.tcb, handle)?));
        own_notifications.push(handle);
    }

    for (index, service) in server.services().iter().enumerate() {
        endpoints.push(Some(service.endpoint));
        let mut handles = Vec::with_capacity(names.len());
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

        let mut notification_handles = Vec::with_capacity(names.len());
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
        let mut task = UserTask::new(
            service.tcb,
            names[index],
            Some(root),
            handles,
            notification_handles,
            context,
            arena_base,
            arena_size,
        );
        task.device_authority = names[index] == crate::DEVICE_MANAGER_SERVICE;
        tasks.push(task);
    }

    kernel.exit(server.root())?;
    Ok(UserRuntime::new(
        kernel,
        root,
        endpoints,
        notifications,
        tasks,
        platform,
    ))
}
