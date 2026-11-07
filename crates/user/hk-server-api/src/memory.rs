//! HMP 类型化能力与 Region 映射的系统调用封装。

use hk_syscall::{
    CapabilityKind, REGION_FLAG_COW, REGION_FLAG_GUARD, REGION_FLAG_PINNED, REGION_FLAG_SEALED,
    RIGHT_EXECUTE, RIGHT_READ, RIGHT_WRITE, SYS_ARENA_INFO, SYS_CAPABILITY_BIND_PROVIDER,
    SYS_CAPABILITY_COMMIT, SYS_CAPABILITY_CREATE, SYS_CAPABILITY_CREATE_LAZY,
    SYS_CAPABILITY_EXPORT, SYS_CAPABILITY_PHYS, SYS_CAPABILITY_PIN, SYS_CAPABILITY_SCANOUT,
    SYS_CAPACITY_ALLOC, SYS_CAPACITY_FREE, SYS_CAPACITY_INFO, SYS_CAPACITY_SPLIT,
    SYS_DMA_MAP_CAPABILITY, SYS_DMA_MAP_FRAME, SYS_FRAME_FREE, SYS_FRAME_INFO, SYS_FRAME_MAP,
    SYS_FRAME_PIN, SYS_FRAME_RELEASE, SYS_FRAME_SHARE, SYS_LEDGER_INFO, SYS_MEMORY_STATS,
    SYS_REGION_INFO, SYS_REGION_MAP, SYS_REGION_UNMAP,
};

use crate::syscall_call;

/// Memory Server 分配请求。
pub const MEMORY_ALLOC: u64 = 0x4D45_4D41;
/// Memory Server 分配成功回复。
pub const MEMORY_ALLOCATED: u64 = 0x4D45_4D42;
/// Memory Server 分配 Frame 请求。
pub const MEMORY_FRAME: u64 = 0x4D45_4D43;
/// Memory Server 分配 Frame 成功回复。
pub const MEMORY_FRAME_ALLOCATED: u64 = 0x4D45_4D44;

/// Region 可读。
pub const REGION_READ: u32 = RIGHT_READ;
/// Region 可写。
pub const REGION_WRITE: u32 = RIGHT_WRITE;
/// Region 可执行。
pub const REGION_EXECUTE: u32 = RIGHT_EXECUTE;
/// Region 写时复制。
pub const REGION_COW: u32 = REGION_FLAG_COW;
/// Region guard。
pub const REGION_GUARD: u32 = REGION_FLAG_GUARD;
/// Region 固定驻留。
pub const REGION_PINNED: u32 = REGION_FLAG_PINNED;
/// Region 只读共享。
pub const REGION_SEALED: u32 = REGION_FLAG_SEALED;

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

/// 把类型化能力的一段映射进当前 VSpace，返回 Region 句柄。
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
    let region = region_map(capability, 0, length, REGION_READ | REGION_WRITE, 0)?;
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
        REGION_READ | REGION_WRITE,
        0,
    )?;
    let (virt, _) = region_info(region)?;
    Some((region, virt))
}

/// 把类型化能力绑定到 DeviceGrant 的 IOMMU 域。
pub fn dma_map_capability(grant: u64, capability: u64, length: usize) -> u64 {
    // SAFETY: 内核校验授权、能力句柄与长度后建立 IOMMU 映射。
    unsafe { syscall_call(SYS_DMA_MAP_CAPABILITY, grant, capability, length as u64, 0).value }
}

/// 把 Frame 绑定到 DeviceGrant 的 IOMMU 域。
pub fn dma_map_frame(grant: u64, frame: u64, length: usize) -> u64 {
    // SAFETY: 内核校验授权、Frame 句柄与长度后建立 IOMMU 映射。
    unsafe { syscall_call(SYS_DMA_MAP_FRAME, grant, frame, length as u64, 0).value }
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
    /// 起始物理地址。
    pub start: u64,
    /// 区间长度。
    pub len: u64,
    /// 用途编码。
    pub kind: u8,
    /// 页大小。
    pub page_size: u64,
    /// 配额字节数。
    pub limit: u64,
    /// 已用字节数。
    pub used: u64,
}

