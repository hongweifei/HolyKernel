//! 内核态 Shell 宿主。
//!
//! 命令解析与输出由 `hk-shell` 提供；本 crate 把运行时视图、服务表、装载器与
//! 调度回调适配成 `ShellHost`。平台只提供控制台 I/O、服务装载与调度副作用。

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_hal::{SyscallFrame, UserContext};
use hk_root_server::service::{self, ServiceLoader};
use hk_root_server::{RuntimeHost, ServiceTable, UserPlatform};
use hk_shell::{RuntimeStats, ServiceInfo, Session, ShellHost, ShellIo, TaskInfo, TraceEvent};

/// 从启动模块读取 HolyShell 脚本。
pub trait ScriptSource {
    /// 按名称读取脚本；名称可省略 `.hsh` 后缀。
    fn script(&self, name: &str) -> Result<String, String>;
}

/// 平台调度回调，供内核态 Shell 的 `net` 扩展驱动调度。
pub trait Scheduling {
    /// 调度至多 `steps` 步；返回是否已无可运行线程。
    fn run_budgeted(&mut self, services: &mut ServiceTable, steps: usize) -> bool;
}

/// 内核态 Shell 宿主。
pub struct Host<'a, C, P, L, S> {
    runtime: &'a RuntimeHost<C, P>,
    services: &'a mut ServiceTable,
    loader: &'a mut L,
    scheduler: &'a mut S,
}

impl<C, P, L, S> ShellHost for Host<'_, C, P, L, S>
where
    C: UserContext + SyscallFrame,
    P: UserPlatform,
    L: ServiceLoader<C> + ScriptSource,
    S: Scheduling,
{
    fn tasks(&self) -> Vec<TaskInfo> {
        self.runtime
            .tasks()
            .into_iter()
            .map(|task| TaskInfo {
                tcb: task.tcb,
                name: task.name,
                parent: task.parent,
                endpoint: task.endpoint,
                state: task.state,
                pending: task.pending,
                handles: task.handles,
                arena_used: task.arena_used,
                arena_size: task.arena_size,
                vspace: task.vspace,
                exit_code: task.exit_code,
            })
            .collect()
    }

    fn services(&mut self) -> Vec<ServiceInfo> {
        self.runtime.drain_exits(self.services);
        self.services
            .services()
            .iter()
            .map(|service| ServiceInfo {
                name: service.name.clone(),
                state: service.state.name(),
                restarts: service.restarts,
                exit_code: service.exit_code,
            })
            .collect()
    }

    fn trace(&self, limit: usize) -> Vec<TraceEvent> {
        self.runtime
            .trace(limit)
            .into_iter()
            .map(|event| TraceEvent {
                sequence: event.sequence,
                category: event.category.name(),
                thread: event.thread,
                code: event.code,
                argument: event.argument,
            })
            .collect()
    }

    fn stats(&self) -> RuntimeStats {
        let stats = self.runtime.stats();
        RuntimeStats {
            switches: stats.switches,
            switch_cycles: stats.switch_cycles,
            switch_max_cycles: stats.switch_max_cycles,
            calls: stats.calls,
            call_cycles: stats.call_cycles,
            call_max_cycles: stats.call_max_cycles,
            resumes: stats.resumes.to_vec(),
            tsc_frequency: stats.tsc_frequency,
        }
    }

    fn extension(&mut self, name: &str, args: &str) -> Option<String> {
        match name {
            "net" => Some(self.client_status("hk-server-netstat")),
            "replace" => Some(self.replace(args)),
            "checkpoint" => Some(self.checkpoint(args)),
            "restore-checkpoint" => Some(self.restore_checkpoint(args)),
            _ => None,
        }
    }

    fn help_extra(&self) -> String {
        String::from(
            "扩展：net、replace <服务> <镜像>、checkpoint <任务>、restore-checkpoint <任务>\n",
        )
    }

    fn start(&mut self, name: &str, parent: Option<&str>) -> String {
        service::start(self.runtime, self.services, self.loader, name, parent)
    }

    fn restart(&mut self, name: &str, parent: Option<&str>) -> String {
        service::restart(self.runtime, self.services, self.loader, name, parent)
    }

    fn script(&self, name: &str) -> Result<String, String> {
        self.loader.script(name)
    }
}

