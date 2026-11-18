//! GDT、TSS 与 per-CPU 内核栈。

use core::arch::asm;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

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

/// 动态分配的 per-CPU 系统调用栈顶；0 表示仍使用静态栈。
static DYNAMIC_STACK_TOP: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// 系统调用/中断栈顶。
///
/// 优先使用启动后分配的带 guard page 的动态栈；否则退回静态栈。两者都留出
/// 16 字节，使压入硬件帧与 15 个通用寄存器（共 176 字节）后的 `rsp` 为
/// 64 字节对齐，满足 XSAVE 区域的对齐要求。
fn syscall_stack_top(cpu: usize) -> u64 {
    let dynamic = DYNAMIC_STACK_TOP[cpu].load(Ordering::Acquire);
    if dynamic != 0 {
        return dynamic;
    }
    CPUS[cpu].syscall.0.get() as u64 + SYSCALL_STACK_SIZE as u64
        - super::frame::STACK_RESERVE as u64
}

/// 为 CPU 设置动态系统调用栈顶，并同步 TSS.rsp0。
pub(crate) fn set_syscall_stack_top(cpu: usize, top: u64) {
    DYNAMIC_STACK_TOP[cpu].store(top, Ordering::Release);
    // SAFETY: 该 CPU 尚未使用新栈；写入自己的 TSS 槽位与 GS 入口槽位。
    unsafe {
        (*CPUS[cpu].tss.get()).rsp0 = top;
        (*USER_RETURNS[cpu].0.get()).syscall_rsp = top;
    }
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

/// per-CPU 用户态返回槽；`gs` 基址指向本结构。
#[repr(C)]
struct UserReturn {
    /// 进入用户态前保存的内核栈指针。
    kernel_rsp: u64,
    /// 系统调用入口切换到内核栈时使用的栈顶。
    syscall_rsp: u64,
    /// 系统调用入口保存的用户栈指针。
    user_rsp: u64,
}

struct UserReturnCell(UnsafeCell<UserReturn>);

// SAFETY: 每 CPU 只写自己的槽位，且只在自身上下文切换路径访问。
unsafe impl Sync for UserReturnCell {}

static USER_RETURNS: [UserReturnCell; MAX_CPUS] = [const {
    UserReturnCell(UnsafeCell::new(UserReturn {
        kernel_rsp: 0,
        syscall_rsp: 0,
        user_rsp: 0,
    }))
}; MAX_CPUS];

// 汇编按固定偏移访问该结构，偏移变化必须同步修改 `cpu/assembly.rs`。
const _: () = {
    assert!(core::mem::offset_of!(UserReturn, kernel_rsp) == 0);
    assert!(core::mem::offset_of!(UserReturn, syscall_rsp) == 8);
    assert!(core::mem::offset_of!(UserReturn, user_rsp) == 16);
};

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
        // `syscall` 入口通过 gs:[8] 取得内核栈顶。
        (*USER_RETURNS[cpu].0.get()).syscall_rsp = tss.rsp0;

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
    enable_syscall();
}

/// 启用 `syscall`/`sysret` 快路径。
fn enable_syscall() {
    const IA32_EFER: u32 = 0xC000_0080;
    const IA32_STAR: u32 = 0xC000_0081;
    const IA32_LSTAR: u32 = 0xC000_0082;
    const IA32_FMASK: u32 = 0xC000_0084;
    const EFER_SCE: u64 = 1;

    unsafe extern "C" {
        fn hk_syscall_entry();
    }

    // SAFETY: 这些是当前 CPU 的系统调用 MSR；SCE 只在长模式有效。
    unsafe {
        let mut efer = Msr::new(IA32_EFER);
        efer.write(efer.read() | EFER_SCE);
        // SYSCALL: CS=0x08、SS=0x10；SYSRET: CS=0x33、SS=0x2b。
        Msr::new(IA32_STAR).write((0x23_u64 << 48) | (u64::from(KERNEL_CODE) << 32));
        Msr::new(IA32_LSTAR).write(hk_syscall_entry as *const () as usize as u64);
        // 进入内核时清除 TF/IF/DF/AC：不嵌套中断，且关闭 SMAP 绕过。
        Msr::new(IA32_FMASK).write(0x0004_0700);
    }
}
