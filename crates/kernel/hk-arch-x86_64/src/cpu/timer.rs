//! LAPIC 定时器与调度 tick。

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use x86_64::instructions::interrupts;

use super::apic::{lapic_read, lapic_write};
use super::{
    LAPIC_LVT_TIMER, LAPIC_TIMER_CURRENT, LAPIC_TIMER_DIVIDE, LAPIC_TIMER_INITIAL, LVT_MASKED,
    LVT_PERIODIC, TICK_INITIAL_COUNT, TIMER_INITIAL_COUNT, TIMER_VECTOR,
};

static TIMER_TICKS: AtomicU64 = AtomicU64::new(0);
static UPTIME_TICKS: AtomicU64 = AtomicU64::new(0);
static TIMER_TARGET: AtomicU64 = AtomicU64::new(0);
static TICK_COUNT: AtomicU32 = AtomicU32::new(TICK_INITIAL_COUNT);
/// 每 CPU 是否已启动周期 tick。
static TICK_ARMED: AtomicU64 = AtomicU64::new(0);
/// 每 CPU 的调度 tick 是否处于投递状态。
static TICK_ENABLED: AtomicU64 = AtomicU64::new(0);
/// 每 CPU 是否已编程一次性唤醒。
static ONESHOT: AtomicU64 = AtomicU64::new(0);
/// 每 CPU 上次 watchdog tick 的契约时刻（微秒）。
static LAST_TICK_US: [AtomicU64; super::MAX_CPUS] = [const { AtomicU64::new(0) }; super::MAX_CPUS];

fn cpu_bit() -> u64 {
    let cpu = super::apic::cpu_index();
    if cpu < super::MAX_CPUS {
        1_u64 << cpu
    } else {
        0
    }
}
/// 启动周期定时器，目标中断次数为 `target`。
pub fn arm_timer(target: u64) {
    let bit = cpu_bit();
    TICK_ARMED.fetch_or(bit, Ordering::SeqCst);
    TICK_ENABLED.fetch_or(bit, Ordering::SeqCst);
    TIMER_TICKS.store(0, Ordering::SeqCst);
    TIMER_TARGET.store(target, Ordering::SeqCst);

    lapic_write(LAPIC_LVT_TIMER, LVT_PERIODIC | TIMER_VECTOR as u32);
    lapic_write(LAPIC_TIMER_INITIAL, TIMER_INITIAL_COUNT);

    // 开中断允许定时器中断进入；IDT 与本地 APIC 已就绪。
    interrupts::enable();
}

/// 停止定时器并关中断。
pub fn disarm_timer() {
    let bit = cpu_bit();
    TICK_ARMED.fetch_and(!bit, Ordering::SeqCst);
    TICK_ENABLED.fetch_and(!bit, Ordering::SeqCst);
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
    let bit = cpu_bit();
    if TICK_ARMED.fetch_or(bit, Ordering::SeqCst) & bit == 0 {
        TIMER_TICKS.store(0, Ordering::SeqCst);
        lapic_write(LAPIC_TIMER_INITIAL, TICK_COUNT.load(Ordering::SeqCst));
    }
    TIMER_TARGET.store(u64::MAX, Ordering::SeqCst);
    lapic_write(LAPIC_LVT_TIMER, LVT_PERIODIC | TIMER_VECTOR as u32);
    TICK_ENABLED.fetch_or(bit, Ordering::SeqCst);

    // 中断表与本地 APIC 已就绪；打开中断允许 tick 投递。
    interrupts::enable();
}

/// 启动 AP 的周期性调度 tick，复用 BSP 校准的计数，不重置全局 tick 计数。
pub fn start_ap_tick() {
    let bit = cpu_bit();
    TICK_ARMED.fetch_or(bit, Ordering::SeqCst);
    TICK_ENABLED.fetch_or(bit, Ordering::SeqCst);
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
    let bit = cpu_bit();
    TICK_ARMED.fetch_or(bit, Ordering::SeqCst);
    TIMER_TARGET.store(u64::MAX, Ordering::SeqCst);
    lapic_write(LAPIC_TIMER_INITIAL, TICK_COUNT.load(Ordering::SeqCst));
    lapic_write(LAPIC_LVT_TIMER, LVT_PERIODIC | TIMER_VECTOR as u32);
    TICK_ENABLED.fetch_or(bit, Ordering::SeqCst);

    // 中断表与本地 APIC 已就绪；打开中断允许 tick 投递。
    interrupts::enable();
}

