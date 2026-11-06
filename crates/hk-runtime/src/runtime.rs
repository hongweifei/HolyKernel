//! 用户态任务运行时：任务表、系统调用分派与任务 Arena。
//!
//! 微内核提供 TCB、端点、句柄与调度；本模块维护用户任务视图、端点编号到句柄
//! 的映射、系统调用语义与 Arena 记账。平台只提供现场保存/恢复、PCI 访问、
//! 计时与 Arena 布局。

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use hk_hal::{CivilTime, DeviceGrant, MAX_CPUS, SyscallFrame, UserContext};
use hk_handle::{Handle, HandleTable, ObjectId, Rights};
use hk_ipc::Message;
use hk_syscall::CONSOLE_NOTIFY_INPUT;
use hk_syscall::{
    FAULT_EVENT, FUTEX_CHANGED, SYS_ALLOC, SYS_BOOT_CMDLINE, SYS_CALL, SYS_CIVIL_TIME,
    SYS_CONSOLE_READ, SYS_CONSOLE_SUBSCRIBE, SYS_DEVICE_GRANT_CREATE, SYS_DEVICE_INFO,
    SYS_DEVICE_MAP, SYS_DMA_ALLOC, SYS_DMA_MAP_FRAME, SYS_DMA_SYNC, SYS_EXIT, SYS_FAULT_SUBSCRIBE,
    SYS_FORK, SYS_FRAME_ALLOC, SYS_FRAME_INFO, SYS_FRAME_MAP, SYS_FRAME_UNMAP,
    SYS_FRAMEBUFFER_INFO, SYS_FRAMEBUFFER_MAP, SYS_FUTEX_WAIT, SYS_FUTEX_WAKE, SYS_HANDLE_CLOSE,
    SYS_HANDLE_COUNT, SYS_HANDLE_DUPLICATE, SYS_IRQ_BIND, SYS_KERNEL_STATS, SYS_NOTIFY,
    SYS_PCI_READ, SYS_PCI_WRITE, SYS_RECV, SYS_RECV_DATA, SYS_RECV_DATA_FROM, SYS_RECV_DATA_HANDLE,
    SYS_RECV_DATA_HANDLE_TRY, SYS_RECV_DATA_TRY, SYS_REPLY, SYS_SELF_NOTIFICATION, SYS_SEND,
    SYS_SEND_DATA, SYS_SEND_HANDLE, SYS_SERVICE_ENDPOINT, SYS_THREAD_CREATE, SYS_THREAD_JOIN,
    SYS_THREAD_SET_TLS, SYS_TICKS, SYS_TSC, SYS_TSC_FREQ, SYS_WAIT_NOTIFY, SYS_YIELD,
};

use hk_kernel::{Kernel, KernelError, Priority, ReplyToken, TcbId, ThreadState};

/// 随消息接收的句柄编号起点。
mod arena;
mod platform;
mod state;
mod syscall;
mod task;
mod user_task;
mod view;

pub use arena::Arena;
pub use platform::UserPlatform;
pub use state::{Pending, Step, Switch};
pub use user_task::UserTask;
pub use view::{RuntimeStats, TaskInfo};

const EXTRA_HANDLE_BASE: usize = 0x1000;
const GRANT_HANDLE_BASE: usize = 0x2000;
const FRAME_HANDLE_BASE: usize = 0x3000;
const DMA_WINDOW_BYTES: usize = 256 * 1024;

/// 任务持有的共享 Frame。
pub(crate) struct FrameMap {
    pub handle: Handle,
}

/// 保存的 Native Task 现场；VSpace 仍留在平台任务槽中。
pub(crate) struct Checkpoint<C> {
    pub(crate) slot: usize,
    pub(crate) context: C,
    pub(crate) parent: Option<u64>,
    pub(crate) arena_used: u64,
}

