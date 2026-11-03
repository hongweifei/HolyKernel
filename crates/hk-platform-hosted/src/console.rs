//! 标准输出控制台。

use std::io::Write;
use std::sync::Mutex;

use hk_hal::Console;

/// 基于标准输出的控制台。
#[derive(Debug, Default)]
pub struct HostedConsole {
    exclusive: Mutex<()>,
}

impl HostedConsole {
    /// 构造控制台。
    pub fn new() -> Self {
        Self::default()
    }
}

impl Console for HostedConsole {
    fn write_str(&self, text: &str) {
        let _guard = self.exclusive.lock().expect("控制台锁中毒");
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(text.as_bytes()).expect("写入标准输出失败");
        stdout.flush().expect("刷新标准输出失败");
    }
}
