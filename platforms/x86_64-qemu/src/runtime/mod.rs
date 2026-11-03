//! 用户态任务调度与任务视图。
//!
//! 线程状态、就绪队列、端点消息与阻塞唤醒由 `hk-kernel` 提供；本模块维护任务
//! 表、用户现场与端点编号到句柄的映射。系统调用分派位于 [`syscall`]。

mod syscall;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use hk_hal::UserContext;
use hk_handle::{Handle, HandleTable, ObjectId, Rights};
use hk_kernel::{Kernel, Priority, TcbId, ThreadState};
use hk_platform_x86_64::cpu::{self, InterruptFrame};
use hk_root_server::{RootServer, ServiceSpec, ServiceTable, TaskInfo};
use spin::Mutex;

/// 每个任务的 Arena 大小。
const ARENA_SIZE: u64 = 64 * 1024;

/// Arena 虚拟基址。
const ARENA_BASE: u64 = 0x0100_0000;

/// 相邻任务 Arena 的间隔。
const ARENA_STRIDE: u64 = 0x2_0000;

/// 返回任务槽位的 Arena 基址与大小。
pub fn arena(slot: usize) -> (u64, u64) {
    (ARENA_BASE + slot as u64 * ARENA_STRIDE, ARENA_SIZE)
}

/// 被挂起的系统调用。
#[derive(Clone, Copy)]
enum Pending {
    /// 没有挂起操作。
    None,
    /// 发送已排队，等待接收者取走。
    Send,
    /// 接收已登记，等待消息到达。
    Recv {
        /// 接收使用的端点句柄。
        handle: Handle,
    },
}

struct Task {
    tcb: TcbId,
    name: String,
    parent: Option<u64>,
    handles: Vec<Option<Handle>>,
    context: InterruptFrame,
    pending: Pending,
    arena_next: u64,
    arena_end: u64,
    exited: bool,
    exit_code: u64,
}

struct Runtime {
    kernel: Kernel,
    root: u64,
    tasks: Vec<Task>,
    endpoints: Vec<Option<ObjectId>>,
    current: Option<usize>,
}

static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);

/// 按服务清单创建线程、端点与句柄，注册系统调用处理函数。
pub fn init(names: &[&'static str]) -> Result<(), String> {
    let kernel = Kernel::new();
    let specs: Vec<ServiceSpec> = names.iter().map(|name| ServiceSpec::new(name)).collect();
    let server = RootServer::boot(&kernel, &specs).map_err(|error| format!("{error}"))?;
    let root = server.root().get();

    let mut tasks = Vec::with_capacity(names.len());
    let mut endpoints = Vec::with_capacity(names.len());
    for (index, service) in server.services().iter().enumerate() {
        endpoints.push(Some(service.endpoint));
        let mut handles = Vec::with_capacity(names.len());
        for endpoint in server.services() {
            let handle = kernel
                .grant(service.tcb, endpoint.endpoint, Rights::READ | Rights::WRITE)
                .map_err(|error| format!("授予端点句柄失败：{error}"))?;
            handles.push(Some(handle));
        }
        let (arena_base, arena_size) = arena(index);
        tasks.push(Task {
            tcb: service.tcb,
            name: names[index].to_string(),
            parent: Some(root),
            handles,
            context: cpu::user_frame(0, 0, (index + 1) as u64),
            pending: Pending::None,
            arena_next: arena_base,
            arena_end: arena_base + arena_size,
            exited: false,
            exit_code: 0,
        });
    }

    kernel
        .exit(server.root())
        .map_err(|error| format!("回收 Root 线程失败：{error}"))?;

    *RUNTIME.lock() = Some(Runtime {
        kernel,
        root,
        tasks,
        endpoints,
        current: None,
    });
    cpu::set_syscall_hook(syscall::dispatch);
    cpu::set_preempt_hook(preempt);
    Ok(())
}

/// 设置服务的用户态初始现场。
pub fn attach(index: usize, context: InterruptFrame) {
    let mut guard = RUNTIME.lock();
    if let Some(runtime) = guard.as_mut() {
        if let Some(task) = runtime.tasks.get_mut(index) {
            task.context = context;
        }
    }
}

/// 返回已登记任务数量。
pub fn task_count() -> usize {
    RUNTIME
        .lock()
        .as_ref()
        .map_or(0, |runtime| runtime.tasks.len())
}

/// 按名称返回任务的 TCB 编号。
pub fn tcb_of(name: &str) -> Option<u64> {
    let guard = RUNTIME.lock();
    let runtime = guard.as_ref()?;
    runtime
        .tasks
        .iter()
        .find(|task| task.name == name)
        .map(|task| task.tcb.get())
}

/// 返回 Root 线程编号，作为引导期服务的父任务。
pub fn root() -> u64 {
    RUNTIME.lock().as_ref().map_or(0, |runtime| runtime.root)
}

/// 装载一个新任务，返回分配的端点编号。
pub fn start(name: &str, mut context: InterruptFrame, parent: Option<u64>) -> Result<u64, String> {
    let mut guard = RUNTIME.lock();
    let runtime = guard
        .as_mut()
        .ok_or_else(|| String::from("运行时未初始化"))?;

    let index = runtime.tasks.len();
    let endpoint = index as u64 + 1;
    context.rdi = endpoint;

    let kernel = &runtime.kernel;
    let tcb = kernel.spawn_thread(HandleTable::with_default_quota(), Priority::NORMAL);
    let own = kernel
        .create_endpoint(tcb, Rights::READ | Rights::WRITE)
        .map_err(|error| format!("创建端点失败：{error}"))?;
    let own_id = kernel
        .handle_id(tcb, own)
        .map_err(|error| format!("解析端点失败：{error}"))?;
    runtime.endpoints.push(Some(own_id));

    let mut handles = Vec::with_capacity(runtime.endpoints.len());
    for object in &runtime.endpoints {
        let handle = match object {
            Some(id) => kernel.grant(tcb, *id, Rights::READ | Rights::WRITE).ok(),
            None => None,
        };
        handles.push(handle);
    }

    for task in &mut runtime.tasks {
        let handle = kernel
            .grant(task.tcb, own_id, Rights::READ | Rights::WRITE)
            .ok();
        task.handles.push(handle);
    }

    let (arena_base, arena_size) = arena(index);
    runtime.tasks.push(Task {
        tcb,
        name: name.to_string(),
        parent,
        handles,
        context,
        pending: Pending::None,
        arena_next: arena_base,
        arena_end: arena_base + arena_size,
        exited: false,
        exit_code: 0,
    });
    Ok(endpoint)
}

/// 生成当前任务视图快照。
pub fn tasks() -> Vec<TaskInfo> {
    let guard = RUNTIME.lock();
    let Some(runtime) = guard.as_ref() else {
        return Vec::new();
    };

    runtime
        .tasks
        .iter()
        .enumerate()
        .map(|(index, task)| TaskInfo {
            tcb: task.tcb.get(),
            name: task.name.clone(),
            parent: task.parent,
            endpoint: index as u64 + 1,
            state: state_name(runtime.kernel.state_of(task.tcb)),
            pending: pending_name(task.pending),
            handles: runtime.kernel.handles_live(task.tcb).unwrap_or(0),
            arena_used: ARENA_SIZE - (task.arena_end - task.arena_next),
            arena_size: ARENA_SIZE,
            exit_code: task.exited.then_some(task.exit_code),
        })
        .collect()
}

fn state_name(state: Option<ThreadState>) -> &'static str {
    match state {
        Some(ThreadState::Runnable) => "runnable",
        Some(ThreadState::Running) => "running",
        Some(ThreadState::Blocked) => "blocked",
        Some(ThreadState::Inactive) | None => "exited",
    }
}

