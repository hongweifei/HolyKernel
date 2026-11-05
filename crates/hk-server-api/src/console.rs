//! 控制台服务协议与客户端。
//!
//! 客户端把文本发给 Console 服务（`CONSOLE_WRITE`），并以请求/应答方式读取
//! 一行（`CONSOLE_READ_REQUEST` → `CONSOLE_READ_LINE`）。输入事件由内核通过
//! 通知位 `CONSOLE_NOTIFY_INPUT` 唤醒 Console 服务。

pub use hk_syscall::{CONSOLE_NOTIFY_INPUT, CONSOLE_NOTIFY_MESSAGE};

use crate::{notify, recv_data_from, send, send_data};

/// 写文本（UTF-8）；载荷为文本。
pub const CONSOLE_WRITE: u64 = 0x900;
/// 请求读取一行。
pub const CONSOLE_READ_REQUEST: u64 = 0x901;
/// 一行输入（不含换行）；载荷为文本。
pub const CONSOLE_READ_LINE: u64 = 0x902;
/// 请求 Console 服务退出。
pub const CONSOLE_SHUTDOWN: u64 = 0x903;
/// 图形终端完成一次提交（仅终端模式发送）。
pub const CONSOLE_WRITE_ACK: u64 = 0x904;

/// 控制台客户端。
#[derive(Clone, Copy)]
pub struct Client {
    endpoint: u64,
    console: u64,
}

impl Client {
    /// 等待 Console 服务并构造客户端。
    pub fn discover(endpoint: u64) -> Option<Self> {
        let console = crate::runtime::wait_service("hk-server-console")?;
        Some(Self { endpoint, console })
    }

    /// 写入文本。
    pub fn write(&self, text: &[u8]) {
        let _ = send_data(self.console, CONSOLE_WRITE, text);
        let _ = notify(self.console, CONSOLE_NOTIFY_MESSAGE);
    }

    /// 请求并读取一行，返回字节数。
    pub fn read_line(&self, buffer: &mut [u8]) -> usize {
        let _ = send(self.console, CONSOLE_READ_REQUEST, 0);
        let _ = notify(self.console, CONSOLE_NOTIFY_MESSAGE);
        let (label, _word, sender, length) = recv_data_from(self.endpoint, self.console, buffer);
        if label == CONSOLE_READ_LINE && sender == self.console {
            length.min(buffer.len())
        } else {
            0
        }
    }

    /// 请求 Console 服务退出。
    pub fn shutdown(&self) {
        let _ = send(self.console, CONSOLE_SHUTDOWN, 0);
        let _ = notify(self.console, CONSOLE_NOTIFY_MESSAGE);
    }
}
