//! 网络协议栈错误。

use core::fmt;

/// `hk-net` 操作错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetError {
    /// 帧长度超过 MTU 或队列已满。
    Queue,
    /// IP 地址或前缀无效。
    Address,
    /// 路由表已满。
    Route,
    /// 端口绑定失败。
    Bind,
    /// 发送失败。
    Send,
    /// 接收失败。
    Recv,
}

impl fmt::Display for NetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Queue => "网络队列不可用",
            Self::Address => "网络地址无效",
            Self::Route => "路由表已满",
            Self::Bind => "端口绑定失败",
            Self::Send => "发送失败",
            Self::Recv => "接收失败",
        })
    }
}

impl core::error::Error for NetError {}
