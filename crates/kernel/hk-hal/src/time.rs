//! 单调时钟与一次性定时器。
//!
//! 时间值与民用时间由 `hk-base` 定义并在内核侧重导出。

pub use hk_base::{CivilClock, CivilTime, Duration, Moment};

/// 单调时钟与一次性定时器。
pub trait Timer: Send + Sync {
    /// 返回每秒 tick 数。
    fn frequency(&self) -> u64;

    /// 读取当前单调时间。
    fn now(&self) -> Moment;

    /// 编程一次性定时器；到期的中断由平台中断控制器投递。
    fn set_oneshot(&self, deadline: Moment);

    /// 取消一次性定时器。
    fn cancel(&self);
}
