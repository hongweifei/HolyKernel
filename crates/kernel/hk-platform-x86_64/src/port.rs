//! x86 端口 I/O。

use hk_hal::Io;

/// x86 端口 I/O 实现。
pub struct PortIo;

/// 全局端口 I/O。
pub const IO: PortIo = PortIo;

impl Io for PortIo {
    unsafe fn read8(&self, address: u64) -> u8 {
        // SAFETY: 调用方保证端口可访问。
        unsafe { inb(address as u16) }
    }

    unsafe fn write8(&self, address: u64, value: u8) {
        // SAFETY: 调用方保证端口可访问。
        unsafe { outb(address as u16, value) }
    }

    unsafe fn read32(&self, address: u64) -> u32 {
        // SAFETY: 调用方保证端口可访问。
        unsafe { inl(address as u16) }
    }

    unsafe fn write32(&self, address: u64, value: u32) {
        // SAFETY: 调用方保证端口可访问。
        unsafe { outl(address as u16, value) }
    }
}

/// 从端口读取一个字节。
///
/// # Safety
/// 调用方保证端口号对当前上下文可访问。
pub unsafe fn inb(port: u16) -> u8 {
    // SAFETY: 由调用方保证端口可访问。
    unsafe { hk_arch::inb(port) }
}

/// 向端口写入一个字节。
///
/// # Safety
/// 调用方保证端口号对当前上下文可访问。
pub unsafe fn outb(port: u16, value: u8) {
    // SAFETY: 由调用方保证端口可访问。
    unsafe { hk_arch::outb(port, value) }
}

/// 从端口读取一个 32 位值。
///
/// # Safety
/// 调用方保证端口号对当前上下文可访问。
pub unsafe fn inl(port: u16) -> u32 {
    // SAFETY: 由调用方保证端口可访问。
    unsafe { hk_arch::inl(port) }
}

/// 向端口写入一个 32 位值。
///
/// # Safety
/// 调用方保证端口号对当前上下文可访问。
pub unsafe fn outl(port: u16, value: u32) {
    // SAFETY: 由调用方保证端口可访问。
    unsafe { hk_arch::outl(port, value) }
}
