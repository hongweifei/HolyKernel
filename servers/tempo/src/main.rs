//! Tempo：调度策略服务。
//!
//! Root Server 授予 Tempo 一个带硬预算的初始 Cadence（策略池）。Tempo 在池内
//! 派生收窄的子契约，并把新契约句柄随回复转移给调用方；调用方随后用
//! `SYS_TASK_ATTACH_CADENCE(task = 0, cadence)` 附加到自己。
//!
//! 本 crate 只在 `x86_64-unknown-none` 目标上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_cadence::CadenceSnapshot;
#[cfg(target_os = "none")]
use hk_proto::tempo::{
    MAX_NAME, REJECT_INVALID, REJECT_OUT_OF_RANGE, SNAPSHOT_LEN, TEMPO_ADMIT, TEMPO_REJECTED,
    TEMPO_REQUEST, TEMPO_RESPONSE, TEMPO_REVOKE,
};
#[cfg(target_os = "none")]
use hk_server_api::{
    RIGHT_READ, RIGHT_TRANSFER, RIGHT_WRITE, cadence_self_handle, cadence_split, close_handle,
    exit, handle_duplicate, recv_data, send, send_handle, task_attach_named, task_regrant_named,
    write,
};

/// 请求缓冲区：容纳按名请求的名字前缀与 Cadence 快照。
#[cfg(target_os = "none")]
const MAX_REQUEST: usize = 2 + MAX_NAME + CadenceSnapshot::ENCODED_LEN;

/// 解析按名请求；返回任务名与快照。
#[cfg(target_os = "none")]
fn named_request(bytes: &[u8]) -> Option<(&str, &[u8])> {
    if bytes.len() < 2 + SNAPSHOT_LEN {
        return None;
    }
    let length = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
    if length == 0 || length > MAX_NAME || bytes.len() < 2 + length + SNAPSHOT_LEN {
        return None;
    }
    let name = core::str::from_utf8(&bytes[2..2 + length]).ok()?;
    let snapshot = &bytes[2 + length..2 + length + SNAPSHOT_LEN];
    CadenceSnapshot::decode(snapshot).ok()?;
    Some((name, snapshot))
}

/// 服务入口：内核以用户态进入，参数是内核分配给本服务的端点编号。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let pool = cadence_self_handle();
    if pool == u64::MAX {
        write(b"hk-server-tempo: cadence handle fail\n");
        exit(1);
    }
    write(b"hk-server-tempo: ready\n");

    let mut buffer = [0_u8; MAX_REQUEST];
    loop {
        let (label, _word, sender, size) = recv_data(endpoint, &mut buffer);
        if label == TEMPO_ADMIT || label == TEMPO_REVOKE {
            let Some((name, snapshot)) = named_request(&buffer[..size]) else {
                let _ = send(sender, TEMPO_REJECTED, REJECT_INVALID);
                continue;
            };
            let child = cadence_split(pool, snapshot);
            if child == u64::MAX {
                let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
                continue;
            }
            let result = if label == TEMPO_ADMIT {
                task_attach_named(name, child)
            } else {
                task_regrant_named(name, child)
            };
            let _ = close_handle(child);
            if result == 0 {
                let _ = send(sender, TEMPO_RESPONSE, 0);
            } else {
                let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
            }
            continue;
        }
        if label != TEMPO_REQUEST {
            continue;
        }
        if size < CadenceSnapshot::ENCODED_LEN
            || CadenceSnapshot::decode(&buffer[..CadenceSnapshot::ENCODED_LEN]).is_err()
        {
            let _ = send(sender, TEMPO_REJECTED, REJECT_INVALID);
            continue;
        }
        let child = cadence_split(pool, &buffer[..CadenceSnapshot::ENCODED_LEN]);
        if child == u64::MAX {
            let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
            continue;
        }
        let granted = handle_duplicate(child, RIGHT_READ | RIGHT_WRITE | RIGHT_TRANSFER);
        let _ = close_handle(child);
        if granted == u64::MAX {
            let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
            continue;
        }
        if send_handle(sender, TEMPO_RESPONSE, 0, granted) != 0 {
            let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
        }
    }
}

hk_server_api::server_boilerplate!();
