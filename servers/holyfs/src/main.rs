//! HolyFS Server：独立挂载 Holy 卷并承载原生文档协议。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::vec;

#[cfg(target_os = "none")]
use hk_block::ipc::ClientDisk;
#[cfg(target_os = "none")]
use hk_fs_holyfs::HolyFs;
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::holy::{HOLY_REQUEST, HOLY_RESPONSE};
#[cfg(target_os = "none")]
use hk_server_api::vfs::{STATUS_INVALID, decode_request, encode_response};
#[cfg(target_os = "none")]
use hk_server_api::{recv_data_handle, send_data, write, yield_now};

#[cfg(target_os = "none")]
mod holy;
#[cfg(target_os = "none")]
mod sessions;

#[cfg(target_os = "none")]
use sessions::Sessions;

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 1024 * 1024;

/// 提供块设备的驱动服务。
#[cfg(target_os = "none")]
const BLOCK_SERVICE: &str = "hk-server-virtio";

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

#[cfg(target_os = "none")]
fn reply(sender: u64, status: u8, data: &[u8], response: &mut [u8]) {
    if let Some(length) = encode_response(response, status, data) {
        let _ = send_data(sender, HOLY_RESPONSE, &response[..length]);
    }
}

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let mut cmdline_buffer = [0_u8; 128];
    let enabled = hk_server_api::boot_cmdline(&mut cmdline_buffer)
        .is_some_and(|line| line.contains("vfs.fs=holyfs"));
    let mut drive = None;
    if enabled {
        for _ in 0..4096 {
            let Ok(disk) = ClientDisk::open(BLOCK_SERVICE, endpoint) else {
                let _ = yield_now();
                continue;
            };
            match HolyFs::mount(disk) {
                Ok(volume) => {
                    drive = Some(volume);
                    break;
                }
                Err(_) => {
                    write("hk-server-holyfs: 挂载失败\n".as_bytes());
                    break;
                }
            }
        }
    }
    match drive {
        Some(_) => {
            write(b"hk-server-holyfs: mounted H:\n");
            write(b"hk-server-holyfs: drive H:\n");
        }
        None if enabled => write("hk-server-holyfs: 文件系统不可用\n".as_bytes()),
        None => write(b"hk-server-holyfs: disabled\n"),
    }

    let mut sessions = Sessions::new();
    let mut request = vec![0_u8; 64 * 1024];
    let mut body = vec![0_u8; 64 * 1024 - 4];
    let mut response = vec![0_u8; 64 * 1024];

    loop {
        let (label, _word, sender, length, frame_handle) =
            recv_data_handle(endpoint, &mut request, 0);
        if label != HOLY_REQUEST || frame_handle != 0 {
            continue;
        }
        let Some(request) = decode_request(&request[..length.min(request.len())]) else {
            reply(sender, STATUS_INVALID, &[], &mut response);
            continue;
        };
        let (status, written) =
            holy::handle(drive.as_mut(), &mut sessions, sender, &request, &mut body);
        reply(sender, status, &body[..written], &mut response);
    }
}

hk_server_api::server_boilerplate!();
