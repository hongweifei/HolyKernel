//! 平台电源控制。

/// 电源与重启控制。
pub trait Power: Send + Sync {
    /// 关闭系统，不返回。
    fn shutdown(&self) -> !;

    /// 重启系统，不返回。
    fn reboot(&self) -> !;
}
