//! ACPI 固定表与拓扑表：按表类型拆分的架构无关视图。
//!
//! 所有解析都基于已通过校验的原始表字节，外部固件库类型不进入公开 API。

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

/// FACS 固件控制结构。
pub mod facs;
/// FADT 电源管理与通用地址。
pub mod fadt;
/// HPET 定时器块。
pub mod hpet;
/// DMAR/IVRS IOMMU 原始表视图。
pub mod iommu;
/// MCFG ECAM 窗口。
pub mod mcfg;
/// PPTT 处理器拓扑。
pub mod pptt;
/// SRAT CPU/内存邻近域。
pub mod srat;

use facs::read_facs;
pub use facs::{Facs, parse_facs};
pub use fadt::{Fadt, GenericAddress, parse_fadt};
pub use hpet::{Hpet, parse_hpet};
pub use iommu::{AmdMemory, AmdUnit, Dmar, Drhd, Ivrs, Rmrr, parse_dmar, parse_ivrs};
pub use mcfg::{Mcfg, McfgEntry, parse_mcfg};
pub use pptt::{Pptt, ProcessorTopology, parse_pptt};
pub use srat::{CpuAffinity, MemoryAffinity, Srat, parse_srat};

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
    /// 平台中性的 IOMMU 描述。
    pub iommu: crate::iommu::IommuInfo,
    /// HPET。
    pub hpet: Option<Hpet>,
    /// FACS。
    pub facs: Option<Facs>,
}

impl Tables {
    /// 转换为 ACPI/DTB 共享的规范化固件描述。
    pub fn firmware_info(&self, madt: Option<&crate::acpi::Madt>) -> crate::FirmwareInfo {
        crate::FirmwareInfo::from_acpi(madt, self)
    }
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
