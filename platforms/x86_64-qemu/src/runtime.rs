//! 用户态任务调度与系统调用分派。
//!
//! 线程状态、就绪队列、端点消息与阻塞唤醒由 `hk-kernel` 提供；本模块维护每个
//! 服务的用户现场与端点编号到句柄的映射，并在系统调用边界保存和恢复现场。

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use hk_hal::UserContext;
use hk_handle::{Handle, HandleTable, ObjectId, Rights};
use hk_ipc::Message;
use hk_kernel::{Kernel, Priority, TcbId, ThreadState};
use hk_platform_x86_64::cpu::{self, InterruptFrame, SyscallAction};
use hk_root_server::{RootServer, ServiceSpec, ServiceTable, TaskInfo};
use spin::Mutex;

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
    handles: Vec<Option<Handle>>,
    context: InterruptFrame,
    pending: Pending,
    exited: bool,
    exit_code: u64,
}

struct Runtime {
    kernel: Kernel,
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
        tasks.push(Task {
            tcb: service.tcb,
            name: names[index].to_string(),
            handles,
            context: cpu::user_frame(0, 0, (index + 1) as u64),
            pending: Pending::None,
            exited: false,
            exit_code: 0,
        });
    }

    kernel
        .exit(server.root())
        .map_err(|error| format!("回收 Root 线程失败：{error}"))?;

    *RUNTIME.lock() = Some(Runtime {
        kernel,
        tasks,
        endpoints,
        current: None,
    });
    cpu::set_syscall_hook(dispatch);
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

/// 装载一个新任务，返回分配的端点编号。
pub fn start(name: &str, mut context: InterruptFrame) -> Result<u64, String> {
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

    runtime.tasks.push(Task {
        tcb,
        name: name.to_string(),
        handles,
        context,
        pending: Pending::None,
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
            endpoint: index as u64 + 1,
            state: state_name(runtime.kernel.state_of(task.tcb)),
            pending: pending_name(task.pending),
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

fn dispatch(frame: &mut InterruptFrame) -> SyscallAction {
    let mut guard = RUNTIME.lock();
    let Some(runtime) = guard.as_mut() else {
        frame.rax = u64::MAX;
        return SyscallAction::Continue;
    };
    let Some(index) = runtime.current else {
        frame.rax = u64::MAX;
        return SyscallAction::Continue;
    };

    let tcb = runtime.tasks[index].tcb;
    match frame.rax {
        hk_server_api::SYS_SEND => send(runtime, index, tcb, frame),
        hk_server_api::SYS_RECV => recv(runtime, index, tcb, frame),
        hk_server_api::SYS_YIELD => {
            runtime.tasks[index].context = *frame;
            runtime.tasks[index].pending = Pending::None;
            SyscallAction::Block
        }
        hk_server_api::SYS_EXIT => {
            let code = frame.rdi;
            let _ = runtime.kernel.exit(tcb);
            runtime.tasks[index].exited = true;
            runtime.tasks[index].exit_code = code;
            SyscallAction::Exit
        }
        hk_server_api::SYS_PCI_READ => {
            let address = frame.rdi;
            let bus = (address >> 8) as u8;
            let device = ((address >> 3) & 0x1F) as u8;
            let function = (address & 0x07) as u8;
            frame.rax = u64::from(hk_platform_x86_64::pci::config_read(
                bus,
                device,
                function,
                frame.rsi as u8,
            ));
            SyscallAction::Continue
        }
        hk_server_api::SYS_TICKS => {
            frame.rax = hk_platform_x86_64::cpu::timer_ticks();
            SyscallAction::Continue
        }
        _ => {
            frame.rax = u64::MAX;
            SyscallAction::Continue
        }
    }
}

fn send(
    runtime: &mut Runtime,
    index: usize,
    tcb: TcbId,
    frame: &mut InterruptFrame,
) -> SyscallAction {
    let endpoint = frame.rdi as usize;
    let Some(handle) = runtime.tasks[index]
        .handles
        .get(endpoint.wrapping_sub(1))
        .copied()
        .flatten()
    else {
        frame.rax = u64::MAX;
        return SyscallAction::Continue;
    };

    let mut words = [0; 8];
    words[0] = frame.rdx;
    let message = Message::new(frame.rsi).with_words(words);

    match runtime.kernel.send(tcb, handle, message) {
        Ok(()) => {
            if runtime.kernel.state_of(tcb) == Some(ThreadState::Blocked) {
                let mut context = *frame;
                context.rax = 0;
                runtime.tasks[index].context = context;
                runtime.tasks[index].pending = Pending::Send;
                SyscallAction::Block
            } else {
                frame.rax = 0;
                SyscallAction::Continue
            }
        }
        Err(_) => {
            frame.rax = u64::MAX;
            SyscallAction::Continue
        }
    }
}

fn recv(
    runtime: &mut Runtime,
    index: usize,
    tcb: TcbId,
    frame: &mut InterruptFrame,
) -> SyscallAction {
    let endpoint = frame.rdi as usize;
    let Some(handle) = runtime.tasks[index]
        .handles
        .get(endpoint.wrapping_sub(1))
        .copied()
        .flatten()
    else {
        frame.rax = u64::MAX;
        return SyscallAction::Continue;
    };

    match runtime.kernel.recv(tcb, handle) {
        Ok(Some(received)) => {
            let sender = endpoint_of(runtime, received.sender);
            frame.rax = received.message.label;
            frame.rdx = received.message.words[0];
            frame.rcx = sender;
            SyscallAction::Continue
        }
        Ok(None) => {
            let mut context = *frame;
            context.rax = 0;
            runtime.tasks[index].context = context;
            runtime.tasks[index].pending = Pending::Recv { handle };
            SyscallAction::Block
        }
        Err(_) => {
            frame.rax = u64::MAX;
            SyscallAction::Continue
        }
    }
}
