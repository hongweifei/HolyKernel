//! 通用 PCI 总线枚举。
//!
//! 在 [`PciConfig`] 之上扫描总线、设备与功能，读取厂商、设备、类代码与头类型。
//! 枚举从根总线开始，遇到 PCI-to-PCI 桥时沿 secondary bus 递归，并用访问表防止
//! 总线环。不依赖架构；ECAM 或 legacy 端口访问由平台实现。

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

/// BAR 资源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bar {
    /// 端口 I/O。
    Io {
        /// 基址。
        base: u32,
    },
    /// 内存映射。
    Memory {
        /// 基址。
        base: u64,
        /// 是否可预取。
        prefetchable: bool,
        /// 是否 64 位。
        is_64bit: bool,
    },
}

/// 配置空间 capability。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capability {
    /// capability ID。
    pub id: u8,
    /// 配置空间偏移。
    pub offset: u8,
}

/// 读取设备的 6 个 BAR；64 位 BAR 会占用两个槽位。
pub fn read_bars(config: &dyn PciConfig, device: &Device) -> [Option<Bar>; 6] {
    let count = if device.header & 0x7F == 1 { 2 } else { 6 };
    let mut bars = [None; 6];
    let mut index = 0;
    while index < count {
        let offset = 0x10 + index as u8 * 4;
        let raw = config.read32(device.address.at(offset));
        if raw == 0 || raw == u32::MAX {
            index += 1;
            continue;
        }
        if raw & 1 != 0 {
            bars[index] = Some(Bar::Io { base: raw & !0x3 });
            index += 1;
            continue;
        }
        let kind = (raw >> 1) & 0x3;
        if kind == 1 {
            index += 1;
            continue;
        }
        let is_64bit = kind == 2;
        let prefetchable = raw & 0x8 != 0;
        let mut base = u64::from(raw & !0xF);
        if is_64bit && index + 1 < 6 {
            base |= u64::from(config.read32(device.address.at(offset + 4))) << 32;
            index += 1;
        }
        bars[index - usize::from(is_64bit)] = Some(Bar::Memory {
            base,
            prefetchable,
            is_64bit,
        });
        index += 1;
    }
    bars
}

/// 探测指定 BAR 的大小。
///
/// 通过临时关闭命令寄存器的 I/O 与内存解码、写入全 1 再读回掩码来测量；
/// 完成后恢复 BAR 与命令寄存器。
pub fn bar_size(config: &dyn PciConfig, device: &Device, index: usize) -> Option<u64> {
    if index >= 6 {
        return None;
    }
    let offset = 0x10 + index as u8 * 4;
    let address = device.address.at(offset);
    let original = config.read32(address);
    if original == 0 || original == u32::MAX {
        return None;
    }

    let command_address = device.address.at(0x04);
    let command = config.read32(command_address) & 0xFFFF;
    config.write32(command_address, command & !0x3);

    let is_io = original & 1 != 0;
    let is_64bit = !is_io && (original >> 1) & 0x3 == 2;
    let high_offset = offset + 4;
    let original_high = if is_64bit && high_offset <= 0x24 {
        Some((high_offset, config.read32(device.address.at(high_offset))))
    } else {
        None
    };

    config.write32(address, u32::MAX);
    let mut mask = u64::from(config.read32(address) & !0x3);
    if let Some((high, _)) = original_high {
        let high_address = device.address.at(high);
        config.write32(high_address, u32::MAX);
        mask |= u64::from(config.read32(high_address)) << 32;
        config.write32(high_address, original_high.unwrap().1);
    }

    config.write32(address, original);
    config.write32(command_address, command);

    let mask = if is_io { mask & !0x3 } else { mask & !0xF };
    let size = if is_64bit {
        (!mask).wrapping_add(1)
    } else {
        u64::from((!(mask as u32)).wrapping_add(1))
    };
    if size == 0 || !size.is_power_of_two() {
        None
    } else {
        Some(size)
    }
}

