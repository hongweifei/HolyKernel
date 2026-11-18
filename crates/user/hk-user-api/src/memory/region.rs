//! Region、Arena 与 DMA 映射的系统调用封装。

use hk_syscall::{
    RIGHT_READ, RIGHT_WRITE, SYS_ARENA_GROW, SYS_ARENA_INFO, SYS_DMA_MAP, SYS_REGION_GROW,
    SYS_REGION_INFO, SYS_REGION_MAP, SYS_REGION_PROTECT, SYS_REGION_UNMAP, SYS_SYSTEM_ARENA_INFO,
    SYS_SYSTEM_ARENA_REGIONS,
};

use super::capability_phys;
use crate::syscall_call;

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

/// 扩展当前任务的 Arena 分配窗口；返回新窗口大小，失败返回 `u64::MAX`。
pub fn arena_grow(additional: usize) -> u64 {
    // SAFETY: 内核只按页对齐长度扩展当前任务的 Arena。
    unsafe { syscall_call(SYS_ARENA_GROW, additional as u64, 0, 0, 0).value }
}

/// System Arena 快照；编码与任务 Arena 相同。
pub type SystemArenaInfo = ArenaInfo;

/// 查询内核 System Arena。
pub fn system_arena_info() -> Option<SystemArenaInfo> {
    let mut bytes = [0_u8; 32];
    // SAFETY: 缓冲区属于本进程；内核写入 System Arena 快照。
    let result = unsafe {
        syscall_call(
            SYS_SYSTEM_ARENA_INFO,
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
    Some(SystemArenaInfo {
        base: u64::from_le_bytes(bytes[..8].try_into().ok()?),
        size: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
        used: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
        regions: u64::from_le_bytes(bytes[24..].try_into().ok()?),
    })
}

/// System Arena 的单条 Region 记录。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemRegionRecord {
    /// Region 起始虚拟地址。
    pub virt: u64,
    /// Region 字节长度。
    pub len: u64,
    /// 权利位图（RIGHT_READ/RIGHT_WRITE/RIGHT_EXECUTE）。
    pub rights: u32,
    /// Region 标志位图（REGION_FLAG_*）。
    pub flags: u32,
}

/// 返回 System Arena 的 Region 数；平台不支持时返回 `None`。
pub fn system_arena_region_count() -> Option<u64> {
    // SAFETY: 只查询数量，不写入用户内存。
    let result = unsafe { syscall_call(SYS_SYSTEM_ARENA_REGIONS, 0, 0, 0, 0).value };
    if result == u64::MAX {
        return None;
    }
    Some(result)
}

/// 按索引查询 System Arena 的 Region；越界或平台不支持时返回 `None`。
pub fn system_arena_region(index: usize) -> Option<SystemRegionRecord> {
    let mut bytes = [0_u8; hk_syscall::SYSTEM_REGION_RECORD_BYTES];
    // SAFETY: 缓冲区属于本进程；内核写入一条 Region 记录。
    let result = unsafe {
        syscall_call(
            SYS_SYSTEM_ARENA_REGIONS,
            index as u64,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
        )
        .value
    };
    if result != bytes.len() as u64 {
        return None;
    }
    Some(SystemRegionRecord {
        virt: u64::from_le_bytes(bytes[..8].try_into().ok()?),
        len: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
        rights: u32::from_le_bytes(bytes[16..20].try_into().ok()?),
        flags: u32::from_le_bytes(bytes[20..].try_into().ok()?),
    })
}
