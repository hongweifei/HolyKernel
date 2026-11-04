//! IDT 与中断入口。

use core::arch::asm;
use core::cell::UnsafeCell;

use super::gdt::DescriptorPointer;
use super::*;

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    attributes: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const EMPTY: Self = Self {
        offset_low: 0,
        selector: 0,
        ist: 0,
        attributes: 0,
        offset_mid: 0,
        offset_high: 0,
        reserved: 0,
    };

    fn new(handler: usize, ist: u8) -> Self {
        Self::with_attributes(handler, ist, INTERRUPT_GATE)
    }

    fn with_attributes(handler: usize, ist: u8, attributes: u8) -> Self {
        let handler = handler as u64;
        Self {
            offset_low: handler as u16,
            selector: KERNEL_CODE,
            ist,
            attributes,
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        }
    }
}

struct IdtState(UnsafeCell<[IdtEntry; 256]>);

// SAFETY: IDT 只由 BSP 初始化一次，之后由所有 CPU 只读访问。
unsafe impl Sync for IdtState {}

static IDT: IdtState = IdtState(UnsafeCell::new([IdtEntry::EMPTY; 256]));

unsafe extern "C" {
    fn isr_0();
    fn isr_1();
    fn isr_2();
    fn isr_3();
    fn isr_4();
    fn isr_5();
    fn isr_6();
    fn isr_7();
    fn isr_8();
    fn isr_9();
    fn isr_10();
    fn isr_11();
    fn isr_12();
    fn isr_13();
    fn isr_14();
    fn isr_15();
    fn isr_16();
    fn isr_17();
    fn isr_18();
    fn isr_19();
    fn isr_20();
    fn isr_21();
    fn isr_22();
    fn isr_23();
    fn isr_24();
    fn isr_25();
    fn isr_26();
    fn isr_27();
    fn isr_28();
    fn isr_29();
    fn isr_30();
    fn isr_31();
    fn isr_32();
    fn isr_33();
    fn isr_128();
    fn isr_240();
}

macro_rules! handler_addresses {
    ($($name:ident),* $(,)?) => {
        [$( $name as *const () as usize ),*]
    };
}

fn handlers() -> [usize; 32] {
    handler_addresses![
        isr_0, isr_1, isr_2, isr_3, isr_4, isr_5, isr_6, isr_7, isr_8, isr_9, isr_10, isr_11,
        isr_12, isr_13, isr_14, isr_15, isr_16, isr_17, isr_18, isr_19, isr_20, isr_21, isr_22,
        isr_23, isr_24, isr_25, isr_26, isr_27, isr_28, isr_29, isr_30, isr_31,
    ]
}

pub(super) fn init_idt() {
    let handlers = handlers();

    // SAFETY: BSP 初始化期间独占写入共享 IDT；完成后各 CPU 只读加载。
    unsafe {
        let idt = &mut *IDT.0.get();
        for entry in idt.iter_mut() {
            *entry = IdtEntry::EMPTY;
        }
        for (vector, handler) in handlers.iter().enumerate() {
            let ist = if vector == 8 { DOUBLE_FAULT_IST } else { 0 };
            idt[vector] = IdtEntry::new(*handler, ist);
        }
        idt[TIMER_VECTOR as usize] = IdtEntry::new(isr_32 as *const () as usize, 0);
        idt[KEYBOARD_VECTOR as usize] = IdtEntry::new(isr_33 as *const () as usize, 0);
        idt[SYSCALL_VECTOR as usize] =
            IdtEntry::with_attributes(isr_128 as *const () as usize, 0, USER_INTERRUPT_GATE);
        idt[IPI_VECTOR as usize] = IdtEntry::new(isr_240 as *const () as usize, 0);
    }
    load_idt();
}

pub(super) fn load_idt() {
    // SAFETY: IDT 已由 BSP 初始化且此后只读。
    unsafe {
        let idt = &*IDT.0.get();
        let pointer = DescriptorPointer {
            limit: (core::mem::size_of::<[IdtEntry; 256]>() - 1) as u16,
            base: idt.as_ptr() as u64,
        };
        asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
    }
}
