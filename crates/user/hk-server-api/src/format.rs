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
