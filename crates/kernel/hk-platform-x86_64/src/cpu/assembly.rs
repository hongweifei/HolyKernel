//! x86_64 中断与用户态恢复汇编。

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
    // SAFETY: CPUID 是纯读取指令；写入的是当前 CPU 的 EFER。
    unsafe {
        let extended = core::arch::x86_64::__cpuid(0x8000_0000).eax;
        assert!(
            extended >= CPUID_EXTENDED,
            "CPU 不支持扩展 CPUID，无法启用 NX"
        );
        assert!(
            core::arch::x86_64::__cpuid(CPUID_EXTENDED).edx & CPUID_NX != 0,
            "CPU 不支持 NX"
        );

        let low: u32;
        let high: u32;
        core::arch::asm!(
            "rdmsr",
            in("ecx") IA32_EFER,
            out("eax") low,
            out("edx") high,
            options(nostack, preserves_flags),
        );
        let value = (u64::from(high) << 32) | u64::from(low);
        let value = value | EFER_NXE;
        core::arch::asm!(
            "wrmsr",
            in("ecx") IA32_EFER,
            in("eax") value as u32,
            in("edx") (value >> 32) as u32,
            options(nostack, preserves_flags),
        );
    }
}

/// 启用 x87/SSE：清除 CR0.EM/TS，置 CR0.MP/NE 与 CR4.OSFXSR/OSXMMEXCPT。
///
/// # Panics
/// CPU 未报告 FXSR 或 SSE 时 panic：用户态恢复依赖 `FXSAVE`/`FXRSTOR`。
pub(super) fn enable_fpu() {
    // SAFETY: CPUID 是纯读取指令；写入的是当前 CPU 的 CR0/CR4。
    unsafe {
        let features = core::arch::x86_64::__cpuid(1).edx;
        const FXSR: u32 = 1 << 24;
        const SSE: u32 = 1 << 25;
        assert!(
            features & FXSR != 0 && features & SSE != 0,
            "CPU 不支持 FXSAVE/SSE"
        );

        let mut cr0: u64;
        core::arch::asm!(
            "mov {}, cr0",
            out(reg) cr0,
            options(nomem, nostack, preserves_flags),
        );
        cr0 &= !((1 << 2) | (1 << 3));
        cr0 |= (1 << 1) | (1 << 5);
        core::arch::asm!(
            "mov cr0, {}",
            in(reg) cr0,
            options(nomem, nostack, preserves_flags),
        );

        let mut cr4: u64;
        core::arch::asm!(
            "mov {}, cr4",
            out(reg) cr4,
            options(nomem, nostack, preserves_flags),
        );
        cr4 |= (1 << 9) | (1 << 10);
        core::arch::asm!(
            "mov cr4, {}",
            in(reg) cr4,
            options(nomem, nostack, preserves_flags),
        );
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
    // 仅用户态现场保存 FPU/SSE；用户栈来自 16 字节对齐的 TSS.rsp0，
    // 因此 [rsp + 176] 满足 FXSAVE 的对齐要求。内核态中断不触碰 FPU。
    "    test byte ptr [rsp + 144], 3",
    "    jz 7f",
    "    fxsave [rsp + 176]",
    "7:",
    "    mov rbx, rsp",
    "    mov rdi, rsp",
    "    and rsp, -16",
    "    call interrupt_dispatch",
    "    mov rsp, rbx",
    "    test byte ptr [rsp + 144], 3",
    "    jz 8f",
    "    fxrstor [rsp + 176]",
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
    "    fxrstor [rbx + 176]",
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
