//! CPU 初始化：GDT、TSS、IDT、本地 APIC 与异常处理。
//!
//! 建立平坦 64 位段、双重故障专用 IST 栈、32 个异常入口与定时器中断。异常会
//! 打印向量、错误码与寄存器现场；定时器中断只累加计数并回 EOI。

use alloc::string::String;
use core::arch::asm;
use core::cell::UnsafeCell;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use hk_hal::Console;
use spin::Mutex;

use crate::port::outb;
use crate::serial::{COM1, Serial};

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

/// 键盘中断向量。
pub const KEYBOARD_VECTOR: u64 = 33;

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

#[repr(C, align(16))]
struct SyscallStack(UnsafeCell<[u8; SYSCALL_STACK_SIZE]>);

// SAFETY: 系统调用栈只作为 ring 0 栈使用，访问由 CPU 特权级切换与内核代码串行化。
unsafe impl Sync for SyscallStack {}

static SYSCALL_STACK: SyscallStack = SyscallStack(UnsafeCell::new([0; SYSCALL_STACK_SIZE]));

fn syscall_stack_top() -> u64 {
    SYSCALL_STACK.0.get() as u64 + SYSCALL_STACK_SIZE as u64
}

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct TaskStateSegment {
    reserved0: u32,
    rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    reserved1: u64,
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
}

