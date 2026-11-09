use alloc::vec::Vec;

/// DMAR/IVRS IOMMU 原始表视图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drhd {
    /// 标志：INCLUDE_PCI_ALL。
    pub flags: u8,
    /// PCI 段号。
    pub segment: u16,
    /// 寄存器页物理基址。
    pub address: u64,
}

/// DMAR 中的保留内存区（RMRR）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rmrr {
    /// PCI 段号。
    pub segment: u16,
    /// 起始物理地址。
    pub base: u64,
    /// 结束物理地址。
    pub limit: u64,
}

/// DMAR 表视图。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dmar {
    /// DMA 重映射单元。
    pub units: Vec<Drhd>,
    /// 保留内存区。
    pub reserved: Vec<Rmrr>,
}

/// 解析 DMAR。
pub fn parse_dmar(bytes: &[u8]) -> Option<Dmar> {
    use intel_iommu::dmar::{DmarTable, RemapStruct};

    let table = DmarTable::new(bytes).ok()?;
    let mut dmar = Dmar::default();
    for entry in table.structs() {
        match entry {
            RemapStruct::Drhd(unit) => dmar.units.push(Drhd {
                flags: u8::from(unit.include_pci_all()),
                segment: unit.segment(),
                address: unit.register_base(),
            }),
            RemapStruct::Rmrr(region) => dmar.reserved.push(Rmrr {
                segment: region.segment(),
                base: region.base(),
                limit: region.limit(),
            }),
            _ => {}
        }
    }
    Some(dmar)
}

/// AMD-Vi IVHD 单元。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AmdUnit {
    /// PCI 段号。
    pub segment: u16,
    /// IOMMU 控制寄存器 MMIO 基址。
    pub address: u64,
    /// IOMMU 的 PCI DeviceID。
    pub device_id: u16,
}

/// AMD-Vi IVMD 保留内存块。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AmdMemory {
    /// PCI 段号。
    pub segment: u16,
    /// 物理基址。
    pub base: u64,
    /// 长度。
    pub length: u64,
    /// IVMD 标志。
    pub flags: u8,
}

/// AMD-Vi IVRS 表视图。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ivrs {
    /// IVHD 单元。
    pub units: Vec<AmdUnit>,
    /// IVMD 保留内存。
    pub memory: Vec<AmdMemory>,
}

/// 解析 AMD-Vi IVRS。
pub fn parse_ivrs(bytes: &[u8]) -> Option<Ivrs> {
    use amd_iommu::ivrs::IvrsTable;

    let table = IvrsTable::new(bytes).ok()?;
    let mut ivrs = Ivrs::default();
    for unit in table.ivhd_units() {
        ivrs.units.push(AmdUnit {
            segment: unit.segment(),
            address: unit.base_address(),
            device_id: unit.device_id(),
        });
    }
    for memory in table.ivmd_units() {
        ivrs.memory.push(AmdMemory {
            segment: memory.segment(),
            base: memory.base(),
            length: memory.length_bytes(),
            flags: memory.flags(),
        });
    }
    Some(ivrs)
}
