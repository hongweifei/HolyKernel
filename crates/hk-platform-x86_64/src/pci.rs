//! PCI 配置空间访问（x86 legacy 机制 #1）。
//!
//! 通过 `0xCF8` 写入配置地址、`0xCFC` 读写数据，只支持 32 位对齐访问。
//! 其他架构可使用 `hk_hal::EcamPci`。

use hk_hal::{PciAddress, PciConfig};

use crate::port::{inl, outl};

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;
const ENABLE: u32 = 1 << 31;

/// x86 legacy PCI 配置访问。
pub struct LegacyPci;

/// 全局 legacy 配置访问。
pub const PCI: LegacyPci = LegacyPci;

impl LegacyPci {
    fn address(&self, address: PciAddress) -> u32 {
        ENABLE
            | (u32::from(address.bus) << 16)
            | (u32::from(address.device & 0x1F) << 11)
            | (u32::from(address.function & 0x07) << 8)
            | u32::from(address.offset & 0xFC)
    }
}

impl PciConfig for LegacyPci {
    fn read32(&self, address: PciAddress) -> u32 {
        if address.segment != 0 {
            return u32::MAX;
        }
        // SAFETY: PCI 配置端口在 x86_64 平台上固定存在；写入地址后读取数据端口。
        unsafe {
            outl(CONFIG_ADDRESS, self.address(address));
            inl(CONFIG_DATA)
        }
    }

    fn write32(&self, address: PciAddress, value: u32) {
        if address.segment != 0 {
            return;
        }
        // SAFETY: 同上；只写配置数据端口。
        unsafe {
            outl(CONFIG_ADDRESS, self.address(address));
            outl(CONFIG_DATA, value);
        }
    }
}

/// 读取配置空间中的一个 32 位字。
pub fn config_read(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    PCI.read32(PciAddress::new(bus, device, function, offset))
}

/// 写入配置空间中的一个 32 位字。
pub fn config_write(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    PCI.write32(PciAddress::new(bus, device, function, offset), value);
}
