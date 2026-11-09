//! x86_64 原语：`x86_64` crate 的薄封装。

use x86_64::instructions::port::Port;
use x86_64::instructions::tables::{self, DescriptorTablePointer};
use x86_64::instructions::{hlt, interrupts, tlb};
use x86_64::registers::control::{Cr0, Cr2, Cr3, Cr3Flags, Cr4};
use x86_64::registers::model_specific::Msr;
use x86_64::registers::segmentation::SegmentSelector;
use x86_64::structures::paging::PhysFrame;
use x86_64::{PhysAddr, VirtAddr};

/// 中断是否开启。
pub fn interrupts_enabled() -> bool {
    interrupts::are_enabled()
}

/// 开启中断。
pub fn enable_interrupts() {
    interrupts::enable();
}

/// 关闭中断。
pub fn disable_interrupts() {
    interrupts::disable();
}

/// 等待下一次中断。
pub fn halt() {
    hlt();
}

/// 原子地开中断并等待中断；唤醒后中断保持开启。
pub fn enable_and_halt() {
    interrupts::enable_and_hlt();
}

/// 全内存屏障。
pub fn fence() {
    // SAFETY: `mfence` 只影响内存顺序。
    unsafe {
        core::arch::asm!("mfence", options(nostack, preserves_flags));
    }
}

/// 读取 CR0。
pub fn read_cr0() -> u64 {
    Cr0::read_raw()
}

/// 写入 CR0。
///
/// # Safety
/// 调用方负责保证 CR0 位组合合法。
pub unsafe fn write_cr0(value: u64) {
    // SAFETY: 由调用方保证位组合合法。
    unsafe { Cr0::write_raw(value) }
}

/// 读取 CR2（页错误地址）。
pub fn read_cr2() -> u64 {
    Cr2::read_raw()
}

/// 读取 CR3 的页表根物理地址。
pub fn read_cr3() -> u64 {
    Cr3::read_raw().0.start_address().as_u64()
}

/// 写入 CR3 的页表根物理地址。
///
/// # Safety
/// 调用方保证 `value` 指向有效的顶层页表。
pub unsafe fn write_cr3(value: u64) {
    let frame = PhysFrame::containing_address(PhysAddr::new(value));
    // SAFETY: 由调用方保证页表有效。
    unsafe { Cr3::write(frame, Cr3Flags::empty()) }
}

/// 读取 CR4。
pub fn read_cr4() -> u64 {
    Cr4::read_raw()
}

/// 写入 CR4。
///
/// # Safety
/// 调用方负责保证 CR4 位组合合法。
pub unsafe fn write_cr4(value: u64) {
    // SAFETY: 由调用方保证位组合合法。
    unsafe { Cr4::write_raw(value) }
}

/// 读取 MSR。
pub fn read_msr(register: u32) -> u64 {
    // SAFETY: 调用方负责选择允许读取的 MSR。
    unsafe { Msr::new(register).read() }
}

/// 写入 MSR。
///
/// # Safety
/// 调用方负责选择允许写入的 MSR 与合法取值。
pub unsafe fn write_msr(register: u32, value: u64) {
    let mut msr = Msr::new(register);
    // SAFETY: 由调用方保证寄存器与取值合法。
    unsafe { msr.write(value) }
}

/// 加载 GDT。
///
/// # Safety
/// 调用方保证 `base`/`limit` 指向有效的 GDT。
pub unsafe fn load_gdt(base: u64, limit: u16) {
    let pointer = DescriptorTablePointer {
        limit,
        base: VirtAddr::new(base),
    };
    // SAFETY: 调用方保证 `base`/`limit` 指向有效的 GDT。
    unsafe { tables::lgdt(&pointer) }
}

/// 加载 IDT。
///
/// # Safety
/// 调用方保证 `base`/`limit` 指向有效的 IDT。
pub unsafe fn load_idt(base: u64, limit: u16) {
    let pointer = DescriptorTablePointer {
        limit,
        base: VirtAddr::new(base),
    };
    // SAFETY: 调用方保证 `base`/`limit` 指向有效的 IDT。
    unsafe { tables::lidt(&pointer) }
}

/// 加载任务状态段选择子。
///
/// # Safety
/// 调用方保证选择子指向有效的 TSS 描述符。
pub unsafe fn load_tss(selector: u16) {
    // SAFETY: 调用方保证选择子指向有效的 TSS 描述符。
    unsafe { tables::load_tss(SegmentSelector(selector)) }
}

/// 刷新单页 TLB。
pub fn flush_tlb_page(address: u64) {
    tlb::flush(VirtAddr::new(address));
}

/// 刷新全部 TLB。
pub fn flush_tlb_all() {
    tlb::flush_all();
}

/// 从端口读取一个字节。
///
/// # Safety
/// 调用方保证端口号对当前上下文可访问。
pub unsafe fn inb(port: u16) -> u8 {
    // SAFETY: 由调用方保证端口可访问。
    unsafe { Port::new(port).read() }
}

/// 向端口写入一个字节。
///
/// # Safety
/// 调用方保证端口号对当前上下文可访问。
pub unsafe fn outb(port: u16, value: u8) {
    // SAFETY: 由调用方保证端口可访问。
    unsafe { Port::new(port).write(value) }
}

/// 从端口读取 32 位值。
///
/// # Safety
/// 调用方保证端口号对当前上下文可访问。
pub unsafe fn inl(port: u16) -> u32 {
    // SAFETY: 由调用方保证端口可访问。
    unsafe { Port::new(port).read() }
}

/// 向端口写入 32 位值。
///
/// # Safety
/// 调用方保证端口号对当前上下文可访问。
pub unsafe fn outl(port: u16, value: u32) {
    // SAFETY: 由调用方保证端口可访问。
    unsafe { Port::new(port).write(value) }
}
