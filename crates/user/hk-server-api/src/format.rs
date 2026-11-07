//! 服务端共享输出格式化。

use crate::{write, write_u64};

/// 以固定宽度、小写十六进制输出 `value` 的低 `digits` 位。
pub fn write_hex(mut value: u64, digits: usize) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digits = digits.clamp(1, 16);
    let mut buffer = [b'0'; 16];
    let mut index = 16;
    for _ in 0..digits {
        index -= 1;
        buffer[index] = HEX[usize::from((value & 0xF) as u8)];
        value >>= 4;
    }
    write(&buffer[index..]);
}

/// 以点分十进制输出 IPv4 地址。
pub fn write_ipv4(address: &[u8; 4]) {
    for (index, octet) in address.iter().enumerate() {
        if index != 0 {
            write(b".");
        }
        write_u64(u64::from(*octet));
    }
}

/// 把无符号整数写成十进制到 `out`，返回写出的字节；空间不足时返回 `None`。
pub fn format_u64(mut value: u64, out: &mut [u8]) -> Option<&[u8]> {
    let mut buffer = [0_u8; 20];
    let mut index = buffer.len();
    if value == 0 {
        index -= 1;
        buffer[index] = b'0';
    }
    while value > 0 {
        index -= 1;
        buffer[index] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    let length = buffer.len() - index;
    if length > out.len() {
        return None;
    }
    out[..length].copy_from_slice(&buffer[index..]);
    Some(&out[..length])
}

/// 单行日志缓冲：所有片段在一次 `write` 里输出，避免与其他任务的输出交错。
pub struct Line {
    buffer: [u8; 160],
    length: usize,
}

impl Line {
    /// 构造空行。
    pub const fn new() -> Self {
        Self {
            buffer: [0_u8; 160],
            length: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) -> &mut Self {
        let end = (self.length + bytes.len()).min(self.buffer.len());
        let length = end - self.length;
        self.buffer[self.length..end].copy_from_slice(&bytes[..length]);
        self.length = end;
        self
    }

    /// 追加一段字节。
    pub fn text(&mut self, text: &[u8]) -> &mut Self {
        self.push(text)
    }

    /// 追加一个无符号整数的十进制形式。
    pub fn number(&mut self, value: u64) -> &mut Self {
        let mut scratch = [0_u8; 20];
        let mut bytes = [0_u8; 20];
        if let Some(digits) = format_u64(value, &mut scratch) {
            bytes[..digits.len()].copy_from_slice(digits);
            self.push(&bytes[..digits.len()]);
        }
        self
    }

    /// 输出整行。
    pub fn finish(&self) {
        write(&self.buffer[..self.length]);
    }
}

impl Default for Line {
    fn default() -> Self {
        Self::new()
    }
}