/// 遍历设备的 capability 链。
pub fn capabilities<F>(config: &dyn PciConfig, device: &Device, mut visit: F)
where
    F: FnMut(Capability),
{
    let status = config.read32(device.address.at(0x04)) >> 16;
    if status & 0x10 == 0 {
        return;
    }
    let mut offset = (config.read32(device.address.at(0x34)) & 0xFF) as u8;
    let mut visited = [false; 256];
    let mut count = 0;
    while offset != 0 && !visited[offset as usize] && count < 48 {
        visited[offset as usize] = true;
        let id = read_u8(config, device.address, offset);
        visit(Capability { id, offset });
        offset = read_u8(config, device.address, offset.wrapping_add(1));
        count += 1;
    }
}

fn read_u8(config: &dyn PciConfig, address: PciAddress, offset: u8) -> u8 {
    let aligned = offset & !0x3;
    let shift = u32::from(offset & 0x3) * 8;
    (config.read32(PciAddress {
        offset: aligned,
        ..address
    }) >> shift) as u8
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
    struct ResourceConfig;

    impl PciConfig for ResourceConfig {
        fn read32(&self, address: PciAddress) -> u32 {
            match address.offset {
                0x04 => 0x0010_0000,
                0x10 => 0xFE00_0000,
                0x14 => 0,
                0x18 => 0x0000_C001,
                0x1C => 0x0000_0004,
                0x20 => 1,
                0x24 => 0,
                0x28 => 0,
                0x34 => 0x40,
                0x40 => 0x0000_0009,
                _ => 0,
            }
        }

        fn write32(&self, _address: PciAddress, _value: u32) {}
    }

    fn resource_device() -> Device {
        Device {
            address: PciAddress {
                segment: 0,
                bus: 0,
                device: 0,
                function: 0,
                offset: 0,
            },
            vendor: 1,
            device: 2,
            revision: 0,
            interface: 0,
            subclass: 0,
            class: 0,
            header: 0,
        }
    }

    #[test]
    fn decodes_bars_and_capabilities() {
        let device = resource_device();
        let bars = read_bars(&ResourceConfig, &device);
        assert_eq!(
            bars[0],
            Some(Bar::Memory {
                base: 0xFE00_0000,
                prefetchable: false,
                is_64bit: false
            })
        );
        assert_eq!(bars[2], Some(Bar::Io { base: 0xC000 }));
        assert_eq!(
            bars[3],
            Some(Bar::Memory {
                base: 0x1_0000_0000,
                prefetchable: false,
                is_64bit: true
            })
        );
        assert_eq!(bars[4], None);

        let mut ids = Vec::new();
        capabilities(&ResourceConfig, &device, |cap| ids.push(cap.id));
        assert_eq!(ids, [0x09]);
    }
    struct SizingConfig {
        registers: spin::Mutex<[u32; 64]>,
    }

    impl PciConfig for SizingConfig {
        fn read32(&self, address: PciAddress) -> u32 {
            self.registers.lock()[usize::from(address.offset / 4)]
        }

        fn write32(&self, address: PciAddress, value: u32) {
            let index = usize::from(address.offset / 4);
            self.registers.lock()[index] = if address.offset == 0x10 && value == u32::MAX {
                0xFFFF_F000
            } else {
                value
            };
        }
    }

    #[test]
    fn probes_bar_size_and_restores_config() {
        let config = SizingConfig {
            registers: spin::Mutex::new([0; 64]),
        };
        config.write32(PciAddress::new(0, 0, 0, 0x04), 0x0003);
        config.write32(PciAddress::new(0, 0, 0, 0x10), 0xFE00_0000);
        let device = resource_device();
        assert_eq!(bar_size(&config, &device, 0), Some(0x1000));
        assert_eq!(config.read32(PciAddress::new(0, 0, 0, 0x10)), 0xFE00_0000);
        assert_eq!(config.read32(PciAddress::new(0, 0, 0, 0x04)) & 0xFFFF, 3);
    }
}
