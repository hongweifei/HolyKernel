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
    /// 累计运行周期数。
    pub cpu_cycles: u64,
    /// 调度类别。
    pub class: &'static str,
    /// Cadence 权重。
    pub weight: u32,
    /// Meter 累计实际执行时间（微秒）。
    pub consumed: u64,
    /// Meter 虚拟进度。
    pub virtual_pass: u64,
    /// Meter 预算耗尽次数。
    pub throttled: u64,
    /// Arena 已用字节数。
    pub arena_used: u64,
    /// Arena 总字节数。
    pub arena_size: u64,
    /// 任务 Ledger 提交字节数。
    pub ledger_charged: u64,
    /// 任务 Ledger 常驻字节数。
    pub ledger_resident: u64,
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
    /// 各 CPU 累计运行用户任务的周期数。
    pub cpu_busy: [u64; MAX_CPUS],
    /// TSC 频率；未知时为 0。
    pub tsc_frequency: u64,
}
