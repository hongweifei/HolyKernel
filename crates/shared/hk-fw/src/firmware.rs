//! ACPI 固定表与拓扑表的架构无关视图。
//!
//! 只解析内核当前需要的字段：FADT（电源管理）、MCFG（ECAM）、SRAT（NUMA）、
//! PPTT（CPU 拓扑）与 DMAR（VT-d 单元）。所有解析都基于已通过校验的原始表
//! 字节，外部类型不泄漏到公开 API。

use alloc::vec::Vec;

/// ACPI 表头长度。
const HEADER: usize = 36;
const SIGNATURE_FADT: &[u8; 4] = b"FACP";
const SIGNATURE_MCFG: &[u8; 4] = b"MCFG";
const SIGNATURE_SRAT: &[u8; 4] = b"SRAT";
const SIGNATURE_PPTT: &[u8; 4] = b"PPTT";
const SIGNATURE_DMAR: &[u8; 4] = b"DMAR";
const SIGNATURE_IVRS: &[u8; 4] = b"IVRS";
const SIGNATURE_HPET: &[u8; 4] = b"HPET";

fn u8_at(bytes: &[u8], offset: usize) -> Option<u8> {
    bytes.get(offset).copied()
}

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn u64_at(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn table_length(bytes: &[u8], signature: &[u8; 4]) -> Option<usize> {
    if bytes.get(0..4)? != signature {
        return None;
    }
    let length = u32_at(bytes, 4)? as usize;
    (HEADER..=bytes.len()).contains(&length).then_some(length)
}

/// ACPI 通用地址结构。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GenericAddress {
    /// 地址空间编号（0 = 系统内存，1 = 系统 I/O）。
    pub space: u8,
    /// 位宽。
    pub bit_width: u8,
    /// 位偏移。
    pub bit_offset: u8,
    /// 访问宽度。
    pub access_width: u8,
    /// 地址。
    pub address: u64,
}

impl GenericAddress {
    fn parse(bytes: &[u8], offset: usize) -> Option<Self> {
        Some(Self {
            space: u8_at(bytes, offset)?,
            bit_width: u8_at(bytes, offset + 1)?,
            bit_offset: u8_at(bytes, offset + 2)?,
            access_width: u8_at(bytes, offset + 3)?,
            address: u64_at(bytes, offset + 4)?,
        })
    }
}

/// 固定 ACPI 描述表（FADT）的电源相关视图。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fadt {
    /// 固件控制结构（FACS）物理地址。
    pub firmware_ctrl: u32,
    /// 扩展固件控制结构地址（FADT rev 6+）。
    pub x_firmware_ctrl: u64,
    /// DSDT 物理地址（32 位字段；0 表示使用扩展字段）。
    pub dsdt: u64,
    /// SCI 中断号。
    pub sci_interrupt: u16,
    /// PM1a 事件块 I/O 端口。
    pub pm1a_event: u32,
    /// PM1b 事件块 I/O 端口。
    pub pm1b_event: u32,
    /// PM1 事件寄存器长度（字节）。
    pub pm1_event_length: u8,
    /// GPE0 块 I/O 端口。
    pub gpe0_block: u32,
    /// GPE0 块长度。
    pub gpe0_length: u8,
    /// GPE1 块 I/O 端口。
    pub gpe1_block: u32,
    /// GPE1 块长度。
    pub gpe1_length: u8,
    /// GPE1 基址。
    pub gpe1_base: u8,
    /// PM1a 控制块 I/O 端口。
    pub pm1a_control: u32,
    /// PM1b 控制块 I/O 端口；0 表示不存在。
    pub pm1b_control: u32,
    /// PM1 控制寄存器长度（字节）。
    pub pm1_control_length: u8,
    /// 复位寄存器。
    pub reset_register: GenericAddress,
    /// 复位值。
    pub reset_value: u8,
    /// ACPI 主版本。
    pub major: u8,
    /// ACPI 次版本。
    pub minor: u8,
    /// S5 休眠控制寄存器（FADT rev 6+）。
    pub sleep_control: u32,
    /// S5 休眠状态寄存器（FADT rev 6+）。
    pub sleep_status: u32,
}

