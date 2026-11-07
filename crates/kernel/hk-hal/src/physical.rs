//! 物理内存访问。
//!
//! 内核通过 direct map 读写物理内存：COW 复制、ELF 加载与诊断都依赖这条通道。
//! 平台实现保证地址落在 direct map 覆盖范围内。

use core::fmt;

use crate::addr::PhysAddr;

/// 物理内存访问错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicalError {
    /// 地址超出 direct map 覆盖范围。
    OutOfRange,
}

impl fmt::Display for PhysicalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfRange => formatter.write_str("物理地址超出 direct map 范围"),
        }
    }
}

impl core::error::Error for PhysicalError {}

/// 物理内存访问通道。
pub trait PhysicalAccess: Send + Sync {
    /// 读取物理内存到缓冲。
    fn read(&self, address: PhysAddr, buffer: &mut [u8]) -> Result<(), PhysicalError>;

    /// 把缓冲写入物理内存。
    fn write(&self, address: PhysAddr, bytes: &[u8]) -> Result<(), PhysicalError>;

    /// 在物理地址之间复制，用于写时复制与镜像加载。
    fn copy(&self, from: PhysAddr, to: PhysAddr, len: usize) -> Result<(), PhysicalError>;
}
