//! aarch64 系统调用入口：`svc #0`，x8 调用号、x0/x1/x2 参数。

use core::arch::asm;

use super::SyscallResult;

/// # Safety
/// 调用方必须处于用户态或由内核显式调用；参数语义由调用号决定。
pub(crate) unsafe fn call(
    number: u64,
    first: u64,
    second: u64,
    third: u64,
    fourth: u64,
) -> SyscallResult {
    let value: u64;
    let word: u64;
    let sender: u64;
    let extra: u64;

    // SAFETY: `svc #0` 由内核异常向量接收；寄存器约定与内核分派一致。
    unsafe {
        asm!(
            "svc #0",
            in("x8") number,
            inlateout("x0") first => value,
            inlateout("x1") second => word,
            inlateout("x2") third => sender,
            inlateout("x3") fourth => extra,
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
