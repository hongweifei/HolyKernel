//! 字节与双字设备访问：x86 端口 I/O 或 MMIO。

/// 设备寄存器访问。
///
/// 地址可以是 x86 I/O 端口或 MMIO 虚拟地址；由平台实现解释。
pub trait Io: Send + Sync {
    /// 读取 8 位寄存器。
    ///
    /// # Safety
    /// 地址必须映射到有效设备寄存器。
    unsafe fn read8(&self, address: u64) -> u8;

    /// 写入 8 位寄存器。
    ///
    /// # Safety
    /// 地址必须映射到有效设备寄存器。
    unsafe fn write8(&self, address: u64, value: u8);

    /// 读取 32 位寄存器。
    ///
    /// # Safety
    /// 地址必须映射到有效设备寄存器。
    unsafe fn read32(&self, address: u64) -> u32;

    /// 写入 32 位寄存器。
    ///
    /// # Safety
    /// 地址必须映射到有效设备寄存器。
    unsafe fn write32(&self, address: u64, value: u32);
}
