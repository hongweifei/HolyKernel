//! 运行时视图与统计。

use super::*;

/// 任务视图。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskInfo {
    /// TCB 编号。
    pub tcb: u64,
    /// 任务名称。
    pub name: String,
    /// 父任务 TCB 编号。
    pub parent: Option<u64>,
    /// 端点编号。
    pub endpoint: u64,
    /// 线程状态。
    pub state: &'static str,
    /// 挂起的系统调用。
    pub pending: &'static str,
    /// 在用句柄数。
    pub handles: usize,
    /// Arena 已用字节数。
    pub arena_used: u64,
    /// Arena 总字节数。
    pub arena_size: u64,
    /// 页表根物理地址。
    pub vspace: u64,
    /// 退出码。
    pub exit_code: Option<u64>,
}

/// 运行时统计。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuntimeStats {
    /// 上下文切换次数。
    pub switches: u64,
    /// 上下文切换总周期数。
    pub switch_cycles: u64,
    /// 单次切换最大周期数。
    pub switch_max_cycles: u64,
    /// IPC call/reply 次数。
    pub calls: u64,
    /// IPC call/reply 总周期数。
    pub call_cycles: u64,
    /// 单次 call/reply 最大周期数。
    pub call_max_cycles: u64,
    /// 各 CPU 恢复用户任务的次数。
    pub resumes: [u64; MAX_CPUS],
    /// TSC 频率；未知时为 0。
    pub tsc_frequency: u64,
}
