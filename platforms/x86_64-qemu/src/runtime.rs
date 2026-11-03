//! 用户态任务的协作式调度与内核消息端点。
//!
//! 每个任务保存一份用户态中断现场；系统调用要求切换时，调度器保存现场并
//! 选择下一个可运行任务，用 `iretq` 恢复执行。端点在内核中排队消息，任务
//! 收不到消息时阻塞，等到发送者投递后重新变为可运行。

use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use hk_platform_x86_64::cpu::{self, InterruptFrame, SyscallAction};
use spin::Mutex;

use crate::service::ServiceTable;

/// 任务状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    /// 可运行。
    Runnable,
    /// 正在运行。
    Running,
    /// 等待消息。
    Blocked,
    /// 已退出。
    Exited,
}

/// 用户态任务。
pub struct Task {
    /// 服务名称。
    pub name: String,
    /// 任务自己的端点编号。
    pub endpoint: u64,
    /// 用户态现场。
    pub context: InterruptFrame,
    /// 当前状态。
    pub state: TaskState,
    /// 退出码。
    pub exit_code: u64,
}

#[derive(Clone, Copy)]
struct Message {
    label: u64,
    word: u64,
    sender: u64,
}

#[derive(Default)]
struct Endpoint {
    queue: VecDeque<Message>,
    receiver: Option<usize>,
}

struct Runtime {
    tasks: Vec<Task>,
    endpoints: Vec<Endpoint>,
    current: Option<usize>,
    cursor: usize,
}

static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);

/// 初始化任务表，`endpoints` 是端点槽位数（编号从 1 开始）。
pub fn init(endpoints: usize) {
    let endpoints = (0..endpoints).map(|_| Endpoint::default()).collect();
    *RUNTIME.lock() = Some(Runtime {
        tasks: Vec::new(),
        endpoints,
        current: None,
        cursor: 0,
    });
}

/// 登记一个可运行任务。
pub fn spawn(name: &str, endpoint: u64, context: InterruptFrame) {
    let mut runtime = RUNTIME.lock();
    let runtime = runtime.as_mut().expect("运行时已初始化");
    runtime.tasks.push(Task {
        name: name.to_string(),
        endpoint,
        context,
        state: TaskState::Runnable,
        exit_code: 0,
    });
}

/// 注册系统调用处理函数。
pub fn install() {
    cpu::set_syscall_hook(dispatch);
}

/// 依次运行任务直到全部退出，并把退出结果写入服务表。
pub fn run(services: &mut ServiceTable) {
    while let Some(index) = next_task() {
        let (name, context) = {
            let mut guard = RUNTIME.lock();
            let runtime = guard.as_mut().expect("运行时已初始化");
            runtime.tasks[index].state = TaskState::Running;
            runtime.current = Some(index);
            (
                runtime.tasks[index].name.clone(),
                runtime.tasks[index].context,
            )
        };

        cpu::resume_user(&context);

        let (exited, code) = {
            let mut guard = RUNTIME.lock();
            let runtime = guard.as_mut().expect("运行时已初始化");
            let state = runtime.tasks[index].state;
            runtime.current = None;
            (state == TaskState::Exited, runtime.tasks[index].exit_code)
        };
        if exited {
            services.mark_exited(&name, code);
        }
    }
}

fn next_task() -> Option<usize> {
    let mut guard = RUNTIME.lock();
    let runtime = guard.as_mut()?;
    let length = runtime.tasks.len();
    for offset in 0..length {
        let index = (runtime.cursor + offset) % length;
        if runtime.tasks[index].state == TaskState::Runnable {
            runtime.cursor = (index + 1) % length;
            return Some(index);
        }
    }
    None
}

fn dispatch(frame: &mut InterruptFrame) -> SyscallAction {
    let mut guard = RUNTIME.lock();
    let Some(runtime) = guard.as_mut() else {
        frame.rax = u64::MAX;
        return SyscallAction::Continue;
    };
    let Some(current) = runtime.current else {
        frame.rax = u64::MAX;
        return SyscallAction::Continue;
    };

    match frame.rax {
        hk_server_api::SYS_SEND => send(runtime, current, frame),
        hk_server_api::SYS_RECV => recv(runtime, current, frame),
        hk_server_api::SYS_YIELD => {
            runtime.tasks[current].state = TaskState::Runnable;
            SyscallAction::Block
        }
        hk_server_api::SYS_EXIT => {
            let task = &mut runtime.tasks[current];
            task.state = TaskState::Exited;
            task.exit_code = frame.rdi;
            SyscallAction::Exit
        }
        _ => {
            frame.rax = u64::MAX;
            SyscallAction::Continue
        }
    }
}

fn send(runtime: &mut Runtime, current: usize, frame: &mut InterruptFrame) -> SyscallAction {
    let endpoint = frame.rdi as usize;
    if endpoint >= runtime.endpoints.len() {
        frame.rax = u64::MAX;
        return SyscallAction::Continue;
    }

    let label = frame.rsi;
    let word = frame.rdx;
    let sender = runtime.tasks[current].endpoint;
    match runtime.endpoints[endpoint].receiver.take() {
        Some(receiver) => {
            let task = &mut runtime.tasks[receiver];
            task.context.rax = label;
            task.context.rdx = word;
            task.context.rcx = sender;
            task.state = TaskState::Runnable;
        }
        None => {
            runtime.endpoints[endpoint].queue.push_back(Message {
                label,
                word,
                sender,
            });
        }
    }

    frame.rax = 0;
    SyscallAction::Continue
}

fn recv(runtime: &mut Runtime, current: usize, frame: &mut InterruptFrame) -> SyscallAction {
    let endpoint = frame.rdi as usize;
    if endpoint >= runtime.endpoints.len() {
        frame.rax = u64::MAX;
        return SyscallAction::Continue;
    }

    match runtime.endpoints[endpoint].queue.pop_front() {
        Some(message) => {
            frame.rax = message.label;
            frame.rdx = message.word;
            frame.rcx = message.sender;
            SyscallAction::Continue
        }
        None => {
            runtime.endpoints[endpoint].receiver = Some(current);
            runtime.tasks[current].context = *frame;
            runtime.tasks[current].state = TaskState::Blocked;
            SyscallAction::Block
        }
    }
}
