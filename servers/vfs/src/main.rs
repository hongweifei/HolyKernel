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
use hk_base::CivilTime;
#[cfg(target_os = "none")]
use hk_block_ipc::ClientDisk;
#[cfg(target_os = "none")]
use hk_fs::{Backend, DocSessions};
#[cfg(target_os = "none")]
use hk_fs_ext4::{Ext4Backend, WallClock};
#[cfg(target_os = "none")]
use hk_proto::holy::{
    DocId as WireDocId, HOLY_REQUEST, HOLY_RESPONSE, HolyClient, VolumeView, decode_drives,
    decode_request,
};
#[cfg(target_os = "none")]
use hk_proto::vfs::encode_response;
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{recv_data_handle, send_data, write, yield_now};
#[cfg(target_os = "none")]
use hk_vfs::{Devfs, PathObject, Procfs, Tmpfs};

#[cfg(target_os = "none")]
mod drives;
#[cfg(target_os = "none")]
mod holy;
#[cfg(target_os = "none")]
mod remote;

#[cfg(target_os = "none")]
use drives::Drives;
#[cfg(target_os = "none")]
use remote::RemoteHolyFs;

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
    } else if let Some(ext4) = mount_ext4(endpoint) {
        register(&mut drives, b'E', ext4);
    }
    register(&mut drives, b'T', PathObject::new(Tmpfs::new()));
    register(&mut drives, b'D', PathObject::new(Devfs::new()));
    register(&mut drives, b'P', PathObject::new(Procfs::new()));

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
        let (status, written) =
            holy::handle(&mut drives, &mut sessions, sender, &request, &mut body);
        reply(sender, status, &body[..written], &mut response);
    }
}

#[cfg(target_os = "none")]
struct RealtimeClock;

#[cfg(target_os = "none")]
impl WallClock for RealtimeClock {
    fn now(&self) -> Option<CivilTime> {
        hk_server_api::civil_time()
    }
}

#[cfg(target_os = "none")]
fn mount_ext4(endpoint: u64) -> Option<PathObject<Ext4Backend<ClientDisk>>> {
    for _ in 0..4096 {
        let Ok(disk) = ClientDisk::open("hk-server-virtio", endpoint) else {
            let _ = yield_now();
            continue;
        };
        return Ext4Backend::mount_with_wall_clock(disk, RealtimeClock)
            .ok()
            .map(PathObject::new);
    }
    None
}

#[cfg(target_os = "none")]
fn register<P: Backend + 'static>(drives: &mut Drives, letter: u8, backend: P) {
    let root = backend.root();
    let volume = VolumeView {
        volume: 0x484F_4C59_4653_0200 + letter as u128,
        generation: 0,
        epoch: 0,
        root: WireDocId::new(root.get()),
    };
    if drives.insert(letter, Box::new(backend), volume, root) {
        write(b"hk-server-vfs: drive ");
        write(&[letter, b':', b'\n']);
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
