//! 用户态任务运行时：任务表、系统调用分派与任务 Arena。
//!
//! 微内核提供 TCB、端点、句柄与调度；本模块维护用户任务视图、端点编号到句柄
//! 的映射、系统调用语义与 Arena 记账。平台只提供现场保存/恢复、PCI 访问、
//! 计时与 Arena 布局。

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use hk_hal::{DeviceGrant, MAX_CPUS, SyscallFrame, UserContext};
use hk_handle::{Handle, HandleTable, ObjectId, Rights};
use hk_ipc::Message;
use hk_syscall::{
    SYS_ALLOC, SYS_CALL, SYS_DEVICE_GRANT_CREATE, SYS_DEVICE_MAP, SYS_EXIT, SYS_FORK,
    SYS_HANDLE_CLOSE, SYS_HANDLE_COUNT, SYS_NOTIFY, SYS_PCI_READ, SYS_PCI_WRITE, SYS_RECV,
    SYS_RECV_DATA, SYS_REPLY, SYS_SEND, SYS_SEND_DATA, SYS_SEND_HANDLE, SYS_TICKS, SYS_TSC,
    SYS_TSC_FREQ, SYS_WAIT_NOTIFY, SYS_YIELD,
};

use hk_kernel::{Kernel, KernelError, Priority, ReplyToken, TcbId, ThreadState};

/// 随消息接收的句柄编号起点。
mod arena;
mod syscall;
mod task;

const EXTRA_HANDLE_BASE: usize = 0x1000;
const GRANT_HANDLE_BASE: usize = 0x2000;

/// 被挂起的系统调用。
#[derive(Clone, Copy)]
pub enum Pending {
    /// 没有挂起操作。
    None,
    /// 发送已排队，等待接收者取走。
    Send,
    /// 接收已登记，等待消息到达。
    Recv {
        /// 接收使用的端点句柄。
        handle: Handle,
    },
    /// 字节接收已登记。
    RecvData {
        /// 接收使用的端点句柄。
        handle: Handle,
        /// 用户缓冲区地址。
        buffer: u64,
        /// 缓冲区容量。
        capacity: usize,
    },
    /// 通知等待已登记。
    Notify {
        /// 通知句柄。
        handle: Handle,
        /// 等待的通知位。
        mask: u64,
    },
    /// 调用已发出，等待服务回复。
    Call {
        /// 调用目标端点编号，用于回报发送者。
        endpoint: u64,
        /// 调用开始时间戳。
        started: u64,
    },
}

/// 系统调用后的调度动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
    /// 返回用户态。
    Continue,
    /// 保存现场并切换到下一个任务。
    Block,
    /// 任务退出。
    Exit,
}

/// 调度一步的结果。
pub enum Step<C> {
    /// 恢复该任务的用户态现场。
    Resume {
        /// 任务下标，供 [`UserRuntime::finish`] 收尾。
        index: usize,
        /// 用户态现场。
        context: C,
    },
    /// 当前任务阻塞，继续调度下一个。
    Blocked,
    /// 没有可运行任务。
    Done,
}

/// 平台提供的用户态运行支持。
pub trait UserPlatform {
    /// 返回调度 tick 计数。
    fn timer_ticks(&self) -> u64;
    /// 读取 PCI 配置空间中的一个 32 位字。
    fn pci_read(&self, bus: u8, device: u8, function: u8, offset: u8) -> u32;
    /// 写入 PCI 配置空间中的一个 32 位字。
    fn pci_write(&self, _bus: u8, _device: u8, _function: u8, _offset: u8, _value: u32) {}
    /// 把设备 MMIO 映射到任务地址空间。
    fn map_mmio(&self, _slot: usize, _virt: u64, _phys: u64, _len: usize) -> bool {
        false
    }
    /// 把用户地址区间清零。
    fn zero(&self, address: u64, length: usize);
    /// 从当前任务用户内存读取字节；地址无效时返回 `false`。
    fn read_user(&self, _address: u64, _buffer: &mut [u8]) -> bool {
        false
    }
    /// 向当前任务用户内存写入字节；地址无效时返回 `false`。
    fn write_user(&self, _address: u64, _data: &[u8]) -> bool {
        false
    }
    /// 返回任务槽位的 Arena 基址与大小。
    fn arena_layout(&self, slot: usize) -> (u64, u64);
    /// 切换到任务槽位的地址空间。
    fn activate(&self, _slot: usize) {}
    /// 返回任务槽位的页表根；没有地址空间时为 0。
    fn vspace(&self, _slot: usize) -> u64 {
        0
    }
    /// 把父任务的用户地址空间复制为子任务的写时复制副本。
    fn fork(&self, _parent: usize, _child: usize) -> bool {
        false
    }
    /// 返回单调时间戳；无计时能力时为 0。
    fn timestamp(&self) -> u64 {
        0
    }
    /// 返回时间戳频率；未知时为 0。
    fn tsc_frequency(&self) -> u64 {
        0
    }
    /// 唤醒空闲 CPU 重新检查调度队列。
    fn wake_aps(&self) {}
}

