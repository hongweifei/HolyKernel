//! 契约时基。
//!
//! Cadence 的预算、周期、截止期与延迟目标使用固定 1 MHz 时基（1 tick = 1 µs），
//! 与平台硬件 tick 频率无关。Beat 在编程一次性定时器时按平台频率换算。

use hk_base::Duration;

/// 契约时基：每秒 tick 数。
pub const TIME_BASE_HZ: u64 = 1_000_000;

/// 以微秒构造契约时长。
pub const fn micros(value: u64) -> Duration {
    Duration::from_ticks(value)
}

/// 以毫秒构造契约时长。
pub const fn millis(value: u64) -> Duration {
    Duration::from_ticks(value.saturating_mul(1_000))
}

/// 把契约时长换算为平台 tick。
pub const fn to_platform(duration: Duration, frequency: u64) -> Duration {
    Duration::from_nanos(duration.as_nanos(TIME_BASE_HZ), frequency)
}

/// 把平台 tick 换算为契约时长。
pub const fn from_platform(duration: Duration, frequency: u64) -> Duration {
    Duration::from_nanos(duration.as_nanos(frequency), TIME_BASE_HZ)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_between_contract_and_platform_time() {
        assert_eq!(millis(3), micros(3_000));
        assert_eq!(millis(3).as_nanos(TIME_BASE_HZ), 3_000_000);

        let platform = to_platform(millis(3), 10_000_000);
        assert_eq!(from_platform(platform, 10_000_000), millis(3));
    }
}
