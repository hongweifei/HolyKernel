//! Shell 的输入输出与宿主契约。

use alloc::string::String;
use alloc::vec::Vec;

use crate::view::{RuntimeStats, ServiceInfo, TaskInfo, TraceEvent};

/// 命令执行所需的系统视图。
#[derive(Clone, Copy)]
pub struct ShellState<'a> {
    /// 服务视图。
    pub services: &'a [ServiceInfo],
    /// 任务视图。
    pub tasks: &'a [TaskInfo],
}

impl<'a> ShellState<'a> {
    /// 以服务与任务视图构造上下文。
    pub const fn new(services: &'a [ServiceInfo], tasks: &'a [TaskInfo]) -> Self {
        Self { services, tasks }
    }
}

/// Shell 的输入与输出。
pub trait ShellIo {
    /// 写出文本。
    fn write(&self, text: &str);

    /// 清屏。
    fn clear(&self);

    /// 读取一行输入，不包含换行符。
    fn read_line(&self) -> String;
}

/// Shell 命令宿主：任务视图与服务启停。
pub trait ShellHost {
    /// 返回任务视图快照。
    fn tasks(&self) -> Vec<TaskInfo>;

    /// 返回服务视图快照（宿主可先同步其他 CPU 的退出事件）。
    fn services(&mut self) -> Vec<ServiceInfo>;

    /// 返回最近的内核 trace 事件。
    fn trace(&self, limit: usize) -> Vec<TraceEvent>;

    /// 返回运行时统计。
    fn stats(&self) -> RuntimeStats;

    /// 启动服务；`parent` 是当前会话选中的任务名。
    fn start(&mut self, name: &str, parent: Option<&str>) -> String;

    /// 重启已退出的服务；`parent` 是当前会话选中的任务名。
    fn restart(&mut self, name: &str, parent: Option<&str>) -> String;

    /// 读取脚本内容；名称可省略 `.hsh` 后缀。
    fn script(&self, name: &str) -> Result<String, String>;
}
