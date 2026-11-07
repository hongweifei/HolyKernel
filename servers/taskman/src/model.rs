//! 任务管理器数据模型：解析性能快照、排序与采样历史。

use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

/// 自动刷新间隔（调度 tick，约 10 ms 一个）。
pub const REFRESH_TICKS: u64 = 100;
/// CPU 曲线最多保留的采样数。
pub const HISTORY: usize = 60;
/// 单CPU 曲线支持的最大核数。
pub const MAX_CORES: usize = 64;

/// 顶部页面。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// 进程列表。
    Processes,
    /// CPU / 内存性能曲线。
    Performance,
}

impl Page {
    /// 页面标题。
    pub const fn title(self) -> &'static str {
        match self {
            Self::Processes => "进程",
            Self::Performance => "性能",
        }
    }
}

/// 单行任务。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRow {
    /// TCB 编号。
    pub tcb: u64,
    /// 任务名称。
    pub name: String,
    /// 父任务 TCB。
    pub parent: Option<u64>,
    /// 端点编号。
    pub endpoint: u64,
    /// 线程状态。
    pub state: String,
    /// 挂起的系统调用。
    pub pending: String,
    /// 退出码。
    pub exit_code: Option<u64>,
    /// 在用句柄数。
    pub handles: u64,
    /// 累计运行周期数。
    pub cpu_cycles: u64,
    /// 最近一个采样区间的 CPU 占用百分比。
    pub cpu_percent: u64,
    /// Arena 已用字节数。
    pub arena_used: u64,
    /// Arena 总字节数。
    pub arena_size: u64,
}

impl TaskRow {
    /// 状态排序权重：运行中优先，其后就绪、阻塞、退出。
    pub fn state_rank(&self) -> u8 {
        match self.state.as_str() {
            "running" => 0,
            "runnable" => 1,
            "blocked" => 2,
            "exited" => 3,
            _ => 4,
        }
    }

    /// Arena 使用率百分比。
    pub fn arena_percent(&self) -> u64 {
        if self.arena_size == 0 {
            return 0;
        }
        self.arena_used.saturating_mul(100) / self.arena_size
    }
}

/// System Capacity 记账快照。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemMemory {
    /// 总字节数。
    pub total: u64,
    /// 已用字节数。
    pub used: u64,
    /// 峰值字节数。
    pub peak: u64,
    /// 空闲字节数。
    pub free: u64,
    /// 内核堆。
    pub heap: u64,
    /// 页表。
    pub page_table: u64,
    /// 写时复制。
    pub cow: u64,
    /// 检查点恢复。
    pub restore: u64,
    /// 用户镜像。
    pub image: u64,
    /// DMA。
    pub dma: u64,
    /// 其他。
    pub other: u64,
}

/// 一次性能快照。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PerfSample {
    /// 在线 CPU 数。
    pub cpus: usize,
    /// 任务数。
    pub tasks: u64,
    /// 就绪任务数。
    pub runnable: u64,
    /// TSC 频率。
    pub tsc_hz: u64,
    /// 调度 tick。
    pub ticks: u64,
    /// 每 CPU 累计忙周期。
    pub cpu_busy: Vec<u64>,
    /// 每 CPU 恢复次数。
    pub cpu_resumes: Vec<u64>,
    /// 任务行。
    pub rows: Vec<TaskRow>,
    /// System Capacity 快照。
    pub system: SystemMemory,
}

impl PerfSample {
    /// System Capacity 已用字节。
    pub fn memory_used(&self) -> u64 {
        self.system.used
    }

    /// System Capacity 总量。
    pub fn memory_total(&self) -> u64 {
        self.system.total
    }

    /// 所有任务 Arena 已用字节。
    pub fn arena_used(&self) -> u64 {
        self.rows.iter().map(|row| row.arena_used).sum()
    }

    /// 所有任务 Arena 总量。
    pub fn arena_total(&self) -> u64 {
        self.rows.iter().map(|row| row.arena_size).sum()
    }
}

/// 排序键。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    /// 按 Arena 用量降序。
    Arena,
    /// 按名称。
    Name,
    /// 按状态分组。
    State,
    /// 按句柄数降序。
    Handles,
    /// 按最近 CPU 占用降序。
    Cpu,
    /// 按 TCB 编号。
    Tcb,
}

impl SortKey {
    /// 状态栏显示的短名。
    pub const fn title(self) -> &'static str {
        match self {
            Self::Arena => "内存",
            Self::Name => "名称",
            Self::State => "状态",
            Self::Handles => "句柄",
            Self::Cpu => "CPU",
            Self::Tcb => "TCB",
        }
    }

    /// 从快捷键字节映射到排序键。
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            b'a' => Some(Self::Arena),
            b'n' => Some(Self::Name),
            b's' => Some(Self::State),
            b'h' => Some(Self::Handles),
            b'c' => Some(Self::Cpu),
            b't' => Some(Self::Tcb),
            _ => None,
        }
    }
}

