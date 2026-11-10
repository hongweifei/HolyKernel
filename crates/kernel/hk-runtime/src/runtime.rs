//! 用户态任务运行时：任务表、系统调用分派与任务 Arena。
//!
//! 微内核提供 TCB、端点、句柄与调度；本模块维护用户任务视图、端点编号到句柄
//! 的映射、系统调用语义与 Arena 记账。平台只提供现场保存/恢复、PCI 访问、
//! 计时与 Arena 布局。

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_base::CivilTime;
use hk_checkpoint::Page;
use hk_device::DeviceGrant;
use hk_hal::{MAX_CPUS, SyscallFrame, UserContext, VirtAddr};
use hk_handle::{Handle, HandleTable, ObjectId, Rights};
use hk_ipc::Message;
use hk_mem::hmp::{
    Arena as HmpArena, Capability, CapabilityKind, Charge, HmpError, Ledger, Region, RegionFlags,
};
use hk_syscall::CONSOLE_NOTIFY_INPUT;
use hk_syscall::{
    FAULT_EVENT, FUTEX_CHANGED, SYS_ACPI_EVENTS, SYS_ACPI_LISTEN, SYS_ALLOC, SYS_ARENA_INFO,
    SYS_BOOT_CMDLINE, SYS_CADENCE_INFO, SYS_CADENCE_NAMED, SYS_CADENCE_REVOKE, SYS_CADENCE_SELF,
    SYS_CADENCE_SPLIT, SYS_CADENCE_UPDATE, SYS_CALL, SYS_CALL_TIMEOUT,
    SYS_CAPABILITY_BIND_PROVIDER, SYS_CAPABILITY_COMMIT, SYS_CAPABILITY_CREATE,
    SYS_CAPABILITY_CREATE_LAZY, SYS_CAPABILITY_EXPORT, SYS_CAPABILITY_PHYS, SYS_CAPABILITY_PIN,
    SYS_CAPABILITY_SCANOUT, SYS_CAPACITY_ALLOC, SYS_CAPACITY_FREE, SYS_CAPACITY_INFO,
    SYS_CAPACITY_MERGE, SYS_CAPACITY_SPLIT, SYS_CAPACITY_WINDOWS, SYS_CIVIL_TIME, SYS_CONSOLE_READ,
    SYS_CONSOLE_SUBSCRIBE, SYS_CPU_TOPOLOGY, SYS_DEVICE_GRANT_CREATE, SYS_DEVICE_INFO,
    SYS_DEVICE_MAP, SYS_DMA_MAP, SYS_DMA_SYNC, SYS_EXIT, SYS_FAULT_SUBSCRIBE, SYS_FORK,
    SYS_FRAME_BANK_STATS, SYS_FRAME_INFO, SYS_FRAME_MAP, SYS_FRAME_PIN, SYS_FRAME_RELEASE,
    SYS_FRAME_SHARE, SYS_FRAMEBUFFER_INFO, SYS_FRAMEBUFFER_MAP, SYS_FUTEX_WAIT, SYS_FUTEX_WAKE,
    SYS_HANDLE_CLOSE, SYS_HANDLE_COUNT, SYS_HANDLE_DUPLICATE, SYS_IRQ_BIND, SYS_KERNEL_STATS,
    SYS_LEDGER_ENUM, SYS_LEDGER_INFO, SYS_METER_INFO, SYS_MSI_BIND, SYS_NOTIFY, SYS_OBJECT_LEDGER,
    SYS_PCI_READ, SYS_PCI_WRITE, SYS_RECV, SYS_RECV_DATA, SYS_RECV_DATA_FROM, SYS_RECV_DATA_HANDLE,
    SYS_RECV_DATA_HANDLE_TRY, SYS_RECV_DATA_TIMEOUT, SYS_RECV_DATA_TRY, SYS_REGION_GROW,
    SYS_REGION_INFO, SYS_REGION_MAP, SYS_REGION_PROTECT, SYS_REGION_UNMAP, SYS_REPLY,
    SYS_REPLY_HANDLE, SYS_SELF_NOTIFICATION, SYS_SEND, SYS_SEND_DATA, SYS_SEND_HANDLE,
    SYS_SERVICE_ENDPOINT, SYS_SLEEP, SYS_TASK_ATTACH_CADENCE, SYS_TASK_ATTACH_NAMED,
    SYS_TASK_REGRANT_NAMED, SYS_THERMAL, SYS_THREAD_CREATE, SYS_THREAD_JOIN, SYS_THREAD_SET_TLS,
    SYS_TICKS, SYS_TSC, SYS_TSC_FREQ, SYS_WAIT_NOTIFY, SYS_WAIT_NOTIFY_TRY, SYS_YIELD,
};

