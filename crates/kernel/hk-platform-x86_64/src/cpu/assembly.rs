//! x86_64 中断与用户态恢复汇编。

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use x86_64::registers::control::{Cr0, Cr4};
use x86_64::registers::model_specific::Msr;

/// 是否启用了 SMAP（决定 STAC/CLAC 是否可执行）。
static SMAP_ENABLED: AtomicBool = AtomicBool::new(false);

/// 是否启用 XSAVE/XRSTOR 上下文保存；0 表示使用 FXSAVE/FXRSTOR。
#[unsafe(no_mangle)]
pub static HK_XSAVE_ENABLED: AtomicU8 = AtomicU8::new(0);

/// 当前 XCR0 掩码；XSAVE/XRSTOR 的 EDX:EAX 必须与之一致。
#[unsafe(no_mangle)]
pub static HK_XSAVE_MASK: AtomicU64 = AtomicU64::new(0);

/// 返回当前 XCR0 掩码；未启用 XSAVE 时为 0。
pub fn fpu_xstate_mask() -> u64 {
    HK_XSAVE_MASK.load(Ordering::Acquire)
}

/// 启用 SMEP/SMAP；CPU 不支持时跳过。
pub(super) fn enable_hardening() {
    let features = crate::cpu::Features::detect();
    let mut cr4 = Cr4::read_raw();
    if features.smep {
        cr4 |= 1 << 20;
    }
    if features.smap {
        cr4 |= 1 << 21;
        SMAP_ENABLED.store(true, Ordering::Release);
    }
    // SAFETY: 只置 CPUID 报告的 SMEP/SMAP 位。
    unsafe { Cr4::write_raw(cr4) };
}

/// 开始访问用户内存；启用 SMAP 时清除 AC。
pub fn user_access_begin() {
    if SMAP_ENABLED.load(Ordering::Acquire) {
        // SAFETY: CPUID 已确认支持 SMAP；STAC 只设置 AC 标志。
        // 不声明 nomem/preserves_flags：AC 属于 EFLAGS，且必须阻止用户访问被移出保护区。
        unsafe { core::arch::asm!("stac", options(nostack)) };
    }
}

/// 结束用户内存访问；启用 SMAP 时恢复 AC。
pub fn user_access_end() {
    if SMAP_ENABLED.load(Ordering::Acquire) {
        // SAFETY: CLAC 只清除 AC 标志。
        unsafe { core::arch::asm!("clac", options(nostack)) };
    }
}

/// EFER 寄存器的 MSR 编号。
const IA32_EFER: u32 = 0xC000_0080;
/// EFER.NXE：允许页表项表达不可执行。
const EFER_NXE: u64 = 1 << 11;
/// CPUID 扩展功能叶。
const CPUID_EXTENDED: u32 = 0x8000_0001;
/// 扩展功能叶 EDX 的 NX 支持位。
const CPUID_NX: u32 = 1 << 20;

/// 启用 EFER.NXE，让页表项的 NX 位生效。
///
/// # Panics
/// CPU 不支持 NX 时 panic：HMP 的 W^X 依赖 NX 表达不可执行页。
pub(super) fn enable_no_execute() {
    let extended = core::arch::x86_64::__cpuid(0x8000_0000).eax;
    assert!(
        extended >= CPUID_EXTENDED,
        "CPU 不支持扩展 CPUID，无法启用 NX"
    );
    assert!(
        core::arch::x86_64::__cpuid(CPUID_EXTENDED).edx & CPUID_NX != 0,
        "CPU 不支持 NX"
    );

    let mut efer = Msr::new(IA32_EFER);
    // SAFETY: 读取 EFER 不改变机器状态；只在 ring 0 执行。
    let value = unsafe { efer.read() } | EFER_NXE;
    // SAFETY: EFER.NXE 是合法位；写入只影响当前 CPU。
    unsafe { efer.write(value) };
}

