//! 用户态异常隔离演示：写入未映射地址，验证内核只终止该任务。
//!
//! 本 crate 只在 `x86_64-unknown-none` 目标上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_server_api::{exit, write};

/// 服务入口：触发用户态页错误。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_endpoint: u64) -> ! {
    write("hk-server-fault: 触发用户页错误\n".as_bytes());
    // SAFETY: 故意写入未映射地址，用于验证用户态异常隔离。
    unsafe {
        core::ptr::write_volatile(0x4000_0000 as *mut u64, 1);
    }
    write("hk-server-fault: 未捕获\n".as_bytes());
    exit(1)
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
    eprintln!("hk-server-fault 只在 x86_64-unknown-none 目标上构建");
}
