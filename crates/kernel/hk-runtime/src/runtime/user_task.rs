//! 用户任务条目：现场、句柄映射与线程状态。

use super::*;

/// 一起保存的句柄与容量记账。
#[derive(Clone, Copy)]
pub(crate) struct HandleSlot {
    pub handle: Handle,
    pub charge: Option<Charge>,
}

/// 构造用户任务所需的全部输入。
pub(crate) struct TaskInit<C> {
    pub tcb: TcbId,
    pub process: usize,
    pub name: String,
    pub parent: Option<u64>,
    pub handles: Vec<Option<Handle>>,
    pub notifications: Vec<Option<Handle>>,
    pub context: C,
}

/// 用户线程。
pub struct UserTask<C> {
    /// 线程控制块。
    pub tcb: TcbId,
    /// 所属进程索引。
    pub process: usize,
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
    /// 额外句柄空槽。
    pub(crate) extra_free: Vec<usize>,
    /// 本线程持有的类型化能力。
    pub(crate) capabilities: Vec<Option<CapabilityMap>>,
    /// 能力空槽。
    pub(crate) capability_free: Vec<usize>,
    /// 本线程持有的 Capacity。
    pub(crate) capacities: Vec<Option<Handle>>,
    /// Capacity 空槽。
    pub(crate) capacity_free: Vec<usize>,
    /// 本线程持有的 Frame。
    pub(crate) frames: Vec<Option<Handle>>,
    /// Frame 空槽。
    pub(crate) frame_free: Vec<usize>,
    /// 本线程持有的 DeviceGrant 句柄与 DMA 窗口记账。
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
    /// 是否已退出。
    pub exited: bool,
    /// 退出码。
    pub exit_code: u64,
}

impl<C> UserTask<C> {
    /// 构造线程条目。
    pub(crate) fn new(init: TaskInit<C>) -> Self {
        Self {
            tcb: init.tcb,
            process: init.process,
            leader: 0,
            tls: 0,
            name: init.name,
            parent: init.parent,
            handles: init.handles,
            notifications: init.notifications,
            extra: Vec::new(),
            extra_free: Vec::new(),
            capabilities: Vec::new(),
            capability_free: Vec::new(),
            capacities: Vec::new(),
            capacity_free: Vec::new(),
            frames: Vec::new(),
            frame_free: Vec::new(),
            grants: Vec::new(),
            bound_irqs: Vec::new(),
            dma_mappings: Vec::new(),
            context: init.context,
            cpu_cycles: 0,
            pending: Pending::None,
            reply: None,
            exited: false,
            exit_code: 0,
        }
    }
}
