//! 内核操作错误。

use core::fmt;

use hk_handle::{HandleError, ObjectId, ObjectKind};

use crate::tcb::TcbId;

/// 内核操作错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelError {
    /// 句柄操作失败。
    Handle(HandleError),
    /// 线程编号不存在。
    UnknownThread(TcbId),
    /// 对象已析构，注册表项失效。
    StaleObject(ObjectId),
    /// 对象类别与操作不符。
    WrongKind {
        /// 期望的对象类别。
        expected: ObjectKind,
    },
    /// 线程没有处于预期的阻塞状态。
    NotBlocked(TcbId),
    /// 回复令牌与调用方状态不符。
    InvalidReply,
    /// 调用方没有待取回复。
    NoReply,
    /// Endpoint 的消息队列已满。
    QueueFull,
    /// 能力正在 scanout 驻留，不能解除固定。
    ScanoutActive,
}

impl fmt::Display for KernelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Handle(error) => write!(formatter, "句柄错误：{error}"),
            Self::UnknownThread(id) => write!(formatter, "线程 {} 不存在", id.get()),
            Self::StaleObject(id) => write!(formatter, "对象 {} 已析构", id.get()),
            Self::WrongKind { expected } => {
                write!(formatter, "对象类别不是 {}", expected.name())
            }
            Self::NotBlocked(id) => write!(formatter, "线程 {} 未处于阻塞状态", id.get()),
            Self::InvalidReply => formatter.write_str("回复令牌无效"),
            Self::NoReply => formatter.write_str("没有待取回复"),
            Self::QueueFull => formatter.write_str("Endpoint 消息队列已满"),
            Self::ScanoutActive => formatter.write_str("能力正在 scanout，不能解除固定"),
        }
    }
}

impl core::error::Error for KernelError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Handle(error) => Some(error),
            _ => None,
        }
    }
}

impl From<HandleError> for KernelError {
    fn from(error: HandleError) -> Self {
        Self::Handle(error)
    }
}
