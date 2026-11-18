//! HMP 类型化能力与 Capacity 的系统调用封装。

use hk_syscall::{
    CapabilityKind, NUMA_NODE_BYTES, SYS_CAPABILITY_COMMIT, SYS_CAPABILITY_CREATE,
    SYS_CAPABILITY_CREATE_LAZY, SYS_CAPABILITY_CREATE_ON, SYS_CAPABILITY_PHYS, SYS_CAPABILITY_PIN,
    SYS_CAPABILITY_SCANOUT, SYS_CAPABILITY_SWAP, SYS_CAPABILITY_WRITEBACK, SYS_CAPACITY_ALLOC,
    SYS_CAPACITY_FREE, SYS_CAPACITY_GPU, SYS_CAPACITY_INFO, SYS_CAPACITY_MERGE,
    SYS_CAPACITY_SOFT_LIMIT, SYS_CAPACITY_SPLIT, SYS_CAPACITY_WINDOWS, SYS_NUMA_INFO,
    SYS_NUMA_MIGRATE,
};

use crate::syscall_call;

/// 创建类型化能力，返回能力句柄。
pub fn capability_create(kind: CapabilityKind, length: usize) -> Option<u64> {
    // SAFETY: 内核校验类型与长度后分配常驻物理页。
    let handle = unsafe {
        syscall_call(
            SYS_CAPABILITY_CREATE,
            u64::from(kind.as_u8()),
            length as u64,
            0,
            0,
        )
        .value
    };
    (handle != u64::MAX).then_some(handle)
}

/// 从指定 Capacity 创建常驻类型化能力；`capacity == 0` 使用任务默认容量。
pub fn capability_create_on(kind: CapabilityKind, length: usize, capacity: u64) -> Option<u64> {
    // SAFETY: 内核校验类型、长度与容量句柄后分配常驻物理页。
    let handle = unsafe {
        syscall_call(
            SYS_CAPABILITY_CREATE_ON,
            u64::from(kind.as_u8()),
            length as u64,
            capacity,
            0,
        )
        .value
    };
    (handle != u64::MAX).then_some(handle)
}

/// 查询类型化能力的物理地址与长度。
pub fn capability_phys(capability: u64) -> Option<(u64, u64)> {
    let mut bytes = [0_u8; 16];
    // SAFETY: 缓冲区属于本进程；内核校验能力句柄后写入。
    let result = unsafe {
        syscall_call(
            SYS_CAPABILITY_PHYS,
            capability,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
        )
    };
    if result.value != 16 {
        return None;
    }
    let phys = u64::from_le_bytes(bytes[..8].try_into().ok()?);
    let length = u64::from_le_bytes(bytes[8..].try_into().ok()?);
    Some((phys, length))
}

/// 固定或解除固定类型化能力。
pub fn capability_pin(capability: u64, pinned: bool) -> u64 {
    // SAFETY: 内核校验能力类型与驻留状态。
    unsafe { syscall_call(SYS_CAPABILITY_PIN, capability, u64::from(pinned), 0, 0).value }
}

/// 设置 Surface 的 scanout 驻留状态。
pub fn capability_scanout(capability: u64, enabled: bool) -> u64 {
    // SAFETY: 内核校验 Surface 类型与驻留状态。
    unsafe { syscall_call(SYS_CAPABILITY_SCANOUT, capability, u64::from(enabled), 0, 0).value }
}

/// 请求绑定 Provider 保存/压缩指定页；未绑定 Provider 时返回 `u64::MAX`。
///
/// 当前实现一次处理 `offset` 所在的一页，`length` 暂未使用。
pub fn capability_swap(capability: u64, offset: u64, length: usize) -> u64 {
    // SAFETY: 内核校验 Anonymous 类型、范围与 Provider 确认。
    unsafe { syscall_call(SYS_CAPABILITY_SWAP, capability, offset, length as u64, 0).value }
}

/// 请求 Provider 回写能力的常驻页；返回 0 或 `u64::MAX`。
pub fn capability_writeback(capability: u64) -> u64 {
    // SAFETY: 内核校验能力类型与 Provider 绑定后转发通知。
    unsafe { syscall_call(SYS_CAPABILITY_WRITEBACK, capability, 0, 0, 0).value }
}

