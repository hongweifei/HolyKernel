//! 服务启动与重启策略。
//!
//! 平台只负责把服务名装载为用户现场；运行检查、父任务解析、运行时启动与服务表
//! 更新在这里统一处理。

use alloc::format;
use alloc::string::String;

use hk_hal::{SyscallFrame, UserContext};

use crate::{RuntimeHost, ServiceTable, UserPlatform};

/// 服务镜像装载器。
pub trait ServiceLoader<C> {
    /// 把服务装载为初始用户现场；`slot` 是运行时任务槽位。
    fn load(&mut self, name: &str, slot: usize) -> Result<C, String>;
}

/// 启动服务；已在运行时直接返回提示。
pub fn start<C, P, L>(
    host: &RuntimeHost<C, P>,
    services: &mut ServiceTable,
    loader: &mut L,
    name: &str,
    parent: Option<&str>,
) -> String
where
    C: UserContext + SyscallFrame,
    P: UserPlatform,
    L: ServiceLoader<C>,
{
    let running = host
        .tasks()
        .iter()
        .any(|task| task.name == name && task.state != "exited");
    if running {
        return format!("已在运行：{name}\n");
    }
    launch(host, services, loader, name, parent, "已启动")
}

/// 重启已退出的服务。
pub fn restart<C, P, L>(
    host: &RuntimeHost<C, P>,
    services: &mut ServiceTable,
    loader: &mut L,
    name: &str,
    parent: Option<&str>,
) -> String
where
    C: UserContext + SyscallFrame,
    P: UserPlatform,
    L: ServiceLoader<C>,
{
    let Some(task) = host.tasks().into_iter().find(|task| task.name == name) else {
        return format!("未知服务：{name}\n");
    };
    if task.state != "exited" {
        return format!("仍在运行：{name}\n");
    }
    launch(host, services, loader, name, parent, "已重启")
}

fn launch<C, P, L>(
    host: &RuntimeHost<C, P>,
    services: &mut ServiceTable,
    loader: &mut L,
    name: &str,
    parent: Option<&str>,
    verb: &str,
) -> String
where
    C: UserContext + SyscallFrame,
    P: UserPlatform,
    L: ServiceLoader<C>,
{
    let parent = parent
        .and_then(|name| host.tcb_of(name))
        .or(Some(host.root()));
    let slot = host.task_count();
    let context = match loader.load(name, slot) {
        Ok(context) => context,
        Err(error) => return format!("装载失败：{error}\n"),
    };

    let device_authority = services.device_authority(name);
    if let Err(error) = host.start(name, device_authority, context, parent) {
        return format!("启动失败：{error}\n");
    }

    services.mark_running(name);
    host.run(services);
    format!("{verb}：{name}\n")
}