/// 任务 Arena：线性分配，不回收。
#[derive(Clone, Copy)]
pub struct Arena {
    base: u64,
    next: u64,
    end: u64,
}

/// 任务视图。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskInfo {
    /// TCB 编号。
    pub tcb: u64,
    /// 任务名称。
    pub name: String,
    /// 父任务 TCB 编号。
    pub parent: Option<u64>,
    /// 端点编号。
    pub endpoint: u64,
    /// 线程状态。
    pub state: &'static str,
    /// 挂起的系统调用。
    pub pending: &'static str,
    /// 在用句柄数。
    pub handles: usize,
    /// Arena 已用字节数。
    pub arena_used: u64,
    /// Arena 总字节数。
    pub arena_size: u64,
    /// 页表根物理地址。
    pub vspace: u64,
    /// 退出码。
    pub exit_code: Option<u64>,
}

/// 用户任务。
pub struct UserTask<C> {
    /// 线程控制块。
    pub tcb: TcbId,
    /// 任务名称。
    pub name: String,
    /// 父任务 TCB 编号。
    pub parent: Option<u64>,
    /// 端点编号到句柄的映射。
    pub handles: Vec<Option<Handle>>,
    /// 通知编号到句柄的映射。
    pub notifications: Vec<Option<Handle>>,
    /// 随消息接收的额外句柄。
    pub extra: Vec<Option<Handle>>,
    /// 本任务持有的 DeviceGrant 句柄。
    pub grants: Vec<Handle>,
    /// 用户现场。
    pub context: C,
    /// 挂起的系统调用。
    pub pending: Pending,
    /// 最近一次收到的调用令牌。
    pub reply: Option<ReplyToken>,
    /// 任务 Arena。
    pub arena: Arena,
    /// 是否已退出。
    pub exited: bool,
    /// 退出码。
    pub exit_code: u64,
}

impl<C> UserTask<C> {
    /// 构造任务条目。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tcb: TcbId,
        name: &str,
        parent: Option<u64>,
        handles: Vec<Option<Handle>>,
        notifications: Vec<Option<Handle>>,
        context: C,
        arena_base: u64,
        arena_size: u64,
    ) -> Self {
        Self {
            tcb,
            name: name.to_string(),
            parent,
            handles,
            notifications,
            extra: Vec::new(),
            grants: Vec::new(),
            context,
            pending: Pending::None,
            reply: None,
            arena: Arena::new(arena_base, arena_size),
            exited: false,
            exit_code: 0,
        }
    }
}

/// 运行时统计。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuntimeStats {
    /// 上下文切换次数。
    pub switches: u64,
    /// 上下文切换总周期数。
    pub switch_cycles: u64,
    /// 单次切换最大周期数。
    pub switch_max_cycles: u64,
    /// IPC call/reply 次数。
    pub calls: u64,
    /// IPC call/reply 总周期数。
    pub call_cycles: u64,
    /// 单次 call/reply 最大周期数。
    pub call_max_cycles: u64,
    /// 各 CPU 恢复用户任务的次数。
    pub resumes: [u64; MAX_CPUS],
    /// TSC 频率；未知时为 0。
    pub tsc_frequency: u64,
}

