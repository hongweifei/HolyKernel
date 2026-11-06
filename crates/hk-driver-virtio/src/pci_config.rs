//! PCI 配置空间访问：x86 legacy 机制只支持 DWORD 对齐，这里统一做掩码与读改写。

use hk_hal::DeviceGrant;
use hk_server_api::{pci_read, pci_write};

/// 一个设备的配置空间访问器。
pub(crate) struct Config {
    bus: u8,
    device: u8,
    function: u8,
}

impl Config {
    pub(crate) fn new(grant: &DeviceGrant) -> Self {
        let address = grant.address;
        Self {
            bus: address.bus,
            device: address.device,
            function: address.function,
        }
    }

    /// 读取 32 位；偏移对齐到 4 字节。
    pub(crate) fn read(&self, offset: u8) -> u32 {
        pci_read(self.bus, self.device, self.function, offset & 0xFC)
    }

    /// 写入 32 位；偏移对齐到 4 字节。
    pub(crate) fn write(&self, offset: u8, value: u32) {
        pci_write(self.bus, self.device, self.function, offset & 0xFC, value);
    }

    pub(crate) fn read16(&self, offset: u8) -> u16 {
        let word = self.read(offset);
        (word >> ((offset & 3) * 8)) as u16
    }

    pub(crate) fn write16(&self, offset: u8, value: u16) {
        let shift = u32::from(offset & 3) * 8;
        let current = self.read(offset);
        let mask = 0xFFFF_u32 << shift;
        self.write(offset, (current & !mask) | (u32::from(value) << shift));
    }

    /// 遍历 capability 链，返回指定 ID 的偏移。
    pub(crate) fn find(&self, id: u8) -> Option<u8> {
        let mut offset = (self.read(0x34) & 0xFC) as u8;
        for _ in 0..48 {
            if offset == 0 {
                break;
            }
            if (self.read(offset) & 0xFF) as u8 == id {
                return Some(offset);
            }
            offset = ((self.read(offset) >> 8) & 0xFC) as u8;
        }
        None
    }
}