/// 从 FADT 字节解析电源字段。
pub fn parse_fadt(bytes: &[u8]) -> Option<Fadt> {
    let length = table_length(bytes, SIGNATURE_FADT)?;
    let bytes = &bytes[..length];
    if bytes.len() < 116 {
        return None;
    }
    Some(Fadt {
        firmware_ctrl: u32_at(bytes, 36)?,
        x_firmware_ctrl: if bytes.len() >= 140 {
            u64_at(bytes, 132).unwrap_or(0)
        } else {
            0
        },
        dsdt: if bytes.len() >= 148 {
            let extended = u64_at(bytes, 140).unwrap_or(0);
            if extended != 0 {
                extended
            } else {
                u64::from(u32_at(bytes, 40)?)
            }
        } else {
            u64::from(u32_at(bytes, 40)?)
        },
        sci_interrupt: u16_at(bytes, 49)?,
        pm1a_event: u32_at(bytes, 56)?,
        pm1b_event: u32_at(bytes, 60)?,
        pm1_event_length: u8_at(bytes, 88)?,
        gpe0_block: u32_at(bytes, 80)?,
        gpe0_length: u8_at(bytes, 92)?,
        gpe1_block: u32_at(bytes, 84)?,
        gpe1_length: u8_at(bytes, 93)?,
        gpe1_base: u8_at(bytes, 94)?,
        pm1a_control: u32_at(bytes, 64)?,
        pm1b_control: u32_at(bytes, 68)?,
        pm1_control_length: u8_at(bytes, 89)?,
        reset_register: GenericAddress::parse(bytes, 116).unwrap_or_default(),
        reset_value: u8_at(bytes, 128)?,
        major: u8_at(bytes, 8)?,
        minor: u8_at(bytes, 131).unwrap_or(0),
        sleep_control: if bytes.len() >= 248 {
            u32_at(bytes, 244).unwrap_or(0)
        } else {
            0
        },
        sleep_status: if bytes.len() >= 252 {
            u32_at(bytes, 248).unwrap_or(0)
        } else {
            0
        },
    })
}

/// MCFG 中的一段 ECAM 窗口。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct McfgEntry {
    /// ECAM 物理基址。
    pub base: u64,
    /// PCI 段号。
    pub segment: u16,
    /// 起始总线。
    pub bus_start: u8,
    /// 结束总线（含）。
    pub bus_end: u8,
}

impl McfgEntry {
    /// 该窗口覆盖的总线数。
    pub fn bus_count(&self) -> usize {
        usize::from(self.bus_end.saturating_sub(self.bus_start)) + 1
    }

    /// 窗口字节数：每条总线 1 MiB。
    pub fn length(&self) -> u64 {
        (self.bus_count() as u64) << 20
    }
}

/// MCFG 表视图。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mcfg {
    /// ECAM 窗口。
    pub entries: Vec<McfgEntry>,
}

/// 解析 MCFG。
pub fn parse_mcfg(bytes: &[u8]) -> Option<Mcfg> {
    let length = table_length(bytes, SIGNATURE_MCFG)?;
    let bytes = &bytes[..length];
    if bytes.len() < 44 {
        return None;
    }
    let mut entries = Vec::new();
    let mut offset = 44;
    while offset + 16 <= bytes.len() {
        let base = u64_at(bytes, offset)?;
        let segment = u16_at(bytes, offset + 8)?;
        let bus_start = u8_at(bytes, offset + 10)?;
        let bus_end = u8_at(bytes, offset + 11)?;
        entries.push(McfgEntry {
            base,
            segment,
            bus_start,
            bus_end,
        });
        offset += 16;
    }
    Some(Mcfg { entries })
}