/// 解析 `ROOT_OP_PERF` 的性能快照。
pub fn parse_perf(text: &str) -> PerfSample {
    let mut sample = PerfSample {
        cpus: 1,
        ..PerfSample::default()
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("perf ") {
            for field in rest.split_whitespace() {
                let Some((key, value)) = field.split_once('=') else {
                    continue;
                };
                match key {
                    "cpus" => sample.cpus = value.parse().unwrap_or(1).max(1),
                    "tasks" => sample.tasks = value.parse().unwrap_or(0),
                    "runnable" => sample.runnable = value.parse().unwrap_or(0),
                    "tsc" => sample.tsc_hz = value.parse().unwrap_or(0),
                    "ticks" => sample.ticks = value.parse().unwrap_or(0),
                    _ => {}
                }
            }
        } else if let Some(rest) = line.strip_prefix("mem ") {
            for field in rest.split_whitespace() {
                let Some((key, value)) = field.split_once('=') else {
                    continue;
                };
                let value = value.parse().unwrap_or(0);
                match key {
                    "total" => sample.system.total = value,
                    "used" => sample.system.used = value,
                    "peak" => sample.system.peak = value,
                    "free" => sample.system.free = value,
                    "heap" => sample.system.heap = value,
                    "page_table" => sample.system.page_table = value,
                    "cow" => sample.system.cow = value,
                    "restore" => sample.system.restore = value,
                    "image" => sample.system.image = value,
                    "dma" => sample.system.dma = value,
                    "other" => sample.system.other = value,
                    _ => {}
                }
            }
        } else if let Some(rest) = line.strip_prefix("cpu ") {
            let mut fields = rest.split_whitespace();
            let (Some(index), Some(busy), Some(resumes)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let (Ok(index), Ok(busy), Ok(resumes)) = (
                index.parse::<usize>(),
                busy.parse::<u64>(),
                resumes.parse::<u64>(),
            ) else {
                continue;
            };
            if index >= MAX_CORES {
                continue;
            }
            ensure(&mut sample.cpu_busy, index + 1);
            ensure(&mut sample.cpu_resumes, index + 1);
            sample.cpu_busy[index] = busy;
            sample.cpu_resumes[index] = resumes;
        } else if let Some(rest) = line.strip_prefix("task ") {
            if let Some(row) = parse_task(rest) {
                sample.rows.push(row);
            }
        }
    }
    if sample.cpus > MAX_CORES {
        sample.cpus = MAX_CORES;
    }
    if sample.tasks == 0 {
        sample.tasks = sample.rows.len() as u64;
    }
    sample
}

fn ensure(values: &mut Vec<u64>, len: usize) {
    if values.len() < len {
        values.resize(len, 0);
    }
}

fn parse_task(rest: &str) -> Option<TaskRow> {
    let mut fields = rest.splitn(12, ' ');
    let tcb = fields.next()?.parse().ok()?;
    let cpu_cycles = fields.next()?.parse().ok()?;
    let arena_used = fields.next()?.parse().ok()?;
    let arena_size = fields.next()?.parse().ok()?;
    let handles = fields.next()?.parse().ok()?;
    let state = fields.next()?;
    let parent = fields.next()?;
    let endpoint = fields.next()?.parse().ok()?;
    let pending = fields.next()?;
    let exit_code = fields.next()?;
    let name = fields.next()?.trim();
    if name.is_empty() {
        return None;
    }
    Some(TaskRow {
        tcb,
        name: String::from(name),
        parent: parent.parse().ok(),
        endpoint,
        state: String::from(state),
        pending: String::from(pending),
        exit_code: exit_code.parse().ok(),
        handles,
        cpu_cycles,
        cpu_percent: 0,
        arena_used,
        arena_size,
    })
}

/// 把 `delta` 周期换算成 `wall` 区间的千分比。
pub fn permille(delta: u64, wall: u64) -> u64 {
    if wall == 0 {
        return 0;
    }
    delta.saturating_mul(1000) / wall
}

/// 按排序键就地排序。
pub fn sort(rows: &mut [TaskRow], key: SortKey) {
    rows.sort_unstable_by(|left, right| compare(left, right, key));
}

fn compare(left: &TaskRow, right: &TaskRow, key: SortKey) -> Ordering {
    match key {
        SortKey::Arena => right
            .arena_used
            .cmp(&left.arena_used)
            .then_with(|| left.name.cmp(&right.name)),
        SortKey::Name => left.name.cmp(&right.name),
        SortKey::State => left
            .state_rank()
            .cmp(&right.state_rank())
            .then_with(|| right.arena_used.cmp(&left.arena_used)),
        SortKey::Handles => right
            .handles
            .cmp(&left.handles)
            .then_with(|| left.name.cmp(&right.name)),
        SortKey::Cpu => right
            .cpu_percent
            .cmp(&left.cpu_percent)
            .then_with(|| right.arena_used.cmp(&left.arena_used)),
        SortKey::Tcb => left.tcb.cmp(&right.tcb),
    }
}
