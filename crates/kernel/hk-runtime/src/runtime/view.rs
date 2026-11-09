//! 运行时视图与统计。

use super::*;

impl<C, P> UserRuntime<C, P>
where
    C: UserContext + SyscallFrame,
    P: UserPlatform,
{
    /// 返回任务索引；名称匹配且未退出。
    fn task_index(&self, name: &str) -> Option<usize> {
        self.tasks
            .iter()
            .position(|task| task.name == name && !task.exited)
    }

    /// 输出任务的 Cadence 契约。
    pub fn cadence_text(&self, name: &str) -> String {
        let Some(index) = self.task_index(name) else {
            return format!("未知任务：{name}\n");
        };
        let Some(snapshot) = self.kernel.task_cadence_snapshot(self.tasks[index].tcb) else {
            return format!("任务缺少契约：{name}\n");
        };
        format!(
            "cadence {name}：class={} weight={} budget={}us period={}us deadline={}us latency={}us affinity={:#x} migratable={} lending={} epoch={}\n",
            snapshot.class.name(),
            snapshot.weight,
            snapshot.budget,
            snapshot.period,
            snapshot.deadline,
            snapshot.latency,
            snapshot.affinity,
            snapshot.migratable,
            if snapshot.lending.enabled() {
                "caller-pays"
            } else {
                "off"
            },
            snapshot.epoch
        )
    }

    /// 输出任务的 Meter 计量。
    pub fn meter_text(&self, name: &str) -> String {
        let Some(index) = self.task_index(name) else {
            return format!("未知任务：{name}\n");
        };
        let Some(snapshot) = self.kernel.task_meter_snapshot(self.tasks[index].tcb) else {
            return format!("任务缺少计量：{name}\n");
        };
        let cpu = if snapshot.last_cpu == u32::MAX {
            String::from("none")
        } else {
            format!("{}", snapshot.last_cpu)
        };
        format!(
            "meter {name}：consumed={}us waited={}us throttled={} overruns={} migrations={} switches={} virtual_pass={} last_cpu={}\n",
            snapshot.consumed,
            snapshot.waited,
            snapshot.throttled,
            snapshot.overruns,
            snapshot.migrations,
            snapshot.switches,
            snapshot.virtual_pass,
            cpu
        )
    }

    /// 输出 Tempo 策略池与自身计量。
    pub fn tempo_text(&self) -> String {
        if self.task_index("hk-server-tempo").is_none() {
            return String::from("Tempo 未运行\n");
        }
        let mut text = String::from("tempo：hk-server-tempo\n");
        text.push_str(&self.cadence_text("hk-server-tempo"));
        text.push_str(&self.meter_text("hk-server-tempo"));
        text
    }
}

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