impl TaskStateSegment {
    const fn new() -> Self {
        Self {
            reserved0: 0,
            rsp0: 0,
            rsp1: 0,
            rsp2: 0,
            reserved1: 0,
            ist: [0; 7],
            reserved2: 0,
            reserved3: 0,
            iomap_base: core::mem::size_of::<Self>() as u16,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
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

/// 中断与异常现场。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct InterruptFrame {
    /// RAX。
    pub rax: u64,
    /// RBX。
    pub rbx: u64,
    /// RCX。
    pub rcx: u64,
    /// RDX。
    pub rdx: u64,
    /// RSI。
    pub rsi: u64,
    /// RDI。
    pub rdi: u64,
    /// RBP。
    pub rbp: u64,
    /// R8。
    pub r8: u64,
    /// R9。
    pub r9: u64,
    /// R10。
    pub r10: u64,
    /// R11。
    pub r11: u64,
    /// R12。
    pub r12: u64,
    /// R13。
    pub r13: u64,
    /// R14。
    pub r14: u64,
    /// R15。
    pub r15: u64,
    /// 中断向量。
    pub vector: u64,
    /// 错误码。
    pub error: u64,
    /// 出错指令地址。
    pub rip: u64,
    /// 代码段选择子。
    pub cs: u64,
    /// 标志寄存器。
    pub rflags: u64,
    /// 中断发生时的栈指针。
    pub rsp: u64,
    /// 栈段选择子。
    pub ss: u64,
}

const _: () = {
    assert!(core::mem::offset_of!(InterruptFrame, rax) == 0);
    assert!(core::mem::offset_of!(InterruptFrame, rbx) == 8);
    assert!(core::mem::offset_of!(InterruptFrame, rcx) == 16);
    assert!(core::mem::offset_of!(InterruptFrame, rdx) == 24);
    assert!(core::mem::offset_of!(InterruptFrame, rsi) == 32);
    assert!(core::mem::offset_of!(InterruptFrame, rdi) == 40);
    assert!(core::mem::offset_of!(InterruptFrame, rbp) == 48);
    assert!(core::mem::offset_of!(InterruptFrame, r8) == 56);
    assert!(core::mem::offset_of!(InterruptFrame, r15) == 112);
    assert!(core::mem::offset_of!(InterruptFrame, vector) == 120);
    assert!(core::mem::offset_of!(InterruptFrame, error) == 128);
    assert!(core::mem::offset_of!(InterruptFrame, rip) == 136);
    assert!(core::mem::offset_of!(InterruptFrame, cs) == 144);
    assert!(core::mem::offset_of!(InterruptFrame, rflags) == 152);
    assert!(core::mem::offset_of!(InterruptFrame, rsp) == 160);
    assert!(core::mem::offset_of!(InterruptFrame, ss) == 168);
};

/// 系统调用处理结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyscallAction {
    /// 已就地处理，返回用户态。
    Continue,
    /// 任务阻塞，调度器切换到下一个任务。
    Block,
    /// 任务退出，调度器切换到下一个任务。
    Exit,
}

/// 扩展系统调用处理函数。
pub type SyscallHook = fn(&mut InterruptFrame) -> SyscallAction;

/// 用户态抢占钩子：保存现场并返回是否切换任务。
pub type PreemptHook = fn(&InterruptFrame) -> bool;

struct CpuState {
    gdt: UnsafeCell<[u64; 7]>,
    idt: UnsafeCell<[IdtEntry; 256]>,
    tss: UnsafeCell<TaskStateSegment>,
    double_fault_stack: UnsafeCell<[u8; DOUBLE_FAULT_STACK_SIZE]>,
}

// SAFETY: 这些表只在 `init` 中写入一次，之后由 CPU 只读访问；中断路径只读。
unsafe impl Sync for CpuState {}

static CPU: CpuState = CpuState {
    gdt: UnsafeCell::new([0; 7]),
    idt: UnsafeCell::new([IdtEntry::EMPTY; 256]),
    tss: UnsafeCell::new(TaskStateSegment::new()),
    double_fault_stack: UnsafeCell::new([0; DOUBLE_FAULT_STACK_SIZE]),
};

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static LAPIC_BASE: AtomicU64 = AtomicU64::new(0);
static TIMER_TICKS: AtomicU64 = AtomicU64::new(0);
static TIMER_TARGET: AtomicU64 = AtomicU64::new(0);
static SYSCALL_HOOK: AtomicUsize = AtomicUsize::new(0);
static PREEMPT_HOOK: AtomicUsize = AtomicUsize::new(0);
static TICK_COUNT: AtomicU32 = AtomicU32::new(TICK_INITIAL_COUNT);

/// 初始化 GDT、TSS 与 IDT。
///
/// # Panics
/// 重复调用时 panic：CPU 表只建立一次。
pub fn init() {
    assert!(
        !INITIALIZED.swap(true, Ordering::AcqRel),
        "CPU 表重复初始化"
    );
    init_gdt();
    init_idt();
}

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

fn init_gdt() {
    let tss_base = CPU.tss.get() as u64;
    let tss_limit = (core::mem::size_of::<TaskStateSegment>() - 1) as u64;

    // SAFETY: 初始化期间独占写入 GDT 与 TSS；此时没有多核或中断并发访问。
    unsafe {
        let tss = &mut *CPU.tss.get();
        tss.rsp0 = syscall_stack_top();
        tss.ist[DOUBLE_FAULT_IST as usize - 1] =
            CPU.double_fault_stack.get().cast::<u8>() as u64 + DOUBLE_FAULT_STACK_SIZE as u64;

        let gdt = &mut *CPU.gdt.get();
        gdt[0] = 0;
        gdt[1] = CODE_DESCRIPTOR;
        gdt[2] = DATA_DESCRIPTOR;
        gdt[3] = (tss_limit & 0xFFFF)
            | ((tss_base & 0xFF_FFFF) << 16)
            | (TSS_TYPE_AVAILABLE << 40)
            | (((tss_limit >> 16) & 0xF) << 48)
            | (((tss_base >> 24) & 0xFF) << 56);
        gdt[4] = tss_base >> 32;
        gdt[5] = USER_DATA_DESCRIPTOR;
        gdt[6] = USER_CODE_DESCRIPTOR;

        let pointer = DescriptorPointer {
            limit: (core::mem::size_of::<[u64; 7]>() - 1) as u16,
            base: gdt.as_ptr() as u64,
        };
        asm!("lgdt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));

        let code = u64::from(KERNEL_CODE);
        asm!(
            "push {code}",
            "lea {target}, [rip + 2f]",
            "push {target}",
            "retfq",
            "2:",
            code = in(reg) code,
            target = out(reg) _,
        );

        let data = u64::from(KERNEL_DATA);
        asm!(
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            "mov fs, ax",
            "mov gs, ax",
            in("ax") data,
            options(nostack, preserves_flags),
        );
        asm!("ltr ax", in("ax") TSS_SELECTOR, options(nostack, preserves_flags));
    }
}

fn init_idt() {
    let handlers = handlers();

    // SAFETY: 初始化期间独占写入 IDT；写入完成后由 CPU 只读访问。
    unsafe {
        let idt = &mut *CPU.idt.get();
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

        let pointer = DescriptorPointer {
            limit: (core::mem::size_of::<[IdtEntry; 256]>() - 1) as u16,
            base: idt.as_ptr() as u64,
        };
        asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
    }
}

/// 初始化本地 APIC：屏蔽 8259 PIC，配置伪中断向量与定时器中断。
///
/// `lapic_base` 是 LAPIC 寄存器页的虚拟地址：调用方先用内核页表把
/// `0xFEE00000` 映射到 HHDM 窗口。
pub fn init_apic(lapic_base: u64) {
    LAPIC_BASE.store(lapic_base, Ordering::SeqCst);

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
pub fn arm_timer(target: u64) {
    TIMER_TICKS.store(0, Ordering::SeqCst);
    TIMER_TARGET.store(target, Ordering::SeqCst);

    lapic_write(LAPIC_LVT_TIMER, LVT_PERIODIC | TIMER_VECTOR as u32);
    lapic_write(LAPIC_TIMER_INITIAL, TIMER_INITIAL_COUNT);

    // SAFETY: 开中断允许定时器中断进入；IDT 与本地 APIC 已就绪。
    unsafe { asm!("sti", options(nomem, nostack)) };
}

/// 停止定时器并关中断。
pub fn disarm_timer() {
    // SAFETY: 关中断后写 LVT 屏蔽位，避免仍在途的中断再次进入。
    unsafe { asm!("cli", options(nomem, nostack)) };
    lapic_write(LAPIC_LVT_TIMER, LVT_MASKED | TIMER_VECTOR as u32);
}

/// 启动周期性调度 tick。
pub fn start_tick() {
    TIMER_TICKS.store(0, Ordering::SeqCst);
    TIMER_TARGET.store(u64::MAX, Ordering::SeqCst);
    lapic_write(LAPIC_LVT_TIMER, LVT_PERIODIC | TIMER_VECTOR as u32);
    lapic_write(LAPIC_TIMER_INITIAL, TICK_COUNT.load(Ordering::SeqCst));

    // SAFETY: 中断表与本地 APIC 已就绪；打开中断允许 tick 投递。
    unsafe { asm!("sti", options(nomem, nostack)) };
}

/// 停止调度 tick 并关中断。
pub fn stop_tick() {
    // SAFETY: 关中断后写 LVT 屏蔽位，避免仍在途的中断再次进入。
    unsafe { asm!("cli", options(nomem, nostack)) };
    lapic_write(LAPIC_LVT_TIMER, LVT_MASKED | TIMER_VECTOR as u32);
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

/// 返回已记录的定时器中断次数。
pub fn timer_ticks() -> u64 {
    TIMER_TICKS.load(Ordering::SeqCst)
}

/// 判断定时器是否达到目标次数。
pub fn timer_finished() -> bool {
    TIMER_TICKS.load(Ordering::SeqCst) >= TIMER_TARGET.load(Ordering::SeqCst)
}

fn lapic_write(offset: u64, value: u32) {
    let base = LAPIC_BASE.load(Ordering::SeqCst);
    assert!(base != 0, "本地 APIC 未初始化");
    // SAFETY: 基址由调用方映射为设备页，偏移是 LAPIC 寄存器。
    unsafe { ((base + offset) as *mut u32).write_volatile(value) };
}

fn lapic_read(offset: u64) -> u32 {
    let base = LAPIC_BASE.load(Ordering::SeqCst);
    assert!(base != 0, "本地 APIC 未初始化");
    // SAFETY: 基址由调用方映射为设备页，偏移是 LAPIC 寄存器。
    unsafe { ((base + offset) as *const u32).read_volatile() }
}

/// 返回本地 APIC 编号，用于配置 I/O APIC 的目标。
pub fn lapic_id() -> u8 {
    (lapic_read(LAPIC_ID) >> 24) as u8
}

/// 返回异常向量名称。
pub fn exception_name(vector: u64) -> &'static str {
    match vector {
        0 => "除零",
        1 => "调试",
        3 => "断点",
        6 => "非法指令",
        8 => "双重故障",
        13 => "通用保护",
        14 => "页错误",
        17 => "对齐检查",
        19 => "SIMD 浮点",
        20 => "虚拟化",
        21 => "控制保护",
        _ => "未命名异常",
    }
}

core::arch::global_asm!(
    ".macro ISR_NOERR n",
    ".global isr_\\n",
    "isr_\\n:",
    "    push 0",
    "    push \\n",
    "    jmp isr_common",
    ".endm",
    ".macro ISR_ERR n",
    ".global isr_\\n",
    "isr_\\n:",
    "    push \\n",
    "    jmp isr_common",
    ".endm",
    ".section .text",
    "ISR_NOERR 0",
    "ISR_NOERR 1",
    "ISR_NOERR 2",
    "ISR_NOERR 3",
    "ISR_NOERR 4",
    "ISR_NOERR 5",
    "ISR_NOERR 6",
    "ISR_NOERR 7",
    "ISR_ERR 8",
    "ISR_NOERR 9",
    "ISR_ERR 10",
    "ISR_ERR 11",
    "ISR_ERR 12",
    "ISR_ERR 13",
    "ISR_ERR 14",
    "ISR_NOERR 15",
    "ISR_NOERR 16",
    "ISR_ERR 17",
    "ISR_NOERR 18",
    "ISR_NOERR 19",
    "ISR_NOERR 20",
    "ISR_ERR 21",
    "ISR_NOERR 22",
    "ISR_NOERR 23",
    "ISR_NOERR 24",
    "ISR_NOERR 25",
    "ISR_NOERR 26",
    "ISR_NOERR 27",
    "ISR_NOERR 28",
    "ISR_ERR 29",
    "ISR_ERR 30",
    "ISR_NOERR 31",
    "ISR_NOERR 32",
    "ISR_NOERR 33",
    "ISR_NOERR 128",
    ".global isr_common",
    "isr_common:",
    "    push r15",
    "    push r14",
    "    push r13",
    "    push r12",
    "    push r11",
    "    push r10",
    "    push r9",
    "    push r8",
    "    push rbp",
    "    push rdi",
    "    push rsi",
    "    push rdx",
    "    push rcx",
    "    push rbx",
    "    push rax",
    "    mov rbx, rsp",
    "    mov rdi, rsp",
    "    and rsp, -16",
    "    call interrupt_dispatch",
    "    mov rsp, rbx",
    "    pop rax",
    "    pop rbx",
    "    pop rcx",
    "    pop rdx",
    "    pop rsi",
    "    pop rdi",
    "    pop rbp",
    "    pop r8",
    "    pop r9",
    "    pop r10",
    "    pop r11",
    "    pop r12",
    "    pop r13",
    "    pop r14",
    "    pop r15",
    "    add rsp, 16",
    "    iretq",
    ".section .bss",
    ".align 8",
    "hk_kernel_rsp:",
    "    .quad 0",
    "hk_kernel_rip:",
    "    .quad 0",
    ".section .text",
    ".global hk_resume_user",
    "hk_resume_user:",
    "    push rbx",
    "    push rbp",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    mov [rip + hk_kernel_rsp], rsp",
    "    lea rax, [rip + hk_user_return]",
    "    mov [rip + hk_kernel_rip], rax",
    "    mov rbx, rdi",
    "    mov ax, 0x2b",
    "    mov ds, ax",
    "    mov es, ax",
    "    push qword ptr [rbx + 168]",
    "    push qword ptr [rbx + 160]",
    "    push qword ptr [rbx + 152]",
    "    push qword ptr [rbx + 144]",
    "    push qword ptr [rbx + 136]",
    "    mov rax, [rbx + 0]",
    "    mov rcx, [rbx + 16]",
    "    mov rdx, [rbx + 24]",
    "    mov rsi, [rbx + 32]",
    "    mov rdi, [rbx + 40]",
    "    mov rbp, [rbx + 48]",
    "    mov r8, [rbx + 56]",
    "    mov r9, [rbx + 64]",
    "    mov r10, [rbx + 72]",
    "    mov r11, [rbx + 80]",
    "    mov r12, [rbx + 88]",
    "    mov r13, [rbx + 96]",
    "    mov r14, [rbx + 104]",
    "    mov r15, [rbx + 112]",
    "    mov rbx, [rbx + 8]",
    "    iretq",
    "hk_user_return:",
    "    mov ax, 0x10",
    "    mov ds, ax",
    "    mov es, ax",
    "    mov rsp, [rip + hk_kernel_rsp]",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop rbp",
    "    pop rbx",
    "    ret",
    ".global hk_leave_user",
    "hk_leave_user:",
    "    mov rsp, [rip + hk_kernel_rsp]",
    "    mov rax, [rip + hk_kernel_rip]",
    "    jmp rax",
);

/// 用户态异常 hook：返回 `true` 表示异常已处理并应返回调度器。
pub type FaultHook = fn(&mut InterruptFrame) -> bool;

static FAULT_HOOK: Mutex<Option<FaultHook>> = Mutex::new(None);

/// 注册用户态异常处理 hook。
pub fn set_fault_hook(hook: FaultHook) {
    *FAULT_HOOK.lock() = Some(hook);
}

/// 中断分派：定时器计数、键盘输入、系统调用分派，其余向量按异常处理。
#[unsafe(no_mangle)]
extern "C" fn interrupt_dispatch(frame: &mut InterruptFrame) {
    if frame.vector == TIMER_VECTOR {
        TIMER_TICKS.fetch_add(1, Ordering::SeqCst);
        let switch = frame.cs == u64::from(USER_CODE_SELECTOR) && preempt(frame);
        lapic_write(LAPIC_EOI, 0);
        if switch {
            leave_user();
        }
        return;
    }
    if frame.vector == KEYBOARD_VECTOR {
        crate::keyboard::handle_interrupt();
        lapic_write(LAPIC_EOI, 0);
        return;
    }
    if frame.vector == SYSCALL_VECTOR {
        handle_syscall(frame);
        return;
    }

    if frame.cs == u64::from(USER_CODE_SELECTOR) {
        let hook = *FAULT_HOOK.lock();
        if let Some(hook) = hook
            && hook(frame)
        {
            leave_user();
        }
    }

    report_exception(frame);
}

/// 用户态可访问的地址上限。
const USER_LIMIT: u64 = 0x0000_0080_0000_0000;

/// 系统调用允许的最大写入长度。
const MAX_WRITE: usize = 4096;

fn handle_syscall(frame: &mut InterruptFrame) {
    if frame.rax == hk_server_api::SYS_WRITE {
        let pointer = frame.rdi;
        let length = frame.rsi as usize;
        frame.rax = if user_range_valid(pointer, length) {
            write_user(pointer, length)
        } else {
            u64::MAX
        };
        return;
    }

    let hook = SYSCALL_HOOK.load(Ordering::SeqCst);
    if hook != 0 {
        // SAFETY: 钩子由内核在进入用户态之前注册，指向静态函数并覆盖整个内核运行期。
        let hook: SyscallHook = unsafe { core::mem::transmute(hook) };
        match hook(frame) {
            SyscallAction::Continue => {}
            SyscallAction::Block | SyscallAction::Exit => leave_user(),
        }
        return;
    }

    match frame.rax {
        hk_server_api::SYS_EXIT => leave_user(),
        _ => frame.rax = u64::MAX,
    }
}

/// 注册扩展系统调用处理函数。
pub fn set_syscall_hook(hook: SyscallHook) {
    SYSCALL_HOOK.store(hook as usize, Ordering::SeqCst);
}

/// 注册用户态抢占钩子。
pub fn set_preempt_hook(hook: PreemptHook) {
    PREEMPT_HOOK.store(hook as usize, Ordering::SeqCst);
}

fn preempt(frame: &InterruptFrame) -> bool {
    let hook = PREEMPT_HOOK.load(Ordering::SeqCst);
    if hook == 0 {
        return false;
    }
    // SAFETY: 钩子在进入用户态前注册，指向静态函数并覆盖整个内核运行期。
    let hook: PreemptHook = unsafe { core::mem::transmute(hook) };
    hook(frame)
}

fn user_range_valid(pointer: u64, length: usize) -> bool {
    length <= MAX_WRITE
        && pointer >= 0x1000
        && pointer
            .checked_add(length as u64)
            .is_some_and(|end| end <= USER_LIMIT)
}

fn write_user(pointer: u64, length: usize) -> u64 {
    // SAFETY: 调用前已校验地址落在用户可访问范围内，且进程页表中有映射。
    let bytes = unsafe { core::slice::from_raw_parts(pointer as *const u8, length) };
    match core::str::from_utf8(bytes) {
        Ok(text) => {
            Serial::new(COM1).write_str(text);
            length as u64
        }
        Err(_) => u64::MAX,
    }
}

/// 构造用户态初始现场：入口、栈顶与首个参数。
pub fn user_frame(entry: u64, stack_top: u64, argument: u64) -> InterruptFrame {
    InterruptFrame {
        rax: 0,
        rbx: 0,
        rcx: 0,
        rdx: 0,
        rsi: 0,
        rdi: argument,
        rbp: 0,
        r8: 0,
        r9: 0,
        r10: 0,
        r11: 0,
        r12: 0,
        r13: 0,
        r14: 0,
        r15: 0,
        vector: 0,
        error: 0,
        rip: entry,
        cs: u64::from(USER_CODE_SELECTOR),
        rflags: 0x202,
        rsp: stack_top,
        ss: u64::from(USER_DATA_SELECTOR),
    }
}

/// 按保存的现场恢复用户态执行，直到任务再次阻塞或退出。
///
/// 返回时机是系统调用处理函数要求切换任务之后；`frame` 只在使用期间读取。
pub fn resume_user(frame: &InterruptFrame) {
    // SAFETY: `frame` 按 `InterruptFrame` 布局保存完整用户态寄存器与栈，
    // 对应地址已映射为用户可执行或可写页。
    unsafe { hk_resume_user(frame) };
}

impl hk_hal::UserContext for InterruptFrame {
    fn resume(&self) {
        resume_user(self);
    }

    fn set_start_argument(&mut self, argument: u64) {
        self.rdi = argument;
    }
}

impl hk_hal::SyscallFrame for InterruptFrame {
    fn number(&self) -> u64 {
        self.rax
    }

    fn arg0(&self) -> u64 {
        self.rdi
    }

    fn arg1(&self) -> u64 {
        self.rsi
    }

    fn arg2(&self) -> u64 {
        self.rdx
    }

    fn arg3(&self) -> u64 {
        self.r10
    }

    fn set_result(&mut self, value: u64) {
        self.rax = value;
    }

    fn set_word(&mut self, value: u64) {
        self.rdx = value;
    }

    fn set_sender(&mut self, value: u64) {
        self.rcx = value;
    }

    fn set_handle(&mut self, value: u64) {
        self.r8 = value;
    }
}

/// 从系统调用回到内核调度器，不返回。
pub fn leave_user() -> ! {
    // SAFETY: 只在系统调用处理内部调用；内核上下文由 `resume_user` 保存。
    unsafe { hk_leave_user() }
}

unsafe extern "C" {
    fn hk_resume_user(frame: *const InterruptFrame);
    fn hk_leave_user() -> !;
}

fn report_exception(frame: &InterruptFrame) -> ! {
    let console = Serial::new(COM1);
    let mut report = String::new();

    let _ = writeln!(
        report,
        "\n内核异常：{}（向量 {}，错误码 {:#x}）",
        exception_name(frame.vector),
        frame.vector,
        frame.error
    );
    let _ = writeln!(
        report,
        "  rip={:#018x} cs={:#x} rflags={:#018x}",
        frame.rip, frame.cs, frame.rflags
    );
    let _ = writeln!(report, "  rsp={:#018x} ss={:#x}", frame.rsp, frame.ss);

    if frame.vector == 14 || frame.vector == 8 {
        let cr2: u64;
        // SAFETY: CR2 只在异常处理中使用，读取不影响机器状态。
        unsafe {
            asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack, preserves_flags));
        }
        let _ = writeln!(report, "  出错地址 cr2={cr2:#018x}");
    }

    let _ = writeln!(
        report,
        "  rax={:#018x} rbx={:#018x} rcx={:#018x} rdx={:#018x}",
        frame.rax, frame.rbx, frame.rcx, frame.rdx
    );
    let _ = writeln!(
        report,
        "  rsi={:#018x} rdi={:#018x} rbp={:#018x}",
        frame.rsi, frame.rdi, frame.rbp
    );
    let _ = writeln!(
        report,
        "  r8={:#018x} r9={:#018x} r10={:#018x} r11={:#018x}",
        frame.r8, frame.r9, frame.r10, frame.r11
    );
    let _ = writeln!(
        report,
        "  r12={:#018x} r13={:#018x} r14={:#018x} r15={:#018x}",
        frame.r12, frame.r13, frame.r14, frame.r15
    );

    console.write_str(&report);
    hk_test::exit(hk_test::ExitCode::Failure(70))
}
