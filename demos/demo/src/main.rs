//! 演示服务：向 echo 服务发送请求，等待回复后验证并退出。
//!
//! 本 crate 只在 `x86_64-unknown-none` 目标上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。各类自检拆在
//! `fs`、`concurrency` 与 `memory` 模块里。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

// 服务堆大小；协程栈从该堆分配。
hk_user_api::server_heap!();

#[cfg(target_os = "none")]
use hk_user_api::{
    boot_cmdline, call, exit, recv, send, send_handle, service_endpoint, wait_notify, write,
    write_u64,
};

#[cfg(target_os = "none")]
mod concurrency;
#[cfg(target_os = "none")]
mod fs;
#[cfg(target_os = "none")]
mod memory;

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

/// 服务入口：内核以用户态进入，参数是内核分配给本服务的端点编号。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    write(b"hk-server-demo running\n");
    let echo = service_endpoint("hk-server-echo");
    let (label, word, _sender) = call(echo, REQUEST, 21);

    if label == REPLY && word == 42 {
        write("hk-server-demo 收到回复 42\n".as_bytes());
        if memory::verify_arena() {
            write(b"hk-server-demo arena ok\n");
        } else {
            write("hk-server-demo arena 失败\n".as_bytes());
            exit(1)
        }
        if concurrency::verify_threads() {
            write(b"hk-server-demo thread ok\n");
        } else {
            write(b"hk-server-demo thread fail\n");
            exit(1)
        }
        if concurrency::verify_coroutines() {
            write(b"hk-server-demo coroutine ok\n");
        } else {
            write(b"hk-server-demo coroutine fail\n");
            exit(1)
        }
        if concurrency::verify_async() {
            write(b"hk-server-demo async ok\n");
        } else {
            write(b"hk-server-demo async fail\n");
            exit(1)
        }
        if concurrency::verify_sync() {
            write(b"hk-server-demo sync ok\n");
        } else {
            write(b"hk-server-demo sync fail\n");
            exit(1)
        }
        if concurrency::verify_worker(endpoint) {
            write(b"hk-server-demo worker ok\n");
        } else {
            write(b"hk-server-demo worker fail\n");
            exit(1)
        }
        if memory::verify_memory(endpoint) {
            write(b"hk-server-demo hmp ok\n");
        } else {
            write(b"hk-server-demo hmp fail\n");
            exit(1)
        }

        send(echo, NOTIFY_REQUEST, 0);
        if wait_notify(endpoint, 1) != 1 {
            write("hk-server-demo 通知等待失败\n".as_bytes());
            exit(1)
        }
        write("hk-server-demo 通知等待 ok\n".as_bytes());

        send_handle(echo, HANDOFF, 7, echo);
        let (label, word, _sender) = recv(endpoint);
        if label != HANDOFF_REPLY || word != 14 {
            write("hk-server-demo 句柄转移失败\n".as_bytes());
            exit(1)
        }
        write("hk-server-demo 句柄转移 ok\n".as_bytes());
        if fs::verify_vfs(endpoint) {
            write("VFS 读写 ok\n".as_bytes());
        } else {
            write("VFS 读写失败\n".as_bytes());
            exit(1)
        }
        if fs::verify_ext4(endpoint) {
            write("hk-server-demo fs ok\n".as_bytes());
        } else {
            write("hk-server-demo fs 失败\n".as_bytes());
            exit(1)
        }

        let mut cmdline_buffer = [0_u8; 128];
        let cmdline = boot_cmdline(&mut cmdline_buffer).unwrap_or("");
        if cmdline.contains("restart-test") {
            if fs::verify_driver_restart(endpoint) {
                write("hk-server-demo 驱动重启 ok\n".as_bytes());
            } else {
                write("hk-server-demo 驱动重启失败\n".as_bytes());
                exit(1)
            }
        }
        // 没有块设备时不跑 ext4 顺序读基准，避免 tmpfs 上长时间阻塞启动。
        if service_endpoint("hk-server-virtio") != 0
            && let Some(tenths) = fs::benchmark_ext4(endpoint)
        {
            write("fs 1 MiB 读：".as_bytes());
            write_u64(tenths / 10);
            write(".".as_bytes());
            write_u64(tenths % 10);
            write(" MiB/s\n".as_bytes());
        }
        exit(0)
    }

    write("hk-server-demo 回复异常\n".as_bytes());
    exit(1)
}

hk_user_api::server_boilerplate!();