/// 为 idle 唤醒启用本地定时器，不重置周期计数。
///
/// 进入时关中断并在返回时保持关闭：调用方在关中断状态下编程唤醒期限，再用
/// [`wait_for_interrupt`] 原子地开中断并 `hlt`，避免一次性唤醒在 `hlt` 之前
/// 被投递后进入无定时器的永久等待。
pub fn enable_wakeup_timer() {
    interrupts::disable();
    let bit = cpu_bit();
    TICK_ARMED.fetch_or(bit, Ordering::SeqCst);
    TICK_ENABLED.fetch_or(bit, Ordering::SeqCst);
}

/// 等待下一次中断；进入前关中断，返回时中断仍关闭。
pub fn wait_for_interrupt() {
    interrupts::disable();
    interrupts::enable_and_hlt();
    interrupts::disable();
}

/// 停止调度 tick 并关中断。
pub fn stop_tick() {
    // 关中断后写 LVT 屏蔽位，避免仍在途的中断再次进入。
    interrupts::disable();
    lapic_write(LAPIC_LVT_TIMER, LVT_MASKED | TIMER_VECTOR as u32);
    TICK_ENABLED.fetch_and(!cpu_bit(), Ordering::SeqCst);
}

/// 调度 tick 当前是否在投递。
pub fn tick_enabled() -> bool {
    TICK_ENABLED.load(Ordering::SeqCst) & cpu_bit() != 0
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

/// 用 HPET 校准 LAPIC 定时器，把调度 tick 设为约 10 ms。
///
/// 不依赖 TSC；HPET 不可用时返回 `false`，调用方保留既有校准结果。
pub fn calibrate_tick_hpet() -> bool {
    if !crate::hpet::available() {
        return false;
    }
    let frequency = crate::hpet::frequency();
    if frequency == 0 {
        return false;
    }
    let window = (frequency / 100).max(1);

    lapic_write(LAPIC_TIMER_DIVIDE, 0x03);
    lapic_write(LAPIC_LVT_TIMER, LVT_MASKED | TIMER_VECTOR as u32);
    lapic_write(LAPIC_TIMER_INITIAL, u32::MAX);

    let start = crate::hpet::now();
    while crate::hpet::now().wrapping_sub(start) < window {
        core::hint::spin_loop();
    }
    let counts = u64::from(u32::MAX).saturating_sub(u64::from(lapic_read(LAPIC_TIMER_CURRENT)));

    lapic_write(LAPIC_TIMER_INITIAL, 0);
    lapic_write(LAPIC_LVT_TIMER, LVT_MASKED | TIMER_VECTOR as u32);

    if counts > 0 {
        TICK_COUNT.store(counts.min(u64::from(u32::MAX)) as u32, Ordering::SeqCst);
        return true;
    }
    false
}

/// 记录一次定时器中断。
fn note_tick(cpu: usize) {
    TIMER_TICKS.fetch_add(1, Ordering::SeqCst);
    UPTIME_TICKS.fetch_add(1, Ordering::SeqCst);
    if let Some(last) = LAST_TICK_US.get(cpu) {
        last.store(now_us(), Ordering::Relaxed);
    }
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
    let bit = cpu_bit();
    if TICK_ENABLED.load(Ordering::SeqCst) & bit == 0 {
        return;
    }
    let per_10ms = u64::from(TICK_COUNT.load(Ordering::SeqCst));
    let counts = (micros.saturating_mul(per_10ms).saturating_add(9_999) / 10_000)
        .clamp(1, u64::from(u32::MAX)) as u32;
    ONESHOT.fetch_or(bit, Ordering::SeqCst);
    lapic_write(LAPIC_LVT_TIMER, TIMER_VECTOR as u32);
    lapic_write(LAPIC_TIMER_INITIAL, counts);
}

/// 指定 CPU 是否已编程一次性唤醒。
pub fn oneshot_armed(cpu: usize) -> bool {
    cpu < super::MAX_CPUS && ONESHOT.load(Ordering::SeqCst) & (1_u64 << cpu) != 0
}

/// 恢复周期 watchdog。
pub fn resume_periodic() {
    if TICK_ENABLED.load(Ordering::SeqCst) & cpu_bit() == 0 {
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
    let cpu = super::apic::cpu_index();
    let bit = cpu_bit();
    if ONESHOT.fetch_and(!bit, Ordering::SeqCst) & bit != 0 {
        let last = LAST_TICK_US
            .get(cpu)
            .map_or(0, |last| last.load(Ordering::Relaxed));
        if now_us().saturating_sub(last) >= 10_000 {
            note_tick(cpu);
        }
        return true;
    }
    note_tick(cpu);
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
