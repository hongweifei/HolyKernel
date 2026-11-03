//! CPU 初始化：GDT、TSS、IDT、本地 APIC 与异常处理。
//!
//! 建立平坦 64 位段、双重故障专用 IST 栈、32 个异常入口与定时器中断。异常会
//! 打印向量、错误码与寄存器现场；定时器中断只累加计数并回 EOI。

use alloc::string::String;
use core::arch::asm;
use core::cell::UnsafeCell;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use hk_hal::Console;

use crate::port::outb;
use crate::serial::{COM1, Serial};

/// 内核代码段选择子。
pub const KERNEL_CODE: u16 = 0x08;

/// 内核数据段选择子。
pub const KERNEL_DATA: u16 = 0x10;

/// TSS 选择子。
pub const TSS_SELECTOR: u16 = 0x18;

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

/// 中断门属性：P=1、DPL=0、type=0xE。
const INTERRUPT_GATE: u8 = 0x8E;

const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

const PIT_CHANNEL0: u16 = 0x40;
const PIT_COMMAND: u16 = 0x43;

/// PIT 基准频率。
const PIT_FREQUENCY: u32 = 1_193_182;

/// 定时器中断频率。
const TIMER_HZ: u32 = 100;

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
        let handler = handler as u64;
        Self {
            offset_low: handler as u16,
            selector: KERNEL_CODE,
            ist,
            attributes: INTERRUPT_GATE,
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        }
    }
}

/// 中断与异常现场。
#[repr(C)]
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
static TIMER_TICKS: AtomicU64 = AtomicU64::new(0);
static TIMER_TARGET: AtomicU64 = AtomicU64::new(0);

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

        let pointer = DescriptorPointer {
            limit: (core::mem::size_of::<[IdtEntry; 256]>() - 1) as u16,
            base: idt.as_ptr() as u64,
        };
        asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
    }
}

/// 初始化定时器中断：重映射 8259 PIC 到向量 32..47，编程 8254 PIT。
///
/// 8254 与 8259 走端口 I/O，不依赖内核分页；APIC/IOAPIC 需要映射 MMIO 页，
/// 随内核分页在 M3 接入。
pub fn init_timer() {
    // SAFETY: 8259 与 8254 端口在 x86_64 上固定存在；初始化序列只写这些端口。
    unsafe {
        // ICW1：边沿触发、级联、需要 ICW4。
        outb(PIC1_COMMAND, 0x11);
        outb(PIC2_COMMAND, 0x11);
        // ICW2：主片向量基址 32，从片向量基址 40。
        outb(PIC1_DATA, 0x20);
        outb(PIC2_DATA, 0x28);
        // ICW3：主片 IRQ2 接从片，从片级联到 IRQ2。
        outb(PIC1_DATA, 0x04);
        outb(PIC2_DATA, 0x02);
        // ICW4：8086 模式。
        outb(PIC1_DATA, 0x01);
        outb(PIC2_DATA, 0x01);
        // 只开主片 IRQ0（定时器），其余屏蔽。
        outb(PIC1_DATA, 0xFE);
        outb(PIC2_DATA, 0xFF);

        // 8254 通道 0：模式 3（方波），先低字节后高字节。
        let divisor = (PIT_FREQUENCY / TIMER_HZ) as u16;
        outb(PIT_COMMAND, 0x36);
        outb(PIT_CHANNEL0, (divisor & 0xFF) as u8);
        outb(PIT_CHANNEL0, (divisor >> 8) as u8);
    }
}

/// 开中断，等待定时器中断。
pub fn arm_timer(target: u64) {
    TIMER_TICKS.store(0, Ordering::SeqCst);
    TIMER_TARGET.store(target, Ordering::SeqCst);

    // SAFETY: 开中断允许定时器中断进入；IDT 与 PIC 已就绪。
    unsafe { asm!("sti", options(nomem, nostack)) };
}

/// 停止定时器并关中断。
pub fn disarm_timer() {
    // SAFETY: 关中断后屏蔽 IRQ0，避免仍在途的中断再次进入。
    unsafe {
        asm!("cli", options(nomem, nostack));
        outb(PIC1_DATA, 0xFF);
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
);

/// 中断分派：定时器只计数并回 EOI，其余向量按异常处理。
#[unsafe(no_mangle)]
extern "C" fn interrupt_dispatch(frame: &InterruptFrame) {
    if frame.vector == TIMER_VECTOR {
        TIMER_TICKS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: 8259 主片命令端口固定存在；写 EOI 允许后续中断进入。
        unsafe { outb(PIC1_COMMAND, 0x20) };
        return;
    }
    report_exception(frame);
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

    if frame.vector == 14 {
        let cr2: u64;
        // SAFETY: CR2 只在页错误处理中使用，读取不影响机器状态。
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
