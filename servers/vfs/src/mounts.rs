//! 块设备挂载：HolyFS 与 ext4。

use alloc::boxed::Box;

use hk_block::ipc::ClientDisk;
use hk_fs_ext4::mount_at_with_wall_clock;
use hk_fs_holyfs::HolyFs;
use hk_hal::CivilTime;
use hk_server_api::yield_now;
use hk_vfs::Vfs;

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

/// 等待驱动就绪并挂载 HolyFS。
#[cfg(target_os = "none")]
pub(crate) fn mount_holyfs(filesystem: &mut Vfs, endpoint: u64, target: &str) -> bool {
    for _ in 0..4096 {
        let Ok(disk) = ClientDisk::open(BLOCK_SERVICE, endpoint) else {
            let _ = yield_now();
            continue;
        };
        let Ok(backend) = HolyFs::mount(disk) else {
            return false;
        };
        return filesystem.mount(target, Box::new(backend)).is_ok();
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
