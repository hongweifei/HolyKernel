//! 用户任务条目：现场、句柄映射与 Arena。

use super::*;
use hk_mem::hmp::{Arena as HmpArena, Ledger};

/// 一起保存的句柄与容量记账。
#[derive(Clone, Copy)]
pub(crate) struct HandleSlot {
    pub handle: Handle,
    pub charge: Option<Charge>,
}

/// 构造用户任务所需的全部输入。
pub(crate) struct TaskInit<C> {
    pub tcb: TcbId,
    pub name: String,
    pub parent: Option<u64>,
    pub handles: Vec<Option<Handle>>,
    pub notifications: Vec<Option<Handle>>,
    pub context: C,
    pub arena_base: u64,
    pub arena_size: u64,
    pub ledger_limit: u64,
}

/// 用户任务。
pub struct UserTask<C> {
    /// 线程控制块。
    pub tcb: TcbId,
    /// 所属进程的主线程槽位；主线程指向自身。
    pub leader: usize,
    /// 线程指针（TLS 基址）。
    pub tls: u64,
    /// 任务名称。
    pub name: String,
    /// 父任务 TCB 编号。
    pub parent: Option<u64>,
    /// 端点编号到句柄的映射。
    pub handles: Vec<Option<Handle>>,
    /// 通知编号到句柄的映射。
    pub notifications: Vec<Option<Handle>>,
    /// 随消息接收的额外句柄与容量记账。
    pub(crate) extra: Vec<Option<HandleSlot>>,
    /// 本任务持有的类型化能力。
    pub(crate) capabilities: Vec<Option<CapabilityMap>>,
    /// 本任务持有的 Region。
    pub(crate) regions: Vec<Option<RegionMap>>,
    /// 本任务持有的 Capacity。
    pub(crate) capacities: Vec<Option<Handle>>,
    /// 本任务持有的 Frame。
    pub(crate) frames: Vec<Option<Handle>>,
    /// 任务内存根与 Region 表。
    pub(crate) hmp_arena: HmpArena,
    /// 本任务的内存记账。
    pub(crate) ledger: Ledger,
    /// 自动虚拟地址窗口。
    pub(crate) user_window: UserWindow,
    /// 本任务持有的 DeviceGrant 句柄与 DMA 窗口记账。
    pub(crate) grants: Vec<Option<HandleSlot>>,
    /// DeviceGrant 句柄到已绑定 IRQ、通知与通知位的映射。
    pub bound_irqs: Vec<(Handle, u8, hk_hal::NotificationId, u64)>,
    /// DeviceGrant 的 IOMMU 映射。
    pub dma_mappings: Vec<(Handle, hk_hal::IommuDomain, hk_hal::Iova, usize)>,
    /// 用户现场。
    pub context: C,
    /// 累计运行周期数（TSC）。
    pub cpu_cycles: u64,
    /// 挂起的系统调用。
    pub pending: Pending,
    /// 最近一次收到的调用令牌。
    pub reply: Option<ReplyToken>,
    /// 任务 Arena。
    pub arena: Arena,
    /// 设备授权工厂句柄；存在时才允许创建 DeviceGrant。
    pub device_factory: Option<Handle>,
    /// 是否已退出。
    pub exited: bool,
    /// 退出码。
    pub exit_code: u64,
}

impl<C> UserTask<C> {
    /// 构造任务条目。
    pub(crate) fn new(init: TaskInit<C>) -> Self {
        Self {
            tcb: init.tcb,
            leader: 0,
            tls: 0,
            name: init.name,
            parent: init.parent,
            handles: init.handles,
            notifications: init.notifications,
            extra: Vec::new(),
            capabilities: Vec::new(),
            regions: Vec::new(),
            capacities: Vec::new(),
            frames: Vec::new(),
            hmp_arena: HmpArena::create(hk_hal::VirtAddr::new(0), hk_base::user::AUTO_WINDOW_LIMIT)
                .expect("用户 Arena 布局有效"),
            ledger: Ledger::create(init.ledger_limit),
            user_window: UserWindow::new(),
            grants: Vec::new(),
            bound_irqs: Vec::new(),
            dma_mappings: Vec::new(),
            context: init.context,
            cpu_cycles: 0,
            pending: Pending::None,
            reply: None,
            arena: Arena::new(init.arena_base, init.arena_size),
            device_factory: None,
            exited: false,
            exit_code: 0,
        }
    }
}