use hk_kernel::{Kernel, KernelError, ReplyToken, TcbId, ThreadState};
use user_window::UserWindow;

/// 随消息接收的句柄编号起点。
mod arena;
mod handles;
mod platform;
mod reclaim;
mod state;
mod syscall;
mod task;
mod user_task;
mod user_window;
mod view;

pub use arena::Arena;
pub use platform::{CowOutcome, UserPlatform};
pub use state::{FaultOutcome, Pending, Step, Switch};
pub use user_task::UserTask;
pub(crate) use user_task::{HandleSlot, TaskInit};
pub use view::{RuntimeStats, TaskInfo};

const EXTRA_HANDLE_BASE: usize = 0x1000;
const GRANT_HANDLE_BASE: usize = 0x2000;
const CAPABILITY_HANDLE_BASE: usize = 0x3000;
const REGION_HANDLE_BASE: usize = 0x4000;
pub(crate) const CAPACITY_HANDLE_BASE: usize = 0x5000;
const FRAME_HANDLE_BASE: usize = 0x6000;
const DMA_WINDOW_BYTES: usize = 256 * 1024;

/// 任务持有的共享 Frame。
pub(crate) struct CapabilityMap {
    pub handle: Handle,
    pub charge: Charge,
}

pub(crate) struct RegionMap {
    pub handle: Handle,
    pub capability: Handle,
    pub owner: TcbId,
    pub region: hk_mem::hmp::RegionId,
    /// capability 是否由内核代建（`SYS_FRAME_MAP` / 设备映射），解除映射时一并关闭。
    pub internal_capability: bool,
    pub virt: u64,
    pub len: usize,
    pub offset: u64,
    pub rights: Rights,
    pub flags: hk_mem::hmp::RegionFlags,
    /// 该映射占用的进程记账；解除映射时归还。
    pub charge: Option<Charge>,
}

/// 进程级共享状态；同进程的线程共享 Arena、记账、虚拟窗口与 Region 表。
pub(crate) struct Process {
    pub hmp_arena: HmpArena,
    pub ledger: Ledger,
    pub user_window: UserWindow,
    pub regions: Vec<Option<RegionMap>>,
    pub region_free: Vec<usize>,
    pub arena: Arena,
    /// 覆盖整个 Arena 的惰性匿名 Region；不对外暴露句柄。
    pub arena_region: Option<RegionMap>,
    /// 进程级 COW 私有页覆盖：能力 ObjectId + 页索引 → 私有物理页。
    pub cow_pages: BTreeMap<(u64, usize), hk_hal::PhysRange>,
    /// 进程能力分配使用的 Capacity 句柄；关闭句柄即撤销分配权。
    pub allocation_capacity: Handle,
    pub device_factory: Option<Handle>,
}

impl Process {
    pub(crate) fn new(
        arena_base: u64,
        arena_size: u64,
        ledger_limit: u64,
        allocation_capacity: Handle,
    ) -> Self {
        Self {
            hmp_arena: HmpArena::create(VirtAddr::new(0), hk_base::user::AUTO_WINDOW_LIMIT)
                .expect("用户 Arena 布局有效"),
            ledger: Ledger::create(ledger_limit),
            user_window: UserWindow::new(),
            regions: Vec::new(),
            region_free: Vec::new(),
            arena: Arena::new(arena_base, arena_size),
            arena_region: None,
            cow_pages: BTreeMap::new(),
            allocation_capacity,
            device_factory: None,
        }
    }

