//! VFS Server：DocId、集合、修订与文档令牌的原生命名空间。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::boxed::Box;
#[cfg(target_os = "none")]
use alloc::vec;
#[cfg(target_os = "none")]
use alloc::vec::Vec;

#[cfg(target_os = "none")]
use hk_fs::Backend;
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::holy::{HOLY_REQUEST, HOLY_RESPONSE, HolyClient, decode_drives, decode_request};
#[cfg(target_os = "none")]
use hk_server_api::vfs::encode_response;
#[cfg(target_os = "none")]
use hk_server_api::{recv_data_handle, send_data, write, yield_now};

#[cfg(target_os = "none")]
mod drives;
#[cfg(target_os = "none")]
mod holy;
#[cfg(target_os = "none")]
mod remote;
#[cfg(target_os = "none")]
mod sessions;

#[cfg(target_os = "none")]
use drives::Drives;
#[cfg(target_os = "none")]
use remote::RemoteHolyFs;
#[cfg(target_os = "none")]
use sessions::Sessions;

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 2 * 1024 * 1024;

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
    let mut drives = Drives::new();
    if enabled {
        let letters = discover(endpoint);
        for letter in letters {
            if let Some(backend) = remote(endpoint, letter) {
                let volume = backend.volume();
                let root = backend.root();
                if drives.insert(letter, Box::new(backend), volume, root) {
                    write(b"hk-server-vfs: drive ");
                    write(&[letter, b':', b'\n']);
                }
            }
        }
        if drives.is_empty() {
            write(b"hk-server-vfs: no drives\n");
        }
    } else {
        write(b"hk-server-vfs: disabled\n");
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
            reply(
                sender,
                hk_server_api::vfs::STATUS_INVALID,
                &[],
                &mut response,
            );
            continue;
        };
        let (status, written) =
            holy::handle(&mut drives, &mut sessions, sender, &request, &mut body);
        reply(sender, status, &body[..written], &mut response);
    }
}

#[cfg(target_os = "none")]
fn discover(endpoint: u64) -> Vec<u8> {
    for _ in 0..4096 {
        if hk_server_api::service_endpoint("hk-server-holyfs") == 0 {
            let _ = yield_now();
            continue;
        }
        let mut probe = Box::new(HolyClient::new_with_service(endpoint, "hk-server-holyfs"));
        if let Ok(bytes) = probe.drives() {
            return decode_drives(bytes).map(|drive| drive.letter).collect();
        }
        let _ = yield_now();
    }
    Vec::new()
}

#[cfg(target_os = "none")]
fn remote(endpoint: u64, letter: u8) -> Option<RemoteHolyFs> {
    for _ in 0..4096 {
        if let Ok(backend) = RemoteHolyFs::new(endpoint, letter, "hk-server-holyfs") {
            return Some(backend);
        }
        let _ = yield_now();
    }
    None
}

hk_server_api::server_boilerplate!();
