//! HPET 时钟：作为 TSC 不可用时的单调时间回退。
//!
//! HPET 由 ACPI HPET 表描述；驱动只读主计数器，周期取自能力寄存器。

use core::sync::atomic::{AtomicU64, Ordering};

use hk_mmio::Mmio;

const GENERAL_CONFIG: usize = 0x10;
const MAIN_COUNTER: usize = 0xF0;

static BASE: AtomicU64 = AtomicU64::new(0);
static FREQUENCY: AtomicU64 = AtomicU64::new(0);

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

/// 读取主计数器；不可用时为 0。
pub fn now() -> u64 {
    let base = BASE.load(Ordering::Acquire);
    if base == 0 {
        return 0;
    }
    // SAFETY: 基址由 `init` 记录的 HPET 页。
    unsafe { Mmio::new(base as usize).read_u64(MAIN_COUNTER) }
}
