//! echo 服务：在端点等待请求，打印消息字并回复两倍值。
//!
//! 本 crate 只在 `x86_64-unknown-none` 目标上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_server_api::{handle_count, notify, recv_handle, reply, send, write, write_u64};

/// 通知请求标签。
#[cfg(target_os = "none")]
const NOTIFY_REQUEST: u64 = 0x300;

/// 服务入口：内核以用户态进入，参数是内核分配给本服务的端点编号。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    loop {
        let (label, word, sender, handle) = recv_handle(endpoint);
        write_request(word);
        if handle != 0 {
            write("hk-server-echo 收到句柄 ".as_bytes());
            write_u64(handle);
            write(" 句柄数 ".as_bytes());
            write_u64(handle_count());
            write(b"\n");
        }
        if label == NOTIFY_REQUEST {
            notify(sender, 1);
            write("hk-server-echo 已发出通知\n".as_bytes());
            continue;
        }
        if reply(label + 1, word * 2) != 0 {
            send(sender, label + 1, word * 2);
        }
    }
}

#[cfg(target_os = "none")]
fn write_request(word: u64) {
    let prefix = "hk-server-echo 收到请求 ";
    let mut buffer = [0_u8; 64];
    buffer[..prefix.len()].copy_from_slice(prefix.as_bytes());

    let mut digits = [0_u8; 20];
    let mut length = 0;
    let mut value = word;
    loop {
        digits[length] = b'0' + (value % 10) as u8;
        length += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }

    let mut end = prefix.len();
    while length > 0 {
        length -= 1;
        buffer[end] = digits[length];
        end += 1;
    }
    buffer[end] = b'\n';
    write(&buffer[..=end]);
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
    eprintln!("hk-server-echo 只在 x86_64-unknown-none 目标上构建");
}
