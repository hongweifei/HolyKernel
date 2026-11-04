//! x86_64 PS/2 键盘：i8042 端口 I/O 适配。

use hk_keyboard_ps2::{PortIo, Ports, Ps2Keyboard};

use crate::port::{inb, outb};

struct PortIoX86;

impl PortIo for PortIoX86 {
    unsafe fn read(&self, port: u16) -> u8 {
        // SAFETY: 调用方保证端口属于键盘控制器。
        unsafe { inb(port) }
    }

    unsafe fn write(&self, port: u16, value: u8) {
        // SAFETY: 调用方保证端口属于键盘控制器。
        unsafe {
            outb(port, value);
        }
    }
}

static KEYBOARD: Ps2Keyboard<PortIoX86> = Ps2Keyboard::new(PortIoX86, Ports::I8042);

/// 初始化 i8042：启用键盘扫描与 IRQ1，打开扫描码翻译。
pub fn init() {
    KEYBOARD.init();
}

/// 从输入缓冲取出一个字节。
pub fn read() -> Option<u8> {
    KEYBOARD.read()
}

/// 处理键盘中断：读取扫描码并解码。
pub fn handle_interrupt() {
    KEYBOARD.handle_interrupt();
}
