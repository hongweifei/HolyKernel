//! Root Server 控制队列。
//!
//! 用户态只能投递请求，真正的服务表/装载器操作在 Root Server 的调度循环里执行。
//! 这样用户态 Shell 不持有内核对象，内核态 Shell 也不需要知道控制协议。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_root_server::{ServiceTable, TraceEvent};
use hk_shell::{RuntimeStats, ServiceInfo, TaskInfo, TraceEvent as ShellTrace};
use hk_syscall::{
    ROOT_OP_CHECKPOINT, ROOT_OP_CHECKPOINT_READ, ROOT_OP_CHECKPOINT_WRITE, ROOT_OP_HANDLES,
    ROOT_OP_LOOKUP, ROOT_OP_PERF, ROOT_OP_REPLACE, ROOT_OP_RESTART, ROOT_OP_RESTORE_CHECKPOINT,
    ROOT_OP_SCRIPT, ROOT_OP_SERVICES, ROOT_OP_START, ROOT_OP_STATS, ROOT_OP_SYMBOLIZE,
    ROOT_OP_TASKS, ROOT_OP_TRACE,
};
use spin::Mutex;

use crate::runtime;
use crate::shell::Loader;

/// 只有这个服务可以执行启动/重启/脚本读取。
const CONTROL_SHELL: &str = "hk-server-shell";
/// Checkpoint Server 可以保存/恢复任务。
const CONTROL_CHECKPOINT: &str = "hk-server-checkpoint";

/// 队列中待处理请求。
struct Request {
    id: u64,
    op: u64,
    argument: String,
    data: Vec<u8>,
}

/// 正在接收的检查点镜像。
struct Restore {
    name: String,
    total: usize,
    data: Vec<u8>,
}

static RESTORE: Mutex<Option<Restore>> = Mutex::new(None);

/// 控制队列与结果。
#[derive(Default)]
struct ControlState {
    next: u64,
    requests: Vec<Request>,
    results: Vec<(u64, Vec<u8>)>,
}

static CONTROL: Mutex<Option<ControlState>> = Mutex::new(None);

/// 投递请求并返回请求编号。
pub fn request(op: u64, argument: &str, caller: &str) -> u64 {
    enqueue(op, argument, Vec::new(), caller)
}

/// 投递带字节载荷的请求并返回请求编号。
pub fn request_data(op: u64, argument: &str, data: Vec<u8>, caller: &str) -> u64 {
    enqueue(op, argument, data, caller)
}

fn enqueue(op: u64, argument: &str, data: Vec<u8>, caller: &str) -> u64 {
    let mut guard = CONTROL.lock();
    let state = guard.get_or_insert_with(ControlState::default);
    state.next = state.next.wrapping_add(1);
    let id = state.next;

    let shell = caller == CONTROL_SHELL;
    let checkpoint = caller == CONTROL_CHECKPOINT || shell;
    let shell_operation = matches!(
        op,
        ROOT_OP_START | ROOT_OP_RESTART | ROOT_OP_REPLACE | ROOT_OP_SCRIPT
    );
    let checkpoint_operation = matches!(
        op,
        ROOT_OP_CHECKPOINT
            | ROOT_OP_RESTORE_CHECKPOINT
            | ROOT_OP_CHECKPOINT_READ
            | ROOT_OP_CHECKPOINT_WRITE
    );
    if (shell_operation && !shell) || (checkpoint_operation && !checkpoint) {
        push_result(state, id, b"permission denied\n".to_vec());
        return id;
    }

    state.requests.push(Request {
        id,
        op,
        argument: String::from(argument),
        data,
    });
    id
}

/// 取出已完成的控制结果；未完成时返回 `None`。
pub fn take_result(id: u64) -> Option<Vec<u8>> {
    let mut guard = CONTROL.lock();
    let state = guard.as_mut()?;
    let index = state
        .results
        .iter()
        .position(|(request, _)| *request == id)?;
    Some(state.results.remove(index).1)
}

