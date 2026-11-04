//! 16550 UART 控制台核心：寄存器序列与轮询收发。
//!
//! 平台通过 [`PortIo`] 提供端口或 MMIO 访问；本 crate 不依赖架构。

#![no_std]

use hk_hal::Console;
use spin::Mutex;

/// UART 寄存器访问。
pub trait PortIo: Send + Sync {
    /// 读取偏移处的寄存器。
    ///
    /// # Safety
    /// `offset` 必须在 UART 寄存器范围内（0..8）。
    unsafe fn read(&self, offset: u8) -> u8;

    /// 写入偏移处的寄存器。
    ///
    /// # Safety
    /// `offset` 必须在 UART 寄存器范围内（0..8）。
    unsafe fn write(&self, offset: u8, value: u8);
}

/// 16550 串口。
pub struct Uart16550<P> {
    port: P,
    exclusive: Mutex<()>,
}

impl<P: PortIo> Uart16550<P> {
    /// 以端口访问对象构造串口。
    pub const fn new(port: P) -> Self {
        Self {
            port,
            exclusive: Mutex::new(()),
        }
    }

    /// 初始化串口：115200 波特、8 位数据、无校验、1 位停止位。
    pub fn init(&self) {
        // SAFETY: 偏移均在 16550 寄存器范围内；调用方保证端口有效。
        unsafe {
            self.port.write(1, 0x00);
            self.port.write(3, 0x80);
            self.port.write(0, 0x03);
            self.port.write(1, 0x00);
            self.port.write(3, 0x03);
            self.port.write(2, 0xC7);
            self.port.write(4, 0x0B);
        }
    }

    /// 写入一个字节。
    pub fn write_byte(&self, byte: u8) {
        // SAFETY: 线路状态与发送寄存器偏移固定。
        unsafe {
            while self.port.read(5) & 0x20 == 0 {
                core::hint::spin_loop();
            }
            self.port.write(0, byte);
        }
    }

    /// 尝试从接收缓冲读取一个字节；没有数据时返回 `None`。
    pub fn try_read_byte(&self) -> Option<u8> {
        // SAFETY: 线路状态与接收缓冲寄存器偏移固定。
        unsafe {
            if self.port.read(5) & 0x01 == 0 {
                None
            } else {
                Some(self.port.read(0))
            }
        }
    }
}

impl<P: PortIo> Console for Uart16550<P> {
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

    struct MockPort(Mutex<[u8; 8]>);

    impl PortIo for MockPort {
        unsafe fn read(&self, offset: u8) -> u8 {
            let registers = self.0.lock();
            if offset == 5 {
                0x20
            } else {
                registers[offset as usize]
            }
        }

        unsafe fn write(&self, offset: u8, value: u8) {
            self.0.lock()[offset as usize] = value;
        }
    }

    #[test]
    fn initializes_and_writes_bytes() {
        let uart = Uart16550::new(MockPort(Mutex::new([0; 8])));
        uart.init();
        uart.write_byte(b'A');

        let registers = uart.port.0.lock();
        assert_eq!(registers[0], b'A');
        assert_eq!(registers[2], 0xC7);
        assert_eq!(registers[4], 0x0B);
    }
}
