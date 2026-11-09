//! 固件设备的规范化表。
//!
//! 表项只描述 MMIO 范围、IRQ、PCI ECAM 与兼容字符串，不包含驱动策略。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::Device;
use crate::acpi::Madt;
use crate::acpi::tables::Tables;
use crate::dtb::DtbInfo;

/// 一个 MMIO 设备窗口。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MmioDevice {
    /// 设备路径或固件标识。
    pub path: String,
    /// 兼容字符串列表。
    pub compatible: Vec<String>,
    /// `(物理基址, 长度)` 窗口列表。
    pub regions: Vec<(u64, u64)>,
    /// 该设备可路由的 IRQ。
    pub irqs: Vec<u32>,
}

impl MmioDevice {
    /// 判断设备是否声明了给定兼容字符串。
    pub fn is_compatible(&self, name: &str) -> bool {
        self.compatible.iter().any(|entry| entry == name)
    }
}

impl From<&Device> for MmioDevice {
    fn from(device: &Device) -> Self {
        Self {
            path: device.path.clone(),
            compatible: device.compatible.clone(),
            regions: device.regions.clone(),
            irqs: device.irqs.clone(),
        }
    }
}

/// PCI ECAM 窗口。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PciEcamRange {
    /// PCI 段号。
    pub segment: u16,
    /// 起始总线。
    pub bus_start: u8,
    /// 结束总线（含）。
    pub bus_end: u8,
    /// ECAM 物理基址。
    pub base: u64,
    /// 窗口长度。
    pub len: u64,
}

/// 固件声明的 IRQ 路由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IrqRoute {
    /// 源 IRQ。
    pub source: u32,
    /// 目标全局中断号。
    pub target: u32,
}

/// ACPI/DTB 后端的规范化设备表。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FirmwareDeviceTable {
    /// MMIO 设备。
    pub mmio: Vec<MmioDevice>,
    /// PCI ECAM 窗口。
    pub pci_ecam: Vec<PciEcamRange>,
    /// IRQ 路由。
    pub irq_routes: Vec<IrqRoute>,
}

impl FirmwareDeviceTable {
    /// 从 ACPI 的 MADT、MCFG 与 HPET 构造。
    pub fn from_acpi(madt: Option<&Madt>, tables: &Tables) -> Self {
        let mut table = Self::default();
        if let Some(mcfg) = &tables.mcfg {
            table
                .pci_ecam
                .extend(mcfg.entries.iter().map(|entry| PciEcamRange {
                    segment: entry.segment,
                    bus_start: entry.bus_start,
                    bus_end: entry.bus_end,
                    base: entry.base,
                    len: entry.length(),
                }));
        }
        if let Some(hpet) = &tables.hpet
            && hpet.address.space == 0
            && hpet.address.address != 0
        {
            table.mmio.push(MmioDevice {
                path: String::from("acpi:hpet"),
                compatible: alloc::vec![String::from("acpi,hpet")],
                regions: alloc::vec![(hpet.address.address, 0x400)],
                irqs: Vec::new(),
            });
        }
        if let Some(madt) = madt {
            for ioapic in &madt.io_apics {
                table.mmio.push(MmioDevice {
                    path: format!("acpi:ioapic:{}", ioapic.id),
                    compatible: alloc::vec![String::from("acpi,ioapic")],
                    regions: alloc::vec![(ioapic.address, 0x1000)],
                    irqs: Vec::new(),
                });
            }
            for route in &madt.overrides {
                table.irq_routes.push(IrqRoute {
                    source: u32::from(route.source),
                    target: route.gsi,
                });
            }
        }
        table
    }

    /// 从 DTB 设备节点构造。
    pub fn from_dtb(info: &DtbInfo) -> Self {
        let mut table = Self::default();
        for device in &info.devices {
            if device.is_compatible("pci-host-ecam-generic") {
                for (base, len) in &device.regions {
                    let buses = (*len >> 20).min(256) as u32;
                    if buses != 0 {
                        table.pci_ecam.push(PciEcamRange {
                            segment: 0,
                            bus_start: 0,
                            bus_end: (buses - 1) as u8,
                            base: *base,
                            len: *len,
                        });
                    }
                }
            }
            table.mmio.push(MmioDevice::from(device));
        }
        table
    }