/// 启用 x87/SSE：清除 CR0.EM/TS，置 CR0.MP/NE 与 CR4.OSFXSR/OSXMMEXCPT。
///
/// # Panics
/// CPU 未报告 FXSR 或 SSE 时 panic：用户态恢复依赖 `FXSAVE`/`FXRSTOR`。
pub(super) fn enable_fpu() {
    let edx = core::arch::x86_64::__cpuid(1).edx;
    const FXSR: u32 = 1 << 24;
    const SSE: u32 = 1 << 25;
    assert!(edx & FXSR != 0 && edx & SSE != 0, "CPU 不支持 FXSAVE/SSE");
    let features = crate::cpu::Features::detect();

    let cr0 = Cr0::read_raw();
    let cr0 = (cr0 & !((1 << 2) | (1 << 3))) | (1 << 1) | (1 << 5);
    // SAFETY: 清除 EM/TS 并置 MP/NE 是启用 x87/SSE 的合法组合。
    unsafe { Cr0::write_raw(cr0) };

    let mut cr4 = Cr4::read_raw() | (1 << 9) | (1 << 10);
    if features.xsave {
        cr4 |= 1 << 18; // OSXSAVE
    }
    // SAFETY: OSFXSR/OSXMMEXCPT（以及可选的 OSXSAVE）是合法组合。
    unsafe { Cr4::write_raw(cr4) };

    if features.xsave {
        // 只启用 x87/SSE，CPU 支持 AVX 时再启用 AVX；不启用 AVX-512。
        let supported = core::arch::x86_64::__cpuid_count(0xD, 0).eax as u64;
        let mut xcr0 = 0b11;
        if features.avx && supported & 0b100 != 0 {
            xcr0 |= 0b100;
        }
        if supported & 0b11 == 0b11 {
            // SAFETY: CR4.OSXSAVE 已置位，XCR0 只写 CPUID 支持的位。
            unsafe {
                core::arch::asm!(
                    "xsetbv",
                    in("ecx") 0_u32,
                    in("eax") xcr0 as u32,
                    in("edx") (xcr0 >> 32) as u32,
                    options(nostack),
                );
            }
            HK_XSAVE_MASK.store(xcr0, Ordering::Release);
            HK_XSAVE_ENABLED.store(1, Ordering::Release);
        }
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
    "ISR_NOERR 64",
    "ISR_NOERR 65",
    "ISR_NOERR 66",
    "ISR_NOERR 67",
    "ISR_NOERR 68",
    "ISR_NOERR 69",
    "ISR_NOERR 70",
    "ISR_NOERR 71",
    "ISR_NOERR 72",
    "ISR_NOERR 73",
    "ISR_NOERR 74",
    "ISR_NOERR 75",
    "ISR_NOERR 76",
    "ISR_NOERR 77",
    "ISR_NOERR 78",
    "ISR_NOERR 79",
    "ISR_NOERR 80",
    "ISR_NOERR 81",
    "ISR_NOERR 82",
    "ISR_NOERR 83",
    "ISR_NOERR 84",
    "ISR_NOERR 85",
    "ISR_NOERR 86",
    "ISR_NOERR 87",
    "ISR_NOERR 88",
    "ISR_NOERR 89",
    "ISR_NOERR 90",
    "ISR_NOERR 91",
    "ISR_NOERR 92",
    "ISR_NOERR 93",
    "ISR_NOERR 94",
    "ISR_NOERR 95",
    "ISR_NOERR 128",
    "ISR_NOERR 240",
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
    // 仅用户态现场保存 FPU/AVX；TSS.rsp0 调整为 64 字节对齐减 16，
    // 因此 [rsp + 192] 满足 XSAVE/FXSAVE 的对齐要求。内核态中断不触碰 FPU。
    "    test byte ptr [rsp + 144], 3",
    "    jz 7f",
    "    lea rdi, [rsp + 192]",
    "    call hk_fpu_save",
    "7:",
    "    mov rbx, rsp",
    "    mov rdi, rsp",
    "    and rsp, -16",
    "    call interrupt_dispatch",
    "    mov rsp, rbx",
    "    test byte ptr [rsp + 144], 3",
    "    jz 8f",
    "    lea rdi, [rsp + 192]",
    "    call hk_fpu_restore",
    "8:",
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
    ".section .text",
    ".global hk_resume_user",
    "hk_resume_user:",
    "    push rbx",
    "    push rbp",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    mov gs:[0], rsp",
    "    mov rbx, rdi",
    "    lea rdi, [rbx + 192]",
    "    call hk_fpu_restore",
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
    "    mov rsp, gs:[0]",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop rbp",
    "    pop rbx",
    "    ret",
    ".global hk_leave_user",
    "hk_leave_user:",
    "    mov rsp, gs:[0]",
    // 返回标签是固定的，直接用 RIP 相对地址，避免依赖可能被清零的 gs:[8]。
    "    lea rax, [rip + hk_user_return]",
    "    jmp rax",
);

core::arch::global_asm!(
    ".section .text",
    ".global hk_fpu_save",
    "hk_fpu_save:",
    "    cmp byte ptr [rip + HK_XSAVE_ENABLED], 0",
    "    je hk_fpu_save_legacy",
    "    mov eax, dword ptr [rip + HK_XSAVE_MASK]",
    "    mov edx, dword ptr [rip + HK_XSAVE_MASK + 4]",
    "    xsave [rdi]",
    "    ret",
    "hk_fpu_save_legacy:",
    "    fxsave [rdi]",
    "    ret",
    ".global hk_fpu_restore",
    "hk_fpu_restore:",
    "    cmp byte ptr [rip + HK_XSAVE_ENABLED], 0",
    "    je hk_fpu_restore_legacy",
    "    mov eax, dword ptr [rip + HK_XSAVE_MASK]",
    "    mov edx, dword ptr [rip + HK_XSAVE_MASK + 4]",
    "    xrstor [rdi]",
    "    ret",
    "hk_fpu_restore_legacy:",
    "    fxrstor [rdi]",
    "    ret",
);
