//! 写时复制 fork 演示：父任务与子任务写入同一栈变量，各自看到独立副本。
//!
//! 本 crate 只在 `x86_64-unknown-none` 目标上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_server_api::{exit, fork, write, write_u64};

/// 服务入口：分叉后分别写入栈变量并输出。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_endpoint: u64) -> ! {
    let mut marker = 11_u64;
    if marker != 11 {
        exit(1)
    }
    let child = fork();

    if child == 0 {
        marker = 22;
        write("hk-server-fork: child ".as_bytes());
        write_u64(marker);
        write(b"\n");
        exit(0)
    }

    marker = 33;
    write("hk-server-fork: parent ".as_bytes());
    write_u64(marker);
    write(b"\n");
    exit(0)
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(not(target_os = "none"))]
fn main() {
    eprintln!("hk-server-fork 只在 x86_64-unknown-none 目标上构建");
}
