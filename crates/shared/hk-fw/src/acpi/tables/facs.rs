use alloc::vec::Vec;

use super::{Fadt, u8_at, u32_at, u64_at};

/// FACS 固件控制结构。
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

pub(super) fn read_facs<F>(read: &mut F, fadt: &Fadt) -> Option<Facs>
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
