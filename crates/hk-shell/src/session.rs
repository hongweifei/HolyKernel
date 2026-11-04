//! Shell 会话状态。

use alloc::string::String;

/// Shell 会话：当前任务的选择与脚本嵌套深度。
#[derive(Default)]
pub struct Session {
    pub(crate) current: Option<String>,
    pub(crate) depth: usize,
}

impl Session {
    /// 构造空会话。
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回当前任务名称。
    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }
}
