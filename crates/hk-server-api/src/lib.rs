//! 内核与服务器之间的系统调用 ABI。
//!
//! 服务在用户态通过 `int 0x80` 进入内核：`rax` 为调用号，`rdi`/`rsi` 为参数。

#![no_std]

use core::arch::asm;

/// 退出服务。
pub const SYS_EXIT: u64 = 0;

/// 把一段 UTF-8 文本写入内核控制台。
pub const SYS_WRITE: u64 = 1;

/// 发起系统调用。
///
/// # Safety
/// 调用方必须处于用户态或由内核显式调用；参数语义由调用号决定。
#[inline(always)]
unsafe fn syscall(number: u64, first: u64, second: u64) -> u64 {
    let result: u64;
    // SAFETY: `int 0x80` 由内核在 IDT 中注册；寄存器约定与内核分派一致。
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") number => result,
            in("rdi") first,
            in("rsi") second,
            options(nostack),
        );
    }
    result
}

/// 把文本写入内核控制台。
pub fn write(bytes: &[u8]) {
    // SAFETY: 指针与长度来自本进程的只读数据。
    unsafe {
        syscall(SYS_WRITE, bytes.as_ptr() as u64, bytes.len() as u64);
    }
}

/// 结束服务，不返回。
pub fn exit(code: u64) -> ! {
    // SAFETY: 退出调用不返回；内核据此回到加载服务的上下文。
    unsafe {
        syscall(SYS_EXIT, code, 0);
    }
    loop {
        core::hint::spin_loop();
    }
}
