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

/// echo 服务的端点编号。
pub const ENDPOINT_ECHO: u64 = 2;
