//! Tempo：调度策略服务。
//!
//! Root Server 授予 Tempo 一个带硬预算的初始 Cadence（策略池）。Tempo 在池内
//! 派生收窄的子契约，并把新契约句柄随回复转移给调用方；调用方随后用
//! `SYS_TASK_ATTACH_CADENCE(task = 0, cadence)` 附加到自己。
//!
//! Realtime 准入直接读取内核中的真实需求（`SYS_CADENCE_DEMAND`），服务内不再
//! 维护第二份契约账本；任务退出、撤销与 checkpoint 恢复天然一致。
//!
//! 本 crate 只在 `x86_64-unknown-none` 目标上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;
#[cfg(target_os = "none")]
hk_user_api::server_heap!(64 * 1024);

#[cfg(target_os = "none")]
mod admit;

#[cfg(target_os = "none")]
use admit::{admission_spec, encode_spec, name_request, named_request};
#[cfg(target_os = "none")]
use hk_cadence::CadenceSnapshot;
#[cfg(target_os = "none")]
use hk_proto::tempo::{
    MAX_NAME, REJECT_INVALID, REJECT_OUT_OF_RANGE, TEMPO_ADMIT, TEMPO_READMIT, TEMPO_REJECTED,
    TEMPO_REQUEST, TEMPO_RESPONSE, TEMPO_REVOKE,
};
#[cfg(target_os = "none")]
use hk_user_api::{
    RIGHT_READ, RIGHT_TRANSFER, cadence_named, cadence_self, cadence_self_handle, cadence_split,
    close_handle, exit, handle_duplicate, recv_data, send, send_handle, task_attach_named,
    task_regrant_named, write,
};

/// 请求缓冲区：容纳按名请求的名字前缀与 Cadence 快照。
#[cfg(target_os = "none")]
const MAX_REQUEST: usize = 2 + MAX_NAME + CadenceSnapshot::ENCODED_LEN;

/// 读取任务当前契约的标识，用于在需求查询中排除旧契约。
#[cfg(target_os = "none")]
fn current_cadence_id(name: &str) -> u64 {
    let mut bytes = [0_u8; CadenceSnapshot::ENCODED_LEN];
    if cadence_named(name, &mut bytes) != CadenceSnapshot::ENCODED_LEN as u64 {
        return 0;
    }
    CadenceSnapshot::decode(&bytes).map_or(0, |snapshot| snapshot.id)
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
    let mut pool_bytes = [0_u8; CadenceSnapshot::ENCODED_LEN];
    if cadence_self(&mut pool_bytes) != CadenceSnapshot::ENCODED_LEN as u64 {
        write(b"hk-server-tempo: pool info fail\n");
        exit(1);
    }
    let Some(pool_snapshot) = CadenceSnapshot::decode(&pool_bytes).ok() else {
        write(b"hk-server-tempo: pool decode fail\n");
        exit(1);
    };
    write(b"hk-server-tempo: ready\n");

    let mut buffer = [0_u8; MAX_REQUEST];
    loop {
        let (label, _word, sender, size) = recv_data(endpoint, &mut buffer);
        if label == TEMPO_READMIT {
            let Some(name) = name_request(&buffer[..size]) else {
                let _ = send(sender, TEMPO_REJECTED, REJECT_INVALID);
                continue;
            };
            let mut snapshot = [0_u8; CadenceSnapshot::ENCODED_LEN];
            if cadence_named(name, &mut snapshot) != CadenceSnapshot::ENCODED_LEN as u64 {
                let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
                continue;
            }
            let Some(request) = CadenceSnapshot::decode(&snapshot).ok() else {
                let _ = send(sender, TEMPO_REJECTED, REJECT_INVALID);
                continue;
            };
            let Some(spec) = admission_spec(
                &request,
                pool_snapshot.budget,
                pool_snapshot.period,
                pool_snapshot.deadline,
                pool_snapshot.latency,
                request.id,
            ) else {
                let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
                continue;
            };
            let admitted = encode_spec(spec);
            let child = cadence_split(pool, &admitted);
            if child == u64::MAX {
                let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
                continue;
            }
            let result = task_regrant_named(name, child);
            let _ = close_handle(child);
            if result != u64::MAX {
                let _ = send(sender, TEMPO_RESPONSE, 0);
            } else {
                let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
            }
            continue;
        }
        if label == TEMPO_ADMIT || label == TEMPO_REVOKE {
            let Some((name, snapshot)) = named_request(&buffer[..size]) else {
                let _ = send(sender, TEMPO_REJECTED, REJECT_INVALID);
                continue;
            };
            let Some(request) = CadenceSnapshot::decode(snapshot).ok() else {
                let _ = send(sender, TEMPO_REJECTED, REJECT_INVALID);
                continue;
            };
            let Some(spec) = admission_spec(
                &request,
                pool_snapshot.budget,
                pool_snapshot.period,
                pool_snapshot.deadline,
                pool_snapshot.latency,
                current_cadence_id(name),
            ) else {
                let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
                continue;
            };
            let admitted = encode_spec(spec);
            let child = cadence_split(pool, &admitted);
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
            let attached = if label == TEMPO_ADMIT {
                result == 0
            } else {
                result != u64::MAX
            };
            if attached {
                let _ = send(sender, TEMPO_RESPONSE, 0);
            } else {
                let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
            }
            continue;
        }
        if label != TEMPO_REQUEST {
            continue;
        }
        if size < CadenceSnapshot::ENCODED_LEN {
            let _ = send(sender, TEMPO_REJECTED, REJECT_INVALID);
            continue;
        }
        let Some(request) = CadenceSnapshot::decode(&buffer[..CadenceSnapshot::ENCODED_LEN]).ok()
        else {
            let _ = send(sender, TEMPO_REJECTED, REJECT_INVALID);
            continue;
        };
        // 匿名请求的快照 id 由客户端填写，不能用来排除别人的契约；只有按名
        // 路径才使用内核回读的可信 id。
        let Some(spec) = admission_spec(
            &request,
            pool_snapshot.budget,
            pool_snapshot.period,
            pool_snapshot.deadline,
            pool_snapshot.latency,
            0,
        ) else {
            let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
            continue;
        };
        let admitted = encode_spec(spec);
        let child = cadence_split(pool, &admitted);
        if child == u64::MAX {
            let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
            continue;
        }
        // 客户端只需要读取与附着自己的契约；写权限留给 Tempo 与父契约。
        let granted = handle_duplicate(child, RIGHT_READ | RIGHT_TRANSFER);
        let _ = close_handle(child);
        if granted == u64::MAX {
            let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
            continue;
        }
        if send_handle(sender, TEMPO_RESPONSE, 0, granted) != 0 {
            let _ = close_handle(granted);
            let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
        }
    }
}

hk_user_api::server_boilerplate!();
