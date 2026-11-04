//! 块设备契约与内存后端。

#![no_std]

extern crate alloc;

#[cfg(feature = "ipc")]
pub mod ipc;

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

/// 标准块大小。
pub const SECTOR_SIZE: usize = 512;

/// 块设备错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockError {
    /// 访问范围超出设备容量。
    OutOfRange,
    /// 长度未按块对齐或为零。
    InvalidLength,
    /// 只读设备。
    ReadOnly,
    /// 后端 I/O 错误。
    Io,
}

impl fmt::Display for BlockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::OutOfRange => "块范围越界",
            Self::InvalidLength => "块长度非法",
            Self::ReadOnly => "只读块设备",
            Self::Io => "块设备 I/O 错误",
        })
    }
}

impl core::error::Error for BlockError {}

/// 块设备接口。
pub trait BlockDevice {
    /// 返回块大小。
    fn block_size(&self) -> usize;

    /// 返回块数量。
    fn capacity_blocks(&self) -> u64;

    /// 从 `block` 开始读取，返回实际读取的字节数。
    fn read_blocks(&self, block: u64, buffer: &mut [u8]) -> Result<usize, BlockError>;

    /// 从 `block` 开始写入，返回实际写入的字节数。
    fn write_blocks(&mut self, block: u64, data: &[u8]) -> Result<usize, BlockError>;

    /// 刷写挂起写入。
    fn flush(&mut self) -> Result<(), BlockError>;

    /// 返回设备容量，单位为字节。
    fn capacity_bytes(&self) -> u64 {
        self.capacity_blocks()
            .saturating_mul(self.block_size() as u64)
    }
}

/// 内存块设备。
pub struct MemoryDisk {
    data: Vec<u8>,
    block_size: usize,
}

impl MemoryDisk {
    /// 创建指定块数的零填充内存盘。
    pub fn new(blocks: u64, block_size: usize) -> Self {
        let length = (blocks as usize).saturating_mul(block_size);
        Self {
            data: vec![0; length],
            block_size,
        }
    }

    /// 用已有字节构造内存盘；长度必须是块大小的整数倍。
    pub fn from_data(data: Vec<u8>, block_size: usize) -> Option<Self> {
        if block_size == 0 || !data.len().is_multiple_of(block_size) {
            return None;
        }
        Some(Self { data, block_size })
    }

    /// 返回底层字节。
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    fn offset(&self, block: u64, length: usize) -> Result<usize, BlockError> {
        if length == 0 || !length.is_multiple_of(self.block_size) {
            return Err(BlockError::InvalidLength);
        }
        let start = (block as usize)
            .checked_mul(self.block_size)
            .ok_or(BlockError::OutOfRange)?;
        let end = start.checked_add(length).ok_or(BlockError::OutOfRange)?;
        if end > self.data.len() {
            return Err(BlockError::OutOfRange);
        }
        Ok(start)
    }
}

impl BlockDevice for MemoryDisk {
    fn block_size(&self) -> usize {
        self.block_size
    }

    fn capacity_blocks(&self) -> u64 {
        (self.data.len() / self.block_size) as u64
    }

    fn read_blocks(&self, block: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        let start = self.offset(block, buffer.len())?;
        buffer.copy_from_slice(&self.data[start..start + buffer.len()]);
        Ok(buffer.len())
    }

    fn write_blocks(&mut self, block: u64, data: &[u8]) -> Result<usize, BlockError> {
        let start = self.offset(block, data.len())?;
        self.data[start..start + data.len()].copy_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> Result<(), BlockError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_blocks() {
        let mut disk = MemoryDisk::new(4, SECTOR_SIZE);
        let mut buffer = [0_u8; SECTOR_SIZE];
        disk.write_blocks(1, &[0xAA; SECTOR_SIZE]).expect("写");
        disk.read_blocks(1, &mut buffer).expect("读");
        assert_eq!(buffer, [0xAA; SECTOR_SIZE]);
        assert_eq!(disk.capacity_bytes(), 4 * SECTOR_SIZE as u64);
    }

    #[test]
    fn rejects_out_of_range() {
        let disk = MemoryDisk::new(1, SECTOR_SIZE);
        let mut buffer = [0_u8; SECTOR_SIZE * 2];
        assert_eq!(
            disk.read_blocks(0, &mut buffer),
            Err(BlockError::OutOfRange)
        );
    }

    #[test]
    fn round_trips_raw_data() {
        let data = vec![0x5A; SECTOR_SIZE * 2];
        let disk = MemoryDisk::from_data(data.clone(), SECTOR_SIZE).expect("构造");
        assert_eq!(disk.capacity_blocks(), 2);
        assert_eq!(disk.into_data(), data);
        assert!(MemoryDisk::from_data(vec![0; 3], SECTOR_SIZE).is_none());
    }
}
