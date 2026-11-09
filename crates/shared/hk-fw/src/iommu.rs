//! 平台中性的 IOMMU 固件描述。
//!
//! x86 的 DMAR/IVRS 与 ARM/RISC-V 设备树节点统一映射为 [`IommuUnit`]；
//! 具体硬件后端仍由各 `hk-platform-<arch>` 实现 `hk_hal::Iommu`。

use alloc::vec::Vec;

use crate::acpi::tables::{Dmar, Ivrs};
use crate::dtb::DtbInfo;

/// IOMMU 实现类别。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IommuKind {
    /// 未识别。
    #[default]
    Unknown,
    /// Intel VT-d。
    IntelVtd,
    /// AMD-Vi。
    AmdVi,
    /// ARM SMMUv2 / MMU-500。
    ArmSmmuV2,
    /// ARM SMMUv3。
    ArmSmmuV3,
    /// RISC-V IOMMU。
    RiscvIommu,
    /// 其他由固件声明的 IOMMU 节点。
    Other,
}

/// 一个 IOMMU 硬件单元。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IommuUnit {
    /// 实现类别。
    pub kind: IommuKind,
    /// PCI 段号；非 PCI 平台为 0。
    pub segment: u16,
    /// 寄存器页物理基址。
    pub base: u64,
    /// 寄存器页长度；固件未声明时为 0。
    pub length: u64,
    /// 硬件标识：Intel 无、AMD 为 DeviceID、DTB 为节点编号。
    pub id: u32,
}

/// IOMMU 保留/隔离内存区间。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IommuReserved {
    /// PCI 段号。
    pub segment: u16,
    /// 区间物理基址。
    pub base: u64,
    /// 区间长度。
    pub length: u64,
}

/// 平台中性的 IOMMU 描述。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IommuInfo {
    /// 检测到的 IOMMU 单元。
    pub units: Vec<IommuUnit>,
    /// 供 IOMMU 使用的保留内存区间。
    pub reserved: Vec<IommuReserved>,
}

impl IommuInfo {
    /// 从 ACPI DMAR/IVRS 汇总。
    pub fn from_acpi(dmar: Option<&Dmar>, ivrs: Option<&Ivrs>) -> Self {
        let mut info = Self::default();
        if let Some(dmar) = dmar {
            for unit in &dmar.units {
                info.units.push(IommuUnit {
                    kind: IommuKind::IntelVtd,
                    segment: unit.segment,
                    base: unit.address,
                    length: 0,
                    id: 0,
                });
            }
            for region in &dmar.reserved {
                info.reserved.push(IommuReserved {
                    segment: region.segment,
                    base: region.base,
                    length: region.limit.saturating_sub(region.base) + 1,
                });
            }
        }
        if let Some(ivrs) = ivrs {
            for unit in &ivrs.units {
                info.units.push(IommuUnit {
                    kind: IommuKind::AmdVi,
                    segment: unit.segment,
                    base: unit.address,
                    length: 0,
                    id: u32::from(unit.device_id),
                });
            }
            for region in &ivrs.memory {
                info.reserved.push(IommuReserved {
                    segment: region.segment,
                    base: region.base,
                    length: region.length,
                });
            }
        }
        info
    }

    /// 从设备树设备节点汇总。
    pub fn from_dtb(dtb: &DtbInfo) -> Self {
        let mut info = Self::default();
        for (index, device) in dtb.devices.iter().enumerate() {
            let kind = device_kind(device);
            if kind == IommuKind::Unknown {
                continue;
            }
            if device.regions.is_empty() {
                info.units.push(IommuUnit {
                    kind,
                    segment: 0,
                    base: 0,
                    length: 0,
                    id: index as u32,
                });
                continue;
            }
            for (base, length) in &device.regions {
                info.units.push(IommuUnit {
                    kind,
                    segment: 0,
                    base: *base,
                    length: *length,
                    id: index as u32,
                });
            }
        }
        info
    }

    /// 是否检测到任何 IOMMU。
    pub fn available(&self) -> bool {
        !self.units.is_empty()
    }
}

fn device_kind(device: &crate::Device) -> IommuKind {
    let compatible = &device.compatible;
    if compatible.iter().any(|name| name == "arm,smmu-v3") {
        IommuKind::ArmSmmuV3
    } else if compatible
        .iter()
        .any(|name| name == "arm,smmu-v2" || name == "arm,mmu-500")
    {
        IommuKind::ArmSmmuV2
    } else if compatible
        .iter()
        .any(|name| name.starts_with("riscv,") && (name.contains("iommu")))
    {
        IommuKind::RiscvIommu
    } else if compatible
        .iter()
        .any(|name| name.contains("iommu") || name.contains("smmu"))
    {
        IommuKind::Other
    } else {
        IommuKind::Unknown
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec;

    use super::*;
    use crate::Device;

    #[test]
    fn classifies_arm_and_riscv_iommu_devices() {
        let dtb = DtbInfo {
            devices: vec![
                Device {
                    path: String::from("/smmu@2b400000"),
                    compatible: vec![String::from("arm,smmu-v3")],
                    regions: vec![(0x2b40_0000, 0x1_0000)],
                    irqs: Vec::new(),
                },
                Device {
                    path: String::from("/iommu@30000000"),
                    compatible: vec![String::from("riscv,iommu")],
                    regions: vec![(0x3000_0000, 0x1000)],
                    irqs: Vec::new(),
                },
            ],
            ..DtbInfo::default()
        };
        let info = IommuInfo::from_dtb(&dtb);
        assert_eq!(info.units.len(), 2);
        assert_eq!(info.units[0].kind, IommuKind::ArmSmmuV3);
        assert_eq!(info.units[1].kind, IommuKind::RiscvIommu);
        assert!(info.available());
    }
}
