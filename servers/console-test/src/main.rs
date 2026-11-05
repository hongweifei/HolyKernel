//! Console 服务自检：请求一行输入并打印收到的内容。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_server_api::console::Client;
#[cfg(target_os = "none")]
use hk_server_api::{boot_cmdline, exit, write};

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let mut buffer = [0_u8; 128];
    if !boot_cmdline(&mut buffer).is_some_and(|line| line.contains("console-test")) {
        exit(0)
    }
    let Some(client) = Client::discover(endpoint) else {
        write(b"hk-server-console-test: console not found\n");
        exit(1)
    };
    client.write(b"hk-server-console-test: type a line\n");
    let mut line = [0_u8; 64];
    let length = client.read_line(&mut line);
    write(b"hk-server-console-test: got ");
    write(&line[..length]);
    write(b"\n");
    client.shutdown();
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
    eprintln!("hk-server-console-test 只在 x86_64-unknown-none 目标上构建");
}