/// SRAT 中的 CPU 邻近域条目。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuAffinity {
    /// 本地 APIC/x2APIC 编号。
    pub apic_id: u32,
    /// 邻近域。
    pub proximity: u32,
    /// 该 CPU 是否可用。
    pub enabled: bool,
}

/// SRAT 中的内存邻近域条目。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryAffinity {
    /// 物理基址。
    pub base: u64,
    /// 区间长度。
    pub length: u64,
    /// 邻近域。
    pub proximity: u32,
    /// 该区间是否可用。
    pub enabled: bool,
}

/// SRAT 表视图。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Srat {
    /// CPU 邻近域。
    pub cpus: Vec<CpuAffinity>,
    /// 内存邻近域。
    pub memory: Vec<MemoryAffinity>,
}

/// 解析 SRAT。
pub fn parse_srat(bytes: &[u8]) -> Option<Srat> {
    let length = table_length(bytes, SIGNATURE_SRAT)?;
    let bytes = &bytes[..length];
    if bytes.len() < 48 {
        return None;
    }
    let mut srat = Srat::default();
    let mut offset = 48;
    while offset + 2 <= bytes.len() {
        let entry_type = u8_at(bytes, offset)?;
        let entry_len = u8_at(bytes, offset + 1)? as usize;
        if entry_len < 2 || offset + entry_len > bytes.len() {
            break;
        }
        let entry = &bytes[offset..offset + entry_len];
        match entry_type {
            0 if entry_len >= 16 => {
                let flags = u32_at(entry, 12)?;
                srat.cpus.push(CpuAffinity {
                    apic_id: u32::from(u8_at(entry, 8)?),
                    proximity: u32_at(entry, 2)?,
                    enabled: flags & 1 != 0,
                });
            }
            1 if entry_len >= 40 => {
                let flags = u32_at(entry, 28)?;
                srat.memory.push(MemoryAffinity {
                    base: u64_at(entry, 8)?,
                    length: u64_at(entry, 16)?,
                    proximity: u32_at(entry, 2)?,
                    enabled: flags & 1 != 0,
                });
            }
            2 if entry_len >= 24 => {
                let flags = u32_at(entry, 12)?;
                srat.cpus.push(CpuAffinity {
                    apic_id: u32_at(entry, 8)?,
                    proximity: u32_at(entry, 4)?,
                    enabled: flags & 1 != 0,
                });
            }
            _ => {}
        }
        offset += entry_len;
    }
    Some(srat)
}

/// PPTT 中的处理器拓扑条目。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessorTopology {
    /// 条目标志。
    pub flags: u32,
    /// 父节点索引；0 表示根。
    pub parent: u32,
    /// ACPI 处理器编号。
    pub processor_id: u32,
    /// 物理封装编号。
    pub package_id: u32,
}

/// PPTT 表视图。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pptt {
    /// 处理器条目。
    pub processors: Vec<ProcessorTopology>,
}

/// 解析 PPTT。
pub fn parse_pptt(bytes: &[u8]) -> Option<Pptt> {
    let length = table_length(bytes, SIGNATURE_PPTT)?;
    let bytes = &bytes[..length];
    if bytes.len() < HEADER {
        return None;
    }
    let mut pptt = Pptt::default();
    let mut offset = HEADER;
    while offset + 2 <= bytes.len() {
        let entry_type = u8_at(bytes, offset)?;
        let entry_len = u8_at(bytes, offset + 1)? as usize;
        if entry_len < 2 || offset + entry_len > bytes.len() {
            break;
        }
        let entry = &bytes[offset..offset + entry_len];
        if entry_type == 0 && entry_len >= 20 {
            pptt.processors.push(ProcessorTopology {
                flags: u32_at(entry, 4)?,
                parent: u32_at(entry, 8)?,
                processor_id: u32_at(entry, 12)?,
                package_id: u32_at(entry, 16)?,
            });
        }
        offset += entry_len;
    }
    Some(pptt)
}

/// DMAR 中的 DMA 重映射硬件单元（DRHD）。
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

