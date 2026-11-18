//! HPET 时钟：作为 TSC 不可用时的单调时间回退。
//!
//! HPET 由 ACPI HPET 表描述；驱动只读主计数器，周期取自能力寄存器。

use core::sync::atomic::{AtomicU64, Ordering};

use hk_mmio::Mmio;

const GENERAL_CONFIG: usize = 0x10;
const MAIN_COUNTER: usize = 0xF0;
const TIMER_CONFIG: usize = 0x100;
const TIMER_COMPARATOR: usize = 0x108;

static BASE: AtomicU64 = AtomicU64::new(0);
static FREQUENCY: AtomicU64 = AtomicU64::new(0);
static PERIODIC_TICKS: AtomicU64 = AtomicU64::new(0);

/// 用已映射的 HPET 寄存器页初始化时钟。
pub fn init(virt_base: u64) {
    if virt_base == 0 {
        return;
    }
    // SAFETY: 调用方保证该地址映射为 HPET 设备页。
    let mmio = unsafe { Mmio::new(virt_base as usize) };
    let capability = mmio.read_u64(0);
    let period_fs = capability >> 32;
    if period_fs == 0 {
        return;
    }
    let frequency = 1_000_000_000_000_000_u64 / period_fs;
    if frequency == 0 {
        return;
    }
    // 停止计数、清零主计数器后重新启用。
    let config = mmio.read_u64(GENERAL_CONFIG);
    // SAFETY: HPET 寄存器页已映射，偏移在页内。
    unsafe {
        mmio.write_u64(GENERAL_CONFIG, config & !1);
        mmio.write_u64(MAIN_COUNTER, 0);
        mmio.write_u64(GENERAL_CONFIG, config | 1);
    }
    BASE.store(virt_base, Ordering::Release);
    FREQUENCY.store(frequency, Ordering::Release);
}

/// 是否已成功初始化。
pub fn available() -> bool {
    BASE.load(Ordering::Acquire) != 0
}

/// 返回 HPET 计数频率；不可用时为 0。
pub fn frequency() -> u64 {
    FREQUENCY.load(Ordering::Acquire)
}

/// 用 HPET 作为参考校准 TSC，返回每秒周期数。
pub fn calibrate_tsc() -> Option<u64> {
    let frequency = frequency();
    if frequency == 0 {
        return None;
    }
    let window = frequency / 100;
    if window == 0 {
        return None;
    }
    let start_hpet = now();
    let start_tsc = crate::timing::rdtsc();
    while now().wrapping_sub(start_hpet) < window {
        core::hint::spin_loop();
    }
    let hpet_ticks = now().wrapping_sub(start_hpet);
    let cycles = crate::timing::rdtsc().saturating_sub(start_tsc);
    if hpet_ticks == 0 || cycles == 0 {
        return None;
    }
    Some((u128::from(cycles) * u128::from(frequency) / u128::from(hpet_ticks)) as u64)
}

/// 启动 timer0 周期中断（约 1 s），返回用于 IOAPIC 路由的 GSI。
///
/// 路由号来自 capability 的 timer0 路由能力位；没有可用路由时返回 `None`。
pub fn start_periodic(gsi_base: u32) -> Option<u32> {
    let base = BASE.load(Ordering::Acquire);
    let frequency = FREQUENCY.load(Ordering::Acquire);
    if base == 0 || frequency == 0 {
        return None;
    }
    // SAFETY: 基址由 `init` 记录的 HPET 页。
    let mmio = unsafe { Mmio::new(base as usize) };
    let capability = mmio.read_u64(0);
    let timers = ((capability >> 8) & 0x1F) + 1;
    if timers < 1 {
        return None;
    }
    let routes = (capability >> 32) & 0x1F;
    if routes == 0 {
        return None;
    }
    let route = routes.trailing_zeros();
    let config = mmio.read_u64(TIMER_CONFIG);
    let disabled = config & !((1 << 2) | (1 << 3) | (0x1F << 9));
    // SAFETY: HPET timer 寄存器页已映射；先停用、写周期、再启用。
    unsafe {
        mmio.write_u64(TIMER_CONFIG, disabled);
        mmio.write_u64(TIMER_COMPARATOR, frequency.max(1));
        mmio.write_u64(
            TIMER_CONFIG,
            disabled | (u64::from(route) << 9) | (1 << 2) | (1 << 3),
        );
    }
    Some(gsi_base.saturating_add(route))
}

/// 记录一次 HPET 周期中断。
pub fn note_periodic() {
    PERIODIC_TICKS.fetch_add(1, Ordering::Relaxed);
}

/// 返回 HPET 周期中断计数。
pub fn periodic_ticks() -> u64 {
    PERIODIC_TICKS.load(Ordering::Acquire)
}

/// 读取主计数器；不可用时为 0。
pub fn now() -> u64 {
    let base = BASE.load(Ordering::Acquire);
    if base == 0 {
        return 0;
    }
    // SAFETY: 基址由 `init` 记录的 HPET 页。
    unsafe { Mmio::new(base as usize).read_u64(MAIN_COUNTER) }
}
