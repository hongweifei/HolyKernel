//! Hosted 平台配置。

/// Hosted 平台配置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostedConfig {
    /// 逻辑 CPU 数量，至少为 1。
    pub cpu_count: usize,
    /// 确定性模拟的随机种子。
    pub seed: u64,
    /// 是否使用虚拟时钟，按显式推进的时间执行。
    pub deterministic: bool,
}

impl HostedConfig {
    /// 构造配置。
    pub const fn new(cpu_count: usize, seed: u64, deterministic: bool) -> Self {
        Self {
            cpu_count,
            seed,
            deterministic,
        }
    }
}

impl Default for HostedConfig {
    fn default() -> Self {
        Self::new(1, 0, false)
    }
}
