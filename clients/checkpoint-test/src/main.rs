//! Checkpoint Server 自检：保存、恢复并列出检查点。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_server_api::checkpoint::{CHECKPOINT_SERVICE, list, restore, save};
#[cfg(target_os = "none")]
use hk_server_api::{exit, runtime::wait_service, write};

/// 被检查点的任务。
#[cfg(target_os = "none")]
const TARGET: &str = "hk-server-echo";

/// 服务入口：保存、恢复并列出检查点。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let Some(service) = wait_service(CHECKPOINT_SERVICE) else {
        write(b"hk-server-checkpoint-test: service unavailable\n");
        exit(1)
    };
    let Some(_target) = wait_service(TARGET) else {
        write(b"hk-server-checkpoint-test: target unavailable\n");
        exit(2)
    };
    let mut buffer = [0_u8; 512];
    match save(service, endpoint, TARGET, &mut buffer) {
        Ok(length) => {
            write(b"hk-server-checkpoint-test: saved ");
            write(&buffer[..length.min(buffer.len())]);
        }
        Err(_) => {
            write(b"hk-server-checkpoint-test: save failed\n");
            exit(3)
        }
    }
    match restore(service, endpoint, TARGET, &mut buffer) {
        Ok(length) => {
            write(b"hk-server-checkpoint-test: restored ");
            write(&buffer[..length.min(buffer.len())]);
        }
        Err(_) => {
            write(b"hk-server-checkpoint-test: restore failed\n");
            exit(4)
        }
    }
    match list(service, endpoint, &mut buffer) {
        Ok(length) => {
            write(b"hk-server-checkpoint-test: listed ");
            write(&buffer[..length.min(buffer.len())]);
        }
        Err(_) => {
            write(b"hk-server-checkpoint-test: list failed\n");
            exit(5)
        }
    }
    write(b"hk-server-checkpoint-test: done\n");
    exit(0)
}

hk_server_api::server_boilerplate!();
