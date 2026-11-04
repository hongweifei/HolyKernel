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

/// 民用时间（UTC）：年月日时分秒与纳秒。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CivilTime {
    /// 年。
    pub year: u16,
    /// 月，范围 1..=12。
    pub month: u8,
    /// 日，范围 1..=31。
    pub day: u8,
    /// 时，范围 0..=23。
    pub hour: u8,
    /// 分，范围 0..=59。
    pub minute: u8,
    /// 秒，范围 0..=59。
    pub second: u8,
    /// 纳秒，范围 0..1_000_000_000。
    pub nanosecond: u32,
}

impl CivilTime {
    /// 固定编码长度。
    pub const ENCODED_LEN: usize = 12;

    /// 校验字段范围。
    pub fn is_valid(&self) -> bool {
        self.year >= 1
            && (1..=12).contains(&self.month)
            && (1..=31).contains(&self.day)
            && self.hour < 24
            && self.minute < 60
            && self.second < 60
            && self.nanosecond < 1_000_000_000
    }

    /// 编码为 12 字节小端布局。
    pub fn encode(&self, buffer: &mut [u8]) -> Option<usize> {
        if buffer.len() < Self::ENCODED_LEN {
            return None;
        }
        buffer[..2].copy_from_slice(&self.year.to_le_bytes());
        buffer[2] = self.month;
        buffer[3] = self.day;
        buffer[4] = self.hour;
        buffer[5] = self.minute;
        buffer[6] = self.second;
        buffer[7] = 0;
        buffer[8..12].copy_from_slice(&self.nanosecond.to_le_bytes());
        Some(Self::ENCODED_LEN)
    }

    /// 从 12 字节小端布局解码。
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::ENCODED_LEN {
            return None;
        }
        let time = Self {
            year: u16::from_le_bytes([bytes[0], bytes[1]]),
            month: bytes[2],
            day: bytes[3],
            hour: bytes[4],
            minute: bytes[5],
            second: bytes[6],
            nanosecond: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
        };
        time.is_valid().then_some(time)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_civil_time() {
        let time = CivilTime {
            year: 2026,
            month: 11,
            day: 4,
            hour: 12,
            minute: 34,
            second: 56,
            nanosecond: 789,
        };
        let mut buffer = [0_u8; CivilTime::ENCODED_LEN];
        assert_eq!(time.encode(&mut buffer), Some(CivilTime::ENCODED_LEN));
        assert_eq!(CivilTime::decode(&buffer), Some(time));
    }

    #[test]
    fn rejects_invalid_civil_time() {
        let time = CivilTime {
            year: 0,
            ..CivilTime::default()
        };
        assert!(!time.is_valid());
        assert_eq!(CivilTime::decode(&[0; CivilTime::ENCODED_LEN]), None);
    }
}
