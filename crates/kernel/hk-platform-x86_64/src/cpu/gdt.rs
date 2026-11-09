//! GDT、TSS 与 per-CPU 内核栈。

use core::arch::asm;
use core::cell::UnsafeCell;

use x86_64::VirtAddr;
use x86_64::instructions::tables;
use x86_64::instructions::tables::DescriptorTablePointer;
use x86_64::registers::model_specific::Msr;
use x86_64::registers::segmentation::SegmentSelector;

use super::*;

#[repr(C, align(64))]
struct SyscallStack(UnsafeCell<[u8; SYSCALL_STACK_SIZE]>);

#[repr(C, align(16))]
struct DoubleFaultStack(UnsafeCell<[u8; DOUBLE_FAULT_STACK_SIZE]>);

/// 系统调用/中断栈顶。
///
/// 留出 16 字节，使压入硬件帧与 15 个通用寄存器（共 176 字节）后的
/// `rsp` 为 64 字节对齐，满足 XSAVE 区域的对齐要求。
fn syscall_stack_top(cpu: usize) -> u64 {
    CPUS[cpu].syscall.0.get() as u64 + SYSCALL_STACK_SIZE as u64 - 16
}

fn double_fault_stack_top(cpu: usize) -> u64 {
    CPUS[cpu].double_fault.0.get() as u64 + DOUBLE_FAULT_STACK_SIZE as u64
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
struct CpuState {
    gdt: UnsafeCell<[u64; 7]>,
    tss: UnsafeCell<TaskStateSegment>,
    syscall: SyscallStack,
    double_fault: DoubleFaultStack,
}

impl CpuState {
    const fn new() -> Self {
        Self {
            gdt: UnsafeCell::new([0; 7]),
            tss: UnsafeCell::new(TaskStateSegment::new()),
            syscall: SyscallStack(UnsafeCell::new([0; SYSCALL_STACK_SIZE])),
            double_fault: DoubleFaultStack(UnsafeCell::new([0; DOUBLE_FAULT_STACK_SIZE])),
        }
    }
}

// SAFETY: 每 CPU 的表只在自身初始化时写入，之后由该 CPU 只读访问。
unsafe impl Sync for CpuState {}

static CPUS: [CpuState; MAX_CPUS] = [const { CpuState::new() }; MAX_CPUS];

#[repr(C)]
struct UserReturn {
    kernel_rsp: u64,
    kernel_rip: u64,
}

struct UserReturnCell(UnsafeCell<UserReturn>);

// SAFETY: 每 CPU 只写自己的槽位，且只在自身上下文切换路径访问。
unsafe impl Sync for UserReturnCell {}

static USER_RETURNS: [UserReturnCell; MAX_CPUS] = [const {
    UserReturnCell(UnsafeCell::new(UserReturn {
        kernel_rsp: 0,
        kernel_rip: 0,
    }))
}; MAX_CPUS];

pub(super) fn init_gdt(cpu: usize) {
    configure_tss(cpu);
    load_gdt(cpu);
}

fn configure_tss(cpu: usize) {
    let state = &CPUS[cpu];
    let tss_base = state.tss.get() as u64;
    let tss_limit = (core::mem::size_of::<TaskStateSegment>() - 1) as u64;

    // SAFETY: 该 CPU 尚未启用中断，独占写入自己的 GDT 与 TSS。
    unsafe {
        let tss = &mut *state.tss.get();
        tss.rsp0 = syscall_stack_top(cpu);
        tss.ist[DOUBLE_FAULT_IST as usize - 1] = double_fault_stack_top(cpu);

        let gdt = &mut *state.gdt.get();
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
    }
}

fn load_gdt(cpu: usize) {
    let state = &CPUS[cpu];

    // SAFETY: GDT 已由本 CPU 初始化；加载后立即切换到同一 GDT 的段选择子。
    unsafe {
        let gdt = &*state.gdt.get();
        let pointer = DescriptorTablePointer {
            limit: (core::mem::size_of::<[u64; 7]>() - 1) as u16,
            base: VirtAddr::new(gdt.as_ptr() as u64),
        };
        tables::lgdt(&pointer);

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
        tables::load_tss(SegmentSelector(TSS_SELECTOR));

        let base = USER_RETURNS[cpu].0.get() as u64;
        let mut gs_base = Msr::new(0xC000_0101);
        gs_base.write(base);
    }
}
