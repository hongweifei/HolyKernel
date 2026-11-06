//! 用户任务条目：现场、句柄映射与 Arena。

use super::*;
use hk_mem::hmp::Ledger;

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
    /// 随消息接收的额外句柄。
    pub extra: Vec<Option<Handle>>,
    /// 额外句柄对应的容量记账。
    pub(crate) extra_charges: Vec<Option<hk_mem::hmp::Charge>>,
    /// 本任务持有的类型化能力。
    pub(crate) capabilities: Vec<Option<CapabilityMap>>,
    /// 本任务持有的 Region。
    pub(crate) regions: Vec<Option<RegionMap>>,
    /// 本任务的内存记账。
    pub(crate) ledger: Ledger,
    /// 自动虚拟地址窗口。
    pub(crate) user_window: UserWindow,
    /// 本任务持有的 DeviceGrant 句柄。
    pub grants: Vec<Option<Handle>>,
    /// DeviceGrant DMA 窗口记账。
    pub grant_charges: Vec<Option<hk_mem::hmp::Charge>>,
    /// DeviceGrant 句柄到已绑定 IRQ、通知与通知位的映射。
    pub bound_irqs: Vec<(Handle, u8, hk_hal::NotificationId, u64)>,
    /// DeviceGrant 的 IOMMU 映射。
    pub dma_mappings: Vec<(Handle, hk_hal::IommuDomain, hk_hal::Iova, usize)>,
    /// 用户现场。
    pub context: C,
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
        ledger_limit: u64,
    ) -> Self {
        Self {
            tcb,
            leader: 0,
            tls: 0,
            name: name.to_string(),
            parent,
            handles,
            notifications,
            extra: Vec::new(),
            extra_charges: Vec::new(),
            capabilities: Vec::new(),
            regions: Vec::new(),
            ledger: Ledger::create(ledger_limit),
            user_window: UserWindow::new(),
            grants: Vec::new(),
            grant_charges: Vec::new(),
            bound_irqs: Vec::new(),
            dma_mappings: Vec::new(),
            context,
            pending: Pending::None,
            reply: None,
            arena: Arena::new(arena_base, arena_size),
            device_factory: None,
            exited: false,
            exit_code: 0,
        }
    }
}
