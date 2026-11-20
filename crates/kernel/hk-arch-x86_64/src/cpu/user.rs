//! 用户态现场、异常处理与恢复。

use core::fmt::Write;

use x86_64::registers::control::Cr2;
use x86_64::registers::model_specific::Msr;

use hk_hal::Console;
use spin::Mutex;

use super::frame::FpuState;
use super::{InterruptFrame, USER_CODE_SELECTOR, USER_DATA_SELECTOR};
use crate::serial::{COM1, Serial};

/// 用户态可访问的地址上限。
const USER_LIMIT: u64 = 0x0000_0080_0000_0000;

/// 返回异常向量名称。
pub fn exception_name(vector: u64) -> &'static str {
    match vector {
        0 => "除零",
        1 => "调试",
        2 => "不可屏蔽中断",
        3 => "断点",
        6 => "非法指令",
        7 => "设备不可用",
        8 => "双重故障",
        13 => "通用保护",
        14 => "页错误",
        17 => "对齐检查",
        18 => "机器检查",
        19 => "SIMD 浮点",
        20 => "虚拟化",
        21 => "控制保护",
        _ => "未命名异常",
    }
}
/// 用户态异常处理动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultAction {
    /// 已修复，返回用户态重试。
    Resume,
    /// 终止当前任务并返回调度器。
    Exit,
    /// 当前任务已阻塞，返回调度器。
    Block,
}

/// 用户态异常 hook。
pub type FaultHook = fn(&mut InterruptFrame) -> FaultAction;

static FAULT_HOOK: Mutex<Option<FaultHook>> = Mutex::new(None);

pub(super) fn dispatch_fault(frame: &mut InterruptFrame) -> Option<FaultAction> {
    (*FAULT_HOOK.lock()).map(|hook| hook(frame))
}

/// 注册用户态异常处理 hook。
pub fn set_fault_hook(hook: FaultHook) {
    *FAULT_HOOK.lock() = Some(hook);
}

/// NMI 处理 hook。
pub type NmiHook = fn(&mut InterruptFrame);

static NMI_HOOK: Mutex<Option<NmiHook>> = Mutex::new(None);
static MCE_HOOK: Mutex<Option<NmiHook>> = Mutex::new(None);

/// 注册 NMI 处理 hook；未注册时记录并返回。
pub fn set_nmi_hook(hook: NmiHook) {
    *NMI_HOOK.lock() = Some(hook);
}

/// 注册 MCE 处理 hook；未注册时按致命异常处理。
pub fn set_mce_hook(hook: NmiHook) {
    *MCE_HOOK.lock() = Some(hook);
}

/// 处理 NMI：默认只记录，不终止系统。
pub(super) fn handle_nmi(frame: &mut InterruptFrame) {
    if let Some(hook) = *NMI_HOOK.lock() {
        hook(frame);
        return;
    }
    let mut report = DirectWriter(Serial::new(COM1));
    let _ = writeln!(
        report,
        "\n不可屏蔽中断：rip={:#018x} cs={:#x} rsp={:#018x}",
        frame.rip, frame.cs, frame.rsp
    );
}

/// 处理机器检查：hook 可恢复；否则按状态判断能否恢复。
pub(super) fn handle_mce(frame: &mut InterruptFrame) {
    if let Some(hook) = *MCE_HOOK.lock() {
        hook(frame);
        return;
    }
    if report_mce() {
        return;
    }
    report_exception(frame)
}

/// 判断机器检查状态是否允许恢复。
///
/// 需要 MCG_STATUS.MCIP/RIPV/EIPV 全置位，且至少一个有效 bank；
/// 任一有效 bank 标记 UC（未纠正）或 PCC（上下文损坏）时不可恢复。
fn recoverable(mcg_status: u64, banks: &[u64]) -> bool {
    const MCIP: u64 = 1 << 2;
    const RIPV: u64 = 1;
    const EIPV: u64 = 1 << 1;
    const VALID: u64 = 1 << 63;
    const UC: u64 = 1 << 61;
    const PCC: u64 = 1 << 57;

    if mcg_status & (MCIP | RIPV | EIPV) != (MCIP | RIPV | EIPV) {
        return false;
    }
    let mut valid = false;
    for status in banks {
        if status & VALID == 0 {
            continue;
        }
        valid = true;
        if status & (UC | PCC) != 0 {
            return false;
        }
    }
    valid
}

