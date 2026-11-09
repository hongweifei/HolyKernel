//! CPU 初始化：GDT、TSS、IDT、本地 APIC 与异常处理。
//!
//! 具体实现按 GDT/IDT/APIC/定时器/SMP/系统调用/用户现场拆分；本模块保留共享
//! 常量、初始化入口与公开 API。

use core::sync::atomic::{AtomicBool, Ordering};

mod apic;
mod assembly;
mod features;
mod frame;
mod gdt;
mod idt;
mod interrupt;
mod smp;
mod stacks;
mod syscall;
mod timer;
mod user;

pub use apic::{
    apic_id, cpu_index, init_ap, init_apic, ipi_count, lapic_id, online_apic_id, online_cpu_count,
    send_ipi,
};
pub use assembly::{fpu_xstate_mask, user_access_begin, user_access_end};
pub use features::Features;
pub use frame::InterruptFrame;
pub(crate) use gdt::set_syscall_stack_top;
pub use smp::{idle, idle_cpus, reschedule, shootdown_tlb, tlb_shootdowns, wake_aps};
pub use stacks::init as init_stacks;
pub use syscall::{PreemptHook, SyscallAction, SyscallHook, set_preempt_hook, set_syscall_hook};
pub use timer::{
    arm_oneshot, arm_timer, calibrate_tick, disarm_timer, enable_wakeup_timer,
    note_timer_interrupt, oneshot_armed, rearm_tick,
    resume_periodic, start_ap_tick, start_tick, stop_tick, tick_enabled, timer_finished,
    timer_ticks, uptime_ticks,
};
pub use user::{
    FaultAction, FaultHook, exception_name, fault_address, leave_user, resume_user, set_fault_hook,
    set_thread_pointer, user_frame,
};

/// 内核代码段选择子。
pub const KERNEL_CODE: u16 = 0x08;

/// 内核数据段选择子。
pub const KERNEL_DATA: u16 = 0x10;

/// TSS 选择子。
pub const TSS_SELECTOR: u16 = 0x18;

/// 用户数据段选择子。
pub const USER_DATA_SELECTOR: u16 = 0x2B;

/// 用户代码段选择子。
pub const USER_CODE_SELECTOR: u16 = 0x33;

/// 系统调用中断向量。
pub const SYSCALL_VECTOR: u64 = 0x80;

/// 定时器中断向量。
pub const TIMER_VECTOR: u64 = 32;

/// 双重故障使用的中断栈表序号。
const DOUBLE_FAULT_IST: u8 = 1;

/// 双重故障栈大小。
const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;

/// TSS 描述符类型：64 位可用 TSS。
const TSS_TYPE_AVAILABLE: u64 = 0x89;

/// 64 位代码段描述符：L=1、P=1、DPL=0、type=0xA。
const CODE_DESCRIPTOR: u64 = 0x00AF_9A00_0000_FFFF;

/// 64 位数据段描述符：D/B=1、P=1、DPL=0、type=0x92。
const DATA_DESCRIPTOR: u64 = 0x00CF_9200_0000_FFFF;

/// 用户数据段描述符：DPL=3、type=0x92。
const USER_DATA_DESCRIPTOR: u64 = 0x00CF_F200_0000_FFFF;

/// 用户代码段描述符：DPL=3、L=1、type=0xA。
const USER_CODE_DESCRIPTOR: u64 = 0x00AF_FA00_0000_FFFF;

/// 用户态中断门：P=1、DPL=3、type=0xE。
const USER_INTERRUPT_GATE: u8 = 0xEE;

/// 中断门属性：P=1、DPL=0、type=0xE。
const INTERRUPT_GATE: u8 = 0x8E;

const PIC1_DATA: u16 = 0x21;
const PIC2_DATA: u16 = 0xA1;

const LAPIC_EOI: u64 = 0xB0;
const LAPIC_SIVR: u64 = 0xF0;
const LAPIC_ID: u64 = 0x20;
const LAPIC_LVT_TIMER: u64 = 0x320;
const LAPIC_TIMER_INITIAL: u64 = 0x380;
const LAPIC_TIMER_CURRENT: u64 = 0x390;
const LAPIC_TIMER_DIVIDE: u64 = 0x3E0;
const LAPIC_ICR_LOW: u64 = 0x300;
const LAPIC_ICR_HIGH: u64 = 0x310;

const LVT_MASKED: u32 = 1 << 16;
const LVT_PERIODIC: u32 = 1 << 17;
const SPURIOUS_VECTOR: u32 = 0xFF;
const SPURIOUS_ENABLE: u32 = 1 << 8;

/// 定时器初始计数。
const TIMER_INITIAL_COUNT: u32 = 1_000_000;

/// 调度 tick 的初始计数。
const TICK_INITIAL_COUNT: u32 = 100_000;

/// 系统调用栈大小。
const SYSCALL_STACK_SIZE: usize = 32 * 1024;

/// 支持的 CPU 数量上限。
pub const MAX_CPUS: usize = 16;
/// 跨核 IPI 向量。
pub const IPI_VECTOR: u8 = 240;

static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// 初始化 GDT、TSS 与 IDT。
///
/// # Panics
/// 重复调用时 panic：CPU 表只建立一次。
pub fn init() {
    assert!(
        !INITIALIZED.swap(true, Ordering::AcqRel),
        "CPU 表重复初始化"
    );
    assembly::enable_no_execute();
    assembly::enable_fpu();
    crate::paging::init_cpu();
    assembly::enable_hardening();
    gdt::init_gdt(0);
    idt::init_idt();
}
