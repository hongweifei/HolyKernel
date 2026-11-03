//! x86_64 系统调用入口：`int 0x80`，rax 调用号、rdi/rsi/rdx 参数。

use core::arch::asm;

use super::SyscallResult;

/// # Safety
/// 调用方必须处于用户态或由内核显式调用；参数语义由调用号决定。
pub(crate) unsafe fn call(number: u64, first: u64, second: u64, third: u64) -> SyscallResult {
    let value: u64;
    let word: u64;
    let sender: u64;

    // SAFETY: `int 0x80` 由内核在 IDT 中注册；内核恢复 rax/rdx/rcx。
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") number => value,
            in("rdi") first,
            in("rsi") second,
            inlateout("rdx") third => word,
            lateout("rcx") sender,
            options(nostack),
        );
    }

    SyscallResult {
        value,
        word,
        sender,
    }
}
