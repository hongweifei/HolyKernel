//! HMP 类型化能力与 Region 映射的系统调用封装。

use hk_syscall::{
    CapabilityKind, RIGHT_READ, RIGHT_WRITE, SYS_ARENA_INFO, SYS_CAPABILITY_BIND_PROVIDER,
    SYS_CAPABILITY_COMMIT, SYS_CAPABILITY_CREATE, SYS_CAPABILITY_CREATE_LAZY,
    SYS_CAPABILITY_EXPORT, SYS_CAPABILITY_PHYS, SYS_CAPABILITY_PIN, SYS_CAPABILITY_SCANOUT,
    SYS_CAPACITY_ALLOC, SYS_CAPACITY_FREE, SYS_CAPACITY_INFO, SYS_CAPACITY_MERGE,
    SYS_CAPACITY_SPLIT, SYS_CAPACITY_WINDOWS, SYS_DMA_MAP, SYS_FRAME_BANK_STATS, SYS_FRAME_INFO,
    SYS_FRAME_MAP, SYS_FRAME_PIN, SYS_FRAME_RELEASE, SYS_FRAME_SHARE, SYS_LEDGER_ENUM,
    SYS_LEDGER_INFO, SYS_OBJECT_LEDGER, SYS_REGION_GROW, SYS_REGION_INFO, SYS_REGION_MAP,
    SYS_REGION_PROTECT, SYS_REGION_UNMAP,
};

use hk_syscall::{CAPACITY_KIND_COUNT, LEDGER_RECORD_BYTES};

use crate::syscall_call;

/// Memory Server 分配请求；与 `hk-proto::labels::MEMORY` 区间一致。
pub const MEMORY_ALLOC: u64 = 0x1100;
/// Memory Server 分配成功回复。
pub const MEMORY_ALLOCATED: u64 = 0x1101;
/// Memory Server 分配 Frame 请求。
pub const MEMORY_FRAME: u64 = 0x1102;
/// Memory Server 分配 Frame 成功回复。
pub const MEMORY_FRAME_ALLOCATED: u64 = 0x1103;

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

/// 把类型化能力的一段映射进当前任务 Arena，返回 Region 句柄。
pub fn region_map(
    capability: u64,
    offset: u64,
    length: usize,
    rights: u32,
    flags: u32,
) -> Option<u64> {
    let packed = rights as u64 | ((flags as u64) << 32);
    // SAFETY: 内核校验能力、范围、权利与标志后建立映射。
    let region =
        unsafe { syscall_call(SYS_REGION_MAP, capability, offset, length as u64, packed).value };
    (region != u64::MAX && region != 0).then_some(region)
}

/// 查询 Region 的虚拟地址与长度。
pub fn region_info(region: u64) -> Option<(u64, u64)> {
    let mut bytes = [0_u8; 16];
    // SAFETY: 缓冲区属于本进程；内核校验 Region 句柄后写入。
    let result = unsafe {
        syscall_call(
            SYS_REGION_INFO,
            region,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
        )
    };
    if result.value != 16 {
        return None;
    }
    let virt = u64::from_le_bytes(bytes[..8].try_into().ok()?);
    let length = u64::from_le_bytes(bytes[8..].try_into().ok()?);
    Some((virt, length))
}

/// 解除 Region 映射；成功返回 1。
pub fn region_unmap(region: u64) -> u64 {
    // SAFETY: 内核校验 Region 句柄并解除映射。
    unsafe { syscall_call(SYS_REGION_UNMAP, region, 0, 0, 0).value }
}

/// 按 `GROW` 标志扩展 Region；成功返回 0。
pub fn region_grow(region: u64, new_len: usize) -> u64 {
    // SAFETY: 内核校验 Region 的 GROW 标志、能力范围与用户窗口后扩展映射。
    unsafe { syscall_call(SYS_REGION_GROW, region, new_len as u64, 0, 0).value }
}