fn pending_name(pending: Pending) -> &'static str {
    match pending {
        Pending::None => "-",
        Pending::Send => "send",
        Pending::Recv { .. } => "recv",
    }
}

/// 依次调度任务直到没有可运行线程，并把退出结果写入服务表。
pub fn run(table: &mut ServiceTable) {
    cpu::start_tick();
    loop {
        let Some(index) = next_task() else {
            break;
        };

        let (name, context, blocked_again) = prepare(index);
        if blocked_again {
            continue;
        }

        context.resume();

        let (exited, code) = finish(index);
        if exited {
            table.mark_exited(&name, code);
        }
    }
    cpu::stop_tick();
}

fn next_task() -> Option<usize> {
    let mut guard = RUNTIME.lock();
    let runtime = guard.as_mut()?;
    let tcb = runtime.kernel.schedule()?;
    runtime.tasks.iter().position(|task| task.tcb == tcb)
}

fn prepare(index: usize) -> (String, InterruptFrame, bool) {
    let mut guard = RUNTIME.lock();
    let runtime = guard.as_mut().expect("运行时已初始化");
    runtime.current = Some(index);

    let pending = runtime.tasks[index].pending;
    runtime.tasks[index].pending = Pending::None;
    let mut blocked_again = false;

    if let Pending::Recv { handle } = pending {
        let tcb = runtime.tasks[index].tcb;
        match runtime.kernel.recv(tcb, handle) {
            Ok(Some(received)) => {
                let sender = endpoint_of(runtime, received.sender);
                let frame = &mut runtime.tasks[index].context;
                frame.rax = received.message.label;
                frame.rdx = received.message.words[0];
                frame.rcx = sender;
            }
            Ok(None) => blocked_again = true,
            Err(_) => runtime.tasks[index].context.rax = u64::MAX,
        }
    }

    if blocked_again {
        runtime.current = None;
    }

    let name = runtime.tasks[index].name.clone();
    let context = runtime.tasks[index].context;
    (name, context, blocked_again)
}

fn finish(index: usize) -> (bool, u64) {
    let mut guard = RUNTIME.lock();
    let runtime = guard.as_mut().expect("运行时已初始化");
    runtime.current = None;
    let task = &runtime.tasks[index];
    (task.exited, task.exit_code)
}

fn endpoint_of(runtime: &Runtime, tcb: TcbId) -> u64 {
    runtime
        .tasks
        .iter()
        .position(|task| task.tcb == tcb)
        .map(|index| index as u64 + 1)
        .unwrap_or(0)
}

fn preempt(frame: &InterruptFrame) -> bool {
    let mut guard = RUNTIME.lock();
    let Some(runtime) = guard.as_mut() else {
        return false;
    };
    let Some(index) = runtime.current else {
        return false;
    };

    runtime.tasks[index].context = *frame;
    runtime.tasks[index].pending = Pending::None;
    true
}
