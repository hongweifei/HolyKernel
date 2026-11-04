//! 演示服务：向 echo 服务发送请求，等待回复后验证并退出。
//!
//! 本 crate 只在 `x86_64-unknown-none` 目标上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_server_api::vfs::{OPEN_CREATE, OPEN_READ, OPEN_WRITE, VfsClient};
#[cfg(target_os = "none")]
use hk_server_api::{
    ENDPOINT_ECHO, alloc, call, exit, recv, send, send_handle, wait_notify, write,
};

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

/// 通知请求标签。
#[cfg(target_os = "none")]
const NOTIFY_REQUEST: u64 = 0x300;

/// VFS 服务端点（启动服务列表中的第 5 个）。
#[cfg(target_os = "none")]
const ENDPOINT_VFS: u64 = 5;

/// 服务入口：内核以用户态进入，参数是内核分配给本服务的端点编号。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    write(b"hk-server-demo running\n");
    let (label, word, _sender) = call(ENDPOINT_ECHO, REQUEST, 21);

    if label == REPLY && word == 42 {
        write("hk-server-demo 收到回复 42\n".as_bytes());
        if verify_arena() {
            write(b"hk-server-demo arena ok\n");
        } else {
            write("hk-server-demo arena 失败\n".as_bytes());
            exit(1)
        }

        send(ENDPOINT_ECHO, NOTIFY_REQUEST, 0);
        if wait_notify(endpoint, 1) != 1 {
            write("hk-server-demo 通知等待失败\n".as_bytes());
            exit(1)
        }
        write("hk-server-demo 通知等待 ok\n".as_bytes());

        send_handle(ENDPOINT_ECHO, HANDOFF, 7, ENDPOINT_ECHO);
        let (label, word, _sender) = recv(endpoint);
        if label != HANDOFF_REPLY || word != 14 {
            write("hk-server-demo 句柄转移失败\n".as_bytes());
            exit(1)
        }
        write("hk-server-demo 句柄转移 ok\n".as_bytes());
        if verify_vfs(endpoint) {
            write("VFS 读写 ok\n".as_bytes());
        } else {
            write("VFS 读写失败\n".as_bytes());
            exit(1)
        }
        exit(0)
    }

    write("hk-server-demo 回复异常\n".as_bytes());
    exit(1)
}

#[cfg(target_os = "none")]
fn verify_vfs(endpoint: u64) -> bool {
    let mut client = VfsClient::new(ENDPOINT_VFS, endpoint);
    let Ok(handle) = client.open("/tmp/hello", OPEN_READ | OPEN_WRITE | OPEN_CREATE) else {
        return false;
    };
    if client.write(handle, 0, b"vfs").is_err() {
        return false;
    }
    let mut buffer = [0_u8; 16];
    let Ok(length) = client.read(handle, 0, &mut buffer) else {
        return false;
    };
    if length < 3 || &buffer[..3] != b"vfs" {
        return false;
    }
    if client.close(handle).is_err() {
        return false;
    }
    if client.stat("/tmp/hello").ok() != Some((1, 3)) {
        return false;
    }
    if client.truncate("/tmp/hello", 1).is_err() {
        return false;
    }
    if client.stat("/tmp/hello").ok() != Some((1, 1)) {
        return false;
    }
    if client.unlink("/tmp/hello").is_err() {
        return false;
    }
    if client.stat("/tmp/hello").is_ok() {
        return false;
    }

    let mut mounts = [0_u8; 64];
    let Ok(length) = client.mounts(&mut mounts) else {
        return false;
    };
    mounts[..length]
        .split(|byte| *byte == b'\n')
        .any(|line| line == b"/tmp")
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
