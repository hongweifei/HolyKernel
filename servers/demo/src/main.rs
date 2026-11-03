//! 演示服务：通过内核函数表输出一行文本后返回。
//!
//! 本 crate 只在 `x86_64-unknown-none` 上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_server_api::{ServerApi, ServerEntry};

/// 服务入口：内核加载 ELF 后按入口地址调用，返回即交回内核。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(api: &ServerApi) {
    let entry: ServerEntry = server_entry;
    entry(api);
}

#[cfg(target_os = "none")]
extern "C" fn server_entry(api: &ServerApi) {
    let message = b"hk-server-demo running\n";
    (api.write)(message.as_ptr(), message.len());
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
    eprintln!("hk-server-demo 只在 x86_64-unknown-none 目标上构建");
}
