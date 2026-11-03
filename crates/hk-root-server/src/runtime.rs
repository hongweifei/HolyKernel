//! 用户态任务运行时：任务表、系统调用分派与任务 Arena。
//!
//! 微内核提供 TCB、端点、句柄与调度；本模块维护用户任务视图、端点编号到句柄
//! 的映射、系统调用语义与 Arena 记账。平台只提供现场保存/恢复、PCI 访问、
//! 计时与 Arena 布局。

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use hk_hal::{SyscallFrame, UserContext};
use hk_handle::{Handle, HandleTable, ObjectId, Rights};
use hk_ipc::Message;
use hk_syscall::{
    SYS_ALLOC, SYS_EXIT, SYS_HANDLE_CLOSE, SYS_HANDLE_COUNT, SYS_NOTIFY, SYS_PCI_READ, SYS_RECV,
    SYS_SEND, SYS_SEND_HANDLE, SYS_TICKS, SYS_WAIT_NOTIFY, SYS_YIELD,
};

use hk_kernel::{Kernel, KernelError, Priority, TcbId, ThreadState};

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
    /// 通知等待已登记。
    Notify {
        /// 通知句柄。
        handle: Handle,
        /// 等待的通知位。
        mask: u64,
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
    /// 把用户地址区间清零。
    fn zero(&self, address: u64, length: usize);
    /// 返回任务槽位的 Arena 基址与大小。
    fn arena_layout(&self, slot: usize) -> (u64, u64);
}

/// 任务 Arena：线性分配，不回收。
pub struct Arena {
    base: u64,
    next: u64,
    end: u64,
}

impl Arena {
    /// 以基址与大小构造 Arena。
    pub const fn new(base: u64, size: u64) -> Self {
        Self {
            base,
            next: base,
            end: base + size,
        }
    }

    /// 分配一段对齐区间，返回用户虚拟地址。
    pub fn allocate(&mut self, size: usize, align: usize) -> Option<u64> {
        if size == 0 || !align.is_power_of_two() {
            return None;
        }

        let align = align.max(8) as u64;
        let start = (self.next + align - 1) & !(align - 1);
        let end = start.checked_add(size as u64)?;
        if end > self.end {
            return None;
        }
        self.next = end;
        Some(start)
    }

    /// 返回已用字节数。
    pub fn used(&self) -> u64 {
        self.next - self.base
    }

    /// 返回总字节数。
    pub fn size(&self) -> u64 {
        self.end - self.base
    }
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
    /// 用户现场。
    pub context: C,
    /// 挂起的系统调用。
    pub pending: Pending,
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
            context,
            pending: Pending::None,
            arena: Arena::new(arena_base, arena_size),
            exited: false,
            exit_code: 0,
        }
    }
}

