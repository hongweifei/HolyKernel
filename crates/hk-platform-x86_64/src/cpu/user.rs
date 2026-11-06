//! 用户态现场、异常处理与恢复。

use core::arch::asm;
use core::fmt::Write;

use hk_hal::Console;
use spin::Mutex;

use super::*;
use crate::serial::{COM1, Serial};

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
/// 用户态异常处理动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultAction {
    /// 已修复，返回用户态重试。
    Resume,
    /// 终止当前任务并返回调度器。
    Exit,
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

/// 返回页错误地址（CR2）。
pub fn fault_address() -> u64 {
    let value: u64;
    // SAFETY: CR2 只在异常处理中使用，读取不影响机器状态。
    unsafe {
        asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}
/// 设置线程指针（IA32_FS_BASE），供每个线程使用独立 TLS。
pub fn set_thread_pointer(tls: u64) {
    const IA32_FS_BASE: u32 = 0xC000_0100;
    // SAFETY: 只写当前 CPU 的 FS 基址 MSR，不改变其他机器状态。
    unsafe {
        asm!(
            "wrmsr",
            in("ecx") IA32_FS_BASE,
            in("eax") tls as u32,
            in("edx") (tls >> 32) as u32,
            options(nostack, preserves_flags)
        );
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
    const SAVED_BYTES: usize = 22 * 8;

    fn resume(&self) {
        resume_user(self);
    }

    fn set_start_argument(&mut self, argument: u64) {
        self.rdi = argument;
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

/// 直接写串口的格式化器：异常报告不依赖堆分配。
struct DirectWriter(Serial);

impl core::fmt::Write for DirectWriter {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        self.0.write_str(text);
        Ok(())
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

    hk_test::exit(hk_test::ExitCode::Failure(70))
}
