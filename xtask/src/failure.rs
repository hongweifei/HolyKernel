//! 统一的失败类型。
//!
//! xtask 的每个步骤出错即终止，不重试、不降级：构建链路的问题就地暴露。

use std::error::Error;
use std::fmt;

/// 任务失败。
#[derive(Debug)]
pub struct Failure(String);

impl Failure {
    /// 以消息构造失败。
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for Failure {}

/// 任务结果。
pub type Result<T> = std::result::Result<T, Failure>;