    /// 为任务 Arena 建立惰性匿名 Region。
    ///
    /// 装载期不再预映射 Arena：Region 覆盖固定的 VA 窗口，首次访问按页从任务
    /// 容量装入零页；`SYS_ALLOC` 只推进线性分配并计入 Ledger。句柄只在内核侧
    /// 持有，不暴露给用户态。
    pub(crate) fn map_arena(&mut self, kernel: &Kernel, tcb: TcbId) -> Result<(), KernelError> {
        if self.arena_region.is_some() {
            return Ok(());
        }
        let base = self.arena.base();
        let size = self.arena.size();
        let len = usize::try_from(size).map_err(|_| KernelError::Hmp(HmpError::InvalidSize))?;
        let rights = Rights::READ | Rights::WRITE;
        let capability = Capability::new(CapabilityKind::Anonymous, size, hk_hal::PAGE_SIZE, None)
            .map_err(KernelError::Hmp)?;
        let capability_handle = kernel.create_capability(
            tcb,
            capability.clone(),
            None,
            Rights::READ | Rights::WRITE | Rights::ADMIN,
        )?;
        let region = match Region::new(&capability, 0, len, rights, RegionFlags::empty()) {
            Ok(region) => region,
            Err(error) => {
                let _ = kernel.close_handle(tcb, capability_handle);
                return Err(KernelError::Hmp(error));
            }
        };
        let mut region = region;
        if let Err(error) = region.map(VirtAddr::new(base)) {
            let _ = kernel.close_handle(tcb, capability_handle);
            return Err(KernelError::Hmp(error));
        }
        let region_id = region.id();
        if self.hmp_arena.insert(&region).is_err() {
            let _ = kernel.close_handle(tcb, capability_handle);
            return Err(KernelError::Hmp(HmpError::AlreadyMapped));
        }
        let region_handle =
            match kernel.create_region(tcb, region, Rights::READ | Rights::WRITE | Rights::ADMIN) {
                Ok(handle) => handle,
                Err(error) => {
                    let _ = self.hmp_arena.remove(region_id);
                    let _ = kernel.close_handle(tcb, capability_handle);
                    return Err(error);
                }
            };
        self.arena_region = Some(RegionMap {
            handle: region_handle,
            capability: capability_handle,
            owner: tcb,
            region: region_id,
            internal_capability: true,
            virt: base,
            len,
            offset: 0,
            rights,
            flags: RegionFlags::empty(),
            charge: None,
        });
        Ok(())
    }
}

/// 保存的 Native Task 现场；地址空间已释放，页内容保存在序列化镜像中。
pub(crate) struct Checkpoint<C> {
    pub(crate) slot: usize,
    pub(crate) context: C,
    pub(crate) parent: Option<u64>,
    pub(crate) arena_used: u64,
    pub(crate) image: Vec<u8>,
}

/// 捕获但仍未终止的检查点现场。
pub struct CheckpointCapture<C> {
    /// 原任务槽位。
    pub slot: usize,
    /// 寄存器现场。
    pub context: C,
    /// 父任务槽位。
    pub parent: Option<u64>,
    /// Arena 已用字节数。
    pub arena_used: u64,
    /// Cadence 契约快照。
    pub cadence: hk_cadence::CadenceSnapshot,
    /// Meter 计量快照。
    pub meter: hk_cadence::MeterSnapshot,
    /// 用户页。
    pub pages: Vec<Page>,
}