/// 提交 Revision 能力，返回新版本。
pub fn capability_commit(capability: u64) -> Option<u64> {
    // SAFETY: 内核校验能力类型并推进版本。
    let version = unsafe { syscall_call(SYS_CAPABILITY_COMMIT, capability, 0, 0, 0).value };
    (version != u64::MAX).then_some(version)
}

/// 创建按需装入的类型化能力。
pub fn capability_create_lazy(kind: CapabilityKind, length: usize) -> Option<u64> {
    // SAFETY: 内核校验类型与长度后创建能力并返回句柄。
    let handle = unsafe {
        syscall_call(
            SYS_CAPABILITY_CREATE_LAZY,
            u64::from(kind.as_u8()),
            length as u64,
            0,
            0,
        )
        .value
    };
    (handle != u64::MAX).then_some(handle)
}

/// Capacity 快照。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapacityInfo {
    /// 首个窗口的起始物理地址；完整窗口列表使用 [`capacity_windows`]。
    pub start: u64,
    /// 首个窗口长度；完整窗口列表使用 [`capacity_windows`]。
    pub len: u64,
    /// 用途编码。
    pub kind: u8,
    /// 页大小。
    pub page_size: u64,
    /// 配额字节数。
    pub limit: u64,
    /// 已用字节数。
    pub used: u64,
    /// 峰值用量。
    pub peak: u64,
    /// 软上限；0 表示未设置。
    pub soft_limit: u64,
    /// 绑定的 NUMA 节点；`None` 表示任意节点。
    pub node: Option<u8>,
}

/// 查询 Capacity。
pub fn capacity_info(capacity: u64) -> Option<CapacityInfo> {
    let mut bytes = [0_u8; 72];
    // SAFETY: 缓冲区属于本进程；内核写入 Capacity 快照。
    let result = unsafe {
        syscall_call(
            SYS_CAPACITY_INFO,
            capacity,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
        )
        .value
    };
    if result != 72 {
        return None;
    }
    let node = u32::from_le_bytes(bytes[64..68].try_into().ok()?);
    Some(CapacityInfo {
        start: u64::from_le_bytes(bytes[..8].try_into().ok()?),
        len: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
        kind: u64::from_le_bytes(bytes[16..24].try_into().ok()?) as u8,
        page_size: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
        limit: u64::from_le_bytes(bytes[32..40].try_into().ok()?),
        used: u64::from_le_bytes(bytes[40..48].try_into().ok()?),
        peak: u64::from_le_bytes(bytes[48..56].try_into().ok()?),
        soft_limit: u64::from_le_bytes(bytes[56..64].try_into().ok()?),
        node: (node != u32::MAX).then_some(node as u8),
    })
}

/// 返回 Capacity 当前持有的窗口数。
pub fn capacity_window_count(capacity: u64) -> Option<usize> {
    // SAFETY: 内核校验 Capacity 句柄后返回窗口数，不写入用户内存。
    let count = unsafe { syscall_call(SYS_CAPACITY_WINDOWS, capacity, 0, 0, 0).value };
    (count != u64::MAX).then_some(count as usize)
}

/// 把 Capacity 的全部窗口写入调用方缓冲；每条记录 16 字节（起始物理地址 + 长度）。
///
/// 返回写入的字节数；缓冲不足时返回 `None`。
pub fn capacity_windows(capacity: u64, out: &mut [u8]) -> Option<usize> {
    // SAFETY: 缓冲区属于本进程；内核校验 Capacity 与长度后写入窗口记录。
    let result = unsafe {
        syscall_call(
            SYS_CAPACITY_WINDOWS,
            capacity,
            out.as_mut_ptr() as u64,
            out.len() as u64,
            0,
        )
        .value
    };
    (result != u64::MAX).then_some(result as usize)
}

/// 从 Capacity 尾部切出子 Capacity；`page_size` 为 0 时继承父容量。
pub fn capacity_split(
    capacity: u64,
    length: usize,
    kind: u8,
    page_size: usize,
    node: Option<u8>,
) -> Option<u64> {
    let encoded_kind = u64::from(kind) | (u64::from(node.unwrap_or(0)) << 8);
    // SAFETY: 内核校验 Capacity、长度、页大小与节点后切出子容量。
    let child = unsafe {
        syscall_call(
            SYS_CAPACITY_SPLIT,
            capacity,
            length as u64,
            encoded_kind,
            page_size as u64,
        )
        .value
    };
    (child != u64::MAX).then_some(child)
}

