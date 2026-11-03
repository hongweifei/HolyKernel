//! x86 端口 I/O。

use core::arch::asm;

/// 从端口读取一个字节。
///
/// # Safety
/// 调用方保证端口号对当前上下文可访问。
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: 由调用方保证端口可访问；`in` 指令只读取 I/O 空间。
    unsafe {
        asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    }
    value
}

/// 向端口写入一个字节。
///
/// # Safety
/// 调用方保证端口号对当前上下文可访问。
pub unsafe fn outb(port: u16, value: u8) {
    // SAFETY: 由调用方保证端口可访问；`out` 指令只写 I/O 空间。
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}