/// 读取并打印机器检查状态寄存器；返回是否已恢复（并清除状态）。
fn report_mce() -> bool {
    const IA32_MCG_CAP: u32 = 0x179;
    const IA32_MCG_STATUS: u32 = 0x17A;
    const MCG_STATUS_MCIP: u64 = 1 << 2;
    const MCG_STATUS_RIPV: u64 = 1 << 0;
    const MCG_STATUS_EIPV: u64 = 1 << 1;
    const MCI_STATUS_VALID: u64 = 1 << 63;
    const MAX_BANKS: usize = 32;

    let mut report = DirectWriter(Serial::new(COM1));
    // SAFETY: 这些是当前 CPU 的机器检查 MSR。
    let cap = unsafe { Msr::new(IA32_MCG_CAP).read() };
    // SAFETY: IA32_MCG_STATUS 是当前 CPU 的机器检查状态 MSR。
    let status = unsafe { Msr::new(IA32_MCG_STATUS).read() };
    let bank_count = (cap & 0xFF) as usize;
    let banks = bank_count.min(MAX_BANKS);
    let _ = writeln!(
        report,
        "\n机器检查：banks={bank_count} mcg_status={status:#018x} mcip={} ripv={} eipv={}",
        status & MCG_STATUS_MCIP != 0,
        status & MCG_STATUS_RIPV != 0,
        status & MCG_STATUS_EIPV != 0
    );
    let mut bank_status = [0_u64; MAX_BANKS];
    for (bank, slot) in bank_status.iter_mut().enumerate().take(banks) {
        let msr = 0x400 + bank as u32 * 16;
        // SAFETY: bank 数来自 IA32_MCG_CAP，MSR 编号在架构范围内。
        let value = unsafe { Msr::new(msr).read() };
        *slot = value;
        if value & MCI_STATUS_VALID != 0 {
            let _ = writeln!(
                report,
                "  bank {bank}: status={value:#018x} uc={} pcc={}",
                value & (1 << 61) != 0,
                value & (1 << 57) != 0
            );
        }
    }
    let recovered = bank_count <= MAX_BANKS && recoverable(status, &bank_status[..banks]);
    if recovered {
        // SAFETY: 清 MCi 后再清 MCG_STATUS，写 0 清除 valid/MCIP 状态位。
        unsafe {
            for bank in 0..banks {
                Msr::new(0x400 + bank as u32 * 16).write(0);
            }
            Msr::new(IA32_MCG_STATUS).write(0);
        }
        let _ = writeln!(report, "  MCE 可恢复：已清除机器检查状态并返回");
    } else {
        let _ = writeln!(report, "  MCE 不可恢复");
    }
    recovered
}

#[cfg(test)]
mod mce_tests {
    use super::recoverable;

    #[test]
    fn mce_recovery_requires_valid_rip_and_corrected_banks() {
        let mcg = 1 | (1 << 1) | (1 << 2);
        assert!(recoverable(mcg, &[1 << 63]));
        assert!(!recoverable(mcg, &[0]));
        assert!(!recoverable(mcg, &[(1 << 63) | (1 << 61)]));
        assert!(!recoverable(mcg, &[(1 << 63) | (1 << 57)]));
        assert!(!recoverable(1, &[1 << 63]));
    }
}

/// 返回页错误地址（CR2）。
pub fn fault_address() -> u64 {
    Cr2::read_raw()
}
/// 设置线程指针（IA32_FS_BASE），供每个线程使用独立 TLS。
pub fn set_thread_pointer(tls: u64) {
    const IA32_FS_BASE: u32 = 0xC000_0100;
    let mut fs_base = Msr::new(IA32_FS_BASE);
    // SAFETY: 只写当前 CPU 的 FS 基址 MSR，不改变其他机器状态。
    unsafe { fs_base.write(tls) };
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
        fpu: FpuState::reset(),
    }
}

/// 按保存的现场恢复用户态执行，直到任务再次阻塞或退出。
///
/// 返回时机是系统调用处理函数要求切换任务之后；`frame` 只在使用期间读取。
pub fn resume_user(frame: &InterruptFrame) {
    // 现场被踩坏时 iretq 会跳到任意地址（曾见跳到 per-CPU 调用栈帧基址）。
    // 这里先校验，把“跳到栈上”的三重故障换成可定位的现场报告。
    if frame.cs != u64::from(USER_CODE_SELECTOR)
        || frame.rip < 0x1000
        || frame.rip >= USER_LIMIT
        || frame.rsp >= USER_LIMIT
    {
        let mut report = DirectWriter(Serial::new(COM1));
        let _ = writeln!(
            report,
            "\n内核：恢复的用户现场非法 cs={:#x} rip={:#018x} rsp={:#018x} vector={:#x} error={:#x}",
            frame.cs, frame.rip, frame.rsp, frame.vector, frame.error
        );
        hk_hal::exit(hk_hal::ExitCode::Failure(71))
    }
    // SAFETY: `frame` 按 `InterruptFrame` 布局保存完整用户态寄存器与栈，
    // 对应地址已映射为用户可执行或可写页。
    unsafe { hk_resume_user(frame) };
}

impl hk_hal::UserContext for InterruptFrame {
    const SAVED_BYTES: usize = InterruptFrame::ENCODED_BYTES;

    fn resume(&self) {
        resume_user(self);
    }

    fn set_start_argument(&mut self, argument: u64) {
        self.rdi = argument;
    }

    fn set_start_capacity(&mut self, capacity: u64) {
        self.rsi = capacity;
    }

    fn for_thread(entry: u64, stack_top: u64, argument: u64) -> Self {
        user_frame(entry, stack_top, argument)
    }

    fn save(&self, out: &mut [u8]) {
        let encoded = self.encode();
        let length = encoded.len().min(out.len());
        out[..length].copy_from_slice(&encoded[..length]);
    }