/// 从 Capacity 分配一个 Frame。
pub fn capacity_alloc(capacity: u64, length: usize) -> Option<u64> {
    capacity_alloc_on(capacity, length, None)
}

/// 从 Capacity 分配一个 Frame，并给出 NUMA 节点偏好。
///
/// 绑定节点的 Capacity 忽略与自身不一致的偏好并返回失败，不会静默跨节点。
pub fn capacity_alloc_on(capacity: u64, length: usize, node: Option<u8>) -> Option<u64> {
    let hint = node.map_or(0, |node| u64::from(node) + 1);
    // SAFETY: 内核校验 Capacity、长度与节点后分配 Frame。
    let frame = unsafe { syscall_call(SYS_CAPACITY_ALLOC, capacity, length as u64, hint, 0).value };
    (frame != u64::MAX).then_some(frame)
}

/// 把 Frame 归还给 Capacity。
pub fn capacity_free(capacity: u64, frame: u64) -> u64 {
    // SAFETY: 内核校验 Capacity、Frame 与引用状态。
    unsafe { syscall_call(SYS_CAPACITY_FREE, capacity, frame, 0, 0).value }
}

/// 把一个已清空的子 Capacity 并回父 Capacity；成功返回 0。
pub fn capacity_merge(parent: u64, child: u64) -> u64 {
    // SAFETY: 内核校验父子 Capacity、页大小与子容量用量后合并区间。
    unsafe { syscall_call(SYS_CAPACITY_MERGE, parent, child, 0, 0).value }
}

/// 设置 Capacity 软上限；仅 Root/Tempo 可调用。
pub fn set_capacity_soft_limit(capacity: u64, limit: u64) -> u64 {
    // SAFETY: 内核校验调用任务与容量句柄。
    unsafe { syscall_call(SYS_CAPACITY_SOFT_LIMIT, capacity, limit, 0, 0).value }
}

/// 取得共享 Gpu 后备容量句柄；非 virtio 服务或没有 Gpu 容量时返回 `None`。
pub fn gpu_capacity() -> Option<u64> {
    // SAFETY: 内核校验调用任务与平台 Gpu 容量后返回共享句柄。
    let handle = unsafe { syscall_call(SYS_CAPACITY_GPU, 0, 0, 0, 0).value };
    (handle != u64::MAX && handle != 0).then_some(handle)
}

/// 把能力的一段常驻页迁移到目标 NUMA 节点；返回迁移字节数。
pub fn numa_migrate(capability: u64, offset: u64, length: usize, node: u8) -> u64 {
    // SAFETY: 内核校验 Anonymous 能力、范围与目标节点。
    unsafe {
        syscall_call(
            SYS_NUMA_MIGRATE,
            capability,
            offset,
            length as u64,
            u64::from(node),
        )
        .value
    }
}

/// 返回 NUMA 拓扑记录数。
pub fn numa_node_count() -> Option<usize> {
    // SAFETY: 内核只返回拓扑记录数，不写入用户内存。
    let count = unsafe { syscall_call(SYS_NUMA_INFO, 0, 0, 0, 0).value };
    (count != u64::MAX).then_some(count as usize)
}

/// 读取一条 NUMA 拓扑记录：`(node, start, len)`。
pub fn numa_node(index: usize) -> Option<(u8, u64, u64)> {
    let mut bytes = [0_u8; NUMA_NODE_BYTES];
    // SAFETY: 缓冲区属于本进程；内核校验索引后写入一条拓扑记录。
    let result = unsafe {
        syscall_call(
            SYS_NUMA_INFO,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            index as u64,
            0,
        )
        .value
    };
    if result != NUMA_NODE_BYTES as u64 {
        return None;
    }
    Some((
        u32::from_le_bytes(bytes[..4].try_into().ok()?) as u8,
        u64::from_le_bytes(bytes[8..16].try_into().ok()?),
        u64::from_le_bytes(bytes[16..24].try_into().ok()?),
    ))
}
