//! 16550 串口控制台。

use core::arch::asm;

use hk_hal::Console;
use spin::Mutex;

/// COM1 端口基址。
pub const COM1: u16 = 0x3F8;

/// 16550 串口。
pub struct Serial {
    base: u16,
    exclusive: Mutex<()>,
}

impl Serial {
    /// 以端口基址构造串口。
    pub const fn new(base: u16) -> Self {
        Self {
            base,
            exclusive: Mutex::new(()),
        }
    }

    /// 返回端口基址。
    pub const fn base(&self) -> u16 {
        self.base
    }

    /// 初始化串口：115200 波特、8 位数据、无校验、1 位停止位。
    pub fn init(&self) {
        // SAFETY: COM1 的 UART 寄存器在 x86_64 平台上固定存在；初始化序列只访问
        // 该串口的 I/O 端口。
        unsafe {
            outb(self.base + 1, 0x00);
            outb(self.base + 3, 0x80);
            outb(self.base, 0x03);
            outb(self.base + 1, 0x00);
            outb(self.base + 3, 0x03);
            outb(self.base + 2, 0xC7);
            outb(self.base + 4, 0x0B);
        }
    }

    /// 写入一个字节。
    pub fn write_byte(&self, byte: u8) {
        // SAFETY: 线路状态寄存器只读取该串口的 I/O 端口。
        while unsafe { inb(self.base + 5) } & 0x20 == 0 {
            core::hint::spin_loop();
        }
        // SAFETY: 发送保持寄存器只写该串口的 I/O 端口。
        unsafe {
            outb(self.base, byte);
        }
    }
}

impl Console for Serial {
    fn write_str(&self, text: &str) {
        let _guard = self.exclusive.lock();
        for byte in text.bytes() {
            self.write_byte(byte);
        }
    }
}

/// 读取端口。
///
/// # Safety
/// 调用方保证端口号对当前上下文可访问。
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: 由调用方保证端口可访问；`in` 指令只读取 I/O 空间。
    unsafe {
        asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    }
    value
}

/// 写入端口。
///
/// # Safety
/// 调用方保证端口号对当前上下文可访问。
unsafe fn outb(port: u16, value: u8) {
    // SAFETY: 由调用方保证端口可访问；`out` 指令只写 I/O 空间。
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}
