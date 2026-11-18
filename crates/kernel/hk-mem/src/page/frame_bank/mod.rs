//! 系统物理帧银行：从全部可用内存区域分配与回收连续物理页。
//!
//! 这是全系统唯一的物理空闲表；HMP `Capacity` 只是它的配额视图，所有
//! 物理分配都记在这里，并按 `CapacityId` 归因。
//!
//! 分配路径（初始化、切页、预留与释放）见 `alloc`，记账路径（容量 Ledger、
//! 用量统计与归属采纳）见 `accounting`，`PhysicalBackend` 实现在 `backend`，
//! 空闲表内部操作在 `inner`。

use ::alloc::collections::{BTreeMap, BTreeSet};
use ::alloc::vec::Vec;

use hk_hal::PhysRange;
use spin::Mutex;

use crate::hmp::{CapacityId, CapacityKind, Charge, Ledger};

/// FrameBank 分配用途。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// 内核堆。
    KernelHeap,
    /// 页表。
    PageTable,
    /// 写时复制。
    Cow,
    /// 检查点恢复。
    Restore,
    /// 用户镜像。
    Image,
    /// DMA。
    Dma,
    /// 其他。
    Other,
}

impl FrameKind {
    /// 把 Capacity 用途映射到 FrameBank 分配用途。
    pub const fn from_capacity(kind: CapacityKind) -> Self {
        match kind {
            CapacityKind::System => Self::Other,
            CapacityKind::User => Self::Image,
            CapacityKind::Dma => Self::Dma,
            CapacityKind::Gpu => Self::Other,
            CapacityKind::Numa(_) => Self::Other,
        }
    }

    /// 判断该用途的页是否属于用户地址空间。
    ///
    /// 用户镜像、写时复制与检查点恢复页都映射在用户低半区；内核堆、
    /// 页表与内核自用页位于内核高半区。System Arena 按此区分用户页表
    /// 页与内核页表页。
    pub const fn is_user_space(self) -> bool {
        matches!(self, Self::Image | Self::Cow | Self::Restore)
    }
}

/// 支持的 NUMA 节点上限。
pub const MAX_NODES: usize = hk_syscall::MAX_NUMA_NODES;

/// FrameBank 记账快照。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameBankStats {
    /// 总字节数。
    pub total: u64,
    /// 已分配字节数。
    pub used: u64,
    /// 峰值已用字节数。
    pub peak: u64,
    /// 空闲字节数。
    pub free: u64,
    /// 各 Capacity 用途已用字节数：System、User、Dma、Gpu、Numa。
    pub by_capacity: [u64; CapacityKind::COUNT],
    /// 各 NUMA 节点已用字节数；未配置拓扑时只有节点 0。
    pub by_node: [u64; MAX_NODES],
}

/// 单个 Capacity 的 Ledger 快照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedgerStats {
    /// Capacity 标识。
    pub capacity: CapacityId,
    /// Capacity 用途。
    pub kind: CapacityKind,
    /// 当前已记账字节数。
    pub used: u64,
    /// 硬上限。
    pub limit: u64,
    /// 峰值。
    pub peak: u64,
    /// 常驻字节数快照。
    pub resident: u64,
    /// 软上限；0 表示未设置。
    pub soft_limit: u64,
    /// 压力百分比（0–100）。
    pub pressure: u64,
}

struct Allocation {
    range: PhysRange,
    kind: FrameKind,
    charge: Charge,
    /// 归属容量自己的 Ledger 记账；内核自用容量同样有一本账。
    owner_charge: Option<Charge>,
    /// 归属容量；`None` 表示内核自用。
    owner: Option<CapacityId>,
    /// 物理页归 HMP 对象持有（`OwnedFrame`），页表拆除时不得代为归还。
    object_owned: bool,
    /// 分配所在的 NUMA 节点。
    node: u8,
}

/// 登记给某个容量的窗口。
struct OwnedWindow {
    capacity: CapacityId,
    range: PhysRange,
}

struct Inner {
    free_by_start: BTreeMap<u64, PhysRange>,
    free_by_len: BTreeMap<u64, BTreeSet<u64>>,
    total: u64,
    /// 全系统物理用量。
    ledger: Ledger,
    /// 每个容量自己的 Ledger。
    ledgers: BTreeMap<CapacityId, Ledger>,
    /// 容量 Ledger 的用途标签，用于枚举。
    ledger_kinds: BTreeMap<CapacityId, CapacityKind>,
    allocations: BTreeMap<u64, Allocation>,
    by_capacity: [u64; CapacityKind::COUNT],
    /// 物理邻近域拓扑；空表示单节点 0。
    nodes: Vec<(PhysRange, u8)>,
    by_node: [u64; MAX_NODES],
    /// 已登记给容量的窗口，按起始地址索引。
    owned: BTreeMap<u64, OwnedWindow>,
    /// split 产生的父子关系；父容量先被 merge 时用于把孙窗口挂回活祖先。
    parents: BTreeMap<CapacityId, CapacityId>,
    /// 内核自用分配的归属容量；`None` 表示尚未建立 System Capacity。
    kernel_owner: Option<CapacityId>,
    /// 各容量的峰值用量。
    peaks: BTreeMap<CapacityId, u64>,
}

/// 系统物理帧银行。
pub struct FrameBank {
    inner: Mutex<Inner>,
}

mod accounting;
mod alloc;
mod backend;
mod inner;

#[cfg(test)]
mod tests;

/// 64 位向上取整；`align` 为 0 时按 1 处理。
fn align_up_u64(value: u64, align: u64) -> Option<u64> {
    let align = align.max(1);
    value
        .checked_add(align - 1)
        .map(|value| value / align * align)
}
