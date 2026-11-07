//! 虚拟 DMA 通道。

use hk_hal::{DmaBuffer, DmaError, DmaOps};

/// Hosted 平台的 DMA 通道。
///
/// 模拟设备在 virtio 驱动接入时提供缓冲区分配；在此之前分配请求返回
/// [`DmaError::Unsupported`]。Hosted 平台的内存由单一进程地址空间承载，
/// 缓存维护是空操作。
#[derive(Debug, Default)]
pub struct HostedDma;

impl HostedDma {
    /// 构造 DMA 通道。
    pub fn new() -> Self {
        Self
    }
}

impl DmaOps for HostedDma {
    fn alloc_coherent(&self, _len: usize) -> Result<DmaBuffer, DmaError> {
        Err(DmaError::Unsupported)
    }

    fn free_coherent(&self, _buffer: DmaBuffer) {}

    fn sync_for_device(&self, _buffer: &DmaBuffer) {}

    fn sync_for_cpu(&self, _buffer: &DmaBuffer) {}
}
