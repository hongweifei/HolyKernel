//! 内核与服务器共享的系统调用 ABI。
//!
//! 只包含编号与端点约定，不依赖任何平台；寄存器映射由平台层实现。

#![no_std]

/// 退出服务。
pub const SYS_EXIT: u64 = 0;

/// 把一段 UTF-8 文本写入内核控制台。
pub const SYS_WRITE: u64 = 1;

/// 让出 CPU。
pub const SYS_YIELD: u64 = 2;

/// 向端点发送消息。
pub const SYS_SEND: u64 = 3;

/// 从端点接收消息。
pub const SYS_RECV: u64 = 4;

/// 读取 PCI 配置空间。
pub const SYS_PCI_READ: u64 = 5;

/// 读取调度 tick 计数。
pub const SYS_TICKS: u64 = 6;

/// 在任务 Arena 内分配内存。
pub const SYS_ALLOC: u64 = 7;

/// 关闭端点句柄。
pub const SYS_HANDLE_CLOSE: u64 = 8;

/// 查询在用句柄数。
pub const SYS_HANDLE_COUNT: u64 = 9;

/// 发送消息并转移一个句柄。
pub const SYS_SEND_HANDLE: u64 = 10;

/// 置位通知位。
pub const SYS_NOTIFY: u64 = 11;

/// 等待通知位。
pub const SYS_WAIT_NOTIFY: u64 = 12;

/// 调用端点并等待回复。
pub const SYS_CALL: u64 = 13;

/// 回复最近一次调用。
pub const SYS_REPLY: u64 = 14;

/// 复制当前任务。
pub const SYS_FORK: u64 = 15;

/// 读取单调时间戳（TSC）。
pub const SYS_TSC: u64 = 16;

/// 读取时间戳频率。
pub const SYS_TSC_FREQ: u64 = 17;

/// 发送带字节载荷的消息。
pub const SYS_SEND_DATA: u64 = 18;

/// 接收带字节载荷的消息。
pub const SYS_RECV_DATA: u64 = 19;

/// 写入 PCI 配置空间。
pub const SYS_PCI_WRITE: u64 = 20;

/// 创建 DeviceGrant 对象。
pub const SYS_DEVICE_GRANT_CREATE: u64 = 21;

/// 把 DeviceGrant 的 MMIO 范围映射到当前任务。
pub const SYS_DEVICE_MAP: u64 = 22;

/// 分配一致性 DMA 缓冲并映射到当前任务。
pub const SYS_DMA_ALLOC: u64 = 23;

/// 把设备中断绑定到通知。
pub const SYS_IRQ_BIND: u64 = 24;

/// 同步 DMA 缓冲缓存。
pub const SYS_DMA_SYNC: u64 = 26;

/// 接收带字节载荷的消息，并指定期望的发送端点。
pub const SYS_RECV_DATA_FROM: u64 = 27;

/// 按服务名查询当前任务端点。
pub const SYS_SERVICE_ENDPOINT: u64 = 28;

/// 查询 DeviceGrant 编码描述。
pub const SYS_DEVICE_INFO: u64 = 29;

/// 读取民用时间。
pub const SYS_CIVIL_TIME: u64 = 30;

/// 返回当前任务自己的通知编号。
pub const SYS_SELF_NOTIFICATION: u64 = 25;

/// echo 服务的端点编号。
pub const ENDPOINT_ECHO: u64 = 2;

/// 服务角色元数据，链接到镜像的 `.hk_service` 段。
#[repr(C)]
pub struct ServiceRole {
    /// 固定魔数。
    pub magic: u32,
    /// 角色位图。
    pub role: u32,
}

impl ServiceRole {
    /// 元数据魔数。
    pub const MAGIC: u32 = 0x484B_5352;
    /// 设备管理服务角色。
    pub const DEVICE: u32 = 1;

    /// 构建设备管理服务角色。
    pub const fn device() -> Self {
        Self {
            magic: Self::MAGIC,
            role: Self::DEVICE,
        }
    }
}