/// HPET 事件定时器块。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hpet {
    /// 定时器块 ID（包含比较器数量与计数位宽）。
    pub block_id: u32,
    /// 寄存器页基址。
    pub address: GenericAddress,
    /// 最小节拍（飞秒）。
    pub period_fs: u16,
}

/// 解析 HPET 表。
pub fn parse_hpet(bytes: &[u8]) -> Option<Hpet> {
    let length = table_length(bytes, SIGNATURE_HPET)?;
    let bytes = &bytes[..length];
    if bytes.len() < 56 {
        return None;
    }
    Some(Hpet {
        block_id: u32_at(bytes, 36)?,
        address: GenericAddress::parse(bytes, 40)?,
        period_fs: u16_at(bytes, 53)?,
    })
}

/// 固件控制结构（FACS）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facs {
    /// 硬件签名。
    pub hardware_signature: u32,
    /// 固件唤醒向量（32 位）。
    pub firmware_waking_vector: u32,
    /// 全局锁。
    pub global_lock: u32,
    /// 标志。
    pub flags: u32,
    /// 扩展固件唤醒向量。
    pub x_firmware_waking_vector: u64,
    /// 版本。
    pub version: u8,
}

/// 解析 FACS（无校验和，按长度截断）。
pub fn parse_facs(bytes: &[u8]) -> Option<Facs> {
    if bytes.len() < 64 || bytes.get(0..4)? != b"FACS" {
        return None;
    }
    let length = u32_at(bytes, 4)? as usize;
    if length < 64 || length > bytes.len() {
        return None;
    }
    Some(Facs {
        hardware_signature: u32_at(bytes, 8)?,
        firmware_waking_vector: u32_at(bytes, 12)?,
        global_lock: u32_at(bytes, 16)?,
        flags: u32_at(bytes, 20)?,
        x_firmware_waking_vector: if length >= 32 { u64_at(bytes, 24)? } else { 0 },
        version: if length >= 33 { u8_at(bytes, 32)? } else { 0 },
    })
}

fn read_facs<F>(read: &mut F, fadt: &Fadt) -> Option<Facs>
where
    F: FnMut(u64, usize) -> Option<Vec<u8>>,
{
    let address = if fadt.x_firmware_ctrl != 0 {
        fadt.x_firmware_ctrl
    } else {
        u64::from(fadt.firmware_ctrl)
    };
    if address == 0 {
        return None;
    }
    let header = read(address, 8)?;
    let length = u32_at(&header, 4)? as usize;
    if !(64..=4096).contains(&length) {
        return None;
    }
    let bytes = read(address, length)?;
    parse_facs(&bytes)
}

/// 查找指定签名的全部 ACPI 表。
pub fn find_tables<F>(rsdp_phys: u64, signature: &[u8; 4], read: F) -> Vec<Vec<u8>>
where
    F: FnMut(u64, usize) -> Option<Vec<u8>>,
{
    crate::acpi::rsdp::find_tables(rsdp_phys, signature, read)
}

/// 读取并按表头校验一张 ACPI 表（如 DSDT）。
pub fn read_table<F>(phys: u64, read: F) -> Option<Vec<u8>>
where
    F: FnMut(u64, usize) -> Option<Vec<u8>>,
{
    crate::acpi::rsdp::read_table(phys, read)
}

/// 从 DSDT 中解析 `_S5` 包的前两个 SLP_TYP 值。
///
/// 只做最小 AML 扫描：定位 `Name (\_S5_, Package (...))`，读取前两个整数元素。
/// 找不到或结构不符时返回 `None`，调用方可使用固件默认值。
pub fn parse_s5(dsdt: &[u8]) -> Option<(u8, u8)> {
    const NAME_S5: [u8; 6] = [0x08, 0x5C, 0x5F, 0x53, 0x35, 0x5F];
    let mut from = 0;
    while let Some(position) = dsdt
        .get(from..)
        .and_then(|bytes| bytes.windows(NAME_S5.len()).position(|w| w == NAME_S5))
    {
        let mut at = from + position + NAME_S5.len();
        from = at;
        if dsdt.get(at) != Some(&0x12) {
            continue;
        }
        at += 1;
        let first = *dsdt.get(at)?;
        at += match first >> 6 {
            0 => 1,
            1 => 2,
            2 => 3,
            _ => 4,
        };
        let elements = *dsdt.get(at)?;
        if elements < 2 {
            continue;
        }
        at += 1;
        let first = read_aml_integer(dsdt, &mut at)?;
        let second = read_aml_integer(dsdt, &mut at)?;
        return Some((first as u8, second as u8));
    }
    None
}

