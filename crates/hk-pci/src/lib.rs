//! 通用 PCI 总线枚举。
//!
//! 在 [`PciConfig`] 之上扫描总线的设备与功能，读取厂商、设备、类代码与头类型。
//! 不依赖架构；ECAM 或 legacy 端口访问由平台实现。

#![no_std]

#[cfg(test)]
extern crate alloc;

pub use hk_hal::{PciAddress, PciConfig};

const VENDOR_DEVICE: u8 = 0x00;
const CLASS_REVISION: u8 = 0x08;
const HEADER_TYPE: u8 = 0x0C;
const MULTIFUNCTION: u8 = 0x80;
const NO_DEVICE: u16 = 0xFFFF;

/// 枚举到的 PCI 设备。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Device {
    /// 配置空间地址（偏移为 0）。
    pub address: PciAddress,
    /// 厂商编号。
    pub vendor: u16,
    /// 设备编号。
    pub device: u16,
    /// 修订号。
    pub revision: u8,
    /// 编程接口。
    pub interface: u8,
    /// 子类。
    pub subclass: u8,
    /// 基类。
    pub class: u8,
    /// 头类型。
    pub header: u8,
}

/// 扫描全部 256 条总线。
pub fn enumerate<F>(config: &dyn PciConfig, segment: u16, mut visit: F)
where
    F: FnMut(Device),
{
    for bus in 0..=u8::MAX {
        scan_bus(config, segment, bus, &mut visit);
    }
}

/// 扫描一条总线，处理多功能设备。
pub fn scan_bus<F>(config: &dyn PciConfig, segment: u16, bus: u8, mut visit: F)
where
    F: FnMut(Device),
{
    for device in 0..32 {
        let base = PciAddress {
            segment,
            bus,
            device,
            function: 0,
            offset: VENDOR_DEVICE,
        };
        let id = config.read32(base);
        if vendor(id) == NO_DEVICE {
            continue;
        }

        let (first, header) = read_device(config, base, id);
        visit(first);

        if header & MULTIFUNCTION == 0 {
            continue;
        }
        for function in 1..8 {
            let address = PciAddress { function, ..base };
            let id = config.read32(address);
            if vendor(id) != NO_DEVICE {
                let (device, _) = read_device(config, address, id);
                visit(device);
            }
        }
    }
}

fn read_device(config: &dyn PciConfig, address: PciAddress, id: u32) -> (Device, u8) {
    let class = config.read32(address.at(CLASS_REVISION));
    let header = config.read32(address.at(HEADER_TYPE));
    (
        Device {
            address,
            vendor: vendor(id),
            device: (id >> 16) as u16,
            revision: class as u8,
            interface: (class >> 8) as u8,
            subclass: (class >> 16) as u8,
            class: (class >> 24) as u8,
            header: (header >> 16) as u8,
        },
        (header >> 16) as u8,
    )
}

fn vendor(id: u32) -> u16 {
    id as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    struct MockConfig;

    impl PciConfig for MockConfig {
        fn read32(&self, address: PciAddress) -> u32 {
            if address.segment != 0 || address.bus != 0 || address.device != 1 {
                return u32::MAX;
            }
            match (address.function, address.offset) {
                (0, 0x00) => 0x1234_5678,
                (0, 0x08) => 0x0200_0000,
                (0, 0x0C) => 0x0080_0000,
                (1, 0x00) => 0xAAAA_BBBB,
                (1, 0x08) => 0x0100_0000,
                (1, 0x0C) => 0x0000_0000,
                _ => u32::MAX,
            }
        }

        fn write32(&self, _address: PciAddress, _value: u32) {}
    }

    #[test]
    fn enumerates_multifunction_devices() {
        let mut devices = Vec::new();
        enumerate(&MockConfig, 0, |device| devices.push(device));

        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].vendor, 0x5678);
        assert_eq!(devices[0].device, 0x1234);
        assert_eq!(devices[0].class, 0x02);
        assert_eq!(devices[0].header, 0x80);
        assert_eq!(devices[1].address.function, 1);
        assert_eq!(devices[1].vendor, 0xBBBB);
    }
}