    fn load(bytes: &[u8]) -> Option<Self> {
        Self::decode(bytes)
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

    /// 发送者编号放在 `r9`：`sysretq` 必须用 `rcx` 承载返回地址。
    fn set_sender(&mut self, value: u64) {
        self.r9 = value;
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

/// 直接写串口的格式化器：异常报告不依赖堆分配。
struct DirectWriter(Serial);

impl core::fmt::Write for DirectWriter {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        self.0.write_str(text);
        Ok(())
    }
}

/// 从 per-CPU 区读取保留槽位。
fn read_gs(offset: u64) -> u64 {
    let value: u64;
    // SAFETY: GS 基址由平台在内核进入前设置，槽位由入口汇编维护。
    unsafe {
        core::arch::asm!(
            "mov {}, gs:[{}]",
            out(reg) value,
            in(reg) offset,
            options(nostack, preserves_flags)
        );
    }
    value
}

/// 内核态异常的补充现场：per-CPU 栈槽位、钩子地址与被中断的外层现场。
///
/// ISR 把帧基址放进 rbx，嵌套异常因此可以从 rbx 读回外层帧（系统调用编号、
/// 用户返回地址）。只读取当前 CPU 已映射的栈窗口，避免诊断路径再次出错。
fn report_kernel_stack(frame: &InterruptFrame) {
    if frame.cs & 3 != 0 {
        return;
    }
    let mut report = DirectWriter(Serial::new(COM1));
    let cpu = super::cpu_index();
    let (syscall_hook, preempt_hook) = super::syscall::hook_values();
    let _ = writeln!(
        report,
        "  cpu={cpu} gs[0]={:#018x} gs[8]={:#018x} gs[16]={:#018x}",
        read_gs(0),
        read_gs(8),
        read_gs(16)
    );
    let _ = writeln!(
        report,
        "  hooks: syscall={syscall_hook:#018x} preempt={preempt_hook:#018x}"
    );
    // hk_leave_user 会切到 gs[0] 保存的调度器上下文：前 6 个字是被保存的
    // 寄存器，第 7 个字（下标 6）是它随后 `ret` 的目标。
    let resume = read_gs(0);
    if (0xffff_ffff_8000_0000..0xffff_ffff_c000_0000).contains(&resume) {
        // SAFETY: 指向内核镜像 .bss 内的调度器栈；诊断读取允许未对齐。
        let words: [u64; 8] = unsafe { core::ptr::read_unaligned(resume as *const [u64; 8]) };
        let _ = writeln!(
            report,
            "  resume {resume:#018x}: {:#x} {:#x} {:#x} {:#x} {:#x} {:#x} ret={:#x} {:#x}",
            words[0], words[1], words[2], words[3], words[4], words[5], words[6], words[7]
        );
    }
    let Some((low, high)) = super::stacks::window(cpu) else {
        return;
    };
    let _ = writeln!(report, "  kernel stack {low:#018x}..{high:#018x}");
    let outer = frame.rbx;
    if outer >= low && outer.saturating_add(176) <= high {
        // SAFETY: 地址限制在当前 CPU 的已映射栈页内；诊断读取允许未对齐。
        let word =
            |offset: u64| unsafe { core::ptr::read_unaligned((outer + offset) as *const u64) };
        let cs = word(144);
        if cs & 3 == 3 {
            let _ = writeln!(
                report,
                "  outer: rax/vector={:#x}/{:#x} rip={:#018x} cs={:#x} rsp={:#018x}",
                word(0),
                word(120),
                word(136),
                cs,
                word(160)
            );
        }
        // `call interrupt_dispatch` 的返回地址紧贴帧基址下方。
        if outer >= low + 8 {
            // SAFETY: 帧基址下方一个 u64 仍在栈窗口内。
            let ret = unsafe { core::ptr::read_unaligned((outer - 8) as *const u64) };
            let _ = writeln!(report, "  dispatch return slot={ret:#018x}");
        }
    }
    // 调试构建保留帧指针：沿 rbp 链打印返回地址，交给 `xtask symbolize` 解析。
    let mut frame_pointer = frame.rbp;
    for _ in 0..8 {
        if frame_pointer < low || frame_pointer.saturating_add(16) > high {
            break;
        }
        // SAFETY: 帧指针与两个槽位都在本 CPU 的已映射栈页内。
        let (saved, ret) = unsafe {
            (
                core::ptr::read_unaligned(frame_pointer as *const u64),
                core::ptr::read_unaligned((frame_pointer + 8) as *const u64),
            )
        };
        let _ = writeln!(
            report,
            "  frame rbp={frame_pointer:#018x} ret={ret:#018x} prev={saved:#018x}"
        );
        if saved <= frame_pointer || saved >= high {
            break;
        }
        frame_pointer = saved;
    }
}

pub(super) fn report_exception(frame: &InterruptFrame) -> ! {
    let mut report = DirectWriter(Serial::new(COM1));

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
        let cr2 = Cr2::read_raw();
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
    report_kernel_stack(frame);

    hk_hal::exit(hk_hal::ExitCode::Failure(70))
}
