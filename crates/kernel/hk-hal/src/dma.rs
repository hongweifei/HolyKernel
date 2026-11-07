//! DMA 缓冲与缓存维护。

use core::fmt;

use crate::addr::PhysAddr;

/// 一段可供设备访问的 DMA 缓冲。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaBuffer {
    phys: PhysAddr,
    len: usize,
}

impl DmaBuffer {
    /// 以物理地址与长度构造 DMA 缓冲描述。
    pub const fn new(phys: PhysAddr, len: usize) -> Self {
        Self { phys, len }
    }

    /// 返回缓冲的设备可见物理地址。
    pub const fn phys(self) -> PhysAddr {
        self.phys
    }

    /// 返回缓冲长度，单位为字节。
    pub const fn len(self) -> usize {
        self.len
    }

    /// 判断缓冲是否为空。
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }
}

/// DMA 操作错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaError {
    /// 内存不足。
    OutOfMemory,
    /// 平台不提供 DMA 通道。
    Unsupported,
    /// 长度未按页对齐。
    Unaligned,
}

impl fmt::Display for DmaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfMemory => formatter.write_str("DMA 内存不足"),
            Self::Unsupported => formatter.write_str("平台不提供 DMA 通道"),
            Self::Unaligned => formatter.write_str("DMA 长度未按页对齐"),
        }
    }
}

impl core::error::Error for DmaError {}

/// DMA 通道。
pub trait DmaOps: Send + Sync {
    /// 分配一段设备可访问的一致性缓冲。
    fn alloc_coherent(&self, len: usize) -> Result<DmaBuffer, DmaError>;

    /// 释放一致性缓冲。
    fn free_coherent(&self, buffer: DmaBuffer);

    /// 在设备读取前同步缓存。
    fn sync_for_device(&self, buffer: &DmaBuffer);

    /// 在 CPU 读取前同步缓存。
    fn sync_for_cpu(&self, buffer: &DmaBuffer);
}
