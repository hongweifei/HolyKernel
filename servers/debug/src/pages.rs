//! 调试视图：概览、任务、服务、trace、统计与异常回溯。

use alloc::format;
use alloc::string::String;
use alloc::vec;

use hk_server_api::root::{ROOT_OP_SERVICES, ROOT_OP_STATS, ROOT_OP_TASKS, ROOT_OP_TRACE, call};
use hk_server_api::{
    handle_count, kernel_stats, memory_stats, service_endpoint, ticks, tsc_frequency,
};

/// Root 控制结果缓冲长度。
const BUFFER: usize = 32 * 1024;

/// 调试视图。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    /// 内核概览。
    Overview,
    /// 任务视图。
    Tasks,
    /// 服务视图。
    Services,
    /// trace 事件。
    Trace,
    /// 运行时统计。
    Stats,
    /// 最近一次用户异常。
    Fault,
}

impl Page {
    /// 全部视图，顺序与数字键一致。
    pub(crate) const ALL: [Self; 6] = [
        Self::Overview,
        Self::Tasks,
        Self::Services,
        Self::Trace,
        Self::Stats,
        Self::Fault,
    ];

    /// 页面标题。
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Overview => "概览",
            Self::Tasks => "任务",
            Self::Services => "服务",
            Self::Trace => "Trace",
            Self::Stats => "统计",
            Self::Fault => "异常回溯",
        }
    }

    /// 串口日志用的短名。
    pub(crate) fn slug(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Tasks => "tasks",
            Self::Services => "services",
            Self::Trace => "trace",
            Self::Stats => "stats",
            Self::Fault => "fault",
        }
    }

    /// 读取视图文本。
    pub(crate) fn fetch(self) -> String {
        match self {
            Self::Overview => overview(),
            Self::Tasks => root(ROOT_OP_TASKS),
            Self::Services => root(ROOT_OP_SERVICES),
            Self::Trace => root(ROOT_OP_TRACE),
            Self::Stats => root(ROOT_OP_STATS),
            Self::Fault => hk_debug::fault_text(),
        }
    }
}

fn root(op: u64) -> String {
    let mut buffer = vec![0_u8; BUFFER];
    match call(op, "", &mut buffer) {
        Ok(length) => String::from_utf8_lossy(&buffer[..length.min(buffer.len())]).into_owned(),
        Err(code) => format!("Root 控制失败：{code}\n"),
    }
}

fn overview() -> String {
    let stats = kernel_stats();
    let memory = memory_stats().unwrap_or_default();
    let display = service_endpoint("hk-server-display");
    format!(
        "同步统计\n\
         线程: {}\n\
         对象: {}\n\
         就绪: {}\n\
         Trace: {}\n\
         调度 tick: {}\n\
         TSC Hz: {}\n\
         本任务句柄: {}\n\
         Display: {}\n\
         FrameBank: {} / {} KiB（峰值 {}）\n\
         内存分类 KiB: heap {} page-table {} cow {} restore {} image {} dma {} other {}\n",
        stats.threads,
        stats.objects,
        stats.runnable,
        stats.trace_events,
        ticks(),
        tsc_frequency(),
        handle_count(),
        if display == 0 { "离线" } else { "在线" },
        memory.used / 1024,
        memory.total / 1024,
        memory.peak / 1024,
        memory.by_kind[0] / 1024,
        memory.by_kind[1] / 1024,
        memory.by_kind[2] / 1024,
        memory.by_kind[3] / 1024,
        memory.by_kind[4] / 1024,
        memory.by_kind[5] / 1024,
        memory.by_kind[6] / 1024,
    )
}
