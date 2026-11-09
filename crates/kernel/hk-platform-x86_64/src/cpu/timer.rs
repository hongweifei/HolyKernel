//! LAPIC 定时器与调度 tick。

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use x86_64::instructions::interrupts;

use super::apic::{lapic_read, lapic_write};
use super::*;

static TIMER_TICKS: AtomicU64 = AtomicU64::new(0);
static UPTIME_TICKS: AtomicU64 = AtomicU64::new(0);
static TIMER_TARGET: AtomicU64 = AtomicU64::new(0);
static TICK_COUNT: AtomicU32 = AtomicU32::new(TICK_INITIAL_COUNT);
static TICK_ARMED: AtomicBool = AtomicBool::new(false);
/// 当前 CPU 的调度 tick 是否处于投递状态。
static TICK_ENABLED: AtomicBool = AtomicBool::new(false);
/// 每 CPU 是否已编程一次性唤醒。
static ONESHOT: AtomicU32 = AtomicU32::new(0);
/// 上次 watchdog tick 的契约时刻（微秒）。
static LAST_TICK_US: AtomicU64 = AtomicU64::new(0);
/// 启动周期定时器，目标中断次数为 `target`。
pub fn arm_timer(target: u64) {
    TICK_ARMED.store(true, Ordering::SeqCst);
    TIMER_TICKS.store(0, Ordering::SeqCst);
    TIMER_TARGET.store(target, Ordering::SeqCst);

    lapic_write(LAPIC_LVT_TIMER, LVT_PERIODIC | TIMER_VECTOR as u32);
    lapic_write(LAPIC_TIMER_INITIAL, TIMER_INITIAL_COUNT);

    // 开中断允许定时器中断进入；IDT 与本地 APIC 已就绪。
    interrupts::enable();
}

/// 停止定时器并关中断。
pub fn disarm_timer() {
    TICK_ARMED.store(false, Ordering::SeqCst);
    // 关中断后写 LVT 屏蔽位，避免仍在途的中断再次进入。
    interrupts::disable();
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

    // 中断表与本地 APIC 已就绪；打开中断允许 tick 投递。
    interrupts::enable();
}

/// 启动 AP 的周期性调度 tick，复用 BSP 校准的计数，不重置全局 tick 计数。
pub fn start_ap_tick() {
    lapic_write(LAPIC_TIMER_DIVIDE, 0x03);
    lapic_write(LAPIC_LVT_TIMER, LVT_PERIODIC | TIMER_VECTOR as u32);
    lapic_write(LAPIC_TIMER_INITIAL, TICK_COUNT.load(Ordering::SeqCst));
    // 中断表与本地 APIC 已就绪；打开中断允许 tick 投递。
    interrupts::enable();
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

    // 中断表与本地 APIC 已就绪；打开中断允许 tick 投递。
    interrupts::enable();
}

/// 停止调度 tick 并关中断。
pub fn stop_tick() {
    // 关中断后写 LVT 屏蔽位，避免仍在途的中断再次进入。
    interrupts::disable();
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
    LAST_TICK_US.store(now_us(), Ordering::Relaxed);
}

/// 返回当前契约时刻（微秒）。
fn now_us() -> u64 {
    let frequency = crate::timing::tsc_frequency();
    if frequency == 0 {
        return 0;
    }
    ((u128::from(crate::timing::rdtsc()) * 1_000_000) / u128::from(frequency)) as u64
}

/// 编程一次性唤醒；`micros` 为契约微秒。
///
/// 不主动开中断：调用方在调度锁内，中断进入内核态时不会触发抢占。
pub fn arm_oneshot(micros: u64) {
    if !TICK_ENABLED.load(Ordering::SeqCst) {
        return;
    }
    let per_10ms = u64::from(TICK_COUNT.load(Ordering::SeqCst));
    let counts = (micros.saturating_mul(per_10ms) / 10_000).clamp(1, u64::from(u32::MAX)) as u32;
    ONESHOT.fetch_or(1 << super::apic::cpu_index(), Ordering::SeqCst);
    lapic_write(LAPIC_LVT_TIMER, TIMER_VECTOR as u32);
    lapic_write(LAPIC_TIMER_INITIAL, counts);
}

/// 恢复周期 watchdog。
pub fn resume_periodic() {
    if !TICK_ENABLED.load(Ordering::SeqCst) {
        return;
    }
    lapic_write(LAPIC_LVT_TIMER, LVT_PERIODIC | TIMER_VECTOR as u32);
    lapic_write(LAPIC_TIMER_INITIAL, TICK_COUNT.load(Ordering::SeqCst));
}

/// 记录一次定时器中断；返回是否为一次性唤醒。
///
/// 一次性唤醒不改变 `SYS_TICKS` 的 10 ms 语义：距上次 watchdog tick 满 10 ms
/// 时补记一次，这样频繁的 Grant 抢占不会让 tick 计数停摆。
pub fn note_timer_interrupt() -> bool {
    let bit = 1 << super::apic::cpu_index();
    if ONESHOT.fetch_and(!bit, Ordering::SeqCst) & bit != 0 {
        if now_us().saturating_sub(LAST_TICK_US.load(Ordering::Relaxed)) >= 10_000 {
            note_tick();
        }
        return true;
    }
    note_tick();
    false
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
