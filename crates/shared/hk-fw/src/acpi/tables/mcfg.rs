use alloc::vec::Vec;

use super::{SIGNATURE_MCFG, table_length, u8_at, u16_at, u64_at};

/// MCFG ECAM 窗口。
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
