//! 运行与冒烟共用的磁盘镜像生成。

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use hk_block::{MemoryDisk, SECTOR_SIZE};
use hk_hal::CivilTime;

use crate::failure::{Failure, Result};

/// 测试/运行磁盘大小。
pub(crate) const DISK_BYTES: u64 = 64 * 1024 * 1024;

/// ext4 镜像使用的宿主时钟。
pub(crate) struct HostClock;

impl hk_fs_ext4::WallClock for HostClock {
    fn now(&self) -> Option<CivilTime> {
        host_civil_time()
    }
}

/// std::time 的原点；宿主工具只在镜像生成时使用。
const HOST_TIME_ORIGIN: CivilTime = CivilTime {
    year: 1970,
    month: 1,
    day: 1,
    hour: 0,
    minute: 0,
    second: 0,
    nanosecond: 0,
};

fn host_civil_time() -> Option<CivilTime> {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    Some(HOST_TIME_ORIGIN.add_seconds(elapsed.as_secs(), elapsed.subsec_nanos()))
}

/// 确保运行磁盘存在；缺失时格式化为空 ext4。
pub(crate) fn ensure_ext4(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| Failure::new(format!("创建 {} 失败：{error}", parent.display())))?;
    }
    let blocks = DISK_BYTES / SECTOR_SIZE as u64;
    let disk = MemoryDisk::new(blocks, SECTOR_SIZE);
    let disk = hk_fs_ext4::format_with_wall_clock(disk, HostClock)
        .map_err(|error| Failure::new(format!("格式化 ext4 运行磁盘失败：{error}")))?;
    fs::write(path, disk.into_data())
        .map_err(|error| Failure::new(format!("写入 {} 失败：{error}", path.display())))?;
    Ok(())
}
