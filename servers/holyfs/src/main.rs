//! HolyFS Server：独立挂载 Holy 卷并承载原生文档协议。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::vec;
#[cfg(target_os = "none")]
use alloc::vec::Vec;

#[cfg(target_os = "none")]
use hk_block::ipc::ClientDisk;
#[cfg(target_os = "none")]
use hk_block::{BlockDevice, DiskSlice};
#[cfg(target_os = "none")]
use hk_fs_holyfs::{
    HolyError, HolyFs, VOLUME_H, VOLUME_I, VolumeId, format_with_volume_id, is_formatted,
};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::holy::{HOLY_REQUEST, HOLY_RESPONSE, OP_SYNC, decode_request};
#[cfg(target_os = "none")]
use hk_server_api::vfs::{STATUS_INVALID, encode_response};
#[cfg(target_os = "none")]
use hk_server_api::{recv_data_handle, recv_data_handle_try, send_data, write, yield_now};

#[cfg(target_os = "none")]
mod holy;
#[cfg(target_os = "none")]
mod sessions;
#[cfg(target_os = "none")]
mod volumes;

#[cfg(target_os = "none")]
use sessions::Sessions;
#[cfg(target_os = "none")]
use volumes::{ServiceDisk, SharedClient, Volumes};

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
    let enabled = hk_server_api::boot_cmdline(&mut cmdline_buffer)
        .is_some_and(|line| line.contains("vfs.fs=holyfs"));
    let mut volumes = Volumes::new();
    if enabled {
        if let Some(disk) = open_disk(endpoint) {
            let disk = SharedClient::new(disk);
            let total = disk.capacity_blocks();
            let half = total / 2;
            if half == 0 {
                write("hk-server-holyfs: 磁盘容量不足\n".as_bytes());
            } else {
                if let Some(volume) = prepare(DiskSlice::new(disk.clone(), 0, half), VOLUME_H) {
                    let _ = volumes.insert(b'H', volume);
                    volumes.register_service(b'H', BLOCK_SERVICE);
                    write(b"hk-server-holyfs: mounted H:\n");
                    write(b"hk-server-holyfs: drive H:\n");
                }
                if let Some(volume) = prepare(DiskSlice::new(disk, half, total - half), VOLUME_I) {
                    let _ = volumes.insert(b'I', volume);
                    volumes.register_service(b'I', BLOCK_SERVICE);
                    write(b"hk-server-holyfs: mounted I:\n");
                    write(b"hk-server-holyfs: drive I:\n");
                }
            }
        }
        if volumes.list().is_empty() {
            write("hk-server-holyfs: 文件系统不可用\n".as_bytes());
        }
    } else {
        write(b"hk-server-holyfs: disabled\n");
    }

    let mut sessions = Sessions::new();
    let mut packet = vec![0_u8; 64 * 1024];
    let mut body = vec![0_u8; 64 * 1024 - 4];
    let mut response = vec![0_u8; 64 * 1024];
    let mut pending: Vec<(u8, u64)> = Vec::new();

    loop {
        let (label, _word, sender, length, frame_handle) =
            recv_data_handle(endpoint, &mut packet, 0);
        if label != HOLY_REQUEST || frame_handle != 0 {
            continue;
        }
        let Some(request) = decode_request(&packet[..length.min(packet.len())]) else {
            reply(sender, STATUS_INVALID, &[], &mut response);
            continue;
        };
        if request.op != OP_SYNC {
            let (status, written) = holy::handle(
                &mut volumes,
                &mut sessions,
                sender,
                endpoint,
                &request,
                &mut body,
            );
            reply(sender, status, &body[..written], &mut response);
            continue;
        }

        let letter = if request.drive == 0 {
            b'H'
        } else {
            request.drive
        };
        pending.push((letter, sender));
        loop {
            let (label, _word, sender, length, frame_handle) =
                recv_data_handle_try(endpoint, &mut packet, 0);
            if label != HOLY_REQUEST || frame_handle != 0 {
                break;
            }
            let Some(request) = decode_request(&packet[..length.min(packet.len())]) else {
                reply(sender, STATUS_INVALID, &[], &mut response);
                continue;
            };
            if request.op == OP_SYNC {
                let letter = if request.drive == 0 {
                    b'H'
                } else {
                    request.drive
                };
                pending.push((letter, sender));
                continue;
            }
            let (status, written) = holy::handle(
                &mut volumes,
                &mut sessions,
                sender,
                endpoint,
                &request,
                &mut body,
            );
            reply(sender, status, &body[..written], &mut response);
        }

        let mut committed: Vec<(u8, u8)> = Vec::new();
        for (letter, _) in &pending {
            if committed.iter().any(|(done, _)| done == letter) {
                continue;
            }
            committed.push((*letter, holy::sync(&mut volumes, *letter)));
        }
        for (letter, sender) in pending.drain(..) {
            let status = committed
                .iter()
                .find(|(done, _)| *done == letter)
                .map_or(1, |(_, status)| *status);
            reply(sender, status, &[], &mut response);
        }
    }
}

#[cfg(target_os = "none")]
fn open_disk(endpoint: u64) -> Option<ClientDisk> {
    for _ in 0..4096 {
        if let Ok(disk) = ClientDisk::open(BLOCK_SERVICE, endpoint) {
            return Some(disk);
        }
        let _ = yield_now();
    }
    None
}

#[cfg(target_os = "none")]
fn prepare(mut disk: ServiceDisk, volume: VolumeId) -> Option<HolyFs<ServiceDisk>> {
    if !is_formatted(&disk) {
        disk = match format_with_volume_id(disk, volume) {
            Ok(disk) => disk,
            Err(_) => return None,
        };
    }
    mount_volume(disk)
}

#[cfg(target_os = "none")]
fn mount_volume(disk: ServiceDisk) -> Option<HolyFs<ServiceDisk>> {
    match HolyFs::mount(disk) {
        Ok(filesystem) => Some(filesystem),
        Err(error) => {
            write(format_error(error).as_bytes());
            None
        }
    }
}

#[cfg(target_os = "none")]
fn format_error(error: HolyError) -> &'static str {
    match error {
        HolyError::Io => "hk-server-holyfs: I/O 错误\n",
        HolyError::Corrupt => "hk-server-holyfs: 卷损坏\n",
        HolyError::NoSpace => "hk-server-holyfs: 空间不足\n",
        _ => "hk-server-holyfs: 挂载失败\n",
    }
}

hk_server_api::server_boilerplate!();
