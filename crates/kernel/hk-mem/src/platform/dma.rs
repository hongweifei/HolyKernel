//! 连续 DMA 分配器：底层使用 HMP Capacity。

use crate::hmp::{Capacity, CapacityId, CapacityKind};
use hk_hal::{CacheOps, DmaBuffer, DmaError, DmaOps, PhysRange};
use spin::Mutex;

/// 启动期预留、可分配与归还的连续 DMA 分配器。
pub struct DmaAllocator {
    capacity: Mutex<Option<Capacity>>,
    cache: Mutex<Option<&'static dyn CacheOps>>,
}

impl DmaAllocator {
    /// 构造空分配器。
    pub const fn new() -> Self {
        Self {
            capacity: Mutex::new(None),
            cache: Mutex::new(None),
        }
    }

    /// 设置 DMA 缓存维护后端。
    pub fn set_cache(&self, cache: &'static dyn CacheOps) {
        *self.cache.lock() = Some(cache);
    }

    /// 注册启动期预留的连续 DMA 区域。
    ///
    /// `id` 用于在后端归因；调用方应先通过 `FrameBank::reserve_for` 用同一
    /// 个 `id` 预留区间，这里只把描述交给池。未注册后端时容量使用自带的
    /// 窗口分配器。
    pub fn init(&self, id: CapacityId, range: PhysRange) {
        *self.capacity.lock() =
            Capacity::new(id, range, hk_hal::PAGE_SIZE, CapacityKind::Dma, 0).ok();
    }

    /// 分配一段连续内存，长度按页向上取整。
    pub fn allocate(&self, len: usize) -> Option<PhysRange> {
        let mut capacity = self.capacity.lock();
        let frame = capacity.as_mut()?.allocate(len).ok()?;
        Some(PhysRange::new(frame.phys(), frame.len() as u64))
    }

    /// 归还一段 DMA 内存。
    pub fn free(&self, range: PhysRange) {
        if range.is_empty() {
            return;
        }
        if let Some(capacity) = self.capacity.lock().as_mut() {
            let _ = capacity.release(range.start(), range.len() as usize);
        }
    }
}

impl Default for DmaAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl DmaOps for DmaAllocator {
    fn alloc_coherent(&self, len: usize) -> Result<DmaBuffer, DmaError> {
        let range = self.allocate(len).ok_or(DmaError::OutOfMemory)?;
        Ok(DmaBuffer::new(range.start(), range.len() as usize))
    }

    fn free_coherent(&self, buffer: DmaBuffer) {
        self.free(PhysRange::new(buffer.phys(), buffer.len() as u64));
    }

    fn sync_for_device(&self, buffer: &DmaBuffer) {
        if let Some(cache) = *self.cache.lock() {
            cache.clean(PhysRange::new(buffer.phys(), buffer.len() as u64));
        }
    }

    fn sync_for_cpu(&self, buffer: &DmaBuffer) {
        if let Some(cache) = *self.cache.lock() {
            cache.invalidate(PhysRange::new(buffer.phys(), buffer.len() as u64));
        }
    }
}

#[cfg(test)]
mod tests {
    use hk_hal::PhysAddr;

    use super::*;

    #[test]
    fn allocates_reuses_and_splits() {
        let pool = DmaAllocator::new();
        pool.init(
            CapacityId::next(),
            PhysRange::new(PhysAddr::new(0x10_0000), 0x4000),
        );
        let first = pool.allocate(4096).expect("第一页");
        let second = pool.allocate(8192).expect("第二段");
        assert_eq!(first.start(), PhysAddr::new(0x10_0000));
        assert_eq!(first.len(), 4096);
        assert_eq!(second.start(), PhysAddr::new(0x10_1000));
        assert_eq!(second.len(), 8192);

        pool.free(first);
        assert_eq!(
            pool.allocate(4096).expect("复用").start(),
            PhysAddr::new(0x10_0000)
        );
    }

    #[test]
    fn implements_dma_ops() {
        let pool = DmaAllocator::new();
        pool.init(
            CapacityId::next(),
            PhysRange::new(PhysAddr::new(0x20_0000), 0x2000),
        );
        let buffer = pool.alloc_coherent(4096).expect("一致性缓冲");
        assert_eq!(buffer.phys(), PhysAddr::new(0x20_0000));
        assert_eq!(buffer.len(), 4096);
        pool.free_coherent(buffer);
        assert_eq!(
            pool.allocate(4096).expect("复用").start(),
            PhysAddr::new(0x20_0000)
        );
    }
}
