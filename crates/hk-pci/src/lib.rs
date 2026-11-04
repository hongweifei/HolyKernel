//! 通用 PCI 总线枚举。
//!
//! 在 [`PciConfig`] 之上扫描总线、设备与功能，读取厂商、设备、类代码与头类型。
//! 枚举从根总线开始，遇到 PCI-to-PCI 桥时沿 secondary bus 递归，并用访问表防止
//! 总线环。不依赖架构；ECAM 或 legacy 端口访问由平台实现。

#![no_std]

#[cfg(test)]
extern crate alloc;

pub use hk_hal::{PciAddress, PciConfig};

const VENDOR_DEVICE: u8 = 0x00;
const CLASS_REVISION: u8 = 0x08;
const HEADER_TYPE: u8 = 0x0C;
const BUS_NUMBERS: u8 = 0x18;
const MULTIFUNCTION: u8 = 0x80;
const NO_DEVICE: u16 = 0xFFFF;
const BUS_COUNT: usize = 256;

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

impl Device {
    /// 判断是否为 PCI-to-PCI 桥。
    pub fn is_bridge(&self) -> bool {
        self.class == 0x06 && self.subclass == 0x04
    }
}

/// 从根总线 0 开始，按桥递归枚举全部可达设备。
pub fn enumerate<F>(config: &dyn PciConfig, segment: u16, visit: F)
where
    F: FnMut(Device),
{
    enumerate_from(config, segment, 0, visit);
}

/// 从指定根总线开始，按桥递归枚举全部可达设备。
pub fn enumerate_from<F>(config: &dyn PciConfig, segment: u16, root: u8, mut visit: F)
where
    F: FnMut(Device),
{
    let mut visited = [false; BUS_COUNT];
    scan_tree(config, segment, root, &mut visited, &mut visit);
}

/// 扫描一条总线上的设备，不跟随桥。
pub fn scan_bus<F>(config: &dyn PciConfig, segment: u16, bus: u8, visit: F)
where
    F: FnMut(Device),
{
    scan_indices(config, segment, bus, visit);
}

fn scan_tree<F>(
    config: &dyn PciConfig,
    segment: u16,
    bus: u8,
    visited: &mut [bool; BUS_COUNT],
    visit: &mut F,
) where
    F: FnMut(Device),
{
    let index = usize::from(bus);
    if visited[index] {
        return;
    }
    visited[index] = true;

    scan_indices(config, segment, bus, |device| {
        visit(device);
        if !device.is_bridge() {
            return;
        }
        let numbers = config.read32(device.address.at(BUS_NUMBERS));
        let secondary = ((numbers >> 8) & 0xFF) as u8;
        if secondary != 0 {
            scan_tree(config, segment, secondary, visited, visit);
        }
    });
}

fn scan_indices<F>(config: &dyn PciConfig, segment: u16, bus: u8, mut visit: F)
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

    struct BridgeConfig;

    impl PciConfig for BridgeConfig {
        fn read32(&self, address: PciAddress) -> u32 {
            match (
                address.segment,
                address.bus,
                address.device,
                address.function,
                address.offset,
            ) {
                (0, 0, 1, 0, 0x00) => 0x1000_0001,
                (0, 0, 1, 0, 0x08) => 0x0200_0000,
                (0, 0, 1, 0, 0x0C) => 0x0000_0000,
                (0, 0, 2, 0, 0x00) => 0x2000_0002,
                (0, 0, 2, 0, 0x08) => 0x0604_0000,
                (0, 0, 2, 0, 0x0C) => 0x0001_0000,
                (0, 0, 2, 0, 0x18) => 0x0002_0100,
                (0, 1, 0, 0, 0x00) => 0x3000_0003,
                (0, 1, 0, 0, 0x08) => 0x0604_0000,
                (0, 1, 0, 0, 0x0C) => 0x0001_0000,
                (0, 1, 0, 0, 0x18) => 0x0002_0201,
                (0, 2, 0, 0, 0x00) => 0x4000_0004,
                (0, 2, 0, 0, 0x08) => 0x0100_0000,
                (0, 2, 0, 0, 0x0C) => 0x0000_0000,
                (0, 5, 0, 0, 0x00) => 0x5000_0005,
                (0, 5, 0, 0, 0x08) => 0x0100_0000,
                (0, 5, 0, 0, 0x0C) => 0x0000_0000,
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

    #[test]
    fn follows_bridges_and_skips_unreachable_buses() {
        let mut devices = Vec::new();
        enumerate(&BridgeConfig, 0, |device| devices.push(device.address));

        assert_eq!(devices.len(), 4);
        assert!(devices.contains(&PciAddress {
            segment: 0,
            bus: 0,
            device: 1,
            function: 0,
            offset: 0,
        }));
        assert!(devices.contains(&PciAddress {
            segment: 0,
            bus: 0,
            device: 2,
            function: 0,
            offset: 0,
        }));
        assert!(devices.contains(&PciAddress {
            segment: 0,
            bus: 1,
            device: 0,
            function: 0,
            offset: 0,
        }));
        assert!(devices.contains(&PciAddress {
            segment: 0,
            bus: 2,
            device: 0,
            function: 0,
            offset: 0,
        }));
        assert!(!devices.iter().any(|address| address.bus == 5));
    }
}
