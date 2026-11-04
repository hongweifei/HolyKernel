//! DMA 缓存维护。

use crate::addr::PhysRange;

/// 物理区间缓存维护接口。
pub trait CacheOps: Send + Sync {
    /// 在设备读取前把缓存写回内存。
    fn clean(&self, range: PhysRange);

    /// 在 CPU 读取前使缓存失效。
    fn invalidate(&self, range: PhysRange);

    /// 发出完整的缓存屏障。
    fn barrier(&self);
}

/// 一致性平台上的空实现。
pub struct NoCacheOps;

impl CacheOps for NoCacheOps {
    fn clean(&self, _range: PhysRange) {}

    fn invalidate(&self, _range: PhysRange) {}

    fn barrier(&self) {}
}
