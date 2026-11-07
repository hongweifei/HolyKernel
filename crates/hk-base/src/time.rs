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

    /// 增加一段时长，返回新的民用时间。
    pub fn add_seconds(self, seconds: u64, nanoseconds: u32) -> Self {
        let total_nanos = u64::from(self.nanosecond) + u64::from(nanoseconds);
        let carry = total_nanos / 1_000_000_000;
        let nanosecond = (total_nanos % 1_000_000_000) as u32;
        let day_seconds =
            u64::from(self.hour) * 3_600 + u64::from(self.minute) * 60 + u64::from(self.second);
        let total = day_seconds + seconds + carry;
        let days = days_from_civil(
            i64::from(self.year),
            i64::from(self.month),
            i64::from(self.day),
        ) + (total / 86_400) as i64;
        let within = total % 86_400;
        let (year, month, day) = civil_from_days(days);
        Self {
            year: year as u16,
            month: month as u8,
            day: day as u8,
            hour: (within / 3_600) as u8,
            minute: ((within % 3_600) / 60) as u8,
            second: (within % 60) as u8,
            nanosecond,
        }
    }

    /// 返回相对更早民用时间的秒数差；纳秒由调用方单独处理。
    pub fn seconds_since(self, earlier: Self) -> i64 {
        let days = days_from_civil(
            i64::from(self.year),
            i64::from(self.month),
            i64::from(self.day),
        ) - days_from_civil(
            i64::from(earlier.year),
            i64::from(earlier.month),
            i64::from(earlier.day),
        );
        let seconds =
            i64::from(self.hour) * 3_600 + i64::from(self.minute) * 60 + i64::from(self.second)
                - (i64::from(earlier.hour) * 3_600
                    + i64::from(earlier.minute) * 60
                    + i64::from(earlier.second));
        days * 86_400 + seconds
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

/// 民用时间锚点：启动时记录 `(CivilTime, tick)`，之后由单调 tick 推进。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CivilClock {
    anchor: CivilTime,
    anchor_ticks: u64,
    frequency: u64,
}

impl CivilClock {
    /// 以民用时间锚点、锚点 tick 与每秒 tick 数构造。
    pub const fn new(anchor: CivilTime, anchor_ticks: u64, frequency: u64) -> Self {
        Self {
            anchor,
            anchor_ticks,
            frequency,
        }
    }

    /// 返回锚点民用时间。
    pub const fn anchor(&self) -> CivilTime {
        self.anchor
    }

    /// 按当前 tick 计算民用时间。
    pub fn now(&self, ticks: u64) -> CivilTime {
        if self.frequency == 0 {
            return self.anchor;
        }
        let elapsed = u128::from(ticks.saturating_sub(self.anchor_ticks));
        let nanos = elapsed * 1_000_000_000 / u128::from(self.frequency);
        let seconds = (nanos / 1_000_000_000) as u64;
        let nanoseconds = (nanos % 1_000_000_000) as u32;
        self.anchor.add_seconds(seconds, nanoseconds)
    }
}

/// 公历日期到 1970-01-01 的天数（仅用于内部日历运算）。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let shifted_month = (month + 9) % 12;
    let doy = (153 * shifted_month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        (year + 1, month, day)
    } else {
        (year, month, day)
    }
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
    fn advances_civil_time_across_boundaries() {
        let time = CivilTime {
            year: 2026,
            month: 12,
            day: 31,
            hour: 23,
            minute: 59,
            second: 59,
            nanosecond: 500_000_000,
        };
        let next = time.add_seconds(1, 600_000_000);
        assert_eq!(
            next,
            CivilTime {
                year: 2027,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 1,
                nanosecond: 100_000_000,
            }
        );
    }

    #[test]
    fn measures_seconds_since_earlier_time() {
        let later = CivilTime {
            year: 2026,
            month: 11,
            day: 5,
            hour: 0,
            minute: 0,
            second: 10,
            nanosecond: 0,
        };
        let earlier = CivilTime {
            year: 2026,
            month: 11,
            day: 4,
            hour: 23,
            minute: 59,
            second: 50,
            nanosecond: 0,
        };
        assert_eq!(later.seconds_since(earlier), 20);
    }

    #[test]
    fn civil_clock_advances_with_ticks() {
        let anchor = CivilTime {
            year: 2026,
            month: 11,
            day: 4,
            hour: 12,
            minute: 0,
            second: 0,
            nanosecond: 0,
        };
        let clock = CivilClock::new(anchor, 1_000, 1_000_000_000);
        assert_eq!(clock.now(1_000), anchor);
        let advanced = clock.now(1_500_001_000);
        assert_eq!(advanced.second, 1);
        assert_eq!(advanced.nanosecond, 500_000_000);
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