/// 修改已映射 Region 的权利与标志；成功返回 0。
pub fn region_protect(region: u64, rights: u32, flags: u32) -> u64 {
    let packed = rights as u64 | ((flags as u64) << 32);
    // SAFETY: 内核校验 Region 句柄、权利与标志后同步页表。
    unsafe { syscall_call(SYS_REGION_PROTECT, region, packed, 0, 0).value }
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

/// 在不要求常驻物理页的情况下映射能力。
pub fn map_lazy(capability: u64, length: usize) -> Option<(u64, u64)> {
    let region = region_map(capability, 0, length, RIGHT_READ | RIGHT_WRITE, 0)?;
    let (virt, _) = region_info(region)?;
    Some((region, virt))
}

/// 映射已有能力，返回 `(region, virt)`。
pub fn map_attach(capability: u64) -> Option<(u64, u64)> {
    let (_, length) = capability_phys(capability)?;
    let region = region_map(
        capability,
        0,
        usize::try_from(length).ok()?,
        RIGHT_READ | RIGHT_WRITE,
        0,
    )?;
    let (virt, _) = region_info(region)?;
    Some((region, virt))
}

/// 把 Frame 或类型化能力绑定到 DeviceGrant 的 IOMMU 域。
pub fn dma_map(grant: u64, object: u64, length: usize) -> u64 {
    // SAFETY: 内核校验授权、对象句柄与长度后建立 IOMMU 映射。
    unsafe { syscall_call(SYS_DMA_MAP, grant, object, length as u64, 0).value }
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

/// 提交 Revision 能力，返回新版本。
pub fn capability_commit(capability: u64) -> Option<u64> {
    // SAFETY: 内核校验能力类型并推进版本。
    let version = unsafe { syscall_call(SYS_CAPABILITY_COMMIT, capability, 0, 0, 0).value };
    (version != u64::MAX).then_some(version)
}

/// Capacity 用途编码。
pub use hk_syscall::{
    CAPACITY_KIND_DMA as CAPACITY_DMA, CAPACITY_KIND_GPU as CAPACITY_GPU,
    CAPACITY_KIND_NUMA as CAPACITY_NUMA, CAPACITY_KIND_SYSTEM as CAPACITY_SYSTEM,
    CAPACITY_KIND_USER as CAPACITY_USER,
};

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
}

/// 查询 Capacity。
pub fn capacity_info(capacity: u64) -> Option<CapacityInfo> {
    let mut bytes = [0_u8; 64];
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
    if result != 64 {
        return None;
    }
    Some(CapacityInfo {
        start: u64::from_le_bytes(bytes[..8].try_into().ok()?),
        len: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
        kind: u64::from_le_bytes(bytes[16..24].try_into().ok()?) as u8,
        page_size: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
        limit: u64::from_le_bytes(bytes[32..40].try_into().ok()?),
        used: u64::from_le_bytes(bytes[40..48].try_into().ok()?),
        peak: u64::from_le_bytes(bytes[48..56].try_into().ok()?),
        soft_limit: u64::from_le_bytes(bytes[56..].try_into().ok()?),
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

/// 从 Capacity 尾部切出子 Capacity。
pub fn capacity_split(capacity: u64, length: usize, kind: u8) -> Option<u64> {
    // SAFETY: 内核校验 Capacity 与长度后切出子容量。
    let child = unsafe {
        syscall_call(
            SYS_CAPACITY_SPLIT,
            capacity,
            length as u64,
            u64::from(kind),
            0,
        )
        .value
    };
    (child != u64::MAX).then_some(child)
}

/// 从 Capacity 分配一个 Frame。
pub fn capacity_alloc(capacity: u64, length: usize) -> Option<u64> {
    // SAFETY: 内核校验 Capacity 与长度后分配 Frame。
    let frame = unsafe { syscall_call(SYS_CAPACITY_ALLOC, capacity, length as u64, 0, 0).value };
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

/// Frame 快照。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameInfo {
    /// 起始物理地址。
    pub phys: u64,
    /// 区间长度。
    pub len: u64,
    /// 页大小。
    pub page_size: u64,
    /// 所属 Capacity 标识。
    pub capacity: u64,
    /// 共享引用数。
    pub refs: u64,
    /// 是否固定驻留。
    pub pinned: bool,
}

/// 查询 Frame。
pub fn frame_info(frame: u64) -> Option<FrameInfo> {
    let mut bytes = [0_u8; 48];
    // SAFETY: 缓冲区属于本进程；内核写入 Frame 快照。
    let result = unsafe {
        syscall_call(
            SYS_FRAME_INFO,
            frame,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
        )
        .value
    };
    if result != 48 {
        return None;
    }
    Some(FrameInfo {
        phys: u64::from_le_bytes(bytes[..8].try_into().ok()?),
        len: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
        page_size: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
        capacity: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
        refs: u64::from_le_bytes(bytes[32..40].try_into().ok()?),
        pinned: u64::from_le_bytes(bytes[40..].try_into().ok()?) != 0,
    })
}

/// 固定或解除固定 Frame。
pub fn frame_pin(frame: u64, pinned: bool) -> u64 {
    // SAFETY: 内核校验 Frame 与驻留状态。
    unsafe { syscall_call(SYS_FRAME_PIN, frame, u64::from(pinned), 0, 0).value }
}

/// 增加 Frame 共享引用。
pub fn frame_share(frame: u64) -> Option<u64> {
    // SAFETY: 内核校验 Frame 后增加引用。
    let refs = unsafe { syscall_call(SYS_FRAME_SHARE, frame, 0, 0, 0).value };
    (refs != u64::MAX).then_some(refs)
}

/// 释放一次 Frame 共享引用。
pub fn frame_release(frame: u64) -> Option<u64> {
    // SAFETY: 内核校验 Frame 后释放引用。
    let refs = unsafe { syscall_call(SYS_FRAME_RELEASE, frame, 0, 0, 0).value };
    (refs != u64::MAX).then_some(refs)
}

/// 把 Frame 映射进当前任务，返回 Region 句柄。
pub fn frame_map(frame: u64, rights: u32, flags: u32) -> Option<u64> {
    let packed = rights as u64 | ((flags as u64) << 32);
    // SAFETY: 内核校验 Frame 与权利后建立 Region。
    let region = unsafe { syscall_call(SYS_FRAME_MAP, frame, packed, 0, 0).value };
    (region != u64::MAX).then_some(region)
}

/// 任务 Arena 快照。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArenaInfo {
    /// Arena 窗口起始虚拟地址。
    pub base: u64,
    /// Arena 窗口长度。
    pub size: u64,
    /// 已登记 Region 的字节数，包含镜像、栈与动态映射。
    pub used: u64,
    /// Region 数。
    pub regions: u64,
}

/// 查询当前任务的 Arena。
pub fn arena_info() -> Option<ArenaInfo> {
    let mut bytes = [0_u8; 32];
    // SAFETY: 缓冲区属于本进程；内核写入 Arena 快照。
    let result = unsafe {
        syscall_call(
            SYS_ARENA_INFO,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
            0,
        )
        .value
    };
    if result != 32 {
        return None;
    }
    Some(ArenaInfo {
        base: u64::from_le_bytes(bytes[..8].try_into().ok()?),
        size: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
        used: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
        regions: u64::from_le_bytes(bytes[24..].try_into().ok()?),
    })
}

/// FrameBank 物理用量快照。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameBankStats {
    /// 总字节数。
    pub total: u64,
    /// 已用字节数。
    pub used: u64,
    /// 峰值字节数。
    pub peak: u64,
    /// 空闲字节数。
    pub free: u64,
    /// 各 Capacity 用途已用字节数：System、User、Dma、Gpu、Numa。
    pub by_capacity: [u64; CAPACITY_KIND_COUNT],
}

/// 查询 FrameBank 物理用量快照。
pub fn frame_bank_stats() -> Option<FrameBankStats> {
    let mut bytes = [0_u8; 32 + CAPACITY_KIND_COUNT * 8];
    // SAFETY: 缓冲区属于本进程；内核写入 FrameBank 物理用量快照。
    let result = unsafe {
        syscall_call(
            SYS_FRAME_BANK_STATS,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
            0,
        )
        .value
    };
    if result != bytes.len() as u64 {
        return None;
    }
    let mut by_capacity = [0_u64; CAPACITY_KIND_COUNT];
    for (index, value) in by_capacity.iter_mut().enumerate() {
        let start = 32 + index * 8;
        *value = u64::from_le_bytes(bytes[start..start + 8].try_into().ok()?);
    }
    Some(FrameBankStats {
        total: u64::from_le_bytes(bytes[..8].try_into().ok()?),
        used: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
        peak: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
        free: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
        by_capacity,
    })
}

/// 任务内存账本快照。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LedgerInfo {
    /// 已记账字节数。
    pub charged: u64,
    /// 配额字节数。
    pub limit: u64,
    /// 峰值字节数。
    pub peak: u64,
    /// 压力百分比。
    pub pressure: u64,
    /// 常驻字节数。
    pub resident: u64,
    /// 软上限；0 表示未设置。
    pub soft_limit: u64,
}

/// 查询当前任务的内存账本。
pub fn ledger_info() -> Option<LedgerInfo> {
    let mut bytes = [0_u8; 48];
    // SAFETY: 缓冲区属于本进程；内核校验长度后写入账本快照。
    let result = unsafe {
        syscall_call(
            SYS_LEDGER_INFO,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
            0,
        )
        .value
    };
    if result != 48 {
        return None;
    }
    Some(LedgerInfo {
        charged: u64::from_le_bytes(bytes[..8].try_into().ok()?),
        limit: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
        peak: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
        pressure: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
        resident: u64::from_le_bytes(bytes[32..40].try_into().ok()?),
        soft_limit: u64::from_le_bytes(bytes[40..].try_into().ok()?),
    })
}

/// 查询对象利用率快照；只用于报告，不执行硬配额。
pub fn object_ledger_info(handle: u64) -> Option<LedgerInfo> {
    let mut bytes = [0_u8; 48];
    // SAFETY: 缓冲区属于本进程；内核校验对象句柄后写入账本快照。
    let result = unsafe {
        syscall_call(
            SYS_OBJECT_LEDGER,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            handle,
            0,
        )
        .value
    };
    if result != 48 {
        return None;
    }
    Some(LedgerInfo {
        charged: u64::from_le_bytes(bytes[..8].try_into().ok()?),
        limit: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
        peak: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
        pressure: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
        resident: u64::from_le_bytes(bytes[32..40].try_into().ok()?),
        soft_limit: u64::from_le_bytes(bytes[40..].try_into().ok()?),
    })
}

/// 单个 Capacity 的 Ledger 快照。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapacityLedgerInfo {
    /// Capacity 标识。
    pub capacity: u64,
    /// 已记账字节数。
    pub used: u64,
    /// 配额字节数。
    pub limit: u64,
    /// 峰值字节数。
    pub peak: u64,
    /// 压力百分比。
    pub pressure: u64,
    /// 常驻字节数。
    pub resident: u64,
    /// 软上限；0 表示未设置。
    pub soft_limit: u64,
    /// Capacity 用途编码。
    pub kind: u8,
}

