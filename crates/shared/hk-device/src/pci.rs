//! PCI 配置空间访问。

use hk_mmio::Mmio;

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

/// ECAM 配置空间基址。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EcamPci {
    base: u64,
}

impl EcamPci {
    /// 以 ECAM 基址构造配置访问。
    pub const fn new(base: u64) -> Self {
        Self { base }
    }

    /// 返回配置寄存器相对 ECAM 基址的偏移。
    pub const fn offset(address: PciAddress) -> u64 {
        ((address.bus as u64) << 20)
            | (((address.device & 0x1F) as u64) << 15)
            | (((address.function & 0x07) as u64) << 12)
            | ((address.offset & 0xFC) as u64)
    }
}

impl PciConfig for EcamPci {
    fn read32(&self, address: PciAddress) -> u32 {
        // SAFETY: 调用方保证 ECAM 区域已映射为可读设备内存。
        let mmio = unsafe { Mmio::new(self.base as usize) };
        mmio.read_u32(Self::offset(address) as usize)
    }

    fn write32(&self, address: PciAddress, value: u32) {
        // SAFETY: 调用方保证 ECAM 区域已映射为可写设备内存。
        unsafe {
            Mmio::new(self.base as usize).write_u32(Self::offset(address) as usize, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_ecam_offset() {
        let address = PciAddress::new(1, 2, 3, 0x10);
        assert_eq!(
            EcamPci::offset(address),
            (1 << 20) | (2 << 15) | (3 << 12) | 0x10
        );
    }
}
