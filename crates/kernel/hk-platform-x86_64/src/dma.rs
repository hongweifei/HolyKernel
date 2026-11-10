//! x86_64 的 DMA 分配器适配。
//!
//! 池算法在 `hk-mem`；本模块只持有平台启动期注册的实例。

use hk_hal::PhysRange;
use hk_mem::DmaAllocator;

/// DMA 分配器字节数。
pub const POOL_BYTES: usize = 64 * 1024 * 1024;

/// 全局 DMA 分配器。
pub static DMA: DmaAllocator = DmaAllocator::new();

/// 注册启动期预留的连续 DMA 区域。
pub fn init(id: hk_mem::hmp::CapacityId, range: PhysRange) {
    DMA.set_cache(&crate::cache::CACHE);
    DMA.init(id, range);
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
