//! 把原生 VFS 的盘符投影到路径命名空间。

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use hk_server_api::holy::{HolyClient, decode_drives};
use hk_server_api::{service_endpoint, yield_now};
use hk_vfs::Vfs;

use crate::remote::RemoteHolyFs;

/// 等待原生 VFS 就绪，并按盘符建立路径挂载点。
#[cfg(target_os = "none")]
pub(crate) fn mount_drives(filesystem: &mut Vfs, endpoint: u64, ext4_root: bool) -> bool {
    for _ in 0..4096 {
        if service_endpoint("hk-server-vfs") == 0 {
            let _ = yield_now();
            continue;
        }
        let mut probe = Box::new(HolyClient::new_with_service(endpoint, "hk-server-vfs"));
        let Ok(bytes) = probe.drives() else {
            let _ = yield_now();
            continue;
        };
        let drives: Vec<u8> = decode_drives(bytes).map(|drive| drive.letter).collect();
        if drives.is_empty() {
            let _ = yield_now();
            continue;
        }
        let mut mounted = false;
        if ext4_root && drives.contains(&b'E') && mount(filesystem, endpoint, b'E', "/").is_ok() {
            mounted = true;
        }
        for path in ["/disk", "/tmp", "/dev", "/proc"] {
            let _ = filesystem.create_dir(path);
        }
        for drive in drives {
            let path = match drive {
                b'H' => String::from("/disk"),
                b'I' => String::from("/disk-I"),
                b'E' if !ext4_root => String::from("/disk"),
                b'E' => continue,
                b'T' => String::from("/tmp"),
                b'D' => String::from("/dev"),
                b'P' => String::from("/proc"),
                _ => continue,
            };
            let _ = filesystem.create_dir(&path);
            if mount(filesystem, endpoint, drive, &path).is_ok() {
                mounted = true;
            }
        }
        return mounted;
    }
    false
}

#[cfg(target_os = "none")]
fn mount(filesystem: &mut Vfs, endpoint: u64, drive: u8, target: &str) -> Result<(), ()> {
    let backend = RemoteHolyFs::new(endpoint, drive, "hk-server-vfs").map_err(|_| ())?;
    filesystem.mount(target, Box::new(backend)).map_err(|_| ())
}
