//! 设备授权描述：MMIO、I/O、IRQ 与 capability。
//!
//! 这是 Device Manager 与驱动之间的最小权限描述；内核映射与 IRQ 绑定在后续
//! DeviceGrant 对象中执行。

use crate::pci::{Bar, Device, PciConfig, bar_size, capabilities, read_bars};

/// MMIO 范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MmioRange {
    /// 物理基址。
    pub base: u64,
    /// 长度。
    pub len: u64,
}

/// 端口 I/O 范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoRange {
    /// 端口基址。
    pub base: u16,
    /// 长度。
    pub len: u16,
}

/// PCI 设备授权描述。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceGrant {
    /// 配置空间地址。
    pub address: crate::pci::PciAddress,
    /// 厂商编号。
    pub vendor: u16,
    /// 设备编号。
    pub device: u16,
    /// MMIO 范围。
    pub mmio: [Option<MmioRange>; 6],
    /// 端口 I/O 范围。
    pub io: [Option<IoRange>; 6],
    /// 中断线。
    pub irq: Option<u8>,
    /// capability ID。
    pub capabilities: [u8; 8],
    /// capability 数量。
    pub capability_count: usize,
}

impl DeviceGrant {
    /// 从 PCI 设备构造授权描述。
    pub fn from_device(config: &dyn PciConfig, device: &Device) -> Self {
        let mut mmio = [None; 6];
        let mut io = [None; 6];
        let bars = read_bars(config, device);
        for (index, bar) in bars.iter().enumerate() {
            let Some(bar) = bar else {
                continue;
            };
            let size = bar_size(config, device, index).unwrap_or(0);
            if size == 0 {
                continue;
            }
            match *bar {
                Bar::Io { base } => {
                    if base <= u16::MAX as u32 && size <= u16::MAX as u64 {
                        io[index] = Some(IoRange {
                            base: base as u16,
                            len: size as u16,
                        });
                    }
                }
                Bar::Memory { base, .. } => {
                    mmio[index] = Some(MmioRange { base, len: size });
                }
            }
        }

        let mut ids = [0_u8; 8];
        let mut count = 0;
        capabilities(config, device, |capability| {
            if count < ids.len() {
                ids[count] = capability.id;
                count += 1;
            }
        });

        let interrupt = config.read32(device.address.at(0x3C));
        let line = (interrupt & 0xFF) as u8;
        let pin = ((interrupt >> 8) & 0xFF) as u8;
        let irq = (pin != 0 && line != 0xFF).then_some(line);

        Self {
            address: device.address,
            vendor: device.vendor,
            device: device.device,
            mmio,
            io,
            irq,
            capabilities: ids,
            capability_count: count,
        }
    }

    /// 判断是否为 virtio 设备。
    pub fn is_virtio(&self) -> bool {
        self.capabilities[..self.capability_count].contains(&0x09)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pci::PciAddress;
    use spin::Mutex;

    struct MockConfig {
        command: Mutex<u32>,
        bar: Mutex<u32>,
        cap: Mutex<u32>,
    }

    impl PciConfig for MockConfig {
        fn read32(&self, address: PciAddress) -> u32 {
            match address.offset {
                0x04 => 0x0010_0000 | *self.command.lock(),
                0x10 => *self.bar.lock(),
                0x3C => 0x0000_010B,
                0x34 => 0x40,
                0x40 => *self.cap.lock(),
                _ => 0,
            }
        }

        fn write32(&self, address: PciAddress, value: u32) {
            match address.offset {
                0x04 => *self.command.lock() = value & 0xFFFF,
                0x10 => {
                    *self.bar.lock() = if value == u32::MAX {
                        0xFFFF_F000
                    } else {
                        value
                    }
                }
                0x40 => *self.cap.lock() = value,
                _ => {}
            }
        }
    }

    #[test]
    fn derives_grant_from_pci_resources() {
        let config = MockConfig {
            command: Mutex::new(0x3),
            bar: Mutex::new(0xFE00_0000),
            cap: Mutex::new(0x0000_0009),
        };
        let device = Device {
            address: PciAddress::new(0, 0, 0, 0),
            vendor: 0x1AF4,
            device: 0x1001,
            revision: 0,
            interface: 0,
            subclass: 0,
            class: 0,
            header: 0,
        };
        let grant = DeviceGrant::from_device(&config, &device);
        assert_eq!(
            grant.mmio[0],
            Some(MmioRange {
                base: 0xFE00_0000,
                len: 0x1000
            })
        );
        assert_eq!(grant.irq, Some(11));
        assert_eq!(grant.capability_count, 1);
        assert_eq!(grant.capabilities[0], 0x09);
        assert!(grant.is_virtio());
    }
}