/// 用户态任务运行时。
pub struct UserRuntime<C, P> {
    kernel: Kernel,
    root: u64,
    tasks: Vec<UserTask<C>>,
    endpoints: Vec<Option<ObjectId>>,
    notifications: Vec<Option<ObjectId>>,
    current: Option<usize>,
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
            current: None,
            platform,
        }
    }

    /// 返回任务数量。
    pub fn task_count(&self) -> usize {
        self.tasks.len()
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

    /// 设置任务的用户态初始现场。
    pub fn attach(&mut self, index: usize, context: C) {
        if let Some(task) = self.tasks.get_mut(index) {
            task.context = context;
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
                exit_code: task.exited.then_some(task.exit_code),
            })
            .collect()
    }

    /// 装载一个新任务，返回分配的端点编号。
    pub fn start(
        &mut self,
        name: &str,
        mut context: C,
        parent: Option<u64>,
    ) -> Result<u64, KernelError> {
        let index = self.tasks.len();
        let endpoint = index as u64 + 1;
        context.set_start_argument(endpoint);

        let kernel = &self.kernel;
        let tcb = kernel.spawn_thread(HandleTable::with_default_quota(), Priority::NORMAL);
        let own = kernel.create_endpoint(tcb, Rights::READ | Rights::WRITE | Rights::TRANSFER)?;
        let own_id = kernel.handle_id(tcb, own)?;
        self.endpoints.push(Some(own_id));

        let own_notification =
            kernel.create_notification(tcb, Rights::READ | Rights::WRITE | Rights::TRANSFER)?;
        let own_notification_id = kernel.handle_id(tcb, own_notification)?;
        self.notifications.push(Some(own_notification_id));

        let mut handles = Vec::with_capacity(self.endpoints.len());
        for (slot, object) in self.endpoints.iter().enumerate() {
            let handle = if slot == index {
                Some(own)
            } else {
                match object {
                    Some(id) => kernel
                        .grant(tcb, *id, Rights::READ | Rights::WRITE | Rights::TRANSFER)
                        .ok(),
                    None => None,
                }
            };
            handles.push(handle);
        }

        let mut notifications = Vec::with_capacity(self.notifications.len());
        for (slot, object) in self.notifications.iter().enumerate() {
            let handle = if slot == index {
                Some(own_notification)
            } else {
                match object {
                    Some(id) => kernel
                        .grant(tcb, *id, Rights::READ | Rights::WRITE | Rights::TRANSFER)
                        .ok(),
                    None => None,
                }
            };
            notifications.push(handle);
        }

        for task in &mut self.tasks {
            if task.exited {
                continue;
            }
            let handle = kernel
                .grant(
                    task.tcb,
                    own_id,
                    Rights::READ | Rights::WRITE | Rights::TRANSFER,
                )
                .ok();
            task.handles.push(handle);
            let handle = kernel
                .grant(
                    task.tcb,
                    own_notification_id,
                    Rights::READ | Rights::WRITE | Rights::TRANSFER,
                )
                .ok();
            task.notifications.push(handle);
        }

        let (arena_base, arena_size) = self.platform.arena_layout(index);
        self.tasks.push(UserTask::new(
            tcb,
            name,
            parent,
            handles,
            notifications,
            context,
            arena_base,
            arena_size,
        ));
        Ok(endpoint)
    }

    /// 调度一步；调用方在锁外恢复 `context`，再调用 [`Self::finish`]。
    pub fn step(&mut self) -> Step<C> {
        let Some(index) = self.next_task() else {
            return Step::Done;
        };

        if self.prepare(index) {
            return Step::Blocked;
        }

        Step::Resume {
            index,
            context: self.tasks[index].context,
        }
    }

    /// 恢复执行返回后收尾；任务退出时关闭其他任务持有的该端点句柄，并返回名称与退出码。
    pub fn finish(&mut self, index: usize) -> Option<(String, u64)> {
        self.current = None;
        if !self.tasks[index].exited {
            return None;
        }

        for other in 0..self.tasks.len() {
            if other == index || self.tasks[other].exited {
                continue;
            }
            if let Some(handle) = self.tasks[other].handles.get(index).copied().flatten() {
                let _ = self.kernel.close_handle(self.tasks[other].tcb, handle);
                self.tasks[other].handles[index] = None;
            }
            if let Some(handle) = self.tasks[other]
                .notifications
                .get(index)
                .copied()
                .flatten()
            {
                let _ = self.kernel.close_handle(self.tasks[other].tcb, handle);
                self.tasks[other].notifications[index] = None;
            }
        }

        let task = &self.tasks[index];
        Some((task.name.clone(), task.exit_code))
    }

    /// 分派一次系统调用。
    pub fn dispatch(&mut self, frame: &mut C) -> Switch {
        let Some(index) = self.current else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let tcb = self.tasks[index].tcb;

        match frame.number() {
            SYS_SEND => self.send(index, tcb, frame, false),
            SYS_SEND_HANDLE => self.send(index, tcb, frame, true),
            SYS_RECV => self.recv(index, tcb, frame),
            SYS_NOTIFY => {
                let result = self.notify(index, tcb, frame);
                frame.set_result(result);
                Switch::Continue
            }
            SYS_WAIT_NOTIFY => self.wait_notify(index, tcb, frame),
            SYS_YIELD => {
                self.tasks[index].context = *frame;
                self.tasks[index].pending = Pending::None;
                Switch::Block
            }
            SYS_EXIT => {
                let code = frame.arg0();
                let _ = self.kernel.exit(tcb);
                self.tasks[index].exited = true;
                self.tasks[index].exit_code = code;
                Switch::Exit
            }
            SYS_PCI_READ => {
                let address = frame.arg0();
                let bus = (address >> 8) as u8;
                let device = ((address >> 3) & 0x1F) as u8;
                let function = (address & 0x07) as u8;
                frame.set_result(u64::from(self.platform.pci_read(
                    bus,
                    device,
                    function,
                    frame.arg1() as u8,
                )));
                Switch::Continue
            }
            SYS_TICKS => {
                frame.set_result(self.platform.timer_ticks());
                Switch::Continue
            }
            SYS_ALLOC => {
                let address = self
                    .allocate(index, frame.arg0() as usize, frame.arg1() as usize)
                    .unwrap_or(0);
                frame.set_result(address);
                Switch::Continue
            }
            SYS_HANDLE_COUNT => {
                frame.set_result(self.kernel.handles_live(tcb).unwrap_or(0) as u64);
                Switch::Continue
            }
            SYS_HANDLE_CLOSE => {
                let result = self.close_handle(index, tcb, frame.arg0() as usize);
                frame.set_result(result);
                Switch::Continue
            }
            _ => {
                frame.set_result(u64::MAX);
                Switch::Continue
            }
        }
    }

    /// 命中调度 tick 时保存现场；返回是否切换任务。
    pub fn preempt(&mut self, context: C) -> bool {
        let Some(index) = self.current else {
            return false;
        };
        self.tasks[index].context = context;
        self.tasks[index].pending = Pending::None;
        true
    }

    fn next_task(&mut self) -> Option<usize> {
        let tcb = self.kernel.schedule()?;
        self.tasks.iter().position(|task| task.tcb == tcb)
    }

    fn prepare(&mut self, index: usize) -> bool {
        self.current = Some(index);

        let pending = self.tasks[index].pending;
        self.tasks[index].pending = Pending::None;
        let mut blocked_again = false;

        match pending {
            Pending::Recv { handle } => {
                let tcb = self.tasks[index].tcb;
                match self.kernel.recv(tcb, handle) {
                    Ok(Some(received)) => {
                        let sender = self.endpoint_of(received.sender);
                        let handle = self.install_handle(index, &received.message);
                        let context = &mut self.tasks[index].context;
                        context.set_result(received.message.label);
                        context.set_word(received.message.words[0]);
                        context.set_sender(sender);
                        context.set_handle(handle);
                    }
                    Ok(None) => blocked_again = true,
                    Err(_) => self.tasks[index].context.set_result(u64::MAX),
                }
            }
            Pending::Notify { handle, mask } => {
                let tcb = self.tasks[index].tcb;
                match self.kernel.wait(tcb, handle, mask) {
                    Ok(Some(bits)) => self.tasks[index].context.set_result(bits),
                    Ok(None) => blocked_again = true,
                    Err(_) => self.tasks[index].context.set_result(u64::MAX),
                }
            }
            Pending::None | Pending::Send => {}
        }

        if blocked_again {
            self.tasks[index].pending = pending;
            self.current = None;
        }

        blocked_again
    }

    fn endpoint_of(&self, tcb: TcbId) -> u64 {
        self.tasks
            .iter()
            .position(|task| task.tcb == tcb)
            .map(|index| index as u64 + 1)
            .unwrap_or(0)
    }

    fn send(&mut self, index: usize, tcb: TcbId, frame: &mut C, transfer: bool) -> Switch {
        let endpoint = frame.arg0() as usize;
        let Some(handle) = self.tasks[index]
            .handles
            .get(endpoint.wrapping_sub(1))
            .copied()
            .flatten()
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };

        let transferred = if transfer {
            let slot = frame.arg3() as usize;
            match self.tasks[index]
                .handles
                .get(slot.wrapping_sub(1))
                .copied()
                .flatten()
            {
                Some(transferred) => Some((slot.wrapping_sub(1), transferred)),
                None => {
                    frame.set_result(u64::MAX);
                    return Switch::Continue;
                }
            }
        } else {
            None
        };

        let mut words = [0; 8];
        words[0] = frame.arg2();
        let mut message = Message::new(frame.arg1()).with_words(words);
        if let Some((_, transferred)) = transferred {
            message = message.with_handles([Some(transferred), None, None, None]);
        }

        match self.kernel.send(tcb, handle, message) {
            Ok(()) => {
                if self.kernel.state_of(tcb) == Some(ThreadState::Blocked) {
                    let mut context = *frame;
                    context.set_result(0);
                    self.tasks[index].context = context;
                    self.tasks[index].pending = Pending::Send;
                    Switch::Block
                } else {
                    if let Some((slot, _)) = transferred {
                        self.tasks[index].handles[slot] = None;
                    }
                    frame.set_result(0);
                    Switch::Continue
                }
            }
            Err(_) => {
                frame.set_result(u64::MAX);
                Switch::Continue
            }
        }
    }

    fn recv(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let endpoint = frame.arg0() as usize;
        let Some(handle) = self.tasks[index]
            .handles
            .get(endpoint.wrapping_sub(1))
            .copied()
            .flatten()
        else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };

        match self.kernel.recv(tcb, handle) {
            Ok(Some(received)) => {
                let sender = self.endpoint_of(received.sender);
                let handle = self.install_handle(index, &received.message);
                frame.set_result(received.message.label);
                frame.set_word(received.message.words[0]);
                frame.set_sender(sender);
                frame.set_handle(handle);
                Switch::Continue
            }
            Ok(None) => {
                let mut context = *frame;
                context.set_result(0);
                self.tasks[index].context = context;
                self.tasks[index].pending = Pending::Recv { handle };
                Switch::Block
            }
            Err(_) => {
                frame.set_result(u64::MAX);
                Switch::Continue
            }
        }
    }

    fn allocate(&mut self, index: usize, size: usize, align: usize) -> Option<u64> {
        let start = self.tasks[index].arena.allocate(size, align)?;
        self.platform.zero(start, size);
        Some(start)
    }

    fn notification_handle(&self, index: usize, number: usize) -> Option<Handle> {
        self.tasks[index]
            .notifications
            .get(number.wrapping_sub(1))
            .copied()
            .flatten()
    }

    fn notify(&self, index: usize, tcb: TcbId, frame: &mut C) -> u64 {
        let number = frame.arg0() as usize;
        let Some(handle) = self.notification_handle(index, number) else {
            return u64::MAX;
        };
        match self.kernel.signal(tcb, handle, frame.arg1()) {
            Ok(()) => 0,
            Err(_) => u64::MAX,
        }
    }

    fn wait_notify(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let mask = frame.arg1();
        let Some(handle) = self.notification_handle(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };

        match self.kernel.wait(tcb, handle, mask) {
            Ok(Some(bits)) => {
                frame.set_result(bits);
                Switch::Continue
            }
            Ok(None) => {
                let mut context = *frame;
                context.set_result(0);
                self.tasks[index].context = context;
                self.tasks[index].pending = Pending::Notify { handle, mask };
                Switch::Block
            }
            Err(_) => {
                frame.set_result(u64::MAX);
                Switch::Continue
            }
        }
    }

    /// 把随消息收到的句柄加入任务句柄映射，返回新端点编号；没有句柄时为 0。
    fn install_handle(&mut self, index: usize, message: &Message) -> u64 {
        let Some(handle) = message.handles[0] else {
            return 0;
        };
        self.tasks[index].handles.push(Some(handle));
        self.tasks[index].handles.len() as u64
    }

    fn close_handle(&mut self, index: usize, tcb: TcbId, endpoint: usize) -> u64 {
        let slot = endpoint.wrapping_sub(1);
        let Some(handle) = self.tasks[index].handles.get(slot).copied().flatten() else {
            return u64::MAX;
        };

        match self.kernel.close_handle(tcb, handle) {
            Ok(()) => {
                self.tasks[index].handles[slot] = None;
                0
            }
            Err(_) => u64::MAX,
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
        Pending::Notify { .. } => "notify",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    /// 最小用户现场。
    #[derive(Clone, Copy, Default)]
    struct MockContext {
        rax: u64,
        rdi: u64,
        rsi: u64,
        rdx: u64,
        rcx: u64,
        r8: u64,
        r10: u64,
    }

    impl UserContext for MockContext {
        fn resume(&self) {}

        fn set_start_argument(&mut self, argument: u64) {
            self.rdi = argument;
        }
    }

    impl SyscallFrame for MockContext {
        fn number(&self) -> u64 {
            self.rax
        }

        fn arg0(&self) -> u64 {
            self.rdi
        }

        fn arg1(&self) -> u64 {
            self.rsi
        }

        fn arg2(&self) -> u64 {
            self.rdx
        }

        fn arg3(&self) -> u64 {
            self.r10
        }

        fn set_result(&mut self, value: u64) {
            self.rax = value;
        }

        fn set_word(&mut self, value: u64) {
            self.rdx = value;
        }

        fn set_sender(&mut self, value: u64) {
            self.rcx = value;
        }

        fn set_handle(&mut self, value: u64) {
            self.r8 = value;
        }
    }

    struct MockPlatform;

    impl UserPlatform for MockPlatform {
        fn timer_ticks(&self) -> u64 {
            7
        }

        fn pci_read(&self, _bus: u8, _device: u8, _function: u8, _offset: u8) -> u32 {
            0x1234
        }

        fn zero(&self, _address: u64, _length: usize) {}

        fn arena_layout(&self, slot: usize) -> (u64, u64) {
            (0x10_0000 + slot as u64 * 0x2_0000, 0x1_0000)
        }
    }

    fn runtime() -> UserRuntime<MockContext, MockPlatform> {
        crate::boot::user_runtime(&["a", "b"], MockPlatform, |_index, endpoint| MockContext {
            rdi: endpoint,
            ..MockContext::default()
        })
        .expect("装配运行时")
    }

    #[test]
    fn arena_aligns_and_exhausts() {
        let mut arena = Arena::new(0x1000, 4096);
        assert_eq!(arena.allocate(16, 8), Some(0x1000));
        assert_eq!(arena.allocate(8, 64), Some(0x1040));
        assert_eq!(arena.used(), 0x48);
        assert_eq!(arena.allocate(4096, 8), None);
        assert_eq!(arena.allocate(0, 8), None);
        assert_eq!(arena.allocate(8, 3), None);
    }

    #[test]
    fn dispatches_alloc_and_exit() {
        let mut runtime = runtime();
        assert_eq!(runtime.task_count(), 2);

        let Step::Resume { index, mut context } = runtime.step() else {
            panic!("任务应可运行");
        };

        context.rax = SYS_ALLOC;
        context.rdi = 64;
        context.rsi = 16;
        assert_eq!(runtime.dispatch(&mut context), Switch::Continue);
        assert_eq!(context.rax, 0x10_0000);

        context.rax = SYS_EXIT;
        context.rdi = 3;
        assert_eq!(runtime.dispatch(&mut context), Switch::Exit);
        assert_eq!(runtime.finish(index), Some((String::from("a"), 3)));
    }

    #[test]
    fn transfers_handle_over_ipc() {
        let mut runtime = runtime();

        let Step::Resume { index: sender, .. } = runtime.step() else {
            panic!("任务应可运行");
        };
        let mut context = MockContext {
            rax: SYS_YIELD,
            ..MockContext::default()
        };
        assert_eq!(runtime.dispatch(&mut context), Switch::Block);

        let Step::Resume {
            index: receiver,
            mut context,
        } = runtime.step()
        else {
            panic!("任务应可运行");
        };
        assert_eq!(receiver, 1);
        context.rax = SYS_RECV;
        context.rdi = 2;
        assert_eq!(runtime.dispatch(&mut context), Switch::Block);

        let Step::Resume {
            index: sender_again,
            mut context,
        } = runtime.step()
        else {
            panic!("任务应可运行");
        };
        assert_eq!(sender_again, sender);
        context.rax = SYS_SEND_HANDLE;
        context.rdi = 2;
        context.rsi = 9;
        context.rdx = 5;
        context.r10 = 2;
        assert_eq!(runtime.dispatch(&mut context), Switch::Continue);
        assert_eq!(runtime.tasks()[receiver].state, "runnable");

        let Step::Resume { index, context } = runtime.step() else {
            panic!("任务应可运行");
        };
        assert_eq!(index, receiver);
        assert_eq!(context.rax, 9);
        assert_eq!(context.rdx, 5);
        assert_eq!(context.rcx, 1);
        assert_eq!(context.r8, 3);
        assert_eq!(runtime.tasks()[receiver].handles, 5);
    }

    #[test]
    fn waits_for_notification() {
        let mut runtime = runtime();

        let Step::Resume {
            index: waiter,
            mut context,
        } = runtime.step()
        else {
            panic!("任务应可运行");
        };
        assert_eq!(waiter, 0);
        context.rax = SYS_WAIT_NOTIFY;
        context.rdi = 1;
        context.rsi = 1;
        assert_eq!(runtime.dispatch(&mut context), Switch::Block);
        assert_eq!(runtime.tasks()[waiter].pending, "notify");

        let Step::Resume {
            index: signaler,
            mut context,
        } = runtime.step()
        else {
            panic!("任务应可运行");
        };
        assert_eq!(signaler, 1);
        context.rax = SYS_NOTIFY;
        context.rdi = 1;
        context.rsi = 1;
        assert_eq!(runtime.dispatch(&mut context), Switch::Continue);
        assert_eq!(context.rax, 0);

        let Step::Resume { index, context } = runtime.step() else {
            panic!("任务应可运行");
        };
        assert_eq!(index, waiter);
        assert_eq!(context.rax, 1);
        assert_eq!(runtime.tasks()[waiter].pending, "-");
    }

    #[test]
    fn exit_releases_peer_endpoint_handles() {
        let mut runtime = runtime();
        let Step::Resume { index, mut context } = runtime.step() else {
            panic!("任务应可运行");
        };
        let other = 1 - index;
        let before = runtime.tasks()[other].handles;
        assert_eq!(before, 4);

        context.rax = SYS_EXIT;
        context.rdi = 0;
        assert_eq!(runtime.dispatch(&mut context), Switch::Exit);
        assert!(runtime.finish(index).is_some());

        let tasks = runtime.tasks();
        assert_eq!(tasks[index].handles, 0);
        assert_eq!(tasks[other].handles, before - 2);

        let endpoint = runtime
            .start("c", MockContext::default(), None)
            .expect("启动任务");
        assert_eq!(endpoint, 3);

        let tasks = runtime.tasks();
        assert_eq!(tasks[index].handles, 0);
        assert_eq!(tasks[other].handles, before);
        // 退出的端点与通知对象已释放，不再授予新任务。
        assert_eq!(tasks[2].handles, 4);
    }
}
