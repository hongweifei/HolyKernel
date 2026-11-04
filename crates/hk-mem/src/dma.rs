//! 连续 DMA 内存池。

use alloc::vec::Vec;

use hk_hal::{DmaBuffer, DmaError, DmaOps, PAGE_SIZE, PhysAddr, PhysRange};
use spin::Mutex;

struct Pool {
    next: u64,
    end: u64,
}

/// 启动期预留、可分配与归还的连续 DMA 池。
pub struct DmaPool {
    pool: Mutex<Option<Pool>>,
    free: Mutex<Vec<PhysRange>>,
}

impl DmaPool {
    /// 构造空池。
    pub const fn new() -> Self {
        Self {
            pool: Mutex::new(None),
            free: Mutex::new(Vec::new()),
        }
    }

    /// 注册启动期预留的连续 DMA 区域。
    pub fn init(&self, range: PhysRange) {
        self.free.lock().clear();
        *self.pool.lock() = Some(Pool {
            next: range.start().get(),
            end: range.end().get(),
        });
    }

    /// 分配一段连续内存，长度按页向上取整。
    pub fn allocate(&self, len: usize) -> Option<PhysRange> {
        let page = PAGE_SIZE as u64;
        let length = (len as u64).div_ceil(page) * page;
        if length == 0 {
            return None;
        }
        let mut free = self.free.lock();
        if let Some(index) = free.iter().position(|range| range.len() >= length) {
            let range = free.swap_remove(index);
            let start = range.start();
            if range.len() > length {
                free.push(PhysRange::new(
                    PhysAddr::new(start.get() + length),
                    range.len() - length,
                ));
            }
            return Some(PhysRange::new(start, length));
        }

        let mut pool = self.pool.lock();
        let pool = pool.as_mut()?;
        if pool.next.checked_add(length)? > pool.end {
            return None;
        }
        let start = pool.next;
        pool.next += length;
        Some(PhysRange::new(PhysAddr::new(start), length))
    }

    /// 归还一段 DMA 内存。
    pub fn free(&self, range: PhysRange) {
        if !range.is_empty() {
            self.free.lock().push(range);
        }
    }
}

impl Default for DmaPool {
    fn default() -> Self {
        Self::new()
    }
}

impl DmaOps for DmaPool {
    fn alloc_coherent(&self, len: usize) -> Result<DmaBuffer, DmaError> {
        let range = self.allocate(len).ok_or(DmaError::OutOfMemory)?;
        Ok(DmaBuffer::new(range.start(), range.len() as usize))
    }

    fn free_coherent(&self, buffer: DmaBuffer) {
        self.free(PhysRange::new(buffer.phys(), buffer.len() as u64));
    }

    fn sync_for_device(&self, _buffer: &DmaBuffer) {}

    fn sync_for_cpu(&self, _buffer: &DmaBuffer) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_reuses_and_splits() {
        let pool = DmaPool::new();
        pool.init(PhysRange::new(PhysAddr::new(0x10_0000), 0x4000));
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
        let pool = DmaPool::new();
        pool.init(PhysRange::new(PhysAddr::new(0x20_0000), 0x2000));
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
