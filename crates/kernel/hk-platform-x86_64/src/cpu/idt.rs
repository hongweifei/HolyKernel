//! IDT 与中断入口。

use core::cell::UnsafeCell;

use x86_64::VirtAddr;
use x86_64::instructions::tables;
use x86_64::instructions::tables::DescriptorTablePointer;

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
    fn isr_64();
    fn isr_65();
    fn isr_66();
    fn isr_67();
    fn isr_68();
    fn isr_69();
    fn isr_70();
    fn isr_71();
    fn isr_72();
    fn isr_73();
    fn isr_74();
    fn isr_75();
    fn isr_76();
    fn isr_77();
    fn isr_78();
    fn isr_79();
    fn isr_80();
    fn isr_81();
    fn isr_82();
    fn isr_83();
    fn isr_84();
    fn isr_85();
    fn isr_86();
    fn isr_87();
    fn isr_88();
    fn isr_89();
    fn isr_90();
    fn isr_91();
    fn isr_92();
    fn isr_93();
    fn isr_94();
    fn isr_95();
    fn isr_128();
    fn isr_158();
    fn isr_159();
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

fn irq_handlers() -> [usize; 32] {
    handler_addresses![
        isr_64, isr_65, isr_66, isr_67, isr_68, isr_69, isr_70, isr_71, isr_72, isr_73, isr_74,
        isr_75, isr_76, isr_77, isr_78, isr_79, isr_80, isr_81, isr_82, isr_83, isr_84, isr_85,
        isr_86, isr_87, isr_88, isr_89, isr_90, isr_91, isr_92, isr_93, isr_94, isr_95,
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
        for (offset, handler) in irq_handlers().iter().enumerate() {
            idt[crate::irq::IRQ_VECTOR_BASE as usize + offset] = IdtEntry::new(*handler, 0);
        }
        idt[TIMER_VECTOR as usize] = IdtEntry::new(isr_32 as *const () as usize, 0);
        idt[crate::board::KEYBOARD_VECTOR as usize] =
            IdtEntry::new(isr_33 as *const () as usize, 0);
        idt[SYSCALL_VECTOR as usize] =
            IdtEntry::with_attributes(isr_128 as *const () as usize, 0, USER_INTERRUPT_GATE);
        idt[HPET_VECTOR as usize] = IdtEntry::new(isr_158 as *const () as usize, 0);
        idt[SCI_VECTOR as usize] = IdtEntry::new(isr_159 as *const () as usize, 0);
        idt[IPI_VECTOR as usize] = IdtEntry::new(isr_240 as *const () as usize, 0);
    }
    load_idt();
}

pub(super) fn load_idt() {
    // SAFETY: IDT 已由 BSP 初始化且此后只读。
    unsafe {
        let idt = &*IDT.0.get();
        let pointer = DescriptorTablePointer {
            limit: (core::mem::size_of::<[IdtEntry; 256]>() - 1) as u16,
            base: VirtAddr::new(idt.as_ptr() as u64),
        };
        tables::lidt(&pointer);
    }
}
