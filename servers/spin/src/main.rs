//! 抢占演示服务：忙等若干个调度 tick 后退出。
//!
//! 服务不主动让出 CPU；若调度器没有定时抢占，其它可运行任务会被它一直挡住。
//!
//! 本 crate 只在 `x86_64-unknown-none` 目标上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_server_api::{close_handle, exit, handle_count, ticks, write, write_u64};

/// 忙等的调度 tick 数。
#[cfg(target_os = "none")]
const WAIT_TICKS: u64 = 3;

/// 服务入口：忙等若干个 tick 后退出。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_endpoint: u64) -> ! {
    write(b"hk-server-spin: start\n");
    let start = ticks();
    while ticks().wrapping_sub(start) < WAIT_TICKS {
        core::hint::spin_loop();
    }
    write(b"hk-server-spin: done\n");

    let before = handle_count();
    let closed = close_handle(1);
    let after = handle_count();
    write(b"hk-server-spin: handles ");
    write_u64(before);
    write(b" -> ");
    write_u64(after);
    write(b"\n");
    if closed == 0 {
        write(b"hk-server-spin: closed\n");
    }
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
    eprintln!("hk-server-spin 只在 x86_64-unknown-none 目标上构建");
}
