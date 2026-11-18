//! 启动计时：TSC 与 8254 PIT 校准。
//!
//! PIT 频率固定为 1.193182 MHz，用它在启动时校准 TSC，得到以毫秒计的启动
//! 耗时。校准只读通道 2，不产生中断。

use core::sync::atomic::{AtomicU64, Ordering};

use crate::port::{inb, outb};

const PIT_CHANNEL2: u16 = 0x42;
const PIT_COMMAND: u16 = 0x43;
const PIT_GATE: u16 = 0x61;
const PIT_FREQUENCY: u64 = 1_193_182;

/// 校准期间等待的 PIT tick 数，约 16.8 ms。
const CALIBRATION_TICKS: u16 = 20_000;

static FREQUENCY: AtomicU64 = AtomicU64::new(0);

/// 返回校准后的 TSC 频率；尚未校准或校准失败时为 0。
pub fn tsc_frequency() -> u64 {
    FREQUENCY.load(Ordering::Acquire)
}

/// 读取时间戳计数器。
pub fn rdtsc() -> u64 {
    // SAFETY: `rdtsc` 只读取计数器，不改变机器状态。
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// 以 PIT 校准 TSC，返回每秒周期数。
pub fn calibrate() -> Option<u64> {
    // SAFETY: PIT 与端口 0x61 在 x86_64 平台上固定存在。
    let saved_gate = unsafe { inb(PIT_GATE) };
    // SAFETY: 同上；配置通道 2 为一次性计数并打开门控。
    unsafe {
        outb(PIT_COMMAND, 0xB0);
        outb(PIT_CHANNEL2, 0xFF);
        outb(PIT_CHANNEL2, 0xFF);
        outb(PIT_GATE, (saved_gate & !0x02) | 0x01);
    }

    let start_pit = read_counter();
    let start_tsc = rdtsc();
    let mut frequency = None;

    for _ in 0..2_000_000 {
        let elapsed = start_pit.wrapping_sub(read_counter());
        if elapsed >= CALIBRATION_TICKS {
            let cycles = rdtsc().saturating_sub(start_tsc);
            frequency =
                Some((u128::from(cycles) * u128::from(PIT_FREQUENCY) / u128::from(elapsed)) as u64);
            break;
        }
    }

    // SAFETY: 恢复端口 0x61 的原值，结束通道 2 的计数。
    unsafe {
        outb(PIT_GATE, saved_gate);
    }
    if let Some(frequency) = frequency {
        FREQUENCY.store(frequency, Ordering::Release);
    }
    frequency
}

/// 以 HPET 校准 TSC，返回每秒周期数。
pub fn calibrate_with_hpet() -> Option<u64> {
    let frequency = crate::hpet::calibrate_tsc()?;
    FREQUENCY.store(frequency, Ordering::Release);
    Some(frequency)
}

/// 返回从 `start` 到现在的毫秒数。
pub fn elapsed_ms(start: u64, frequency: u64) -> u64 {
    let cycles = rdtsc().saturating_sub(start);
    ((u128::from(cycles) * 1000) / u128::from(frequency)) as u64
}

fn read_counter() -> u16 {
    // SAFETY: PIT 端口固定存在；锁存后先读低字节再读高字节。
    unsafe {
        outb(PIT_COMMAND, 0x80);
        let low = inb(PIT_CHANNEL2);
        let high = inb(PIT_CHANNEL2);
        u16::from(low) | (u16::from(high) << 8)
    }
}
