//! 把原生 VFS 的盘符投影到路径命名空间。

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use hk_proto::holy::decode_drives;
#[cfg(any(target_os = "none", test))]
use hk_proto_client::HolyClient;
use hk_server_api::{service_endpoint, yield_now};
use hk_vfs::Vfs;

use hk_fs_remote::RemoteHolyFs;

/// 等待原生 VFS 就绪，并以原生盘符构造路径命名空间。
#[cfg(target_os = "none")]
pub(crate) fn mount_drives(endpoint: u64, ext4_root: bool) -> Option<Vfs> {
    for _ in 0..4096 {
        if service_endpoint("hk-server-vfs") == 0 {
            let _ = yield_now();
            continue;
        }
        let mut probe = HolyClient::new_with_service(endpoint, "hk-server-vfs");
        let Ok(bytes) = probe.drives() else {
            let _ = yield_now();
            continue;
        };
        let drives: Vec<u8> = decode_drives(bytes).map(|drive| drive.letter).collect();
        if drives.is_empty() {
            let _ = yield_now();
            continue;
        }
        let root = if ext4_root && drives.contains(&b'E') {
            b'E'
        } else if drives.contains(&b'T') {
            b'T'
        } else {
            drives[0]
        };
        let Ok(backend) = RemoteHolyFs::new(endpoint, root, "hk-server-vfs") else {
            let _ = yield_now();
            continue;
        };
        let mut filesystem = Vfs::new(Box::new(backend));
        for path in ["/disk", "/tmp", "/dev", "/proc"] {
            let _ = filesystem.create_dir(path);
        }
        for drive in drives {
            if drive == root && drive != b'T' {
                continue;
            }
            let path = match drive {
                b'H' => String::from("/disk"),
                b'I' => String::from("/disk-I"),
                b'E' => String::from("/disk"),
                b'T' => String::from("/tmp"),
                b'D' => String::from("/dev"),
                b'P' => String::from("/proc"),
                _ => continue,
            };
            let _ = filesystem.create_dir(&path);
            let Ok(backend) = RemoteHolyFs::new(endpoint, drive, "hk-server-vfs") else {
                continue;
            };
            let _ = filesystem.mount(&path, Box::new(backend));
        }
        return Some(filesystem);
    }
    None
}
