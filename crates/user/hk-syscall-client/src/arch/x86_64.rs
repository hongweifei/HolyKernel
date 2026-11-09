//! x86_64 系统调用入口：`syscall`，rax 调用号、rdi/rsi/rdx/r10 参数。

use core::arch::asm;

use super::SyscallResult;

/// # Safety
/// 调用方必须处于用户态或由内核显式调用；参数语义由调用号决定。
pub unsafe fn call(number: u64, first: u64, second: u64, third: u64, fourth: u64) -> SyscallResult {
    let value: u64;
    let word: u64;
    let sender: u64;
    let extra: u64;

    // SAFETY: 内核在 EFER/STAR/LSTAR 中启用了 `syscall`；出口恢复 rax/rdx/rcx/r8。
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") number => value,
            in("rdi") first,
            in("rsi") second,
            inlateout("rdx") third => word,
            lateout("rcx") sender,
            in("r10") fourth,
            lateout("r8") extra,
            lateout("r11") _,
            options(nostack),
        );
    }

    SyscallResult {
        value,
        word,
        sender,
        extra,
    }
}