fn read_aml_integer(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let opcode = *bytes.get(*at)?;
    *at += 1;
    match opcode {
        0x00 => Some(0),
        0x01 => Some(1),
        0x0A => {
            let value = u64::from(*bytes.get(*at)?);
            *at += 1;
            Some(value)
        }
        0x0B => {
            let value = u64::from(u16_at(bytes, *at)?);
            *at += 2;
            Some(value)
        }
        0x0C => {
            let value = u64::from(u32_at(bytes, *at)?);
            *at += 4;
            Some(value)
        }
        0x0E => {
            let value = u64_at(bytes, *at)?;
            *at += 8;
            Some(value)
        }
        _ => None,
    }
}

/// 一次解析全部需要固定表；缺失的表为 `None`。
#[derive(Clone, Debug, Default)]
pub struct Tables {
    /// FADT。
    pub fadt: Option<Fadt>,
    /// MCFG。
    pub mcfg: Option<Mcfg>,
    /// SRAT。
    pub srat: Option<Srat>,
    /// PPTT。
    pub pptt: Option<Pptt>,
    /// DMAR。
    pub dmar: Option<Dmar>,
    /// AMD-Vi IVRS。
    pub ivrs: Option<Ivrs>,
    /// 平台中性的 IOMMU 描述。
    pub iommu: crate::iommu::IommuInfo,
    /// HPET。
    pub hpet: Option<Hpet>,
    /// FACS。
    pub facs: Option<Facs>,
}

