//! ext4 Server：独立挂载 ext4，并以 Holy 文档协议暴露 `E:`。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::vec;

#[cfg(target_os = "none")]
use hk_base::CivilTime;
#[cfg(target_os = "none")]
use hk_block_ipc::ClientDisk;
#[cfg(target_os = "none")]
use hk_fs::DocSessions;
#[cfg(target_os = "none")]
use hk_fs_ext4::{Ext4Backend, WallClock};
#[cfg(target_os = "none")]
use hk_holy_service::{Drives, handle};
#[cfg(target_os = "none")]
use hk_proto::holy::{HOLY_REQUEST, HOLY_RESPONSE, decode_request};
#[cfg(target_os = "none")]
use hk_proto::vfs::{STATUS_INVALID, encode_response};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{recv_data_handle, send_data, write, yield_now};
#[cfg(target_os = "none")]
use hk_vfs::PathObject;

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 2 * 1024 * 1024;

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
    let holyfs = hk_server_api::boot_cmdline(&mut cmdline_buffer)
        .is_some_and(|line| line.contains("vfs.fs=holyfs"));
    let mut drives = Drives::new();
    if holyfs {
        write(b"hk-server-ext4: disabled\n");
    } else if let Some(ext4) = mount(endpoint) {
        if drives.insert_local(b'E', ext4) {
            write(b"hk-server-ext4: mounted E:\n");
            write(b"hk-server-ext4: drive E:\n");
        }
    } else {
        write("hk-server-ext4: 文件系统不可用\n".as_bytes());
    }

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
            reply(sender, STATUS_INVALID, &[], &mut response);
            continue;
        };
        let (status, written) = handle(&mut drives, &mut sessions, sender, &request, &mut body);
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
fn mount(endpoint: u64) -> Option<PathObject<Ext4Backend<ClientDisk>>> {
    for _ in 0..4096 {
        let Ok(disk) = ClientDisk::open(BLOCK_SERVICE, endpoint) else {
            let _ = yield_now();
            continue;
        };
        return Ext4Backend::mount_with_wall_clock(disk, RealtimeClock)
            .ok()
            .map(PathObject::new);
    }
    None
}

hk_server_api::server_boilerplate!();
