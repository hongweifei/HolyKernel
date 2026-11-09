//! 跨核 TLB shootdown 与空闲唤醒。

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use hk_smp::{IdleState, TlbHardware, TlbShootdown};

use super::apic::{APIC_IDS, cpu_index, send_ipi};
use super::*;

pub(super) static IPI_COUNT: AtomicUsize = AtomicUsize::new(0);
static TLB: TlbShootdown<MAX_CPUS> = TlbShootdown::new();
static IDLE: IdleState<MAX_CPUS> = IdleState::new();
static RESCHED: AtomicU64 = AtomicU64::new(0);

/// x86 TLB/IPI 硬件适配。
struct X86Tlb {
    self_id: u32,
}

impl TlbHardware for X86Tlb {
    fn online(&self, cpu: usize) -> bool {
        let id = APIC_IDS[cpu].load(Ordering::Acquire);
        id != u32::MAX && id != self.self_id && id <= u32::from(u8::MAX)
    }

    fn send_ipi(&mut self, cpu: usize) {
        send_ipi(APIC_IDS[cpu].load(Ordering::Acquire) as u8, IPI_VECTOR);
    }

    fn flush(&mut self, address: Option<u64>) {
        flush_local(address);
    }

    fn now(&mut self) -> u64 {
        crate::timing::rdtsc()
    }
}

/// 刷新当前 CPU 的 TLB，并等待其他在线 CPU 确认。
///
/// `address` 为 `None` 时刷新整个 TLB。返回 `false` 表示等待确认超时。
pub fn shootdown_tlb(address: Option<u64>) -> bool {
    let current = cpu_index();
    let mut hardware = X86Tlb {
        self_id: APIC_IDS[current].load(Ordering::Acquire),
    };
    let timeout = match crate::timing::tsc_frequency() {
        0 => 1_000_000_000,
        frequency => frequency,
    };

    TLB.shootdown(current, address, &mut hardware, timeout)
}

/// 返回已完成的 TLB shootdown 次数。
pub fn tlb_shootdowns() -> usize {
    TLB.completed()
}

/// 空闲 CPU 数量。
pub fn idle_cpus() -> usize {
    IDLE.idle_count()
}

/// 让当前 CPU 进入空闲等待，直到收到唤醒 IPI。
///
/// 调用前应关闭本地定时器；返回时中断仍关闭，由调用方重新启动 tick。
pub fn idle(cpu: usize) {
    assert!(cpu < MAX_CPUS, "CPU 索引越界");
    IDLE.enter(cpu);
    // 关中断后检查唤醒标志；`enable_and_halt` 原子地开中断并 `hlt`，不会丢失
    // 在途 IPI 或中断。
    hk_arch::disable_interrupts();
    loop {
        if IDLE.take_wake(cpu) {
            break;
        }
        hk_arch::enable_and_halt();
        hk_arch::disable_interrupts();
        if IDLE.take_wake(cpu) {
            break;
        }
    }
    IDLE.leave(cpu);
}

/// 唤醒空闲 CPU；只在有可运行线程时调用。
pub fn wake_aps() {
    let current = cpu_index();
    let self_id = APIC_IDS[current].load(Ordering::Acquire);
    for (cpu, slot) in APIC_IDS.iter().enumerate() {
        if cpu == current {
            continue;
        }
        let id = slot.load(Ordering::Acquire);
        if id == u32::MAX || id == self_id || id > u32::from(u8::MAX) {
            continue;
        }
        if IDLE.wake(cpu) {
            send_ipi(id as u8, IPI_VECTOR);
        }
    }
}

/// 请求目标 CPU 重新调度；本地 CPU 由调度循环自然处理。
pub fn reschedule(cpu: usize) {
    if cpu >= MAX_CPUS || cpu == cpu_index() {
        return;
    }
    let id = APIC_IDS[cpu].load(Ordering::Acquire);
    if id == u32::MAX || id > u32::from(u8::MAX) {
        return;
    }
    RESCHED.fetch_or(1_u64 << cpu, Ordering::SeqCst);
    send_ipi(id as u8, IPI_VECTOR);
}

/// 取出并清除指定 CPU 的抢占请求。
pub(super) fn take_resched(cpu: usize) -> bool {
    cpu < MAX_CPUS && RESCHED.fetch_and(!(1_u64 << cpu), Ordering::SeqCst) != 0
}

pub(super) fn note_ipi() {
    IPI_COUNT.fetch_add(1, Ordering::SeqCst);
}

pub(super) fn handle_tlb_ipi() {
    TLB.acknowledge(cpu_index(), flush_local);
}

fn flush_local(address: Option<u64>) {
    match address {
        Some(address) => hk_arch::flush_tlb_page(address),
        None => hk_arch::flush_tlb_all(),
    }
}
