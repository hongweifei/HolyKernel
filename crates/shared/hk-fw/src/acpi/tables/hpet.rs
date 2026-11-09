use super::{GenericAddress, SIGNATURE_HPET, table_length, u16_at, u32_at};

/// HPET 定时器块。
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
