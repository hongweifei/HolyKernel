//! 演示服务：向 echo 服务发送请求，等待回复后验证并退出。
//!
//! 本 crate 只在 `x86_64-unknown-none` 目标上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_server_api::{ENDPOINT_ECHO, alloc, exit, recv, send, send_handle, write};

/// 请求标签。
#[cfg(target_os = "none")]
const REQUEST: u64 = 0x100;

/// 回复标签。
#[cfg(target_os = "none")]
const REPLY: u64 = 0x101;

/// 句柄转移标签。
#[cfg(target_os = "none")]
const HANDOFF: u64 = 0x200;

/// 句柄转移回复标签。
#[cfg(target_os = "none")]
const HANDOFF_REPLY: u64 = 0x201;

/// 服务入口：内核以用户态进入，参数是内核分配给本服务的端点编号。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    write(b"hk-server-demo running\n");
    send(ENDPOINT_ECHO, REQUEST, 21);

    let (label, word, _sender) = recv(endpoint);
    if label == REPLY && word == 42 {
        write("hk-server-demo 收到回复 42\n".as_bytes());
        if verify_arena() {
            write(b"hk-server-demo arena ok\n");
        } else {
            write("hk-server-demo arena 失败\n".as_bytes());
            exit(1)
        }

        send_handle(ENDPOINT_ECHO, HANDOFF, 7, ENDPOINT_ECHO);
        let (label, word, _sender) = recv(endpoint);
        if label == HANDOFF_REPLY && word == 14 {
            write("hk-server-demo 句柄转移 ok\n".as_bytes());
            exit(0)
        }
        write("hk-server-demo 句柄转移失败\n".as_bytes());
        exit(1)
    }

    write("hk-server-demo 回复异常\n".as_bytes());
    exit(1)
}

#[cfg(target_os = "none")]
fn verify_arena() -> bool {
    let pointer = alloc(4096, 16);
    if pointer.is_null() {
        return false;
    }

    // SAFETY: 内核在任务 Arena 内分配了 4096 字节，地址有效且可写。
    let buffer = unsafe { core::slice::from_raw_parts_mut(pointer, 4096) };
    buffer.fill(0xA5);
    buffer.iter().all(|&byte| byte == 0xA5)
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