/// 处理 Root Server 控制请求。
pub fn process(loader: &mut Loader<'_>, services: &mut ServiceTable) {
    for _ in 0..8 {
        let request = {
            let mut guard = CONTROL.lock();
            let Some(state) = guard.as_mut() else {
                return;
            };
            if state.requests.is_empty() {
                return;
            }
            state.requests.remove(0)
        };

        let bytes = execute(&request, loader, services);
        let mut guard = CONTROL.lock();
        if let Some(state) = guard.as_mut() {
            push_result(state, request.id, bytes);
        }
    }
}

fn execute(request: &Request, loader: &mut Loader<'_>, services: &mut ServiceTable) -> Vec<u8> {
    match request.op {
        ROOT_OP_CHECKPOINT => runtime::checkpoint_service(services, &request.argument),
        ROOT_OP_RESTORE_CHECKPOINT => {
            runtime::restore_checkpoint_service(services, &request.argument)
        }
        ROOT_OP_CHECKPOINT_READ => checkpoint_read(&request.argument),
        ROOT_OP_CHECKPOINT_WRITE => checkpoint_write(&request.argument, &request.data, services),
        _ => execute_text(request, loader, services).into_bytes(),
    }
}

fn checkpoint_read(argument: &str) -> Vec<u8> {
    let Some((name, offset)) = argument.split_once(' ') else {
        return Vec::new();
    };
    let Ok(offset) = offset.parse::<u64>() else {
        return Vec::new();
    };
    runtime::checkpoint_chunk(name, offset, 16 * 1024)
}

fn checkpoint_write(argument: &str, chunk: &[u8], services: &mut ServiceTable) -> Vec<u8> {
    let Some((name, total)) = argument.split_once(' ') else {
        return "恢复失败：参数错误\n".as_bytes().to_vec();
    };
    let Ok(total) = total.parse::<usize>() else {
        return "恢复失败：参数错误\n".as_bytes().to_vec();
    };
    if total == 0 || total > 64 * 1024 * 1024 {
        return "恢复失败：快照长度非法\n".as_bytes().to_vec();
    }

    let mut guard = RESTORE.lock();
    let restore = guard.get_or_insert_with(|| Restore {
        name: String::from(name),
        total,
        data: Vec::with_capacity(total),
    });
    if restore.name != name || restore.total != total {
        *guard = None;
        return "恢复失败：分块不匹配\n".as_bytes().to_vec();
    }
    restore.data.extend_from_slice(chunk);
    if restore.data.len() < total {
        return format!("已接收：{}/{}\n", restore.data.len(), total).into_bytes();
    }
    let data = core::mem::take(&mut restore.data);
    *guard = None;
    drop(guard);
    runtime::restore_checkpoint_bytes(services, &data)
}

fn execute_text(request: &Request, loader: &mut Loader<'_>, services: &mut ServiceTable) -> String {
    match request.op {
        ROOT_OP_TASKS => hk_shell::format::format_tasks(&tasks(), None),
        ROOT_OP_HANDLES => hk_shell::format::format_handles(&tasks()),
        ROOT_OP_SERVICES => hk_shell::format::format_services(&service_view(services)),
        ROOT_OP_TRACE => hk_shell::format::format_trace(&trace()),
        ROOT_OP_STATS => hk_shell::format::format_stats(&stats()),
        ROOT_OP_PERF => perf(),
        ROOT_OP_START => runtime::start_service(loader, services, &request.argument, None),
        ROOT_OP_RESTART => runtime::restart_service(loader, services, &request.argument, None),
        ROOT_OP_REPLACE => runtime::replace_service(loader, services, &request.argument),
        ROOT_OP_SCRIPT => script(loader, &request.argument),
        ROOT_OP_SYMBOLIZE => runtime::host().symbolize_list(&request.argument),
        ROOT_OP_LOOKUP => lookup(loader, &request.argument),
        _ => String::from("未知控制操作\n"),
    }
}

fn lookup(loader: &Loader<'_>, argument: &str) -> String {
    let (service, rest) = argument.split_once(' ').unwrap_or((argument, ""));
    let addresses: Vec<u64> = rest
        .split_whitespace()
        .filter_map(|token| u64::from_str_radix(token.trim_start_matches("0x"), 16).ok())
        .collect();
    let module_bytes = if service.is_empty() {
        None
    } else {
        module_for(loader, service)
    };
    let mut text = String::new();
    for address in addresses {
        let found = if service.is_empty() {
            hk_root_server::debug::lookup(address)
                .map(|(name, offset)| (offset, String::from(name)))
        } else {
            module_bytes.and_then(|bytes| hk_root_server::loader::lookup_symbol(bytes, address))
        };
        match found {
            Some((offset, name)) => text.push_str(&format!("{offset:x} {name}\n")),
            None => text.push_str("none\n"),
        }
    }
    text
}

