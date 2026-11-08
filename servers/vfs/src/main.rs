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
use hk_fs::{Backend, DocSessions};
#[cfg(target_os = "none")]
use hk_fs_remote::RemoteHolyFs;
#[cfg(target_os = "none")]
use hk_holy_service::{Drives, handle};
#[cfg(target_os = "none")]
use hk_proto::holy::{HOLY_REQUEST, HOLY_RESPONSE, decode_drives, decode_request};
#[cfg(target_os = "none")]
use hk_proto::vfs::encode_response;
#[cfg(target_os = "none")]
use hk_proto_client::HolyClient;
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{recv_data_handle, send_data, write, yield_now};
#[cfg(target_os = "none")]
use hk_vfs::{Devfs, PathObject, Procfs, Tmpfs};

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
    let holyfs = hk_server_api::boot_cmdline(&mut cmdline_buffer)
        .is_some_and(|line| line.contains("vfs.fs=holyfs"));
    let mut drives = Drives::new();
    if holyfs {
        for letter in discover(endpoint, "hk-server-holyfs") {
            if let Some(backend) = remote(endpoint, letter, "hk-server-holyfs") {
                insert_remote(&mut drives, letter, backend);
            }
        }
        if drives.is_empty() {
            write(b"hk-server-vfs: no drives\n");
        }
    } else if let Some(backend) = remote(endpoint, b'E', "hk-server-ext4") {
        insert_remote(&mut drives, b'E', backend);
    } else {
        write(b"hk-server-vfs: no ext4 service\n");
    }
    register_local(&mut drives, b'T', PathObject::new(Tmpfs::new()));
    register_local(&mut drives, b'D', PathObject::new(Devfs::new()));
    register_local(&mut drives, b'P', PathObject::new(Procfs::new()));

    let mut sessions = DocSessions::new();
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
            reply(sender, hk_proto::vfs::STATUS_INVALID, &[], &mut response);
            continue;
        };
        let (status, written) = handle(&mut drives, &mut sessions, sender, &request, &mut body);
        reply(sender, status, &body[..written], &mut response);
    }
}

#[cfg(target_os = "none")]
fn insert_remote(drives: &mut Drives, letter: u8, backend: RemoteHolyFs) {
    let volume = backend.volume();
    let root = backend.root();
    if drives.insert(letter, Box::new(backend), volume, root) {
        log_drive(letter);
    }
}

#[cfg(target_os = "none")]
fn register_local<P: hk_fs::Backend + 'static>(drives: &mut Drives, letter: u8, backend: P) {
    if drives.insert_local(letter, backend) {
        log_drive(letter);
    }
}

#[cfg(target_os = "none")]
fn log_drive(letter: u8) {
    write(b"hk-server-vfs: drive ");
    write(&[letter, b':', b'\n']);
}

#[cfg(target_os = "none")]
fn discover(endpoint: u64, service: &'static str) -> Vec<u8> {
    for _ in 0..4096 {
        if hk_server_api::service_endpoint(service) == 0 {
            let _ = yield_now();
            continue;
        }
        let mut probe = HolyClient::new_with_service(endpoint, service);
        if let Ok(bytes) = probe.drives() {
            return decode_drives(bytes).map(|drive| drive.letter).collect();
        }
        let _ = yield_now();
    }
    Vec::new()
}

#[cfg(target_os = "none")]
fn remote(endpoint: u64, letter: u8, service: &'static str) -> Option<RemoteHolyFs> {
    for _ in 0..4096 {
        if let Ok(backend) = RemoteHolyFs::new(endpoint, letter, service) {
            return Some(backend);
        }
        let _ = yield_now();
    }
    None
}

hk_server_api::server_boilerplate!();
