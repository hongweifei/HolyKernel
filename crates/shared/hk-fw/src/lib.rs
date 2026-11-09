//! 固件描述：ACPI 与 DTB 解析。
//!
//! 本 crate 是外部固件库（`acpi`、`hermit-dtb`）的唯一包装层：外部类型只在此
//! crate 内部出现，对上只暴露 HolyKernel 自己的描述类型。平台 crate 负责提供
//! 物理内存读取，本 crate 负责解释固件表。

#![no_std]

extern crate alloc;

use alloc::vec::Vec;

/// 解析 ACPI 表。
pub mod acpi;
/// 规范化固件设备表。
pub mod device;
/// 解析设备树（DTB）。
pub mod dtb;
pub mod iommu;

/// 固件描述的内存区域用途。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryKind {
    /// 可分配内存。
    Usable,
    /// 保留内存。
    Reserved,
    /// ACPI 可回收内存。
    AcpiReclaimable,
    /// 设备地址窗口。
    Device,
}

/// 固件描述的物理内存区域。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryRegion {
    /// 物理基址。
    pub base: u64,
    /// 区域长度（字节）。
    pub len: u64,
    /// 区域用途。
    pub kind: MemoryKind,
}

/// 固件描述的 CPU。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cpu {
    /// 平台无关的 CPU 序号。
    pub id: u64,
    /// 硬件标识：x86 APIC ID 或 ARM MPIDR。
    pub hardware_id: u64,
    /// 是否已启用。
    pub enabled: bool,
}

/// 固件描述的通用设备：由节点路径、兼容字符串与寄存器窗口组成。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// 设备树节点路径；ACPI 设备使用其硬件标识。
    pub path: alloc::string::String,
    /// 兼容字符串列表。
    pub compatible: alloc::vec::Vec<alloc::string::String>,
    /// `(物理基址, 长度)` 窗口列表。
    pub regions: alloc::vec::Vec<(u64, u64)>,
    /// 设备声明的 IRQ。
    pub irqs: alloc::vec::Vec<u32>,
}

pub use device::{FirmwareDeviceTable, IrqRoute, MmioDevice, PciEcamRange};
pub use iommu::{IommuInfo, IommuKind, IommuReserved, IommuUnit};

/// 固件描述来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareBackend {
    /// ACPI。
    Acpi,
    /// 设备树。
    Dtb,
}

/// ACPI/DTB 共享的规范化固件描述。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirmwareInfo {
    /// 解析来源。
    pub backend: FirmwareBackend,
    /// 内存区域。
    pub memory: Vec<MemoryRegion>,
    /// CPU 列表。
    pub cpus: Vec<Cpu>,
    /// 规范化设备表。
    pub devices: FirmwareDeviceTable,
    /// IOMMU 描述。
    pub iommu: IommuInfo,
}

impl FirmwareInfo {
    /// 从 ACPI 的 MADT 与固定表构造。
    pub fn from_acpi(madt: Option<&acpi::Madt>, tables: &acpi::tables::Tables) -> Self {
        let memory = tables
            .srat
            .as_ref()
            .map(|srat| {
                srat.memory
                    .iter()
                    .filter(|entry| entry.enabled)
                    .map(|entry| MemoryRegion {
                        base: entry.base,
                        len: entry.length,
                        kind: MemoryKind::Usable,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let cpus = madt
            .map(|madt| {
                madt.processors
                    .iter()
                    .enumerate()
                    .map(|(id, processor)| Cpu {
                        id: id as u64,
                        hardware_id: u64::from(processor.apic_id),
                        enabled: processor.enabled,
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            backend: FirmwareBackend::Acpi,
            memory,
            cpus,
            devices: FirmwareDeviceTable::from_acpi(madt, tables),
            iommu: tables.iommu.clone(),
        }
    }

    /// 从设备树构造。
    pub fn from_dtb(info: &dtb::DtbInfo) -> Self {
        Self {
            backend: FirmwareBackend::Dtb,
            memory: info.memory.clone(),
            cpus: info.cpus.clone(),
            devices: FirmwareDeviceTable::from_dtb(info),
            iommu: info.iommu_info(),
        }
    }
}

impl Device {
    /// 判断设备是否声明了给定兼容字符串。
    pub fn is_compatible(&self, name: &str) -> bool {
        self.compatible.iter().any(|entry| entry == name)
    }
}
