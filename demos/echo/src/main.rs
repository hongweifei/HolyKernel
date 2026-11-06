//! echo 服务：在端点等待请求，打印消息字并回复两倍值。
//!
//! 计算请求交给工作线程池并行处理，主线程只负责收发 IPC 与回复。
//!
//! 本 crate 只在 `x86_64-unknown-none` 目标上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::vec::Vec;
#[cfg(target_os = "none")]
use core::sync::atomic::{AtomicU64, Ordering};

#[cfg(target_os = "none")]
use hk_proto::echo::{
    WORK_DONE, WORK_REQUEST, WORK_RESPONSE, WORK_ROUNDS, WORK_STATS, WORK_STATS_RESPONSE,
};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{exit, handle_count, notify, recv_handle, reply, send, write, write_u64};
#[cfg(target_os = "none")]
use hk_worker::Pool;

/// 通知请求标签。
#[cfg(target_os = "none")]
const NOTIFY_REQUEST: u64 = 0x300;

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 256 * 1024;

/// 工作线程数量。
#[cfg(target_os = "none")]
const WORKERS: usize = 3;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// 正在计算的任务数。
#[cfg(target_os = "none")]
static ACTIVE: AtomicU64 = AtomicU64::new(0);
/// 观测到的最大并发数。
#[cfg(target_os = "none")]
static PEAK: AtomicU64 = AtomicU64::new(0);

/// 服务入口：内核以用户态进入，参数是内核分配给本服务的端点编号。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let Some(pool) = Pool::start(WORKERS) else {
        write(b"hk-server-echo: worker pool fail\n");
        exit(1)
    };
    pool.set_wake(endpoint, WORK_DONE);
    let mut pending: Vec<(u64, u64)> = Vec::new();
    let mut free: Vec<u64> = Vec::new();

    loop {
        let (label, word, sender, handle) = recv_handle(endpoint);
        if label == WORK_DONE {
            while let Some((id, result)) = pool.try_next() {
                let slot = id as usize;
                if let Some((sender, label)) = pending.get(slot).copied()
                    && label != 0
                {
                    let _ = send(sender, WORK_RESPONSE, result);
                    pending[slot] = (0, 0);
                    free.push(id);
                }
            }
            continue;
        }
        if label == WORK_REQUEST {
            let id = match free.pop() {
                Some(id) => id,
                None => {
                    pending.push((0, 0));
                    (pending.len() - 1) as u64
                }
            };
            pending[id as usize] = (sender, label);
            if !pool.submit(id, move || work(word)) {
                pending[id as usize] = (0, 0);
                free.push(id);
            }
            continue;
        }
        if label == WORK_STATS {
            let _ = send(sender, WORK_STATS_RESPONSE, PEAK.load(Ordering::Acquire));
            continue;
        }

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

/// 计算任务：忙等若干轮后返回可预测的结果。
#[cfg(target_os = "none")]
fn work(word: u64) -> u64 {
    let active = ACTIVE.fetch_add(1, Ordering::AcqRel) + 1;
    PEAK.fetch_max(active, Ordering::AcqRel);
    let mut value = word;
    for round in 0..WORK_ROUNDS {
        value = value.wrapping_mul(6364136223846793005).wrapping_add(round);
    }
    core::hint::black_box(value);
    ACTIVE.fetch_sub(1, Ordering::AcqRel);
    word * 2
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

hk_server_api::server_boilerplate!();
