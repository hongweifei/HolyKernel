//! x86_64 的 DMA 池适配。
//!
//! 池算法在 `hk-mem`；本模块只持有平台启动期注册的实例。

use hk_hal::PhysRange;
use hk_mem::DmaPool;

/// 全局 DMA 池。
pub static DMA: DmaPool = DmaPool::new();

/// 注册启动期预留的连续 DMA 区域。
pub fn init(range: PhysRange) {
    DMA.set_cache(&crate::cache::CACHE);
    DMA.init(range);
    hk_mem::hmp::set_backing_releaser(|phys, len| {
        DMA.free(PhysRange::new(phys, len as u64));
    });
}

/// 从 DMA 池分配一段连续内存。
pub fn allocate(len: usize) -> Option<PhysRange> {
    DMA.allocate(len)
}

/// 归还一段 DMA 内存。
pub fn free(range: PhysRange) {
    DMA.free(range);
}
