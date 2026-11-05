//! 线格式常量、记录类型与字节读写。

/// 镜像魔数。
pub const MAGIC: [u8; 4] = *b"HKXI";
/// 原生服务镜像的约定后缀。
pub const FILE_SUFFIX: &str = ".hkx";
/// 格式版本。
pub const VERSION: u16 = 1;
/// 设备授权能力。
pub const CAP_DEVICE: u8 = 1;
/// 端点能力。
pub const CAP_ENDPOINT: u8 = 2;
/// 通知能力。
pub const CAP_NOTIFICATION: u8 = 3;

pub(crate) const HEADER_SIZE: usize = 32;
pub(crate) const RECORD_HEADER: usize = 8;

pub(crate) const RECORD_TASK: u16 = 1;
pub(crate) const RECORD_REGION: u16 = 2;
pub(crate) const RECORD_CAPABILITY: u16 = 3;
pub(crate) const RECORD_SYMBOL: u16 = 4;
pub(crate) const RECORD_DEPENDENCY: u16 = 5;

pub(crate) const TASK_LEN: usize = 4 + 1 + 3 + 8 + 4 + 4 + 2;
pub(crate) const REGION_LEN: usize = 8 + 8 + 4 + 4;
pub(crate) const CAPABILITY_LEN: usize = 1 + 4 + 2;
pub(crate) const SYMBOL_LEN: usize = 8 + 2;
pub(crate) const DEPENDENCY_LEN: usize = 2 + 2;

pub(crate) fn push_u16(bytes: &mut alloc::vec::Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn push_u32(bytes: &mut alloc::vec::Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn push_u64(bytes: &mut alloc::vec::Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

pub(crate) fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

pub(crate) fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

pub(crate) fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn align4(value: usize) -> usize {
    (value + 3) & !3
}

pub(crate) fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x82F6_3B78
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
