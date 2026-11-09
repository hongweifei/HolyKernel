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
    let length = table_length(bytes, SIGNATURE_DMAR)?;
    let bytes = &bytes[..length];
    if bytes.len() < 48 {
        return None;
    }
    let mut dmar = Dmar::default();
    let mut offset = 48;
    while offset + 4 <= bytes.len() {
        let entry_type = u16_at(bytes, offset)?;
        let entry_len = u16_at(bytes, offset + 2)? as usize;
        if entry_len < 4 || offset + entry_len > bytes.len() {
            break;
        }
        let entry = &bytes[offset..offset + entry_len];
        match entry_type {
            0 if entry_len >= 16 => dmar.units.push(Drhd {
                flags: u8_at(entry, 4)?,
                segment: u16_at(entry, 6)?,
                address: u64_at(entry, 8)?,
            }),
            1 if entry_len >= 24 => dmar.reserved.push(Rmrr {
                segment: u16_at(entry, 6)?,
                base: u64_at(entry, 8)?,
                limit: u64_at(entry, 16)?,
            }),
            _ => {}
        }
        offset += entry_len;
    }
    Some(dmar)
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
    /// HPET。
    pub hpet: Option<Hpet>,
}

/// 从 RSDP 解析固定表与拓扑表。
pub fn parse<F>(rsdp_phys: u64, mut read: F) -> Tables
where
    F: FnMut(u64, usize) -> Option<Vec<u8>>,
{
    let mut table = |signature: &[u8; 4]| hk_acpi::find_table(rsdp_phys, signature, &mut read);
    Tables {
        fadt: table(SIGNATURE_FADT).and_then(|bytes| parse_fadt(&bytes)),
        mcfg: table(SIGNATURE_MCFG).and_then(|bytes| parse_mcfg(&bytes)),
        srat: table(SIGNATURE_SRAT).and_then(|bytes| parse_srat(&bytes)),
        pptt: table(SIGNATURE_PPTT).and_then(|bytes| parse_pptt(&bytes)),
        dmar: table(SIGNATURE_DMAR).and_then(|bytes| parse_dmar(&bytes)),
        hpet: table(SIGNATURE_HPET).and_then(|bytes| parse_hpet(&bytes)),
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
