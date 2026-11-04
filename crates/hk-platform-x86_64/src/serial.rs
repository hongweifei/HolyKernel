//! x86_64 16550 串口：端口 I/O 适配。

use hk_hal::Console;
use hk_uart_16550::{PortIo, Uart16550};

use crate::port::{inb, outb};

/// COM1 端口基址。
pub const COM1: u16 = 0x3F8;

/// x86 端口 I/O。
struct PortSerial {
    base: u16,
}

impl PortSerial {
    const fn new(base: u16) -> Self {
        Self { base }
    }
}

impl PortIo for PortSerial {
    unsafe fn read(&self, offset: u8) -> u8 {
        // SAFETY: 调用方保证偏移处于 UART 寄存器范围。
        unsafe { inb(self.base + u16::from(offset)) }
    }

    unsafe fn write(&self, offset: u8, value: u8) {
        // SAFETY: 调用方保证偏移处于 UART 寄存器范围。
        unsafe {
            outb(self.base + u16::from(offset), value);
        }
    }
}

/// 16550 串口。
pub struct Serial {
    uart: Uart16550<PortSerial>,
}

impl Serial {
    /// 以端口基址构造串口。
    pub const fn new(base: u16) -> Self {
        Self {
            uart: Uart16550::new(PortSerial::new(base)),
        }
    }

    /// 初始化串口：115200 波特、8 位数据、无校验、1 位停止位。
    pub fn init(&self) {
        self.uart.init();
    }

    /// 写入一个字节。
    pub fn write_byte(&self, byte: u8) {
        self.uart.write_byte(byte);
    }

    /// 尝试从接收缓冲读取一个字节；没有数据时返回 `None`。
    pub fn try_read_byte(&self) -> Option<u8> {
        self.uart.try_read_byte()
    }
}

impl Console for Serial {
    fn write_str(&self, text: &str) {
        self.uart.write_str(text);
    }
}
