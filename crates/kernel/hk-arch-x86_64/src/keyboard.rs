//! x86_64 PS/2 键盘：i8042 端口 I/O 适配。

use core::sync::atomic::{AtomicBool, Ordering};

use hk_drivers::keyboard::{Ports, Ps2Keyboard};

use crate::port::{IO, PortIo};

static KEYBOARD: Ps2Keyboard<PortIo> = Ps2Keyboard::new(IO, Ports::I8042);
static INPUT_PENDING: AtomicBool = AtomicBool::new(false);

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
    INPUT_PENDING.store(true, Ordering::Release);
}

/// 取出并清除“有输入待处理”标志。
pub fn take_input() -> bool {
    INPUT_PENDING.swap(false, Ordering::AcqRel)
}
