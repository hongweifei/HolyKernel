//! 用户态任务运行时：任务表、系统调用分派与任务 Arena。
//!
//! 微内核提供 TCB、端点、句柄与调度；本模块维护用户任务视图、端点编号到句柄
//! 的映射、系统调用语义与 Arena 记账。平台只提供现场保存/恢复、PCI 访问、
//! 计时与 Arena 布局。

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use hk_checkpoint::Page;
use hk_hal::{MAX_CPUS, SyscallFrame, UserContext, VirtAddr};
use hk_handle::{Handle, ObjectId, Rights};
use hk_ipc::Message;
use hk_mem::hmp::{Arena as HmpArena, Charge, FrameRef, SharedLedger};
use hk_syscall::FAULT_EVENT;

use hk_kernel::{Kernel, KernelError, TaskState, TcbId};
use user_window::UserWindow;

/// 随消息接收的句柄编号起点。
mod handles;
mod numa;
mod objects;
mod oom;
mod oom_guard;
mod platform;
mod reclaim;
mod state;
mod syscall;
mod task;
mod user_task;
mod user_window;
mod view;

pub use hk_mem::hmp::provider::ProviderAccess;
pub use platform::{CowOutcome, SystemArenaInfo, SystemRegionRecord, UserPlatform};
pub use state::{FaultOutcome, Pending, Step, Switch};
pub use user_task::UserTask;
pub(crate) use user_task::{HandleSlot, TaskInit};
pub use view::{RuntimeStats, TaskInfo};

const EXTRA_HANDLE_BASE: usize = 0x1000;
const GRANT_HANDLE_BASE: usize = 0x2000;
const CAPABILITY_HANDLE_BASE: usize = 0x3000;
const REGION_HANDLE_BASE: usize = 0x4000;
/// Region 用户句柄的代数命名空间；低值槽位号只在第一代使用。
pub(crate) const REGION_USER_BASE: u64 = 0x1_0000_0000;
pub(crate) const CAPACITY_HANDLE_BASE: usize = 0x5000;
const FRAME_HANDLE_BASE: usize = 0x6000;
const DMA_WINDOW_BYTES: usize = 256 * 1024;

/// 用户态 Image Server 的段注册记录，供服务重启后重放。
#[derive(Clone, Copy, Debug)]
struct ImageRegistration {
    object: u64,
    module: u64,
    file_offset: u64,
    file_len: u64,
    provider: usize,
}

/// 启动模块的只读物理来源。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleRecord {
    /// 模块服务名。
    pub name: alloc::string::String,
    /// 模块文件物理区间。
    pub range: hk_hal::PhysRange,
}

/// 用户态 Image Server 的段注册信息。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageProviderSpec {
    /// 提供者任务槽位（端点编号 = 槽位 + 1）。
    pub endpoint: usize,
    /// 模块在启动模块表中的索引。
    pub module: usize,
    /// 段文件在模块中的偏移。
    pub file_offset: usize,
    /// 段文件长度。
    pub file_len: usize,
}

/// 装载器交付的镜像文件来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageSource {
    /// 段文件内容的起始物理地址。
    pub phys: hk_hal::PhysAddr,
    /// 段文件内容长度；其余页按零填充。
    pub file_len: usize,
}

/// 装载器交付的用户映射区间。
///
/// 镜像段与用户栈在装载期已经由平台映射；运行时按该描述建立对应的 Region，
/// 使这些映射进入对象面，而不是只存在于页表中。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserRegionSpec {
    /// 用户虚拟地址。
    pub virt: u64,
    /// 按页对齐的区间长度。
    pub len: usize,
    /// 是否可写。
    pub writable: bool,
    /// 是否可执行。
    pub executable: bool,
    /// 懒装载的可执行段来源；`None` 表示装载期已映射。
    pub source: Option<ImageSource>,
    /// 用户态 Image Server 注册信息。
    pub provider: Option<ImageProviderSpec>,
}

/// Provider 提供的 COW 私有页后备。
///
/// 该对象由父进程与 fork 子进程共享；最后一个引用释放时才归还物理页。
pub struct CowBacking {
    /// 私有页物理后备。
    pub backing: hk_mem::hmp::FrameBacking,
}

