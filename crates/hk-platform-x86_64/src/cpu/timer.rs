//! LAPIC 定时器与调度 tick。

use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use super::apic::{lapic_read, lapic_write};
use super::*;

static TIMER_TICKS: AtomicU64 = AtomicU64::new(0);
static UPTIME_TICKS: AtomicU64 = AtomicU64::new(0);
static TIMER_TARGET: AtomicU64 = AtomicU64::new(0);
static TICK_COUNT: AtomicU32 = AtomicU32::new(TICK_INITIAL_COUNT);
static TICK_ARMED: AtomicBool = AtomicBool::new(false);
/// 当前 CPU 的调度 tick 是否处于投递状态。
static TICK_ENABLED: AtomicBool = AtomicBool::new(false);
/// 启动周期定时器，目标中断次数为 `target`。
pub fn arm_timer(target: u64) {
    TICK_ARMED.store(true, Ordering::SeqCst);
    TIMER_TICKS.store(0, Ordering::SeqCst);
    TIMER_TARGET.store(target, Ordering::SeqCst);

    lapic_write(LAPIC_LVT_TIMER, LVT_PERIODIC | TIMER_VECTOR as u32);
    lapic_write(LAPIC_TIMER_INITIAL, TIMER_INITIAL_COUNT);

    // SAFETY: 开中断允许定时器中断进入；IDT 与本地 APIC 已就绪。
    unsafe { asm!("sti", options(nomem, nostack)) };
}

/// 停止定时器并关中断。
pub fn disarm_timer() {
    TICK_ARMED.store(false, Ordering::SeqCst);
    // SAFETY: 关中断后写 LVT 屏蔽位，避免仍在途的中断再次进入。
    unsafe { asm!("cli", options(nomem, nostack)) };
    lapic_write(LAPIC_LVT_TIMER, LVT_MASKED | TIMER_VECTOR as u32);
}

/// 启动周期性调度 tick。
///
/// 首次调用编程 LAPIC 并开始计数；之后的调用只解除屏蔽，不重置计数。调度器
/// 会在每个预算切片之间停止并重新启动 tick，若每次都重写初始计数，短于一个
/// tick 周期的切片就会反复清零定时器，导致抢占 tick 停止投递。
pub fn start_tick() {
    if !TICK_ARMED.swap(true, Ordering::SeqCst) {
        TIMER_TICKS.store(0, Ordering::SeqCst);
        lapic_write(LAPIC_TIMER_INITIAL, TICK_COUNT.load(Ordering::SeqCst));
    }
    TIMER_TARGET.store(u64::MAX, Ordering::SeqCst);
    lapic_write(LAPIC_LVT_TIMER, LVT_PERIODIC | TIMER_VECTOR as u32);
    TICK_ENABLED.store(true, Ordering::SeqCst);

    // SAFETY: 中断表与本地 APIC 已就绪；打开中断允许 tick 投递。
    unsafe { asm!("sti", options(nomem, nostack)) };
}

/// 启动 AP 的周期性调度 tick，复用 BSP 校准的计数，不重置全局 tick 计数。
pub fn start_ap_tick() {
    lapic_write(LAPIC_TIMER_DIVIDE, 0x03);
    lapic_write(LAPIC_LVT_TIMER, LVT_PERIODIC | TIMER_VECTOR as u32);
    lapic_write(LAPIC_TIMER_INITIAL, TICK_COUNT.load(Ordering::SeqCst));
    // SAFETY: 中断表与本地 APIC 已就绪；打开中断允许 tick 投递。
    unsafe {
        asm!("sti", options(nomem, nostack));
    }
}

/// 重新装载 tick 计数并启用定时器。
///
/// shell 命令在调度循环之外执行，`stop_tick` 已屏蔽本地定时器；嵌套运行服务前
/// 需要重新写入初始计数，否则解除屏蔽后可能不再产生周期性中断。
pub fn rearm_tick() {
    TICK_ARMED.store(true, Ordering::SeqCst);
    TIMER_TARGET.store(u64::MAX, Ordering::SeqCst);
    lapic_write(LAPIC_TIMER_INITIAL, TICK_COUNT.load(Ordering::SeqCst));
    lapic_write(LAPIC_LVT_TIMER, LVT_PERIODIC | TIMER_VECTOR as u32);
    TICK_ENABLED.store(true, Ordering::SeqCst);

    // SAFETY: 中断表与本地 APIC 已就绪；打开中断允许 tick 投递。
    unsafe { asm!("sti", options(nomem, nostack)) };
}

/// 停止调度 tick 并关中断。
pub fn stop_tick() {
    // SAFETY: 关中断后写 LVT 屏蔽位，避免仍在途的中断再次进入。
    unsafe { asm!("cli", options(nomem, nostack)) };
    lapic_write(LAPIC_LVT_TIMER, LVT_MASKED | TIMER_VECTOR as u32);
    TICK_ENABLED.store(false, Ordering::SeqCst);
}

/// 调度 tick 当前是否在投递。
pub fn tick_enabled() -> bool {
    TICK_ENABLED.load(Ordering::SeqCst)
}

/// 用 TSC 频率校准 LAPIC 定时器，把调度 tick 设为约 10 ms。
pub fn calibrate_tick(tsc_frequency: u64) {
    let window = tsc_frequency / 100;
    if window == 0 {
        return;
    }

    lapic_write(LAPIC_TIMER_DIVIDE, 0x03);
    lapic_write(LAPIC_LVT_TIMER, LVT_MASKED | TIMER_VECTOR as u32);
    lapic_write(LAPIC_TIMER_INITIAL, u32::MAX);

    let start_tsc = crate::timing::rdtsc();
    let start_count = lapic_read(LAPIC_TIMER_CURRENT);
    while crate::timing::rdtsc().saturating_sub(start_tsc) < window {
        core::hint::spin_loop();
    }
    let end_count = lapic_read(LAPIC_TIMER_CURRENT);
    let counts = u64::from(start_count.wrapping_sub(end_count));

    lapic_write(LAPIC_TIMER_INITIAL, 0);
    lapic_write(LAPIC_LVT_TIMER, LVT_MASKED | TIMER_VECTOR as u32);

    if counts > 0 {
        TICK_COUNT.store(counts.min(u64::from(u32::MAX)) as u32, Ordering::SeqCst);
    }
}

/// 记录一次定时器中断。
pub(super) fn note_tick() {
    TIMER_TICKS.fetch_add(1, Ordering::SeqCst);
    UPTIME_TICKS.fetch_add(1, Ordering::SeqCst);
}

/// 返回已记录的定时器中断次数。
pub fn timer_ticks() -> u64 {
    TIMER_TICKS.load(Ordering::SeqCst)
}

/// 返回自启动以来的累计定时器中断次数；不会因重新启动调度 tick 而清零。
pub fn uptime_ticks() -> u64 {
    UPTIME_TICKS.load(Ordering::SeqCst)
}

/// 判断定时器是否达到目标次数。
pub fn timer_finished() -> bool {
    TIMER_TICKS.load(Ordering::SeqCst) >= TIMER_TARGET.load(Ordering::SeqCst)
}
