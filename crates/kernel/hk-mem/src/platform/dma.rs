//! 连续 DMA 分配器：底层使用一个或多个 HMP Capacity 窗口。
//!
//! 启动期只登记第一段窗口；后续窗口在分配失败时通过平台注册的回调从
//! System Capacity 动态切出。窗口数量与总上限有界，避免 DMA 在压力下无限
//! 侵占用户内存。

use crate::hmp::{Capacity, CapacityId, CapacityKind};
use hk_hal::{CacheOps, DmaBuffer, DmaError, DmaOps, PhysRange};
use spin::Mutex;

/// 同时持有的 DMA 窗口上限。
const MAX_WINDOWS: usize = 32;

/// 扩窗回调：请求一段连续 DMA 窗口，由平台从 System Capacity 切出。
pub type DmaGrowFn = fn(usize) -> Option<Capacity>;

/// 启动期预留、可分配与归还的连续 DMA 分配器。
pub struct DmaAllocator {
    capacities: Mutex<[Option<Capacity>; MAX_WINDOWS]>,
    grow: Mutex<Option<DmaGrowFn>>,
    max_bytes: Mutex<usize>,
    cache: Mutex<Option<&'static dyn CacheOps>>,
}

impl DmaAllocator {
    /// 构造空分配器。
    pub const fn new() -> Self {
        Self {
            capacities: Mutex::new([const { None }; MAX_WINDOWS]),
            grow: Mutex::new(None),
            max_bytes: Mutex::new(usize::MAX),
            cache: Mutex::new(None),
        }
    }

    /// 设置 DMA 缓存维护后端。
    pub fn set_cache(&self, cache: &'static dyn CacheOps) {
        *self.cache.lock() = Some(cache);
    }

    /// 注册扩窗回调；平台在 System Capacity 建立后调用。
    pub fn set_grower(&self, grow: DmaGrowFn, max_bytes: usize) {
        *self.grow.lock() = Some(grow);
        *self.max_bytes.lock() = max_bytes;
    }

    /// 注册启动期预留的连续 DMA 区域。
    ///
    /// `id` 用于在后端归因；调用方应先通过 `FrameBank::reserve_for` 用同一
    /// 个 `id` 预留区间，这里只把描述交给池。未注册后端时容量使用自带的
    /// 窗口分配器。
    pub fn init(&self, id: CapacityId, range: PhysRange) {
        let capacity = Capacity::new(id, range, hk_hal::PAGE_SIZE, CapacityKind::Dma, 0).ok();
        let mut capacities = self.capacities.lock();
        capacities[0] = capacity;
    }

    /// 返回当前登记的总窗口字节数。
    pub fn capacity_bytes(&self) -> usize {
        self.capacities
            .lock()
            .iter()
            .flatten()
            .map(|capacity| capacity.limit() as usize)
            .sum()
    }

    /// 分配一段连续内存，长度按页向上取整。
    pub fn allocate(&self, len: usize) -> Option<PhysRange> {
        self.allocate_with_node(len, None)
    }

    /// 按 NUMA 节点偏好分配一段连续内存。
    pub fn allocate_with_node(&self, len: usize, node: Option<u8>) -> Option<PhysRange> {
        // 先尝试已有窗口。
        {
            let mut capacities = self.capacities.lock();
            for capacity in capacities.iter_mut().flatten() {
                if let Ok(frame) = capacity.allocate_on(len, node) {
                    return Some(PhysRange::new(frame.phys(), frame.len() as u64));
                }
            }
        }
        let grow = *self.grow.lock();
        let grow = grow?;
        let max = *self.max_bytes.lock();
        let capacity = grow(len)?;
        {
            let mut capacities = self.capacities.lock();
            let total: usize = capacities
                .iter()
                .flatten()
                .map(|capacity| capacity.limit() as usize)
                .sum();
            if total.saturating_add(capacity.limit() as usize) > max {
                return None;
            }
            let slot = capacities.iter_mut().find(|slot| slot.is_none())?;
            *slot = Some(capacity);
        }
        let mut capacities = self.capacities.lock();
        for capacity in capacities.iter_mut().flatten() {
            if let Ok(frame) = capacity.allocate_on(len, node) {
                return Some(PhysRange::new(frame.phys(), frame.len() as u64));
            }
        }
        None
    }

    /// 归还一段 DMA 内存；清空按需扩展的窗口并并回父容量。
    pub fn free(&self, range: PhysRange) {
        if range.is_empty() {
            return;
        }
        let mut released = false;
        {
            let mut capacities = self.capacities.lock();
            for capacity in capacities.iter_mut().flatten() {
                if capacity
                    .release(range.start(), range.len() as usize)
                    .is_ok()
                {
                    released = true;
                    break;
                }
            }
        }
        if released {
            self.trim();
        }
    }

    /// 释放已清空的按需窗口；启动窗口保留。
    fn trim(&self) {
        let mut released = alloc::vec::Vec::new();
        {
            let mut capacities = self.capacities.lock();
            for slot in capacities.iter_mut().skip(1) {
                if slot.as_ref().is_some_and(|capacity| capacity.used() == 0) {
                    released.extend(slot.take());
                }
            }
        }
        // `Capacity::drop` 把窗口与配额并回父容量。
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
#[allow(clippy::wildcard_imports)]
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
    fn grows_when_initial_window_is_exhausted() {
        fn grow(len: usize) -> Option<Capacity> {
            Capacity::new(
                CapacityId::next(),
                PhysRange::new(PhysAddr::new(0x30_0000), len as u64),
                hk_hal::PAGE_SIZE,
                CapacityKind::Dma,
                0,
            )
            .ok()
        }
        let pool = DmaAllocator::new();
        pool.init(
            CapacityId::next(),
            PhysRange::new(PhysAddr::new(0x20_0000), 0x1000),
        );
        pool.set_grower(grow, 0x20_0000);
        assert!(pool.allocate(4096).is_some());
        let grown = pool.allocate(0x2000).expect("扩窗后的分配");
        assert_eq!(grown.start(), PhysAddr::new(0x30_0000));
        assert_eq!(pool.capacity_bytes(), 0x1000 + 0x2000);
        pool.free(grown);
        assert_eq!(pool.capacity_bytes(), 0x1000, "清空的扩窗并回父容量");
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
