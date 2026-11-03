//! 用户态任务调度与系统调用分派。
//!
//! 线程状态、就绪队列、端点消息与阻塞唤醒由 `hk-kernel` 提供；本模块维护每个
//! 服务的用户现场与端点编号到句柄的映射，并在系统调用边界保存和恢复现场。

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use hk_hal::UserContext;
use hk_handle::{Handle, Rights};
use hk_ipc::Message;
use hk_kernel::{Kernel, TcbId, ThreadState};
use hk_platform_x86_64::cpu::{self, InterruptFrame, SyscallAction};
use hk_root_server::{RootServer, ServiceSpec, ServiceTable};
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
    handles: Vec<Handle>,
    context: InterruptFrame,
    pending: Pending,
    exited: bool,
    exit_code: u64,
}

struct Runtime {
    kernel: Kernel,
    tasks: Vec<Task>,
    current: Option<usize>,
}

static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);

/// 按服务清单创建线程、端点与句柄，注册系统调用处理函数。
pub fn init(names: &[&'static str]) -> Result<(), String> {
    let kernel = Kernel::new();
    let specs: Vec<ServiceSpec> = names.iter().map(|name| ServiceSpec::new(name)).collect();
    let server = RootServer::boot(&kernel, &specs).map_err(|error| format!("{error}"))?;

    let mut tasks = Vec::with_capacity(names.len());
    for (index, service) in server.services().iter().enumerate() {
        let mut handles = Vec::with_capacity(names.len());
        for endpoint in server.services() {
            let handle = kernel
                .grant(service.tcb, endpoint.endpoint, Rights::READ | Rights::WRITE)
                .map_err(|error| format!("授予端点句柄失败：{error}"))?;
            handles.push(handle);
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
        current: None,
    });
    cpu::set_syscall_hook(dispatch);
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

/// 依次调度任务直到没有可运行线程，并把退出结果写入服务表。
pub fn run(table: &mut ServiceTable) {
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
