//! x86_64 的 DMA 分配器适配。
//!
//! 池算法在 `hk-mem`；本模块只持有平台注册的实例与扩窗回调。

use hk_hal::PhysRange;
use hk_mem::DmaAllocator;
use hk_mem::hmp::{Capacity, CapacityKind};

/// 启动期预留的 DMA 窗口字节数；后续按需扩窗，不再受这个数字限制。
pub const POOL_BYTES: usize = 64 * 1024 * 1024;
/// DMA 总窗口上限：防止 DMA 在内存压力下无限侵占用户容量。
pub const MAX_BYTES: usize = 512 * 1024 * 1024;

/// 全局 DMA 分配器。
pub static DMA: DmaAllocator = DmaAllocator::new();

/// 从 System Capacity 切出一段新的 DMA 窗口。
fn grow(len: usize) -> Option<Capacity> {
    let len = len.div_ceil(hk_hal::PAGE_SIZE) * hk_hal::PAGE_SIZE;
    crate::system::split_capacity(len, CapacityKind::Dma)
}

/// 注册启动期预留的连续 DMA 区域。
pub fn init(id: hk_mem::hmp::CapacityId, range: PhysRange) {
    DMA.set_cache(&crate::cache::CACHE);
    DMA.init(id, range);
    DMA.set_grower(grow, MAX_BYTES);
    hk_mem::hmp::set_frame_scrubber(crate::paging::scrub_phys);
}

/// 从 DMA 分配器分配一段连续内存。
pub fn allocate(len: usize) -> Option<PhysRange> {
    DMA.allocate(len)
}

/// 归还一段 DMA 内存。
pub fn free(range: PhysRange) {
    DMA.free(range);
}