/// 从 RSDP 解析固定表与拓扑表。
pub fn parse<F>(rsdp_phys: u64, mut read: F) -> Tables
where
    F: FnMut(u64, usize) -> Option<Vec<u8>>,
{
    let find = |signature: &[u8; 4], read: &mut F| {
        crate::acpi::rsdp::find_table(rsdp_phys, signature, read)
    };
    let fadt = find(SIGNATURE_FADT, &mut read).and_then(|bytes| parse_fadt(&bytes));
    let mcfg = find(SIGNATURE_MCFG, &mut read).and_then(|bytes| parse_mcfg(&bytes));
    let srat = find(SIGNATURE_SRAT, &mut read).and_then(|bytes| parse_srat(&bytes));
    let pptt = find(SIGNATURE_PPTT, &mut read).and_then(|bytes| parse_pptt(&bytes));
    let dmar = find(SIGNATURE_DMAR, &mut read).and_then(|bytes| parse_dmar(&bytes));
    let ivrs = find(SIGNATURE_IVRS, &mut read).and_then(|bytes| parse_ivrs(&bytes));
    let hpet = find(SIGNATURE_HPET, &mut read).and_then(|bytes| parse_hpet(&bytes));
    let facs = fadt.and_then(|fadt| read_facs(&mut read, &fadt));
    let iommu = crate::iommu::IommuInfo::from_acpi(dmar.as_ref(), ivrs.as_ref());
    Tables {
        fadt,
        mcfg,
        srat,
        pptt,
        dmar,
        ivrs,
        iommu,
        hpet,
        facs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(signature: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut bytes = alloc::vec![0_u8; HEADER];
        bytes[0..4].copy_from_slice(signature);
        bytes.extend_from_slice(body);
        let length = bytes.len() as u32;
        bytes[4..8].copy_from_slice(&length.to_le_bytes());
        bytes[9] = 0;
        let sum = bytes.iter().fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
        bytes[9] = 0_u8.wrapping_sub(sum);
        bytes
    }

    #[test]
    fn parses_fadt_power_fields() {
        let mut body = alloc::vec![0_u8; 120];
        body[64 - HEADER..68 - HEADER].copy_from_slice(&0x0600_u32.to_le_bytes());
        body[68 - HEADER..72 - HEADER].copy_from_slice(&0x0604_u32.to_le_bytes());
        body[89 - HEADER] = 2;
        body[128 - HEADER] = 0x0F;
        body[131 - HEADER] = 3;
        let mut bytes = table(SIGNATURE_FADT, &body);
        bytes[8] = 6;
        let fadt = parse_fadt(&bytes).expect("FADT");
        assert_eq!(fadt.pm1a_control, 0x0600);
        assert_eq!(fadt.pm1b_control, 0x0604);
        assert_eq!(fadt.pm1_control_length, 2);
        assert_eq!(fadt.reset_value, 0x0F);
        assert_eq!((fadt.major, fadt.minor), (6, 3));
    }

    #[test]
    fn parses_mcfg_entries() {
        let mut body = alloc::vec![0_u8; 8];
        body.extend_from_slice(&0xE000_0000_u64.to_le_bytes());
        body.extend_from_slice(&0_u16.to_le_bytes());
        body.push(0);
        body.push(255);
        body.extend_from_slice(&0_u32.to_le_bytes());
        let mcfg = parse_mcfg(&table(SIGNATURE_MCFG, &body)).expect("MCFG");
        assert_eq!(mcfg.entries.len(), 1);
        assert_eq!(mcfg.entries[0].base, 0xE000_0000);
        assert_eq!(mcfg.entries[0].bus_count(), 256);
        assert_eq!(mcfg.entries[0].length(), 256 << 20);
    }

    #[test]
    fn parses_srat_cpu_and_memory() {
        let mut body = alloc::vec![0_u8; 12];
        let mut cpu = alloc::vec![0_u8; 16];
        cpu[0] = 0;
        cpu[1] = 16;
        cpu[2..6].copy_from_slice(&1_u32.to_le_bytes());
        cpu[8] = 3;
        cpu[12..16].copy_from_slice(&1_u32.to_le_bytes());
        body.extend_from_slice(&cpu);
        let mut memory = alloc::vec![0_u8; 40];
        memory[0] = 1;
        memory[1] = 40;
        memory[2..6].copy_from_slice(&1_u32.to_le_bytes());
        memory[8..16].copy_from_slice(&0x1000_u64.to_le_bytes());
        memory[16..24].copy_from_slice(&0x2000_u64.to_le_bytes());
        memory[28..32].copy_from_slice(&1_u32.to_le_bytes());
        body.extend_from_slice(&memory);
        let srat = parse_srat(&table(SIGNATURE_SRAT, &body)).expect("SRAT");
        assert_eq!(srat.cpus.len(), 1);
        assert_eq!(srat.cpus[0].apic_id, 3);
        assert!(srat.cpus[0].enabled);
        assert_eq!(srat.memory.len(), 1);
        assert_eq!(srat.memory[0].base, 0x1000);
        assert_eq!(srat.memory[0].length, 0x2000);
    }

    #[test]
    fn parses_pptt_processors() {
        let mut body = alloc::vec![0_u8; 0];
        let mut processor = alloc::vec![0_u8; 20];
        processor[0] = 0;
        processor[1] = 20;
        processor[4..8].copy_from_slice(&1_u32.to_le_bytes());
        processor[8..12].copy_from_slice(&0_u32.to_le_bytes());
        processor[12..16].copy_from_slice(&4_u32.to_le_bytes());
        processor[16..20].copy_from_slice(&2_u32.to_le_bytes());
        body.extend_from_slice(&processor);
        let pptt = parse_pptt(&table(SIGNATURE_PPTT, &body)).expect("PPTT");
        assert_eq!(pptt.processors.len(), 1);
        assert_eq!(pptt.processors[0].processor_id, 4);
        assert_eq!(pptt.processors[0].package_id, 2);
    }

    #[test]
    fn parses_dmar_units() {
        let mut body = alloc::vec![0_u8; 12];
        let mut unit = alloc::vec![0_u8; 16];
        unit[0..2].copy_from_slice(&0_u16.to_le_bytes());
        unit[2..4].copy_from_slice(&16_u16.to_le_bytes());
        unit[4] = 1;
        unit[6..8].copy_from_slice(&0_u16.to_le_bytes());
        unit[8..16].copy_from_slice(&0xFED9_0000_u64.to_le_bytes());
        body.extend_from_slice(&unit);
        let dmar = parse_dmar(&table(SIGNATURE_DMAR, &body)).expect("DMAR");
        assert_eq!(dmar.units.len(), 1);
        assert_eq!(dmar.units[0].address, 0xFED9_0000);
        assert_eq!(dmar.units[0].flags, 1);
    }

    #[test]
    fn parses_facs() {
        let mut bytes = alloc::vec![0_u8; 64];
        bytes[0..4].copy_from_slice(b"FACS");
        bytes[4..8].copy_from_slice(&64_u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&0x1234_u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&0x1000_u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&7_u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
        bytes[24..32].copy_from_slice(&0x2000_u64.to_le_bytes());
        bytes[32] = 2;
        let facs = parse_facs(&bytes).expect("FACS");
        assert_eq!(facs.hardware_signature, 0x1234);
        assert_eq!(facs.firmware_waking_vector, 0x1000);
        assert_eq!(facs.global_lock, 7);
        assert_eq!(facs.x_firmware_waking_vector, 0x2000);
        assert_eq!(facs.version, 2);
    }

    #[test]
    fn parses_s5_package_from_dsdt() {
        // Name (\_S5_, Package (2) { 0x05, 0x05 })
        let dsdt = [
            0x14, 0x10, 0x08, 0x5C, 0x5F, 0x53, 0x35, 0x5F, 0x12, 0x06, 0x02, 0x0A, 0x05, 0x0A,
            0x05,
        ];
        assert_eq!(parse_s5(&dsdt), Some((5, 5)));
        assert_eq!(parse_s5(&[0_u8; 8]), None);
    }

    #[test]
    fn parses_amd_ivrs() {
        let mut body = alloc::vec![0_u8; 12];
        body.extend_from_slice(&[
            0x10, 0x00, 28, 0, // IVHD 10h, length 28
            0x02, 0x00, // DeviceID
            0x40, 0x00, // capability offset
            0x00, 0x00, 0xd8, 0xfe, 0, 0, 0, 0, // base
            0, 0, // segment
            0, 0, // IOMMU info
            0, 0, 0, 0, // feature reporting
            0x02, 0xad, 0x0b, 0x00, // select device entry
        ]);
        let ivrs = parse_ivrs(&table(SIGNATURE_IVRS, &body)).expect("IVRS");
        assert_eq!(ivrs.units.len(), 1);
        assert_eq!(ivrs.units[0].segment, 0);
        assert_eq!(ivrs.units[0].address, 0xFED8_0000);
        assert_eq!(ivrs.units[0].device_id, 0x0002);
    }

    #[test]
    fn parses_hpet() {
        let mut body = alloc::vec![0_u8; 8];
        body[0..4].copy_from_slice(&0x8086_A201_u32.to_le_bytes());
        body[4] = 0;
        body[5] = 64;
        body[6] = 0;
        body[7] = 0;
        let address = 0xFED0_0000_u64;
        body.extend_from_slice(&address.to_le_bytes());
        body.push(0);
        body.extend_from_slice(&1000_u16.to_le_bytes());
        body.push(0);
        let hpet = parse_hpet(&table(SIGNATURE_HPET, &body)).expect("HPET");
        assert_eq!(hpet.block_id, 0x8086_A201);
        assert_eq!(hpet.address.address, address);
        assert_eq!(hpet.period_fs, 1000);
    }
}
