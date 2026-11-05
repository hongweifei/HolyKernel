//! 服务启动与重启策略。
//!
//! 平台只负责把服务名装载为用户现场；运行检查、父任务解析、运行时启动与服务表
//! 更新在这里统一处理。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_hal::{SyscallFrame, UserContext};

use hk_runtime::UserPlatform;

use crate::{RuntimeHost, ServiceTable};

/// 服务退出后的重启策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartPolicy {
    /// 不自动重启。
    Never,
    /// 非零退出码时重启。
    OnFailure,
    /// 退出即重启。
    Always,
}

/// 返回服务的默认重启策略。
pub fn restart_policy(name: &str) -> RestartPolicy {
    match name {
        "hk-server-device" | "hk-server-virtio" | "hk-server-virtio-net" => RestartPolicy::Always,
        "hk-server-display" => RestartPolicy::OnFailure,
        _ => RestartPolicy::Never,
    }
}

/// 服务镜像装载器。
pub trait ServiceLoader<C> {
    /// 把服务装载为初始用户现场；`slot` 是运行时任务槽位。
    fn load(&mut self, name: &str, slot: usize) -> Result<C, String>;

    /// 返回服务镜像声明的角色；未知时返回 0。
    fn role(&self, _name: &str) -> u32 {
        0
    }
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
    launch(host, services, loader, name, parent, "已启动", true)
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
    let Some(task) = host
        .tasks()
        .into_iter()
        .rev()
        .find(|task| task.name == name)
    else {
        return format!("未知服务：{name}\n");
    };
    if task.state != "exited" {
        return format!("仍在运行：{name}\n");
    }
    launch(host, services, loader, name, parent, "已重启", true)
}

fn launch<C, P, L>(
    host: &RuntimeHost<C, P>,
    services: &mut ServiceTable,
    loader: &mut L,
    name: &str,
    parent: Option<&str>,
    verb: &str,
    run: bool,
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

    let role = loader.role(name);
    let device_authority =
        services.device_authority(name) || role & hk_syscall::ServiceRole::DEVICE != 0;
    if let Err(error) = host.start(name, device_authority, context, parent) {
        return format!("启动失败：{error}\n");
    }

    if device_authority {
        services.set_device_authority(name, true);
    }
    services.mark_running(name);
    if run {
        host.run(services);
    }
    format!("{verb}：{name}\n")
}

/// 按重启策略监督已退出的服务；返回重启数量。
pub fn supervise<C, P, L>(
    host: &RuntimeHost<C, P>,
    services: &mut ServiceTable,
    loader: &mut L,
    policy: fn(&str) -> RestartPolicy,
) -> usize
where
    C: UserContext + SyscallFrame,
    P: UserPlatform,
    L: ServiceLoader<C>,
{
    host.drain_exits(services);
    let tasks = host.tasks();
    let mut seen: Vec<&str> = Vec::new();
    let mut restarted = 0;
    for task in tasks.iter().rev() {
        if seen.contains(&task.name.as_str()) {
            continue;
        }
        seen.push(task.name.as_str());
        if task.state != "exited" {
            continue;
        }
        let should = match policy(&task.name) {
            RestartPolicy::Never => false,
            RestartPolicy::OnFailure => task.exit_code != Some(0),
            RestartPolicy::Always => true,
        };
        if should
            && launch(host, services, loader, &task.name, None, "已重启", false)
                .starts_with("已重启")
        {
            restarted += 1;
        }
    }
    restarted
}
