//! 熵源。

/// 随机字节来源。
pub trait Entropy: Send + Sync {
    /// 用随机字节填满缓冲。
    fn fill(&self, buffer: &mut [u8]);
}