/// 查询 Capacity。
pub fn capacity_info(capacity: u64) -> Option<CapacityInfo> {
    let mut bytes = [0_u8; 48];
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
    if result != 48 {
        return None;
    }
    Some(CapacityInfo {
        start: u64::from_le_bytes(bytes[..8].try_into().ok()?),
        len: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
        kind: u64::from_le_bytes(bytes[16..24].try_into().ok()?) as u8,
        page_size: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
        limit: u64::from_le_bytes(bytes[32..40].try_into().ok()?),
        used: u64::from_le_bytes(bytes[40..].try_into().ok()?),
    })
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

/// 把 Frame 归还给 Capacity。
pub fn frame_free(frame: u64, capacity: u64) -> u64 {
    // SAFETY: 内核校验 Frame、Capacity 与引用状态。
    unsafe { syscall_call(SYS_FRAME_FREE, frame, capacity, 0, 0).value }
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
    /// 起始虚拟地址。
    pub base: u64,
    /// 地址区间长度。
    pub size: u64,
    /// 已登记 Region 的字节数。
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

/// System Capacity 记账快照。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryStats {
    /// 总字节数。
    pub total: u64,
    /// 已用字节数。
    pub used: u64,
    /// 峰值字节数。
    pub peak: u64,
    /// 空闲字节数。
    pub free: u64,
    /// 各用途已用字节数：KernelHeap、PageTable、Cow、Restore、Image、Dma、Other。
    pub by_kind: [u64; 7],
}

/// 查询 System Capacity 的 FrameBank 记账快照。
pub fn memory_stats() -> Option<MemoryStats> {
    let mut bytes = [0_u8; 88];
    // SAFETY: 缓冲区属于本进程；内核写入 System Capacity 快照。
    let result = unsafe {
        syscall_call(
            SYS_MEMORY_STATS,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
            0,
        )
        .value
    };
    if result != 88 {
        return None;
    }
    let mut by_kind = [0_u64; 7];
    for (index, value) in by_kind.iter_mut().enumerate() {
        let start = 32 + index * 8;
        *value = u64::from_le_bytes(bytes[start..start + 8].try_into().ok()?);
    }
    Some(MemoryStats {
        total: u64::from_le_bytes(bytes[..8].try_into().ok()?),
        used: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
        peak: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
        free: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
        by_kind,
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
}

/// 查询当前任务的内存账本。
pub fn ledger_info() -> Option<LedgerInfo> {
    let mut bytes = [0_u8; 32];
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
    if result != 32 {
        return None;
    }
    Some(LedgerInfo {
        charged: u64::from_le_bytes(bytes[..8].try_into().ok()?),
        limit: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
        peak: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
        pressure: u64::from_le_bytes(bytes[24..].try_into().ok()?),
    })
}

/// 创建按需装入的匿名能力。
pub fn capability_create_lazy(length: usize) -> Option<u64> {
    // SAFETY: 内核校验长度后创建匿名能力并返回句柄。
    let handle = unsafe {
        syscall_call(
            SYS_CAPABILITY_CREATE_LAZY,
            u64::from(CapabilityKind::Anonymous.as_u8()),
            length as u64,
            0,
            0,
        )
        .value
    };
    (handle != u64::MAX).then_some(handle)
}

/// 把类型化能力绑定到 Provider 端点。
pub fn capability_bind_provider(capability: u64, endpoint: u64) -> u64 {
    // SAFETY: 内核校验能力与端点句柄后建立绑定。
    unsafe { syscall_call(SYS_CAPABILITY_BIND_PROVIDER, capability, endpoint, 0, 0).value }
}

/// 导出可转移的能力句柄。
pub fn capability_export(capability: u64) -> Option<u64> {
    // SAFETY: 内核校验能力类型并复制可转移句柄。
    let exported = unsafe { syscall_call(SYS_CAPABILITY_EXPORT, capability, 0, 0, 0).value };
    (exported != u64::MAX && exported != 0).then_some(exported)
}
