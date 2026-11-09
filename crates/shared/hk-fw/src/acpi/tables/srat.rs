use alloc::vec::Vec;

use super::{SIGNATURE_SRAT, table_length, u8_at, u32_at, u64_at};

/// SRAT CPU/内存邻近域。
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
