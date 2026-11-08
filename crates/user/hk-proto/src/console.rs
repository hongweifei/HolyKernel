//! 控制台服务协议与客户端。
//!
//! 客户端把文本发给 Console 服务（`CONSOLE_WRITE`），并以请求/应答方式读取
//! 一行（`CONSOLE_READ_REQUEST` → `CONSOLE_READ_LINE`）。输入事件由内核通过
//! 通知位 `CONSOLE_NOTIFY_INPUT` 唤醒 Console 服务。

pub use hk_syscall::{CONSOLE_NOTIFY_INPUT, CONSOLE_NOTIFY_MESSAGE};

use hk_server_api::{notify, recv_data_from, send, send_data};

/// Console 服务名。
pub const CONSOLE_SERVICE: &str = "hk-server-console";

/// 写文本（UTF-8）；载荷为文本。
pub const CONSOLE_WRITE: u64 = crate::labels::CONSOLE;
/// 请求读取一行。
pub const CONSOLE_READ_REQUEST: u64 = crate::labels::CONSOLE + 1;
/// 一行输入（不含换行）；载荷为文本。
pub const CONSOLE_READ_LINE: u64 = crate::labels::CONSOLE + 2;
/// 请求 Console 服务退出。
pub const CONSOLE_SHUTDOWN: u64 = crate::labels::CONSOLE + 3;
/// 图形终端完成一次提交（仅终端模式发送）。
pub const CONSOLE_WRITE_ACK: u64 = crate::labels::CONSOLE + 4;
/// 清空终端。
pub const CONSOLE_CLEAR: u64 = crate::labels::CONSOLE + 5;
/// 客户端握手。
pub const CONSOLE_PING: u64 = crate::labels::CONSOLE + 6;
/// 终端已进入接收循环。
pub const CONSOLE_PONG: u64 = crate::labels::CONSOLE + 7;

/// 控制台客户端。
#[derive(Clone, Copy)]
pub struct Client {
    endpoint: u64,
    console: u64,
}

impl Client {
    /// 等待 Console 服务并构造客户端。
    pub fn discover(endpoint: u64) -> Option<Self> {
        let console = hk_server_api::runtime::wait_service(CONSOLE_SERVICE)?;
        Some(Self { endpoint, console })
    }

    /// 返回 Console 服务端点。
    pub fn console_endpoint(&self) -> u64 {
        self.console
    }

    /// 写入文本。
    pub fn write(&self, text: &[u8]) {
        let _ = send_data(self.console, CONSOLE_WRITE, text);
        let _ = notify(self.console, CONSOLE_NOTIFY_MESSAGE);
    }

    /// 请求并读取一行，返回字节数。
    ///
    /// 写入文本时 Console 会回送 `CONSOLE_WRITE_ACK`；握手残留的 PONG 也可能
    /// 先到达。这里只接受真正的 `CONSOLE_READ_LINE`，避免把控制消息当成空行。
    pub fn read_line(&self, buffer: &mut [u8]) -> usize {
        let _ = send(self.console, CONSOLE_READ_REQUEST, 0);
        let _ = notify(self.console, CONSOLE_NOTIFY_MESSAGE);
        loop {
            let (label, _word, sender, length) =
                recv_data_from(self.endpoint, self.console, buffer);
            if label == CONSOLE_READ_LINE && sender == self.console {
                return length.min(buffer.len());
            }
        }
    }

    /// 清空终端。
    pub fn clear(&self) {
        let _ = send(self.console, CONSOLE_CLEAR, 0);
        let _ = notify(self.console, CONSOLE_NOTIFY_MESSAGE);
    }

    /// 请求 Console 服务退出。
    pub fn shutdown(&self) {
        let _ = send(self.console, CONSOLE_SHUTDOWN, 0);
        let _ = notify(self.console, CONSOLE_NOTIFY_MESSAGE);
    }
}