/// 按任务名查找模块；fork 子任务名带 `-fork` 后缀，逐层回退到父服务。
fn module_for<'a>(loader: &Loader<'a>, mut name: &str) -> Option<&'a [u8]> {
    loop {
        if let Some(module) = loader
            .info
            .server_modules
            .iter()
            .find(|module| hk_root_server::loader::service_name(module.name) == name)
        {
            return loader.info.module_bytes(module);
        }
        name = name.strip_suffix("-fork")?;
    }
}

fn push_result(state: &mut ControlState, id: u64, data: Vec<u8>) {
    if state.results.len() >= 64 {
        state.results.remove(0);
    }
    state.results.push((id, data));
}

/// 性能快照：每 CPU 忙时与每任务 CPU/内存。
///
/// 文本格式稳定供任务管理器解析：
/// `perf cpus=.. tasks=.. runnable=.. tsc=.. ticks=..`
/// 之后每 CPU 一行 `cpu <编号> <忙周期> <恢复次数>`，每任务一行
/// `task <TCB> <CPU周期> <Arena已用> <Arena总量> <句柄> <状态> <父> <端点> <挂起> <名称>`。
fn perf() -> String {
    let stats = runtime::stats();
    let tasks = runtime::tasks();
    let cpus = stats
        .resumes
        .iter()
        .rposition(|count| *count > 0)
        .map_or(1, |index| index + 1);
    let runnable = tasks
        .iter()
        .filter(|task| matches!(task.state, "running" | "runnable"))
        .count();
    let mut text = format!(
        "perf cpus={} tasks={} runnable={} tsc={} ticks={}\n",
        cpus,
        tasks.len(),
        runnable,
        stats.tsc_frequency,
        hk_platform_x86_64::cpu::uptime_ticks(),
    );
    for cpu in 0..cpus {
        text.push_str(&format!(
            "cpu {} {} {}\n",
            cpu, stats.cpu_busy[cpu], stats.resumes[cpu]
        ));
    }
    for task in &tasks {
        let parent = match task.parent {
            Some(parent) => format!("{parent}"),
            None => String::from("-"),
        };
        let exit = match task.exit_code {
            Some(code) => format!("{code}"),
            None => String::from("-"),
        };
        text.push_str(&format!(
            "task {} {} {} {} {} {} {} {} {} {} {}\n",
            task.tcb,
            task.cpu_cycles,
            task.arena_used,
            task.arena_size,
            task.handles,
            task.state,
            parent,
            task.endpoint,
            task.pending,
            exit,
            task.name,
        ));
    }
    text
}

fn tasks() -> Vec<TaskInfo> {
    runtime::tasks()
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

fn service_view(services: &ServiceTable) -> Vec<ServiceInfo> {
    services
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

fn trace() -> Vec<ShellTrace> {
    runtime::trace(32)
        .into_iter()
        .map(|event: TraceEvent| ShellTrace {
            sequence: event.sequence,
            category: event.category.name(),
            thread: event.thread,
            code: event.code,
            argument: event.argument,
        })
        .collect()
}

fn stats() -> RuntimeStats {
    let stats = runtime::stats();
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

fn script(loader: &Loader<'_>, name: &str) -> String {
    let wanted = if name.ends_with(".hsh") {
        String::from(name)
    } else {
        format!("{name}.hsh")
    };
    let Some(module) = loader
        .info
        .server_modules
        .iter()
        .find(|module| hk_root_server::loader::module_name(module.name) == wanted)
    else {
        return format!("未找到脚本：{wanted}\n");
    };
    let Some(bytes) = loader.info.module_bytes(module) else {
        return format!("脚本不在 direct map 内：{wanted}\n");
    };
    match core::str::from_utf8(bytes) {
        Ok(text) => String::from(text),
        Err(_) => format!("脚本不是 UTF-8：{wanted}\n"),
    }
}
