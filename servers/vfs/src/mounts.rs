//! 块设备挂载：HolyFS 与 ext4。

use alloc::boxed::Box;
use alloc::format;
use alloc::vec::Vec;

use hk_block::ipc::ClientDisk;
use hk_fs_ext4::mount_at_with_wall_clock;
use hk_hal::CivilTime;
use hk_server_api::holy::{HolyClient, decode_drives};
use hk_server_api::{service_endpoint, yield_now};
use hk_vfs::Vfs;

use crate::remote::RemoteHolyFs;

/// 提供块设备的驱动服务。
#[cfg(target_os = "none")]
const BLOCK_SERVICE: &str = "hk-server-virtio";

/// 平台民用时间来源。
#[cfg(target_os = "none")]
struct RealtimeClock;

#[cfg(target_os = "none")]
impl hk_fs_ext4::WallClock for RealtimeClock {
    fn now(&self) -> Option<CivilTime> {
        hk_server_api::civil_time()
    }
}

/// 等待独立 HolyFS 服务就绪，并把每个注册盘符挂到路径命名空间。
#[cfg(target_os = "none")]
pub(crate) fn mount_holyfs(filesystem: &mut Vfs, endpoint: u64, target: &str) -> bool {
    for _ in 0..4096 {
        if service_endpoint("hk-server-holyfs") == 0 {
            let _ = yield_now();
            continue;
        }
        let mut probe = Box::new(HolyClient::new(endpoint));
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
        for drive in drives {
            let path = if drive == b'H' {
                alloc::string::String::from(target)
            } else {
                format!("{target}-{}", drive as char)
            };
            let _ = filesystem.create_dir(&path);
            let Ok(backend) = RemoteHolyFs::new(endpoint, drive) else {
                continue;
            };
            if filesystem.mount(&path, Box::new(backend)).is_ok() && drive == b'H' {
                mounted = true;
            }
        }
        return mounted;
    }
    false
}

/// 等待驱动就绪并在 `/disk` 挂载 ext4。
#[cfg(target_os = "none")]
pub(crate) fn mount_ext4(filesystem: &mut Vfs, endpoint: u64, target: &str) -> bool {
    for _ in 0..4096 {
        let Ok(disk) = ClientDisk::open(BLOCK_SERVICE, endpoint) else {
            let _ = yield_now();
            continue;
        };
        return mount_at_with_wall_clock(filesystem, target, disk, RealtimeClock).is_ok();
    }
    false
}