/// 按索引枚举容量 Ledger；索引越界或平台不支持时返回 `None`。
pub fn ledger_enum(index: usize) -> Option<CapacityLedgerInfo> {
    let mut bytes = [0_u8; LEDGER_RECORD_BYTES];
    // SAFETY: 缓冲区属于本进程；内核按索引写入容量 Ledger 快照。
    let result = unsafe {
        syscall_call(
            SYS_LEDGER_ENUM,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            index as u64,
            0,
        )
        .value
    };
    if result != LEDGER_RECORD_BYTES as u64 {
        return None;
    }
    Some(CapacityLedgerInfo {
        capacity: u64::from_le_bytes(bytes[..8].try_into().ok()?),
        used: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
        limit: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
        peak: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
        pressure: u64::from_le_bytes(bytes[32..40].try_into().ok()?),
        resident: u64::from_le_bytes(bytes[40..48].try_into().ok()?),
        soft_limit: u64::from_le_bytes(bytes[48..56].try_into().ok()?),
        kind: u64::from_le_bytes(bytes[56..].try_into().ok()?) as u8,
    })
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

/// 把类型化能力绑定到 Provider 端点；`kind` 使用 `PROVIDER_KIND_*` 编码。
pub fn capability_bind_provider(capability: u64, endpoint: u64, kind: u8) -> u64 {
    // SAFETY: 内核校验能力、端点句柄与 Provider 类型后建立绑定。
    unsafe {
        syscall_call(
            SYS_CAPABILITY_BIND_PROVIDER,
            capability,
            endpoint,
            u64::from(kind),
            0,
        )
        .value
    }
}

/// 导出可转移的能力句柄。
pub fn capability_export(capability: u64) -> Option<u64> {
    // SAFETY: 内核校验能力类型并复制可转移句柄。
    let exported = unsafe { syscall_call(SYS_CAPABILITY_EXPORT, capability, 0, 0, 0).value };
    (exported != u64::MAX && exported != 0).then_some(exported)
}