/// 进程私有 COW 页；记账统一由映射该页的 Region `page_charges` 持有，
/// fork 复制出的子进程条目与父进程共享同一份 backing。
pub struct CowPage {
    /// 私有物理页区间。
    pub range: hk_hal::PhysRange,
    /// Provider 提供的私有页；内核 COW 页由页表持有，这里为 `None`。
    pub backing: Option<Arc<CowBacking>>,
}

/// 任务持有的共享 Frame。
pub(crate) struct CapabilityMap {
    pub handle: Handle,
    /// 已常驻物理页的记账；惰性能力在缺页装页前为 `None`。
    pub charge: Option<FrameRef>,
}

pub(crate) struct RegionMap {
    /// 返回给用户态的不可复用编号（槽位 + 代数）。
    pub user: u64,
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
    /// 映射这段 Region 时使用的缓存属性，随能力确定。
    pub cache: hk_hal::CachePolicy,
    /// 已常驻物理页的整段记账；惰性映射为 `None`。
    pub charge: Option<FrameRef>,
    /// 逐页装入与 COW 私有页产生的记账，键为能力内页索引。
    pub page_charges: Vec<(usize, FrameRef)>,
}

/// 进程级共享状态；同进程的线程共享 Arena、记账、虚拟窗口与 Region 表。
pub(crate) struct Process {
    pub hmp_arena: HmpArena,
    pub ledger: SharedLedger,
    /// 本进程在全局 Frame 账本中的作用域。
    pub frame_scope: u64,
    pub user_window: UserWindow,
    pub regions: Vec<Option<RegionMap>>,
    pub region_free: Vec<usize>,
    /// 当前 Region 用户句柄代数；槽位复用时不复用旧编号。
    pub region_generation: u64,
    /// 任务 Arena 的虚拟区间与线性分配游标。
    pub arena_base: u64,
    pub arena_size: u64,
    pub arena_next: u64,
    /// 进程级 COW 私有页覆盖：能力 ObjectId + 页索引 → 私有物理页与记账。
    pub cow_pages: BTreeMap<(u64, usize), CowPage>,
    /// fork 时为子任务复制私有页产生的逐帧记账；任务退出时归还。
    pub cow_charges: Vec<FrameRef>,
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
        let address_size = hk_base::user::AUTO_WINDOW_LIMIT.saturating_sub(arena_base);
        Self {
            hmp_arena: HmpArena::create(VirtAddr::new(arena_base), address_size)
                .expect("用户 Arena 布局有效"),
            ledger: SharedLedger::create(ledger_limit),
            frame_scope: hk_mem::hmp::frame_ledger::scope(),
            user_window: UserWindow::new(),
            regions: Vec::new(),
            region_free: Vec::new(),
            region_generation: 0,
            arena_base,
            arena_size,
            arena_next: arena_base,
            cow_pages: BTreeMap::new(),
            cow_charges: Vec::new(),
            allocation_capacity,
            device_factory: None,
        }
    }

    /// 登记一份进程私有 COW 页；同一能力页只允许一个覆盖。
    pub(crate) fn insert_cow_page(&mut self, key: (u64, usize), page: CowPage) -> bool {
        if self.cow_pages.contains_key(&key) {
            return false;
        }
        self.cow_pages.insert(key, page);
        true
    }

    /// 在任务 Arena 中线性分配一段页对齐区间。
    pub(crate) fn allocate_arena(&mut self, size: usize, align: usize) -> Option<(u64, usize)> {
        if size == 0 || !align.is_power_of_two() {
            return None;
        }
        let page = hk_hal::PAGE_SIZE as u64;
        let align = (align.max(8) as u64).max(page);
        let len = (size as u64).div_ceil(page) * page;
        let start = (self.arena_next + align - 1) & !(align - 1);
        let end = start.checked_add(len)?;
        if end > self.arena_base.checked_add(self.arena_size)? {
            return None;
        }
        self.arena_next = end;
        Some((start, len as usize))
    }

    /// 扩展 `SYS_ALLOC` 使用的 Arena 地址窗口。
    ///
    /// Arena 与同槽位的栈保持固定间隔；限制是“从 ARENA_BASE 到 STACK_BASE
    /// 的窗口长度”，不能按 `STACK_BASE - arena_base` 计算——后者会让非零
    /// 槽位的 Arena 永远无法增长。
    pub(crate) fn grow_arena(&mut self, additional: u64) -> Option<u64> {
        let page = hk_hal::PAGE_SIZE as u64;
        if additional == 0 || !additional.is_multiple_of(page) {
            return None;
        }
        let limit = hk_base::user::STACK_BASE
            .saturating_sub(hk_base::user::ARENA_BASE)
            .max(hk_base::user::ARENA_SIZE);
        let size = self.arena_size.checked_add(additional)?;
        if size > limit {
            return None;
        }
        self.arena_size = size;
        Some(size)
    }

    /// 返回 Arena 已分配的虚拟字节数。
    pub(crate) fn arena_used(&self) -> u64 {
        self.arena_next.saturating_sub(self.arena_base)
    }

    /// 恢复 Arena 已分配字节数。
    pub(crate) fn set_arena_used(&mut self, used: u64) {
        self.arena_next = self
            .arena_base
            .saturating_add(used)
            .min(self.arena_base.saturating_add(self.arena_size));
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
    /// 附加线程的现场与 TLS。
    pub threads: Vec<(C, u64)>,
    /// Region 元数据。
    pub regions: Vec<hk_checkpoint::RegionSnapshot>,
    /// HMP 对象图描述；只记录，不重建授权。
    pub objects: Vec<hk_checkpoint::ObjectSnapshot>,
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
    /// 已退出但仍有 CPU 在使用的地址空间槽位与当时的页表根。
    ///
    /// leader 退出时 worker 可能还在别的 CPU 上跑；页表必须等这些 CPU 都切回
    /// 内核页表后才能归还，否则 AP 会用已回收的页表继续取指与写堆。带上根是
    /// 为了防止槽位在归还前被重启复用后误回收新地址空间。
    deferred_pages: Vec<(usize, u64)>,
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
    /// 全局压力与 OOM 审计。
    oom: oom::OomState,
    /// 懒镜像能力来源：(能力对象标识, 文件段)。
    image_sources: Vec<(u64, ImageSource)>,
    /// 启动模块只读来源。
    modules: Vec<ModuleRecord>,
    /// 用户态 Image Server 段注册。
    image_registrations: Vec<ImageRegistration>,
    /// 服务启动/重启通知：(服务名, Notification 对象)。
    service_watchers: Vec<(alloc::string::String, hk_handle::ObjectId)>,
    platform: P,
    /// 调试：每 CPU 最近一次进入的入口，损坏检测用它指出写入窗口。
    debug_last: [(&'static str, u64); MAX_CPUS],
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
            deferred_pages: Vec::new(),
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
            oom: oom::OomState::new(),
            image_sources: Vec::new(),
            modules: Vec::new(),
            image_registrations: Vec::new(),
            service_watchers: Vec::new(),
            platform,
            debug_last: [("none", 0); MAX_CPUS],
        }
    }

    /// 返回任务数量。
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// 返回槽位当前任务的 TCB；调度器用它检测槽位是否在用户态运行期间被复用。
    pub fn slot_tcb(&self, index: usize) -> Option<TcbId> {
        self.tasks.get(index).map(|task| task.tcb)
    }

    /// 调试：返回任务条目布局与缓冲区地址。
    pub fn layout_debug(&self) -> (usize, usize, usize, usize, usize, usize, usize, usize) {
        (
            core::mem::size_of::<UserTask<C>>(),
            core::mem::offset_of!(UserTask<C>, tcb),
            core::mem::offset_of!(UserTask<C>, process),
            core::mem::offset_of!(UserTask<C>, context),
            self.tasks.as_ptr() as usize,
            self.tasks.capacity(),
            core::mem::offset_of!(UserTask<C>, pending),
            core::mem::size_of::<Pending>(),
        )
    }

    /// 调试：记录本 CPU 最近一次进入的入口。
    pub fn debug_note(&mut self, cpu: usize, tag: &'static str, aux: u64) {
        if let Some(slot) = self.debug_last.get_mut(cpu) {
            *slot = (tag, aux);
        }
        let code = match tag {
            "dispatch" => 1,
            "step" => 2,
            "preempt" => 3,
            "finish" => 4,
            "tasks" => 5,
            _ => 0,
        };
        self.platform.note_entry(cpu, code, aux);
    }

    /// 调试：任务表增长时输出槽位与容量，便于把崩溃槽位对上创建来源。
    pub(crate) fn debug_task_push(&self, kind: &str, slot: usize) {
        self.platform.debug_report(&alloc::format!(
            "[task-add] kind={kind} slot={slot} len={} cap={}\n",
            self.tasks.len(),
            self.tasks.capacity(),
        ));
        let buffer = (
            self.tasks.as_ptr() as usize,
            self.tasks.capacity() * core::mem::size_of::<UserTask<C>>(),
        );
        if self.tasks.capacity() > 0
            && !hk_kernel::allocator::is_live_allocation(buffer.0, buffer.1)
        {
            panic!(
                "任务表缓冲区在 push 后不是活动分配：kind={kind} slot={slot} tasks={:#018x}+{:#x} len={} cap={}",
                buffer.0,
                buffer.1,
                self.tasks.len(),
                self.tasks.capacity()
            );
        }
        // 给已观测的受害槽位（6）的 `tcb` 下写监视：下一次外部写入会打印 RIP。
        if let Some(task) = self.tasks.get(6) {
            self.platform
                .watch_task_word(&task.tcb as *const TcbId as u64);
        }
    }

    /// 调试：检查任务条目尾部不变量，损坏时打印写入窗口并终止。
    ///
    /// `UserTask` 是 `repr(Rust)`，`context` 被放在偏移 0，其余字段在 1 KiB
    /// 之后。条目尾部被踩时进程索引、TCB 与名称会先变成内核地址；这里在每次
    /// 进入内核/调度时先核对，报告最近一次入口，把写入窗口缩小到一次系统调用。
    pub(crate) fn debug_check(&self, cpu: usize, tag: &'static str, aux: u64) {
        let buffer = (
            self.tasks.as_ptr() as usize,
            self.tasks.capacity() * core::mem::size_of::<UserTask<C>>(),
        );
        let mut broken = None;
        for (index, task) in self.tasks.iter().enumerate() {
            if task.process >= self.processes.len()
                || task.leader >= self.tasks.len()
                || task.tcb.get() >= (1_u64 << 32)
                || task.name.len() > task.name.capacity()
                || task.name.capacity() >= (1_usize << 40)
            {
                broken = Some(index);
                break;
            }
        }
        if broken.is_none() {
            // 任务表缓冲区必须始终是活动分配；但登记表查询要拿堆锁，不能每次
            // 抢占都做，按固定间隔抽样即可（异常会在少量入口内被发现）。
            static TICK: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let tick = TICK.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if tick % 1024 != 0 {
                return;
            }
            if self.tasks.capacity() == 0
                || hk_kernel::allocator::is_live_allocation(buffer.0, buffer.1)
            {
                return;
            }
        }
        let index = match broken {
            Some(index) => index,
            None => match self.current[cpu].filter(|index| *index < self.tasks.len()) {
                Some(index) => index,
                None => panic!(
                    "任务表缓冲区已不再登记为活动分配：entry={tag}/{aux:#x} cpu={cpu} tasks={:#018x}+{:#x}",
                    buffer.0, buffer.1
                ),
            },
        };
        let task = &self.tasks[index];
        let registered = self.tasks.capacity() == 0
            || hk_kernel::allocator::is_live_allocation(buffer.0, buffer.1);
        let (last_tag, last_aux) = self.debug_last.get(cpu).copied().unwrap_or(("none", 0));
        let base = core::mem::offset_of!(UserTask<C>, tcb);
        // SAFETY: 条目本身有效；只按 8 字节读取条目内部偏移。
        let word = |offset: usize| unsafe {
            core::ptr::read_unaligned(
                (&self.tasks[index] as *const UserTask<C> as *const u8).add(offset) as *const u64,
            )
        };
        let mut words = [0_u64; 22];
        for (step, value) in words.iter_mut().enumerate() {
            *value = word(base + step * 8);
        }
        let head = [word(0), word(8), word(16), word(24)];
        // BTreeMap 布局为 (root, length, alloc)；把两个 u64 定时器 map 的根也带出来，
        // 判断损坏的是 map 自身还是某个节点里的子指针。
        // SAFETY: 两个 map 字段一直有效；只读取开头 16 字节。
        let sleep_head = unsafe {
            (
                core::ptr::read_unaligned(&self.sleeping as *const _ as *const u64),
                core::ptr::read_unaligned((&self.sleeping as *const _ as *const u64).add(1)),
            )
        };
        // SAFETY: 同上。
        let timeout_head = unsafe {
            (
                core::ptr::read_unaligned(&self.timeouts as *const _ as *const u64),
                core::ptr::read_unaligned((&self.timeouts as *const _ as *const u64).add(1)),
            )
        };
        let mut overlaps = [(0_usize, 0_usize); 4];
        let overlapped = hk_kernel::allocator::allocation_overlaps(
            &self.tasks[index] as *const UserTask<C> as usize,
            core::mem::size_of::<UserTask<C>>(),
            buffer,
            &mut overlaps,
        );
        self.platform.debug_report(&alloc::format!(
            "\n任务条目损坏：entry={tag}/{aux:#x} cpu={cpu} slot={index} size={} element={:#018x} tasks={:#018x} cap={} len={} processes={} registered={registered} pending={}/{} 尾部偏移={base}\n  上一入口={last_tag}/{last_aux:#x} tcb={} process={} leader={} tls={:#x} name_ptr={:#x} name_len={} name_cap={} handles={} exited={} restarts={}\n  尾部字={:#x} {:#x} {:#x} {:#x} {:#x} {:#x} {:#x} {:#x}\n  头部字={:#x} {:#x} {:#x} {:#x} watch={:?} sleep={:#x}/{} timeout={:#x}/{}\n",
            core::mem::size_of::<UserTask<C>>(),
            &self.tasks[index] as *const UserTask<C> as usize,
            self.tasks.as_ptr() as usize,
            self.tasks.capacity(),
            self.tasks.len(),
            self.processes.len(),
            core::mem::offset_of!(UserTask<C>, pending),
            core::mem::size_of::<Pending>(),
            task.tcb.get(),
            task.process,
            task.leader,
            task.tls,
            task.name.as_ptr() as usize,
            task.name.len(),
            task.name.capacity(),
            task.handles.len(),
            task.exited,
            task.restarts,
            words[0],
            words[1],
            words[2],
            words[3],
            words[4],
            words[5],
            words[6],
            words[7],
            head[0],
            head[1],
            head[2],
            head[3],
            self.platform.last_watch(),
            sleep_head.0,
            sleep_head.1,
            timeout_head.0,
            timeout_head.1,
        ));
        self.platform.debug_report(&alloc::format!(
            "  后续字={:#x} {:#x} {:#x} {:#x} {:#x} {:#x} {:#x} {:#x} {:#x} {:#x} {:#x} {:#x} {:#x} {:#x}\n  （若这是从偏移 {base} 写入的中断现场：vector={:#x} rip={:#x} cs={:#x} rsp={:#x}）\n",
            words[8],
            words[9],
            words[10],
            words[11],
            words[12],
            words[13],
            words[14],
            words[15],
            words[16],
            words[17],
            words[18],
            words[19],
            words[20],
            words[21],
            words[15],
            words[17],
            words[18],
            words[20],
        ));
        if overlapped > 0 {
            self.platform.debug_report(&alloc::format!(
                "  条目与 {overlapped} 条其它活动分配重叠：{:#018x}+{:#x} {:#018x}+{:#x} {:#018x}+{:#x} {:#018x}+{:#x}\n",
                overlaps[0].0,
                overlaps[0].1,
                overlaps[1].0,
                overlaps[1].1,
                overlaps[2].0,
                overlaps[2].1,
                overlaps[3].0,
                overlaps[3].1,
            ));
        } else {
            self.platform
                .debug_report("  条目区间没有登记到其它活动分配（写入来自外部）。\n");
        }
        panic!("任务条目损坏：entry={tag}/{aux:#x} slot={index} 上一入口={last_tag}/{last_aux:#x}");
    }

    /// 诊断：条目所在页是否与某个 CPU 的系统调用栈页同一物理页。
    ///
    /// 返回 `(CPU, 栈页虚拟地址, 栈页物理地址, 条目页物理地址)`；不共享时 `None`。
    /// 只在条目校验失败时调用，不在热路径上。
    fn stack_alias(&self, element: u64) -> Option<(usize, u64, u64, u64)> {
        let page = hk_hal::PAGE_SIZE as u64;
        let element_phys = self.platform.kernel_phys(element)?;
        let target = element_phys & !(page - 1);
        for cpu in 0..MAX_CPUS {
            let Some((low, high)) = self.platform.syscall_stack_window(cpu) else {
                continue;
            };
            let mut virt = low & !(page - 1);
            while virt < high {
                if let Some(phys) = self.platform.kernel_phys(virt)
                    && phys & !(page - 1) == target
                {
                    return Some((cpu, virt, phys, element_phys));
                }
                virt += page;
            }
        }
        None
    }

    /// 诊断：条目区间是否与其它活动堆分配重叠。
    ///
    /// 重叠说明堆把同一段内存发了两次（典型是 `Vec` 扩容时源/目标重叠），
    /// 条目内容会被另一名租户覆盖。非 debug 构建没有登记表，返回 `None`。
    fn task_overlaps(&self, element: u64) -> Option<(usize, [(usize, usize); 4])> {
        let mut out = [(0_usize, 0_usize); 4];
        let buffer = (
            self.tasks.as_ptr() as usize,
            self.tasks.capacity() * core::mem::size_of::<UserTask<C>>(),
        );
        let count = hk_kernel::allocator::allocation_overlaps(
            element as usize,
            core::mem::size_of::<UserTask<C>>(),
            buffer,
            &mut out,
        );
        (count > 0).then_some((count, out))
    }

    /// 返回任务所属进程。
    pub(crate) fn process(&self, index: usize) -> &Process {
        let process = self.tasks[index].process;
        assert!(
            process < self.processes.len(),
            "任务进程索引损坏：task={index} element={element:#018x} tasks={tasks:#018x} cap={cap} len={len} tcb={tcb} process={process:#x} leader={leader} tls={tls:#x} exited={exited} restarts={restarts} handles={handles} name_ptr={name_ptr:#x} name_len={name_len} name_cap={name_cap} processes={processes} stack={stack:?} heap_phys={heap_phys:?} overlaps={overlaps:?} watch={watch:?}",
            element = &self.tasks[index] as *const UserTask<C> as usize,
            tasks = self.tasks.as_ptr() as usize,
            cap = self.tasks.capacity(),
            len = self.tasks.len(),
            tcb = self.tasks[index].tcb.get(),
            leader = self.tasks[index].leader,
            tls = self.tasks[index].tls,
            exited = self.tasks[index].exited,
            restarts = self.tasks[index].restarts,
            handles = self.tasks[index].handles.len(),
            name_ptr = self.tasks[index].name.as_ptr() as usize,
            name_len = self.tasks[index].name.len(),
            name_cap = self.tasks[index].name.capacity(),
            processes = self.processes.len(),
            stack = self.stack_alias(&self.tasks[index] as *const UserTask<C> as u64),
            heap_phys = self.platform.kernel_heap(),
            overlaps = self.task_overlaps(&self.tasks[index] as *const UserTask<C> as u64),
            watch = self.platform.last_watch()
        );
        self.processes[process]
            .as_ref()
            .expect("任务所属进程存在")
    }

    /// 返回任务所属进程的可变引用。
    pub(crate) fn process_mut(&mut self, index: usize) -> &mut Process {
        let process = self.tasks[index].process;
        assert!(
            process < self.processes.len(),
            "任务进程索引损坏：task={index} element={element:#018x} tasks={tasks:#018x} cap={cap} len={len} tcb={tcb} process={process:#x} leader={leader} tls={tls:#x} exited={exited} restarts={restarts} handles={handles} name_ptr={name_ptr:#x} name_len={name_len} name_cap={name_cap} processes={processes} stack={stack:?} heap_phys={heap_phys:?} overlaps={overlaps:?} watch={watch:?}",
            element = &self.tasks[index] as *const UserTask<C> as usize,
            tasks = self.tasks.as_ptr() as usize,
            cap = self.tasks.capacity(),
            len = self.tasks.len(),
            tcb = self.tasks[index].tcb.get(),
            leader = self.tasks[index].leader,
            tls = self.tasks[index].tls,
            exited = self.tasks[index].exited,
            restarts = self.tasks[index].restarts,
            handles = self.tasks[index].handles.len(),
            name_ptr = self.tasks[index].name.as_ptr() as usize,
            name_len = self.tasks[index].name.len(),
            name_cap = self.tasks[index].name.capacity(),
            processes = self.processes.len(),
            stack = self.stack_alias(&self.tasks[index] as *const UserTask<C> as u64),
            heap_phys = self.platform.kernel_heap(),
            overlaps = self.task_overlaps(&self.tasks[index] as *const UserTask<C> as u64),
            watch = self.platform.last_watch()
        );
        self.processes[process].as_mut().expect("任务所属进程存在")
    }

    /// 从任务 Ledger 记入一笔常驻区间。
    pub(crate) fn charge_frame(
        &mut self,
        index: usize,
        amount: u64,
    ) -> Result<FrameRef, hk_mem::hmp::HmpError> {
        let ledger = self.process(index).ledger.clone();
        let scope = self.process(index).frame_scope;
        hk_mem::hmp::frame_ledger::charge(scope, &ledger, amount)
    }

    /// 记入一段具体物理区间；同一进程内共享时只计费一次，跨进程按各自 scope 计费。
    pub(crate) fn charge_frame_range(
        &mut self,
        index: usize,
        range: hk_hal::PhysRange,
    ) -> Result<FrameRef, hk_mem::hmp::HmpError> {
        let ledger = self.process(index).ledger.clone();
        let scope = self.process(index).frame_scope;
        hk_mem::hmp::frame_ledger::charge_range(scope, &ledger, range)
    }

    /// 接管一笔内核已经扣过的记账。
    pub(crate) fn adopt_frame(&mut self, index: usize, charge: Charge) -> FrameRef {
        let ledger = self.process(index).ledger.clone();
        let scope = self.process(index).frame_scope;
        hk_mem::hmp::frame_ledger::adopt(scope, ledger, charge)
    }

    /// 释放一次 Frame 引用。
    pub(crate) fn release_frame(&mut self, frame: FrameRef) {
        hk_mem::hmp::frame_ledger::release(frame);
    }

    /// 判断是否有等待调度的线程。
    pub fn has_runnable(&self) -> bool {
        self.kernel.has_runnable()
    }

    /// 返回可运行的普通线程数；唤醒空闲 CPU 前用它避免为单个线程发 IPI。
    pub fn runnable(&self) -> usize {
        self.kernel.runnable()
    }

    /// 同一进程是否正在另一个 CPU 上执行。只有这种映射才需要远端 TLB 失效；
    /// 否则当前 CPU 本地失效 + 下次激活时刷新即可。
    pub(crate) fn process_active_elsewhere(&self, index: usize) -> bool {
        let owner = self.leader_of(index);
        self.current
            .iter()
            .flatten()
            .filter(|current| self.leader_of(**current) == owner)
            .count()
            > 1
    }

    /// 指定进程是否有其它 CPU 正在执行；调用者所在 CPU 已由本地失效覆盖。
    pub(crate) fn process_active_on_other_cpu(&self, process: usize, caller: usize) -> bool {
        let caller_cpu = self
            .current
            .iter()
            .position(|current| *current == Some(caller));
        self.current.iter().enumerate().any(|(cpu, current)| {
            Some(cpu) != caller_cpu && current.is_some_and(|task| self.leader_of(task) == process)
        })
    }

    /// 判断是否有能让任务重新可运行的待办唤醒：预算补充、sleep 到期或 IPC 超时。
    ///
    /// 与 [`Self::next_wakeup`] 不同，这里不含 idle 线程的 grant 结束时间；
    /// 只有这些待办才代表系统还有实质工作要做。
    pub fn has_pending_wakeup(&self) -> bool {
        self.kernel.pending_refills() || !self.sleeping.is_empty() || !self.timeouts.is_empty()
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
        let _ = self.attach_with_regions(index, context, &[]);
    }

    /// 登记启动模块，供受限的模块读取系统调用使用。
    pub fn register_modules(&mut self, modules: Vec<ModuleRecord>) {
        self.modules = modules;
    }

    /// 通知订阅了该服务启动/重启的任务。
    fn notify_service_started(&self, name: &str) {
        for (_, notification) in self
            .service_watchers
            .iter()
            .filter(|(watched, _)| watched == name)
        {
            let _ = self.kernel.signal_id(*notification, 1);
        }
    }

    /// 服务重启后重新绑定能力并重放 Image Server 段注册。
    fn replay_image_provider(&mut self, provider: usize) {
        let Some(endpoint) = self.endpoints.get(provider).copied().flatten() else {
            return;
        };
        let task = self.tasks[provider].tcb.get();
        for registration in self
            .image_registrations
            .iter()
            .filter(|registration| registration.provider == provider)
        {
            let _ = self.kernel.rebind_capability_provider(
                hk_handle::ObjectId::new(registration.object),
                endpoint,
                task,
            );
            let mut words = [0_u64; hk_ipc::MAX_MESSAGE_WORDS];
            words[0] = registration.object;
            words[1] = registration.module;
            words[2] = registration.file_offset;
            words[3] = registration.file_len;
            let _ = self.kernel.post_to_endpoint(
                endpoint,
                Message::new(hk_syscall::IMAGE_REGISTER).with_words(words),
            );
        }
    }

    /// 设置任务的用户态初始现场，并登记装载器交付的 Region。
    pub fn attach_with_regions(
        &mut self,
        index: usize,
        context: C,
        regions: &[UserRegionSpec],
    ) -> Result<(), KernelError> {
        self.register_user_regions(index, regions)?;
        if let Some(task) = self.tasks.get_mut(index) {
            task.context = context;
            if let Some(slot) = task.capacities.iter().position(Option::is_some) {
                task.context
                    .set_start_capacity(CAPACITY_HANDLE_BASE as u64 + slot as u64 + 1);
            }
            let _ = self.kernel.activate(task.tcb);
        }
        Ok(())
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
        self.debug_check(0, "tasks", 0);
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
                    state: self
                        .kernel
                        .task_of(task.tcb)
                        .and_then(|id| self.kernel.task_state(id))
                        .map_or("exited", state_name),
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
                    migration_cost: meter.map_or(0, |meter| meter.migration_cost),
                    last_cpu: meter.map_or(u32::MAX, |meter| meter.last_cpu),
                    arena_used: self.process(index).arena_used(),
                    arena_size: self.process(index).arena_size,
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

    /// 判断期望的发送者是否已经退出。
    ///
    /// 带发送者过滤的接收一旦等到实例退出就必须返回错误：端点对象仍被本地
    /// 句柄引用，但不会再有该实例的消息；继续阻塞会把客户端永久挂死。
    fn expected_exited(&self, expected: Option<TcbId>) -> bool {
        let Some(expected) = expected else {
            return false;
        };
        self.tasks
            .iter()
            .any(|task| task.tcb == expected && task.exited)
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

    /// 切回内核页表；平台不支持时忽略。
    pub fn deactivate(&self) {
        self.platform.deactivate();
    }

    /// 进程是否仍有线程在某个 CPU 上执行（`prepare` 已激活、`finish` 未收尾）。
    fn process_active_on_cpu(&self, leader: usize) -> bool {
        self.current.iter().flatten().any(|&slot| {
            self.tasks
                .get(slot)
                .is_some_and(|task| task.leader == leader)
        })
    }

    /// 归还任务槽位的地址空间；仍有 CPU 在用时推迟到 [`Self::flush_deferred_pages`]。
    pub(crate) fn release_pages_when_idle(&mut self, index: usize) {
        let leader = self.tasks.get(index).map_or(index, |task| task.leader);
        if self.process_active_on_cpu(leader) {
            if !self.deferred_pages.iter().any(|(slot, _)| *slot == index) {
                self.deferred_pages
                    .push((index, self.platform.vspace(index)));
            }
            return;
        }
        self.platform.release_pages(index);
    }

    /// 归还所有已确定没有 CPU 在用的地址空间。
    ///
    /// 调用点必须在把当前 CPU 的 `current` 清空、并切回内核页表之后，否则会
    /// 在释放后继续用旧 CR3 取指。
    pub(crate) fn flush_deferred_pages(&mut self) {
        if self.deferred_pages.is_empty() {
            return;
        }
        let mut remaining = Vec::new();
        for (index, root) in core::mem::take(&mut self.deferred_pages) {
            let leader = self.tasks.get(index).map_or(index, |task| task.leader);
            if self.process_active_on_cpu(leader) {
                remaining.push((index, root));
            } else {
                // 槽位可能已被重启复用；只回收当时那个根。
                self.platform.release_pages_if_root(index, root);
            }
        }
        self.deferred_pages = remaining;
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
            self.release_pages_when_idle(index);
        }
    }
}

fn state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Created => "created",
        TaskState::Eligible => "eligible",
        TaskState::Running => "running",
        TaskState::Blocked => "blocked",
        TaskState::Throttled => "throttled",
        TaskState::Suspended => "suspended",
        TaskState::Exited => "exited",
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
        Pending::ProviderReclaim { .. } => "provider-reclaim",
        Pending::Sleep { .. } => "sleep",
    }
}

#[cfg(test)]
#[allow(clippy::wildcard_imports)]
mod tests;
