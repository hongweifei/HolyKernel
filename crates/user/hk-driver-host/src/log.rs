//! 把 `log` 输出接到内核控制台。

use core::fmt::{self, Write};

use hk_server_api::write;

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        let mut writer = ConsoleWriter;
        let _ = writeln!(&mut writer, "{}", record.args());
    }

    fn flush(&self) {}
}

struct ConsoleWriter;

impl Write for ConsoleWriter {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        write(text.as_bytes());
        Ok(())
    }
}

static LOGGER: Logger = Logger;

/// 安装全局日志器；重复调用无副作用。
pub fn init() {
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
}
