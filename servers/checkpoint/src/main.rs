//! Checkpoint Server：保存、恢复与列出 Native Task 检查点。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::format;
#[cfg(target_os = "none")]
use alloc::string::String;
#[cfg(target_os = "none")]
use alloc::vec::Vec;

#[cfg(target_os = "none")]
use hk_server_api::checkpoint::{
    CHECKPOINT_LIST, CHECKPOINT_RESTORE, CHECKPOINT_RESULT, CHECKPOINT_SAVE,
};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::root::{ROOT_OP_CHECKPOINT, ROOT_OP_RESTORE_CHECKPOINT, call};
#[cfg(target_os = "none")]
use hk_server_api::{recv_data, send_data, write};

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 32 * 1024;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    write(b"hk-server-checkpoint: ready\n");
    let mut names: Vec<String> = Vec::new();
    let mut payload = [0_u8; 128];
    let mut result = [0_u8; 512];
    loop {
        let (label, _word, sender, length) = recv_data(endpoint, &mut payload);
        let name = core::str::from_utf8(&payload[..length.min(payload.len())])
            .unwrap_or("")
            .trim();
        let text = match label {
            CHECKPOINT_SAVE => {
                let text = root_text(ROOT_OP_CHECKPOINT, name, &mut result);
                if text.starts_with("已检查点") && !name.is_empty() {
                    names.retain(|saved| saved != name);
                    names.push(String::from(name));
                }
                text
            }
            CHECKPOINT_RESTORE => root_text(ROOT_OP_RESTORE_CHECKPOINT, name, &mut result),
            CHECKPOINT_LIST => {
                if names.is_empty() {
                    String::from("无检查点\n")
                } else {
                    let mut text = String::from("检查点\n");
                    for name in &names {
                        text.push_str(name);
                        text.push('\n');
                    }
                    text
                }
            }
            _ => String::from("未知检查点操作\n"),
        };
        let _ = send_data(sender, CHECKPOINT_RESULT, text.as_bytes());
    }
}

#[cfg(target_os = "none")]
fn root_text(op: u64, name: &str, buffer: &mut [u8]) -> String {
    match call(op, name, buffer) {
        Ok(length) => String::from_utf8_lossy(&buffer[..length.min(buffer.len())]).into_owned(),
        Err(code) => format!("Root 控制失败：{code}\n"),
    }
}

hk_server_api::server_boilerplate!();