/// 用户态任务运行时。
pub struct UserRuntime<C, P> {
    kernel: Kernel,
    root: u64,
    tasks: Vec<UserTask<C>>,
    processes: Vec<Option<Process>>,
    endpoints: Vec<Option<ObjectId>>,
    notifications: Vec<Option<ObjectId>>,
    checkpoints: Vec<(String, Checkpoint<C>)>,
    /// 等待线程退出的 (目标 TCB, 等待者 TCB)。
    joiners: Vec<(TcbId, TcbId)>,
    /// 休眠 deadline -> 任务槽位。
    sleeping: BTreeMap<u64, Vec<usize>>,
    /// IPC 超时 deadline -> 任务槽位。
    timeouts: BTreeMap<u64, Vec<usize>>,
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
    cpu_busy: [u64; MAX_CPUS],
    run_start: [u64; MAX_CPUS],
    irq_routes: [u16; 256],
    console_subscriber: Option<usize>,
    platform: P,
}

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 以 Kernel、任务与平台适配构造运行时。
    pub(crate) fn new(
        kernel: Kernel,
        root: u64,
        endpoints: Vec<Option<ObjectId>>,
        notifications: Vec<Option<ObjectId>>,
        mut tasks: Vec<UserTask<C>>,
        processes: Vec<Option<Process>>,
        platform: P,
    ) -> Self {
        for (index, task) in tasks.iter_mut().enumerate() {
            task.leader = index;
        }
        Self {
            kernel,
            root,
            tasks,
            processes,
            endpoints,
            notifications,
            checkpoints: Vec::new(),
            joiners: Vec::new(),
            sleeping: BTreeMap::new(),
            timeouts: BTreeMap::new(),
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
            cpu_busy: [0; MAX_CPUS],
            run_start: [0; MAX_CPUS],
            irq_routes: [0; 256],
            console_subscriber: None,
            platform,
        }
    }

    /// 返回任务数量。
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// 返回任务所属进程。
    pub(crate) fn process(&self, index: usize) -> &Process {
        self.processes[self.tasks[index].process]
            .as_ref()
            .expect("任务所属进程存在")
    }

    /// 返回任务所属进程的可变引用。
    pub(crate) fn process_mut(&mut self, index: usize) -> &mut Process {
        let process = self.tasks[index].process;
        self.processes[process].as_mut().expect("任务所属进程存在")
    }

    /// 判断是否有等待调度的线程。
    pub fn has_runnable(&self) -> bool {
        self.kernel.has_runnable()
    }

    /// 是否有尚未到期的休眠任务。
    pub fn has_sleepers(&self) -> bool {
        !self.sleeping.is_empty() || !self.timeouts.is_empty()
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
            cpu_busy: self.cpu_busy,
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

    /// 授予具名任务的调度策略授权；用于 shell 启动 Tempo。
    pub fn grant_policy_authority(&mut self, name: &str) -> bool {
        let Some(task) = self.tasks.iter().find(|task| task.name == name) else {
            return false;
        };
        let Some(task_id) = self.kernel.task_of(task.tcb) else {
            return false;
        };
        self.kernel.set_policy_authority(task_id, true).is_ok()
    }

    /// 注册跨核抢占回调。
    pub fn set_resched_hook(&self, hook: fn(usize)) {
        self.kernel.set_resched_hook(hook);
    }

    /// 设置调度器可见的在线 CPU 数。
    pub fn set_online_cpus(&self, online: usize) {
        self.kernel.set_online_cpus(online);
    }

    /// 为每个在线 CPU 建立 idle TCB。
    pub fn enable_idle_cpus(&self, online: usize) {
        self.kernel.enable_idle_cpus(online);
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
            if let Some(slot) = task.capacities.iter().position(Option::is_some) {
                task.context
                    .set_start_capacity(CAPACITY_HANDLE_BASE as u64 + slot as u64 + 1);
            }
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
            .map(|(index, task)| {
                let cadence = self.kernel.task_cadence_snapshot(task.tcb);
                let meter = self.kernel.task_meter_snapshot(task.tcb);
                TaskInfo {
                    tcb: task.tcb.get(),
                    name: task.name.clone(),
                    parent: task.parent,
                    endpoint: task.leader as u64 + 1,
                    state: state_name(self.kernel.state_of(task.tcb)),
                    pending: pending_name(task.pending),
                    handles: self.kernel.handles_live(task.tcb).unwrap_or(0),
                    cpu_cycles: task.cpu_cycles,
                    class: cadence.map_or("missing", |cadence| cadence.class.name()),
                    weight: cadence.map_or(0, |cadence| cadence.weight),
                    budget: cadence.map_or(0, |cadence| cadence.budget),
                    period: cadence.map_or(0, |cadence| cadence.period),
                    deadline: cadence.map_or(0, |cadence| cadence.deadline),
                    consumed: meter.map_or(0, |meter| meter.consumed),
                    waited: meter.map_or(0, |meter| meter.waited),
                    virtual_pass: meter.map_or(0, |meter| meter.virtual_pass),
                    throttled: meter.map_or(0, |meter| meter.throttled),
                    overruns: meter.map_or(0, |meter| meter.overruns),
                    switches: meter.map_or(0, |meter| meter.switches),
                    migrations: meter.map_or(0, |meter| meter.migrations),
                    last_cpu: meter.map_or(u32::MAX, |meter| meter.last_cpu),
                    arena_used: self.process(index).arena.used(),
                    arena_size: self.process(index).arena.size(),
                    ledger_charged: self.process(index).ledger.charged(),
                    ledger_resident: self.process(index).ledger.resident(),
                    vspace: self.platform.vspace(index),
                    exit_code: task.exited.then_some(task.exit_code),
                }
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
            self.release_task_charges(index);
            let _ = self.kernel.exit(tcb);
            self.tasks[index].exited = true;
            self.tasks[index].exit_code = 0;
            self.tasks[index].pending = Pending::None;
            self.wake_joiners(tcb);
            self.platform.release_pages(index);
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
        Pending::ProviderFault { .. } => "provider-fault",
        Pending::Sleep { .. } => "sleep",
    }
}

#[cfg(test)]
mod tests;
