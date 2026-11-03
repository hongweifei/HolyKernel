//! 内核与服务器共用的字符输出入口。

/// 字符控制台。
///
/// 实现方保证整段写入不与其他 CPU 的写入交错。
pub trait Console: Send + Sync {
    /// 写入 UTF-8 文本。
    fn write_str(&self, text: &str);

    /// 写入单个 ASCII 字节。
    fn write_byte(&self, byte: u8) {
        let buffer = [byte];
        if let Ok(text) = core::str::from_utf8(&buffer) {
            self.write_str(text);
        }
    }
}