impl<C, P, L, S> Host<'_, C, P, L, S>
where
    C: UserContext + SyscallFrame,
    P: UserPlatform,
    L: ServiceLoader<C> + ScriptSource,
    S: Scheduling,
{
    /// 用另一镜像热替换服务。
    fn replace(&mut self, arguments: &str) -> String {
        let mut parts = arguments.split_whitespace();
        let Some(name) = parts.next() else {
            return String::from("用法：replace <服务> <镜像>\n");
        };
        let Some(module) = parts.next() else {
            return String::from("用法：replace <服务> <镜像>\n");
        };
        service::replace(self.runtime, self.services, self.loader, name, module, None)
    }

    /// 保存任务检查点。
    fn checkpoint(&mut self, arguments: &str) -> String {
        let name = arguments.trim();
        if name.is_empty() {
            return String::from("用法：checkpoint <任务>\n");
        }
        match self.runtime.checkpoint(name) {
            Ok(_) => {
                self.services.mark_exited(name, 0);
                format!("已检查点：{name}\n")
            }
            Err(error) => format!("检查点失败：{error}\n"),
        }
    }

    /// 恢复任务检查点并驱动调度。
    fn restore_checkpoint(&mut self, arguments: &str) -> String {
        let name = arguments.trim();
        if name.is_empty() {
            return String::from("用法：restore-checkpoint <任务>\n");
        }
        match self.runtime.restore_checkpoint(name) {
            Ok(_) => {
                self.services.mark_running(name);
                let _ = self.scheduler.run_budgeted(self.services, 4096);
                format!("已恢复：{name}\n")
            }
            Err(error) => format!("恢复失败：{error}\n"),
        }
    }

    /// 启动用户态客户端并驱动调度，等待它输出结果。
    fn client_status(&mut self, name: &str) -> String {
        let message = service::start(self.runtime, self.services, self.loader, name, None);
        if message.contains("失败") || message.contains("未知") {
            return message;
        }
        for _ in 0..64 {
            if self.scheduler.run_budgeted(self.services, 4096) {
                break;
            }
        }
        message
    }
}

/// 执行一组命令。
pub fn run<C, P, L, S>(
    io: &impl ShellIo,
    runtime: &RuntimeHost<C, P>,
    services: &mut ServiceTable,
    loader: &mut L,
    scheduler: &mut S,
    script: &[&str],
) where
    C: UserContext + SyscallFrame,
    P: UserPlatform,
    L: ServiceLoader<C> + ScriptSource,
    S: Scheduling,
{
    io.write("HolyShell\n");
    if script.is_empty() {
        io.write("当前仅支持脚本化执行：shell-test <命令>...\n");
        return;
    }
    let mut host = Host {
        runtime,
        services,
        loader,
        scheduler,
    };
    let mut session = Session::new();
    hk_shell::run_script(io, &mut host, &mut session, script);
}

/// 运行交互式 HolyShell。
pub fn interactive<C, P, L, S>(
    io: &impl ShellIo,
    runtime: &RuntimeHost<C, P>,
    services: &mut ServiceTable,
    loader: &mut L,
    scheduler: &mut S,
) where
    C: UserContext + SyscallFrame,
    P: UserPlatform,
    L: ServiceLoader<C> + ScriptSource,
    S: Scheduling,
{
    io.write("HolyShell 交互模式（help 查看命令）\n");
    let mut host = Host {
        runtime,
        services,
        loader,
        scheduler,
    };
    let mut session = Session::new();
    hk_shell::run_repl(io, &mut host, &mut session);
}
