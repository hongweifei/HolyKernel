//! 固件描述：ACPI 与 DTB 解析。
//!
//! 本 crate 是外部固件库（`acpi`、`hermit-dtb`）的唯一包装层：外部类型只在此
//! crate 内部出现，对上只暴露 HolyKernel 自己的描述类型。平台 crate 负责提供
//! 物理内存读取，本 crate 负责解释固件表。

#![no_std]

extern crate alloc;

/// 解析 ACPI 表。
pub mod acpi;
/// 解析设备树（DTB）。
pub mod dtb;
/// ACPI 固定表与拓扑表。
pub mod firmware;
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
}

pub use iommu::{IommuInfo, IommuKind, IommuReserved, IommuUnit};

impl Device {
    /// 判断设备是否声明了给定兼容字符串。
    pub fn is_compatible(&self, name: &str) -> bool {
        self.compatible.iter().any(|entry| entry == name)
    }
}
