//! 单调时间与定时器。

/// 单调时间点，单位为硬件 tick。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Moment(u64);

impl Moment {
    /// 以 tick 构造时间点。
    pub const fn from_ticks(ticks: u64) -> Self {
        Self(ticks)
    }

    /// 返回 tick 数值。
    pub const fn ticks(self) -> u64 {
        self.0
    }

    /// 增加一段时长，溢出时返回 `None`。
    pub const fn checked_add(self, duration: Duration) -> Option<Self> {
        match self.0.checked_add(duration.ticks()) {
            Some(ticks) => Some(Self(ticks)),
            None => None,
        }
    }

    /// 增加一段时长，溢出时饱和到 `u64::MAX`。
    pub const fn saturating_add(self, duration: Duration) -> Self {
        Self(self.0.saturating_add(duration.ticks()))
    }

    /// 返回与更早时间点的距离。
    pub const fn duration_since(self, earlier: Self) -> Duration {
        Duration::from_ticks(self.0.saturating_sub(earlier.0))
    }
}

/// 时长，单位为硬件 tick。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Duration(u64);

impl Duration {
    /// 零时长。
    pub const ZERO: Self = Self(0);

    /// 以 tick 构造时长。
    pub const fn from_ticks(ticks: u64) -> Self {
        Self(ticks)
    }

    /// 返回 tick 数值。
    pub const fn ticks(self) -> u64 {
        self.0
    }

    /// 判断是否为零时长。
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// 把纳秒换算为时长；频率为每秒 tick 数。
    pub const fn from_nanos(nanos: u64, frequency: u64) -> Self {
        Self(nanos.saturating_mul(frequency) / 1_000_000_000)
    }

    /// 把时长换算为纳秒；频率为每秒 tick 数。
    pub const fn as_nanos(self, frequency: u64) -> u64 {
        self.0.saturating_mul(1_000_000_000) / frequency
    }
}

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
