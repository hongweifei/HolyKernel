//! 16550 UART 控制台核心：寄存器序列与轮询收发。
//!
//! 平台通过 [`Io`] 提供端口或 MMIO 访问；本 crate 不依赖架构。

use hk_hal::{Console, Io};
use spin::Mutex;

/// 16550 串口。
pub struct Uart16550<I> {
    io: I,
    base: u64,
    exclusive: Mutex<()>,
}

impl<I: Io> Uart16550<I> {
    /// 以寄存器访问对象与基址构造串口。
    pub const fn new(io: I, base: u64) -> Self {
        Self {
            io,
            base,
            exclusive: Mutex::new(()),
        }
    }

    /// 初始化串口：115200 波特、8 位数据、无校验、1 位停止位。
    pub fn init(&self) {
        // SAFETY: 偏移均在 16550 寄存器范围内；调用方保证地址有效。
        unsafe {
            self.io.write8(self.base + 1, 0x00);
            self.io.write8(self.base + 3, 0x80);
            self.io.write8(self.base, 0x03);
            self.io.write8(self.base + 1, 0x00);
            self.io.write8(self.base + 3, 0x03);
            self.io.write8(self.base + 2, 0xC7);
            self.io.write8(self.base + 4, 0x0B);
        }
    }

    /// 写入一个字节。
    pub fn write_byte(&self, byte: u8) {
        // SAFETY: 线路状态与发送寄存器偏移固定。
        unsafe {
            while self.io.read8(self.base + 5) & 0x20 == 0 {
                core::hint::spin_loop();
            }
            self.io.write8(self.base, byte);
        }
    }

    /// 尝试从接收缓冲读取一个字节；没有数据时返回 `None`。
    pub fn try_read_byte(&self) -> Option<u8> {
        // SAFETY: 线路状态与接收缓冲寄存器偏移固定。
        unsafe {
            if self.io.read8(self.base + 5) & 0x01 == 0 {
                None
            } else {
                Some(self.io.read8(self.base))
            }
        }
    }
}

impl<I: Io> Console for Uart16550<I> {
    fn write_str(&self, text: &str) {
        let _guard = self.exclusive.lock();
        for byte in text.bytes() {
            self.write_byte(byte);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockIo(Mutex<[u8; 8]>);

    impl Io for MockIo {
        unsafe fn read8(&self, address: u64) -> u8 {
            let registers = self.0.lock();
            if address == 5 {
                0x20
            } else {
                registers[address as usize]
            }
        }

        unsafe fn write8(&self, address: u64, value: u8) {
            self.0.lock()[address as usize] = value;
        }

        unsafe fn read32(&self, _address: u64) -> u32 {
            0
        }

        unsafe fn write32(&self, _address: u64, _value: u32) {}
    }

    #[test]
    fn initializes_and_writes_bytes() {
        let uart = Uart16550::new(MockIo(Mutex::new([0; 8])), 0);
        uart.init();
        uart.write_byte(b'A');

        let registers = uart.io.0.lock();
        assert_eq!(registers[0], b'A');
        assert_eq!(registers[2], 0xC7);
        assert_eq!(registers[4], 0x0B);
    }
}
