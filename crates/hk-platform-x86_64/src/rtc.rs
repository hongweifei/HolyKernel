//! x86_64 CMOS RTC：读取民用时间。
//!
//! QEMU 默认把 RTC 设为 UTC。寄存器可能是 BCD 或二进制、12 或 24 小时制，
//! 都由状态寄存器 B 描述；读值时等待更新周期结束并连续读取两次确认稳定。

use hk_hal::CivilTime;
use spin::Mutex;

use crate::port::{inb, outb};

/// CMOS 索引端口。
const CMOS_INDEX: u16 = 0x70;
/// CMOS 数据端口。
const CMOS_DATA: u16 = 0x71;

const REG_SECOND: u8 = 0x00;
const REG_MINUTE: u8 = 0x02;
const REG_HOUR: u8 = 0x04;
const REG_DAY: u8 = 0x07;
const REG_MONTH: u8 = 0x08;
const REG_YEAR: u8 = 0x09;
const REG_STATUS_A: u8 = 0x0A;
const REG_STATUS_B: u8 = 0x0B;
const REG_CENTURY: u8 = 0x32;

static RTC_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, PartialEq, Eq)]
struct Snapshot {
    second: u8,
    minute: u8,
    hour: u8,
    day: u8,
    month: u8,
    year: u8,
    century: u8,
    status_b: u8,
}

/// 读取当前民用时间；RTC 不可用或时间非法时返回 `None`。
pub fn civil_time() -> Option<CivilTime> {
    let _guard = RTC_LOCK.lock();
    wait_ready();
    let mut last = snapshot();
    for _ in 0..16 {
        wait_ready();
        let current = snapshot();
        if current == last {
            return to_civil(current);
        }
        last = current;
    }
    to_civil(last)
}

fn wait_ready() {
    // 状态 A 的 bit 7 为更新中；最多等待一段时间后照常读取。
    for _ in 0..1_000_000 {
        if read(REG_STATUS_A) & 0x80 == 0 {
            return;
        }
    }
}

fn snapshot() -> Snapshot {
    Snapshot {
        second: read(REG_SECOND),
        minute: read(REG_MINUTE),
        hour: read(REG_HOUR),
        day: read(REG_DAY),
        month: read(REG_MONTH),
        year: read(REG_YEAR),
        century: read(REG_CENTURY),
        status_b: read(REG_STATUS_B),
    }
}

fn read(register: u8) -> u8 {
    // SAFETY: 0x70/0x71 是 CMOS 索引与数据端口，内核运行期可访问。
    unsafe {
        outb(CMOS_INDEX, register);
        inb(CMOS_DATA)
    }
}

fn to_civil(snapshot: Snapshot) -> Option<CivilTime> {
    let binary = snapshot.status_b & 0x04 != 0;
    let hour24 = snapshot.status_b & 0x02 != 0;

    let mut hour = u32::from(decode(snapshot.hour & 0x7F, binary));
    if !hour24 {
        let afternoon = snapshot.hour & 0x80 != 0;
        hour %= 12;
        if afternoon {
            hour += 12;
        }
    }
    let year = u16::from(decode(snapshot.year, binary));
    let century = decode(snapshot.century, binary);
    let full_year = if century == 0 || century == 0xFF {
        if year < 70 { 2000 + year } else { 1900 + year }
    } else {
        u16::from(century) * 100 + year
    };

    let time = CivilTime {
        year: full_year,
        month: decode(snapshot.month, binary),
        day: decode(snapshot.day, binary),
        hour: hour as u8,
        minute: decode(snapshot.minute, binary),
        second: decode(snapshot.second, binary),
        nanosecond: 0,
    };
    time.is_valid().then_some(time)
}

fn decode(value: u8, binary: bool) -> u8 {
    if binary {
        value
    } else {
        (value & 0x0F) + (value >> 4) * 10
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_bcd_and_binary() {
        assert_eq!(decode(0x26, false), 26);
        assert_eq!(decode(0x59, false), 59);
        assert_eq!(decode(0x26, true), 0x26);
    }

    #[test]
    fn converts_snapshot_to_civil_time() {
        let snapshot = Snapshot {
            second: 0x56,
            minute: 0x34,
            hour: 0x12,
            day: 0x04,
            month: 0x11,
            year: 0x26,
            century: 0x20,
            status_b: 0x02,
        };
        assert_eq!(
            to_civil(snapshot),
            Some(CivilTime {
                year: 2026,
                month: 11,
                day: 4,
                hour: 12,
                minute: 34,
                second: 56,
                nanosecond: 0,
            })
        );
    }

    #[test]
    fn rejects_invalid_date() {
        let snapshot = Snapshot {
            second: 0,
            minute: 0,
            hour: 0,
            day: 0,
            month: 0x13,
            year: 0x26,
            century: 0x20,
            status_b: 0x02,
        };
        assert_eq!(to_civil(snapshot), None);
    }
}
