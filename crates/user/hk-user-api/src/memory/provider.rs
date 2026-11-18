//! Provider 协议常量、能力绑定与内存审计/账本查询。

use hk_syscall::{
    LEDGER_RECORD_BYTES, SYS_CAPABILITY_BIND_PROVIDER, SYS_CAPABILITY_EXPORT, SYS_LEDGER_ENUM,
    SYS_LEDGER_INFO, SYS_MEMORY_EVENTS, SYS_MEMORY_PRESSURE, SYS_OBJECT_LEDGER, SYS_OOM_EVENT,
    SYS_OOM_POLICY,
};

use crate::syscall_call;

pub use hk_syscall::{MEMORY_ALLOC, MEMORY_ALLOCATED, MEMORY_FRAME, MEMORY_FRAME_ALLOCATED};

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

/// Provider 按对象标识重新取得自己提供的能力句柄；权利限制在映射所需范围。
pub fn grant_capability(capability: u64, rights: u32) -> Option<u64> {
    let handle = hk_syscall_client::syscall::capability_grant(capability, rights);
    (handle != u64::MAX).then_some(handle)
}

/// 返回内存审计事件数。
pub fn memory_event_count() -> usize {
    // SAFETY: 长度为 0 只查询计数。
    unsafe { syscall_call(SYS_MEMORY_EVENTS, 0, 0, 0, 0).value as usize }
}

/// 读取内存审计事件；返回写入字节数。
pub fn memory_events(buffer: &mut [u8]) -> usize {
    // SAFETY: 缓冲区可写；内核按长度写入。
    unsafe {
        syscall_call(
            SYS_MEMORY_EVENTS,
            buffer.as_mut_ptr() as u64,
            buffer.len() as u64,
            0,
            0,
        )
        .value as usize
    }
}

/// 读取最近一次 OOM 审计记录；返回写入字节数，0 表示暂无记录。
///
/// 前 5 个 u64 为 free%、回收字节、killed、OOM 次数、终止次数；之后是 victim 名。
pub fn oom_event(buffer: &mut [u8]) -> Option<usize> {
    // SAFETY: 缓冲区属于本进程；内核写入最近一次审计记录。
    let result = unsafe {
        syscall_call(
            SYS_OOM_EVENT,
            buffer.as_mut_ptr() as u64,
            buffer.len() as u64,
            0,
            0,
        )
        .value
    };
    (result != u64::MAX).then_some(result as usize)
}

/// 设置 OOM 阈值百分比与动作：0 只回收，1 允许终止；仅 Root 或测试 Provider。
pub fn set_oom_policy(threshold_percent: u8, action: u8) -> u64 {
    // SAFETY: 内核校验调用者身份与阈值/动作范围。
    unsafe {
        syscall_call(
            SYS_OOM_POLICY,
            u64::from(threshold_percent),
            u64::from(action),
            0,
            0,
        )
        .value
    }
}

/// 查询全局内存压力；返回 `(free, total, oom_threshold_percent)`。
///
/// 这是 `SYS_MEMORY_PRESSURE` 的前 3 个 u64；节点明细与审计计数由完整
/// 快照接口读取。
pub fn memory_pressure() -> Option<(u64, u64, u64)> {
    let mut bytes = [0_u8; 9 * 8];
    // SAFETY: 缓冲区属于本进程；内核写入 9 个统计字。
    let result = unsafe {
        syscall_call(
            SYS_MEMORY_PRESSURE,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
            0,
        )
        .value
    };
    if result < 24 {
        return None;
    }
    Some((
        u64::from_le_bytes(bytes[0..8].try_into().ok()?),
        u64::from_le_bytes(bytes[8..16].try_into().ok()?),
        u64::from_le_bytes(bytes[16..24].try_into().ok()?),
    ))
}

/// 返回 `SYS_MEMORY_PRESSURE` 的 9 个统计字。
pub fn memory_pressure_full() -> Option<[u64; 9]> {
    let mut bytes = [0_u8; 9 * 8];
    // SAFETY: 缓冲区属于本进程；内核写入 9 个统计字。
    let result = unsafe {
        syscall_call(
            SYS_MEMORY_PRESSURE,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
            0,
        )
        .value
    };
    if result < 72 {
        return None;
    }
    let mut values = [0_u64; 9];
    for (index, value) in values.iter_mut().enumerate() {
        *value = u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().ok()?);
    }
    Some(values)
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