    /// 查找声明了给定兼容字符串的 MMIO 设备。
    pub fn find_compatible(&self, name: &str) -> impl Iterator<Item = &MmioDevice> {
        self.mmio
            .iter()
            .filter(move |device| device.is_compatible(name))
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::acpi::tables::{
        GenericAddress, Hpet, Mcfg, McfgEntry, MemoryAffinity, Srat, Tables,
    };
    use crate::acpi::{IoApic, IrqOverride, Madt, Processor};
    use crate::{Cpu, Device, FirmwareBackend, FirmwareInfo, MemoryKind, MemoryRegion};

    fn acpi_info() -> FirmwareInfo {
        let madt = Madt {
            local_apic_address: 0,
            io_apics: vec![IoApic {
                id: 0,
                address: 0xFEC0_0000,
                gsi_base: 0,
            }],
            local_apics: vec![7],
            processors: vec![Processor {
                acpi_id: 0,
                apic_id: 7,
                enabled: true,
            }],
            overrides: vec![IrqOverride { source: 1, gsi: 9 }],
            gic_distributors: Vec::new(),
            gic_cpu_interfaces: Vec::new(),
        };
        let tables = Tables {
            mcfg: Some(Mcfg {
                entries: vec![McfgEntry {
                    base: 0xE000_0000,
                    segment: 0,
                    bus_start: 0,
                    bus_end: 255,
                }],
            }),
            srat: Some(Srat {
                cpus: Vec::new(),
                memory: vec![MemoryAffinity {
                    base: 0x1000,
                    length: 0x2000,
                    proximity: 0,
                    enabled: true,
                }],
            }),
            hpet: Some(Hpet {
                block_id: 0,
                address: GenericAddress {
                    space: 0,
                    address: 0xFED0_0000,
                    ..GenericAddress::default()
                },
                period_fs: 1000,
            }),
            ..Tables::default()
        };
        FirmwareInfo::from_acpi(Some(&madt), &tables)
    }

    fn dtb_info() -> FirmwareInfo {
        FirmwareInfo::from_dtb(&DtbInfo {
            memory: vec![MemoryRegion {
                base: 0x1000,
                len: 0x2000,
                kind: MemoryKind::Usable,
            }],
            cpus: vec![Cpu {
                id: 0,
                hardware_id: 7,
                enabled: true,
            }],
            devices: vec![Device {
                path: String::from("/pcie@10000000"),
                compatible: vec![String::from("pci-host-ecam-generic")],
                regions: vec![(0xE000_0000, 256 << 20)],
                irqs: vec![42],
            }],
            boot_cpu: 0,
        })
    }

    #[test]
    fn normalizes_acpi_and_dtb_common_fields() {
        let acpi = acpi_info();
        let dtb = dtb_info();
        assert_eq!(acpi.backend, FirmwareBackend::Acpi);
        assert_eq!(dtb.backend, FirmwareBackend::Dtb);
        assert_eq!(acpi.cpus, dtb.cpus);
        assert_eq!(acpi.memory, dtb.memory);
        assert_eq!(acpi.devices.pci_ecam, dtb.devices.pci_ecam);
        assert_eq!(acpi.devices.irq_routes[0].source, 1);
        assert_eq!(acpi.devices.irq_routes[0].target, 9);
        assert!(acpi.devices.find_compatible("acpi,ioapic").count() == 1);
        let ecam = dtb
            .devices
            .find_compatible("pci-host-ecam-generic")
            .next()
            .expect("ECAM");
        assert_eq!(ecam.irqs, [42]);
    }

    #[test]
    fn dtb_exposes_firmware_info() {
        let info = DtbInfo {
            memory: Vec::new(),
            cpus: Vec::new(),
            devices: Vec::new(),
            boot_cpu: 0,
        };
        assert_eq!(info.firmware_info().backend, FirmwareBackend::Dtb);
    }
}