/// 用户态任务运行时。
pub struct UserRuntime<C, P> {
    kernel: Kernel,
    root: u64,
    tasks: Vec<UserTask<C>>,
    endpoints: Vec<Option<ObjectId>>,
    notifications: Vec<Option<ObjectId>>,
    checkpoints: Vec<(String, Checkpoint<C>)>,
    /// 等待线程退出的 (目标 TCB, 等待者 TCB)。
    joiners: Vec<(TcbId, TcbId)>,
    /// 共享内存字上的等待者：(进程主线程槽位, 地址, 线程)。
    futex_waiters: Vec<(usize, u64, TcbId)>,
    /// 异常事件订阅者：(任务槽位, 端点句柄)。
    fault_subscribers: Vec<(usize, Handle)>,
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
    irq_routes: [u16; 256],
    console_subscriber: Option<usize>,
    platform: P,
}

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 以 Kernel、任务与平台适配构造运行时。
    pub fn new(
        kernel: Kernel,
        root: u64,
        endpoints: Vec<Option<ObjectId>>,
        notifications: Vec<Option<ObjectId>>,
        mut tasks: Vec<UserTask<C>>,
        platform: P,
    ) -> Self {
        for (index, task) in tasks.iter_mut().enumerate() {
            task.leader = index;
        }
        Self {
            kernel,
            root,
            tasks,
            endpoints,
            notifications,
            checkpoints: Vec::new(),
            joiners: Vec::new(),
            futex_waiters: Vec::new(),
            fault_subscribers: Vec::new(),
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
            irq_routes: [0; 256],
            console_subscriber: None,
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

    /// 把原始异常现场投递给所有订阅者；失效的订阅会被移除。
    ///
    /// 内核只保留订阅关系，不保存异常内容；订阅者自行缓存与符号化。
    pub fn deliver_fault_event(&mut self, payload: &[u8]) {
        let alive: Vec<bool> = self.tasks.iter().map(|task| !task.exited).collect();
        let message = Message {
            label: FAULT_EVENT,
            words: [0; hk_ipc::MAX_MESSAGE_WORDS],
            data: Vec::from(payload),
            handles: [None; hk_ipc::MAX_MESSAGE_HANDLES],
        };
        let subscribers = core::mem::take(&mut self.fault_subscribers);
        for (slot, handle) in subscribers {
            if !alive.get(slot).copied().unwrap_or(false) {
                continue;
            }
            let tcb = self.tasks[slot].tcb;
            if self.kernel.handle_id(tcb, handle).is_ok() {
                let _ = self.kernel.send(tcb, handle, message.clone());
                self.fault_subscribers.push((slot, handle));
            }
        }
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

    /// 返回当前 CPU 正在执行的任务名。
    pub fn current_task_name(&self, cpu: usize) -> Option<&str> {
        self.current[cpu].map(|index| self.tasks[index].name.as_str())
    }

    /// 判断地址是否为当前任务可读用户页。
    pub fn user_mapped(&self, address: u64) -> bool {
        self.platform.user_mapped(address)
    }

    /// 从当前任务用户内存读取字节。
    pub fn read_user(&self, address: u64, buffer: &mut [u8]) -> bool {
        self.platform.read_user(address, buffer)
    }

    /// 向当前任务用户内存写入字节。
    pub fn write_user(&self, address: u64, data: &[u8]) -> bool {
        self.platform.write_user(address, data)
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
                endpoint: task.leader as u64 + 1,
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

    /// 把端点编号解析为对应的任务线程。
    fn tcb_of_endpoint(&self, endpoint: u64) -> Option<TcbId> {
        let slot = usize::try_from(endpoint).ok()?.checked_sub(1)?;
        self.tasks.get(slot).map(|task| task.tcb)
    }

    fn endpoint_of(&self, tcb: TcbId) -> u64 {
        self.tasks
            .iter()
            .find(|task| task.tcb == tcb)
            .map(|task| task.leader as u64 + 1)
            .unwrap_or(0)
    }

    /// 返回槽位所属进程的主线程槽位。
    fn leader_of(&self, index: usize) -> usize {
        self.tasks.get(index).map_or(index, |task| task.leader)
    }

    /// 唤醒等待目标线程退出的线程。
    fn wake_joiners(&mut self, target: TcbId) {
        let mut kept = Vec::new();
        for (exited, joiner) in core::mem::take(&mut self.joiners) {
            if exited == target {
                self.kernel.wake_thread(joiner);
            } else {
                kept.push((exited, joiner));
            }
        }
        self.joiners = kept;
    }

    /// 退出主线程名下的全部线程。
    fn exit_threads(&mut self, leader: usize) {
        for index in 0..self.tasks.len() {
            if index == leader || self.tasks[index].exited || self.tasks[index].leader != leader {
                continue;
            }
            let tcb = self.tasks[index].tcb;
            self.release_grants(index);
            let _ = self.kernel.exit(tcb);
            self.tasks[index].exited = true;
            self.tasks[index].exit_code = 0;
            self.tasks[index].pending = Pending::None;
            self.wake_joiners(tcb);
        }
    }

    fn install_handle(&mut self, index: usize, message: &Message) -> u64 {
        let Some(handle) = message.handles[0] else {
            return 0;
        };
        self.install_extra_handle(index, handle)
    }

    fn install_extra_handle(&mut self, index: usize, handle: Handle) -> u64 {
        let slot = self.tasks[index].extra.len();
        self.tasks[index].extra.push(Some(handle));
        (EXTRA_HANDLE_BASE + slot + 1) as u64
    }

    fn install_frame(&mut self, index: usize, handle: Handle) -> u64 {
        let slot = self.tasks[index]
            .frames
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.tasks[index].frames.len());
        let map = FrameMap { handle };
        if slot == self.tasks[index].frames.len() {
            self.tasks[index].frames.push(Some(map));
        } else {
            self.tasks[index].frames[slot] = Some(map);
        }
        (FRAME_HANDLE_BASE + slot + 1) as u64
    }

    fn frame_at(&self, index: usize, number: usize) -> Option<(Handle, hk_hal::PhysRange)> {
        if number >= FRAME_HANDLE_BASE {
            let map = self.tasks[index]
                .frames
                .get(number - FRAME_HANDLE_BASE - 1)?
                .as_ref()?;
            let range = self.kernel.frame(self.tasks[index].tcb, map.handle).ok()?;
            return Some((map.handle, range));
        }
        let handle = self.tasks[index]
            .extra
            .get(number.checked_sub(EXTRA_HANDLE_BASE + 1)?)?
            .as_ref()?;
        let range = self.kernel.frame(self.tasks[index].tcb, *handle).ok()?;
        Some((*handle, range))
    }

    fn grant_at(&self, index: usize, number: usize) -> Option<Handle> {
        if let Some(handle) = number
            .checked_sub(GRANT_HANDLE_BASE + 1)
            .and_then(|slot| self.tasks[index].grants.get(slot))
            .copied()
            .flatten()
        {
            return Some(handle);
        }
        let handle = self.tasks[index]
            .extra
            .get(number.checked_sub(EXTRA_HANDLE_BASE + 1)?)
            .copied()
            .flatten()?;
        self.kernel
            .device_grant(self.tasks[index].tcb, handle)
            .ok()?;
        Some(handle)
    }

    fn install_grant(&mut self, index: usize, handle: Handle) -> u64 {
        let slot = self.tasks[index]
            .grants
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.tasks[index].grants.len());
        if slot == self.tasks[index].grants.len() {
            self.tasks[index].grants.push(Some(handle));
        } else {
            self.tasks[index].grants[slot] = Some(handle);
        }
        (GRANT_HANDLE_BASE + slot + 1) as u64
    }

    fn handle_at(&self, index: usize, number: usize) -> Option<Handle> {
        if number >= FRAME_HANDLE_BASE {
            self.frame_at(index, number).map(|(handle, _)| handle)
        } else if number >= GRANT_HANDLE_BASE {
            self.grant_at(index, number)
        } else if number >= EXTRA_HANDLE_BASE {
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
        if number >= GRANT_HANDLE_BASE {
            if let Some(slot) = number
                .checked_sub(GRANT_HANDLE_BASE + 1)
                .and_then(|slot| self.tasks[index].grants.get_mut(slot))
            {
                *slot = None;
            }
        } else if number >= EXTRA_HANDLE_BASE {
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
        Pending::Send { .. } => "send",
        Pending::Recv { .. } => "recv",
        Pending::RecvData { .. } => "recv-data",
        Pending::Notify { .. } => "notify",
        Pending::Call { .. } => "call",
    }
}

#[cfg(test)]
mod tests;
