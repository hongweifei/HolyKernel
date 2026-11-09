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
    REJECT_INVALID, REJECT_OUT_OF_RANGE, TEMPO_REJECTED, TEMPO_REQUEST, TEMPO_RESPONSE,
};
#[cfg(target_os = "none")]
use hk_server_api::{
    RIGHT_READ, RIGHT_TRANSFER, RIGHT_WRITE, cadence_self_handle, cadence_split, close_handle,
    exit, handle_duplicate, recv_data, send, send_handle, write,
};

/// 请求缓冲区：容纳一个 Cadence 快照与少量余量。
#[cfg(target_os = "none")]
const MAX_REQUEST: usize = CadenceSnapshot::ENCODED_LEN;

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
