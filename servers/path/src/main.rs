//! Path Server：路径/fd 根、请求协议与文件操作。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
#[cfg(target_os = "none")]
use alloc::vec;

#[cfg(target_os = "none")]
use hk_proto::vfs::{STATUS_INVALID, VFS_ATTACH, VFS_DETACH, VFS_REQUEST, decode_request};
#[cfg(target_os = "none")]
#[cfg(target_os = "none")]
#[cfg(target_os = "none")]
#[cfg(target_os = "none")]
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{recv_data_handle, write};

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 2 * 1024 * 1024;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

#[cfg(target_os = "none")]
mod mounts;
#[cfg(target_os = "none")]
mod protocol;
#[cfg(target_os = "none")]
mod sessions;

#[cfg(target_os = "none")]
use mounts::mount_drives;
#[cfg(target_os = "none")]
use protocol::{handle, reply};
#[cfg(target_os = "none")]
use sessions::Sessions;

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let mut cmdline_buffer = [0_u8; 128];
    let cmdline = hk_server_api::boot_cmdline(&mut cmdline_buffer).unwrap_or("");
    let ext4_root = cmdline.contains("vfs.root=ext4");
    let holyfs = cmdline.contains("vfs.fs=holyfs");
    match hk_server_api::civil_time() {
        Some(time) if time.year >= 2000 => write("hk-server-path: rtc ok\n".as_bytes()),
        _ => write("hk-server-path: rtc 不可用\n".as_bytes()),
    }
    let Some(mut filesystem) = mount_drives(endpoint, ext4_root && !holyfs) else {
        write("hk-server-path: 文件系统不可用\n".as_bytes());
        hk_server_api::exit(1)
    };
    if holyfs {
        write("hk-server-path: drives via hk-server-vfs\n".as_bytes());
    } else {
        write("hk-server-path: ext4 mounted\n".as_bytes());
    }
    write("hk-server-path: tmpfs ready\n".as_bytes());

    let mut sessions = Sessions::new();
    let mut request = vec![0_u8; 64 * 1024];
    let mut body = vec![0_u8; 64 * 1024 - 4];
    let mut response = vec![0_u8; 64 * 1024];

    loop {
        let (label, word, sender, length, frame_handle) =
            recv_data_handle(endpoint, &mut request, 0);
        if frame_handle != 0 && label == VFS_ATTACH {
            let _ = sessions.attach_mapping(sender, frame_handle, word as usize);
            continue;
        }
        if label == VFS_DETACH {
            sessions.detach(sender, &mut filesystem);
            continue;
        }
        if label != VFS_REQUEST {
            continue;
        }

        let Some(request) = decode_request(&request[..length.min(request.len())]) else {
            reply(sender, STATUS_INVALID, &[], &mut response);
            continue;
        };
        let mapped = sessions.mapping(sender).map(|(virt, length)| {
            // SAFETY: Path 帧 attach 已把该客户端的共享帧映射到此地址。
            unsafe { core::slice::from_raw_parts_mut(virt as *mut u8, length) }
        });
        let (status, written) = handle(
            &mut filesystem,
            sessions.get(sender),
            &request,
            &mut body,
            mapped,
        );
        reply(sender, status, &body[..written], &mut response);
    }
}

hk_server_api::server_boilerplate!();
