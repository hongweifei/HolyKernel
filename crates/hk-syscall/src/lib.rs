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

/// echo 服务的端点编号。
pub const ENDPOINT_ECHO: u64 = 2;
