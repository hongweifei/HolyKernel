//! Frame、FrameBank 快照与对象枚举的系统调用封装。

use hk_syscall::{
    CAPACITY_KIND_COUNT, MAX_NUMA_NODES, SYS_FRAME_BANK_STATS, SYS_FRAME_INFO, SYS_FRAME_MAP,
    SYS_FRAME_PIN, SYS_FRAME_READ, SYS_FRAME_WRITE, SYS_HANDLE_ID, SYS_MESSAGE_WORDS,
    SYS_OBJECT_ENUM,
};

use crate::syscall_call;

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
    /// 指向该 Frame 的句柄数。
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

/// 把用户缓冲写入 Frame 的常驻内容；返回 0 或 `u64::MAX`。
pub fn frame_write(frame: u64, offset: u64, data: &[u8]) -> u64 {
    // SAFETY: 内核校验 Frame 句柄的 WRITE 权限与范围后复制缓冲。
    unsafe {
        syscall_call(
            SYS_FRAME_WRITE,
            frame,
            offset,
            data.as_ptr() as u64,
            data.len() as u64,
        )
        .value
    }
}

/// 把 Frame 的常驻内容读入用户缓冲；返回 0 或 `u64::MAX`。
pub fn frame_read(frame: u64, offset: u64, data: &mut [u8]) -> u64 {
    // SAFETY: 内核校验 Frame 句柄的 READ 权限与范围后复制缓冲。
    unsafe {
        syscall_call(
            SYS_FRAME_READ,
            frame,
            offset,
            data.as_mut_ptr() as u64,
            data.len() as u64,
        )
        .value
    }
}

/// 把 Frame 映射进当前任务，返回 Region 句柄。
pub fn frame_map(frame: u64, rights: u32, flags: u32) -> Option<u64> {
    let packed = rights as u64 | ((flags as u64) << 32);
    // SAFETY: 内核校验 Frame 与权利后建立 Region。
    let region = unsafe { syscall_call(SYS_FRAME_MAP, frame, packed, 0, 0).value };
    (region != u64::MAX).then_some(region)
}

/// 查询句柄对应的对象标识。
pub fn handle_id(handle: u64) -> Option<u64> {
    // SAFETY: 内核只读取调用者句柄表。
    let id = unsafe { syscall_call(SYS_HANDLE_ID, handle, 0, 0, 0).value };
    (id != u64::MAX).then_some(id)
}

/// 最近一次接收消息的 8 个字；Provider 用它解析 `Fault` 描述。
pub fn message_words() -> Option<[u64; 8]> {
    let mut bytes = [0_u8; 64];
    // SAFETY: 缓冲区属于本进程；内核复制最近一条消息的 8 个字。
    let result =
        unsafe { syscall_call(SYS_MESSAGE_WORDS, bytes.as_mut_ptr() as u64, 0, 0, 0).value };
    if result != 64 {
        return None;
    }
    let mut words = [0_u64; 8];
    for (slot, word) in words.iter_mut().enumerate() {
        *word = u64::from_le_bytes(bytes[slot * 8..slot * 8 + 8].try_into().ok()?);
    }
    Some(words)
}

/// 单条对象记录的编码长度。
pub const OBJECT_RECORD_BYTES: usize = 64;

/// 枚举当前任务持有的 HMP 对象；返回记录数。
pub fn object_enum_count() -> Option<u64> {
    // SAFETY: 只读取当前任务的句柄表，不涉及用户指针。
    let count = unsafe { syscall_call(SYS_OBJECT_ENUM, 0, 0, 0, 0).value };
    (count != u64::MAX).then_some(count)
}

/// 读取第 `index` 条对象记录。
pub fn object_enum(index: u64, out: &mut [u8; OBJECT_RECORD_BYTES]) -> u64 {
    // SAFETY: 缓冲区属于本进程；内核写入定长记录。
    unsafe {
        syscall_call(
            SYS_OBJECT_ENUM,
            index,
            out.as_mut_ptr() as u64,
            out.len() as u64,
            0,
        )
        .value
    }
}

/// FrameBank 物理用量快照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// 各 NUMA 节点已用字节数。
    pub by_node: [u64; MAX_NUMA_NODES],
}

impl Default for FrameBankStats {
    fn default() -> Self {
        Self {
            total: 0,
            used: 0,
            peak: 0,
            free: 0,
            by_capacity: [0; CAPACITY_KIND_COUNT],
            by_node: [0; MAX_NUMA_NODES],
        }
    }
}

/// 查询 FrameBank 物理用量快照。
pub fn frame_bank_stats() -> Option<FrameBankStats> {
    let mut bytes = [0_u8; 32 + CAPACITY_KIND_COUNT * 8 + MAX_NUMA_NODES * 8];
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
    let mut by_node = [0_u64; MAX_NUMA_NODES];
    let base = 32 + CAPACITY_KIND_COUNT * 8;
    for (index, value) in by_node.iter_mut().enumerate() {
        let start = base + index * 8;
        *value = u64::from_le_bytes(bytes[start..start + 8].try_into().ok()?);
    }
    Some(FrameBankStats {
        total: u64::from_le_bytes(bytes[..8].try_into().ok()?),
        used: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
        peak: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
        free: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
        by_capacity,
        by_node,
    })
}