/// 用户态任务运行时。
pub struct UserRuntime<C, P> {
    kernel: Kernel,
    root: u64,
    tasks: Vec<UserTask<C>>,
    endpoints: Vec<Option<ObjectId>>,
    notifications: Vec<Option<ObjectId>>,
    current: [Option<usize>; MAX_CPUS],
    pending_exits: Vec<(String, u64)>,
    last_finish: Option<u64>,
    switch_count: u64,
    switch_cycles: u64,
    switch_max: u64,
    call_count: u64,
    call_cycles: u64,
    call_max: u64,
    resumes: [u64; MAX_CPUS],
    platform: P,
}

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 以 Kernel、任务与平台适配构造运行时。
    pub fn new(
        kernel: Kernel,
        root: u64,
        endpoints: Vec<Option<ObjectId>>,
        notifications: Vec<Option<ObjectId>>,
        tasks: Vec<UserTask<C>>,
        platform: P,
    ) -> Self {
        Self {
            kernel,
            root,
            tasks,
            endpoints,
            notifications,
            current: [None; MAX_CPUS],
            pending_exits: Vec::new(),
            last_finish: None,
            switch_count: 0,
            switch_cycles: 0,
            switch_max: 0,
            call_count: 0,
            call_cycles: 0,
            call_max: 0,
            resumes: [0; MAX_CPUS],
            platform,
        }
    }

    /// 返回任务数量。
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// 判断是否有等待调度的线程。
    pub fn has_runnable(&self) -> bool {
        self.kernel.has_runnable()
    }

    /// 唤醒空闲 CPU。
    pub fn wake_aps(&self) {
        self.platform.wake_aps();
    }

    /// 返回最近的内核 trace 事件。
    pub fn trace(&self, limit: usize) -> Vec<hk_kernel::TraceEvent> {
        self.kernel.trace_events(limit)
    }

    /// 返回运行时统计。
    pub fn stats(&self) -> RuntimeStats {
        RuntimeStats {
            switches: self.switch_count,
            switch_cycles: self.switch_cycles,
            switch_max_cycles: self.switch_max,
            calls: self.call_count,
            call_cycles: self.call_cycles,
            call_max_cycles: self.call_max,
            resumes: self.resumes,
            tsc_frequency: self.platform.tsc_frequency(),
        }
    }

    /// 记录非 BSP 观察到的服务退出，等待 BSP 同步到服务表。
    pub fn defer_exit(&mut self, name: String, code: u64) {
        self.pending_exits.push((name, code));
    }

    /// 取走待同步的服务退出记录。
    pub fn take_exits(&mut self) -> Vec<(String, u64)> {
        core::mem::take(&mut self.pending_exits)
    }

    /// 返回 Root 线程编号。
    pub fn root(&self) -> u64 {
        self.root
    }

    /// 按名称返回任务的 TCB 编号。
    pub fn tcb_of(&self, name: &str) -> Option<u64> {
        self.tasks
            .iter()
            .find(|task| task.name == name)
            .map(|task| task.tcb.get())
    }

    /// 设置任务的用户态初始现场，并使任务进入调度。
    pub fn attach(&mut self, index: usize, context: C) {
        if let Some(task) = self.tasks.get_mut(index) {
            task.context = context;
            let _ = self.kernel.activate(task.tcb);
        }
    }

    /// 生成任务视图快照。
    pub fn tasks(&self) -> Vec<TaskInfo> {
        self.tasks
            .iter()
            .enumerate()
            .map(|(index, task)| TaskInfo {
                tcb: task.tcb.get(),
                name: task.name.clone(),
                parent: task.parent,
                endpoint: index as u64 + 1,
                state: state_name(self.kernel.state_of(task.tcb)),
                pending: pending_name(task.pending),
                handles: self.kernel.handles_live(task.tcb).unwrap_or(0),
                arena_used: task.arena.used(),
                arena_size: task.arena.size(),
                vspace: self.platform.vspace(index),
                exit_code: task.exited.then_some(task.exit_code),
            })
            .collect()
    }

    /// 装载一个新任务，返回分配的端点编号。
    fn endpoint_of(&self, tcb: TcbId) -> u64 {
        self.tasks
            .iter()
            .position(|task| task.tcb == tcb)
            .map(|index| index as u64 + 1)
            .unwrap_or(0)
    }

    fn install_handle(&mut self, index: usize, message: &Message) -> u64 {
        let Some(handle) = message.handles[0] else {
            return 0;
        };

        let slot = self.tasks[index].extra.len();
        self.tasks[index].extra.push(Some(handle));
        (EXTRA_HANDLE_BASE + slot + 1) as u64
    }

    fn grant_at(&self, index: usize, number: usize) -> Option<Handle> {
        number
            .checked_sub(GRANT_HANDLE_BASE + 1)
            .and_then(|slot| self.tasks[index].grants.get(slot))
            .copied()
    }

    fn install_grant(&mut self, index: usize, handle: Handle) -> u64 {
        let slot = self.tasks[index].grants.len();
        self.tasks[index].grants.push(handle);
        (GRANT_HANDLE_BASE + slot + 1) as u64
    }

    fn handle_at(&self, index: usize, number: usize) -> Option<Handle> {
        if number >= EXTRA_HANDLE_BASE {
            self.tasks[index]
                .extra
                .get(number - EXTRA_HANDLE_BASE - 1)
                .copied()
                .flatten()
        } else {
            self.tasks[index]
                .handles
                .get(number.wrapping_sub(1))
                .copied()
                .flatten()
        }
    }

    fn clear_handle(&mut self, index: usize, number: usize) {
        if number >= EXTRA_HANDLE_BASE {
            if let Some(slot) = self.tasks[index]
                .extra
                .get_mut(number - EXTRA_HANDLE_BASE - 1)
            {
                *slot = None;
            }
        } else if let Some(slot) = number
            .checked_sub(1)
            .and_then(|slot| self.tasks[index].handles.get_mut(slot))
        {
            *slot = None;
        }
    }
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
        Pending::RecvData { .. } => "recv-data",
        Pending::Notify { .. } => "notify",
        Pending::Call { .. } => "call",
    }
}

#[cfg(test)]
mod tests;
