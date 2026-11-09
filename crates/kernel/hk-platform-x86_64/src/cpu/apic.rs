//! 本地 APIC 与跨核 IPI。

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::port::outb;

use super::gdt::init_gdt;
use super::idt::load_idt;
use super::smp::IPI_COUNT;
use super::*;

pub(super) static APIC_IDS: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(u32::MAX) }; MAX_CPUS];
static LAPIC_BASE: AtomicU64 = AtomicU64::new(0);
/// AP 初始化：建立 per-CPU TSS/GDT、加载共享 IDT、启用 LAPIC 与中断。
pub fn init_ap(cpu: usize) {
    assert!(cpu < MAX_CPUS, "CPU 索引越界");
    // EFER.NXE 是每 CPU 的；AP 也要启用，否则 NX 页在本核不生效。
    super::assembly::enable_no_execute();
    // CR0/CR4 也是每 CPU 的；没有 SSE 位 AP 上的 FXSAVE/FXRSTOR 会触发 #UD。
    super::assembly::enable_fpu();
    crate::paging::init_cpu();
    super::assembly::enable_hardening();
    init_gdt(cpu);
    load_idt();

    lapic_write(LAPIC_SIVR, SPURIOUS_ENABLE | SPURIOUS_VECTOR);
    lapic_write(LAPIC_LVT_TIMER, LVT_MASKED | TIMER_VECTOR as u32);
    APIC_IDS[cpu].store(u32::from(lapic_id()), Ordering::Release);
    // GDT/TSS/IDT 与 LAPIC 就绪；开启中断以接收 IPI。
    x86_64::instructions::interrupts::enable();
}

/// 向指定本地 APIC 发送固定投递 IPI。
pub fn send_ipi(destination: u8, vector: u8) {
    lapic_write(LAPIC_ICR_HIGH, u32::from(destination) << 24);
    lapic_write(LAPIC_ICR_LOW, 0x0000_4000 | u32::from(vector));
}

/// 返回已接收的 IPI 数量。
pub fn ipi_count() -> usize {
    IPI_COUNT.load(Ordering::Acquire)
}

/// 初始化本地 APIC：屏蔽 8259 PIC，配置伪中断向量与定时器中断。
pub fn init_apic(lapic_base: u64) {
    LAPIC_BASE.store(lapic_base, Ordering::SeqCst);
    APIC_IDS[0].store(u32::from(lapic_id()), Ordering::Release);

    // SAFETY: 8259 PIC 端口固定存在；屏蔽后中断只经本地 APIC 投递。
    unsafe {
        outb(PIC1_DATA, 0xFF);
        outb(PIC2_DATA, 0xFF);
    }

    lapic_write(LAPIC_SIVR, SPURIOUS_ENABLE | SPURIOUS_VECTOR);
    lapic_write(LAPIC_TIMER_DIVIDE, 0x03);
    lapic_write(LAPIC_LVT_TIMER, LVT_MASKED | TIMER_VECTOR as u32);
}

/// 启动周期定时器，目标中断次数为 `target`。
pub(super) fn lapic_write(offset: u64, value: u32) {
    let base = LAPIC_BASE.load(Ordering::SeqCst);
    assert!(base != 0, "本地 APIC 未初始化");
    // SAFETY: 基址由调用方映射为设备页，偏移是 LAPIC 寄存器。
    unsafe {
        hk_mmio::Mmio::new(base as usize).write_u32(offset as usize, value);
    }
}

pub(super) fn lapic_read(offset: u64) -> u32 {
    let base = LAPIC_BASE.load(Ordering::SeqCst);
    assert!(base != 0, "本地 APIC 未初始化");
    // SAFETY: 基址由调用方映射为设备页，偏移是 LAPIC 寄存器。
    let mmio = unsafe { hk_mmio::Mmio::new(base as usize) };
    mmio.read_u32(offset as usize)
}

/// 返回本地 APIC 编号，用于配置 I/O APIC 的目标。
pub fn lapic_id() -> u8 {
    (lapic_read(LAPIC_ID) >> 24) as u8
}

/// 返回指定 CPU 索引的本地 APIC 编号；该 CPU 尚未启动时返回 `None`。
pub fn apic_id(cpu: usize) -> Option<u8> {
    if cpu >= MAX_CPUS {
        return None;
    }
    let id = APIC_IDS[cpu].load(Ordering::Acquire);
    (id != u32::MAX).then_some(id as u8)
}

/// 返回第 `index` 个已启动 CPU 的本地 APIC 编号。
pub fn online_apic_id(index: usize) -> Option<u8> {
    APIC_IDS
        .iter()
        .filter_map(|slot| {
            let id = slot.load(Ordering::Acquire);
            (id != u32::MAX).then_some(id as u8)
        })
        .nth(index)
}

/// 返回已启动的 CPU 数量。
pub fn online_cpu_count() -> usize {
    APIC_IDS
        .iter()
        .filter(|slot| slot.load(Ordering::Acquire) != u32::MAX)
        .count()
}

/// 返回当前 CPU 的运行时索引；未登记时返回 0。
pub fn cpu_index() -> usize {
    let id = u32::from(lapic_id());
    for (index, slot) in APIC_IDS.iter().enumerate() {
        if slot.load(Ordering::Acquire) == id {
            return index;
        }
    }
    0
}
