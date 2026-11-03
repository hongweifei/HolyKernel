//! PCI 配置空间访问。

/// PCI 配置空间地址。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PciAddress {
    /// 段号。
    pub segment: u16,
    /// 总线号。
    pub bus: u8,
    /// 设备号。
    pub device: u8,
    /// 功能号。
    pub function: u8,
    /// 寄存器偏移，按 4 字节对齐。
    pub offset: u8,
}

impl PciAddress {
    /// 在 0 号段上构造地址。
    pub const fn new(bus: u8, device: u8, function: u8, offset: u8) -> Self {
        Self {
            segment: 0,
            bus,
            device,
            function,
            offset,
        }
    }

    /// 更换寄存器偏移。
    pub const fn at(self, offset: u8) -> Self {
        Self { offset, ..self }
    }
}

/// PCI 配置空间访问。
///
/// 实现方提供 ECAM 或 legacy 端口访问；调用方保证偏移按 4 字节对齐。
pub trait PciConfig: Send + Sync {
    /// 读取 32 位寄存器。
    fn read32(&self, address: PciAddress) -> u32;

    /// 写入 32 位寄存器。
    fn write32(&self, address: PciAddress, value: u32);
}
