//! 运行与冒烟共用的磁盘镜像生成与 H: 卷文档播种。

use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use hk_block::{BlockDevice, BlockError, MemoryDisk, SECTOR_SIZE};
use hk_fs_holyfs::{
    DocumentKind, HolyError, HolyFs, VOLUME_H, VOLUME_I, VolumeId, format_with_volume_id,
    is_formatted,
};
use hk_hal::CivilTime;

use crate::failure::{Failure, Result};
use crate::workspace;

/// 测试/运行磁盘大小。
pub(crate) const DISK_BYTES: u64 = 64 * 1024 * 1024;

/// 播种到 H: 卷的文档：名字、源文件与所属集合。
const HOLY_DOCS: &[(&str, &str, Option<&str>)] = &[
    ("Welcome", "docs/holy/Welcome.hd", None),
    ("Home", "docs/holy/Home.hd", None),
    ("Once", "docs/holy/Once.hd", None),
    ("Help", "docs/holy/Help.hd", None),
    ("Overview", "docs/holy/System/Overview.hd", Some("System")),
    ("HolyFS", "docs/holy/System/HolyFS.hd", Some("System")),
    ("UI", "docs/holy/System/UI.hd", Some("System")),
];

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

/// 确保 ext4 运行磁盘存在；缺失时格式化。
pub(crate) fn ensure_ext4(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    ensure_parent(path)?;
    let blocks = DISK_BYTES / SECTOR_SIZE as u64;
    let disk = MemoryDisk::new(blocks, SECTOR_SIZE);
    let disk = hk_fs_ext4::format_with_wall_clock(disk, HostClock)
        .map_err(|error| Failure::new(format!("格式化 ext4 运行磁盘失败：{error}")))?;
    write_image(path, disk)
}

/// 确保 HolyFS 运行磁盘存在，并把 H: 卷文档同步到镜像。
pub(crate) fn ensure_holyfs(path: &Path) -> Result<()> {
    let disk = if path.exists() {
        let bytes = fs::read(path)
            .map_err(|error| Failure::new(format!("读取 {} 失败：{error}", path.display())))?;
        MemoryDisk::from_data(bytes, SECTOR_SIZE)
            .ok_or_else(|| Failure::new("HolyFS 运行磁盘长度不是扇区整数倍"))?
    } else {
        ensure_parent(path)?;
        let blocks = DISK_BYTES / SECTOR_SIZE as u64;
        MemoryDisk::new(blocks, SECTOR_SIZE)
    };
    let disk = seed_holy_docs(disk)?;
    write_image(path, disk)
}

/// 把 H: 文档写入镜像，并把 I: 初始化为第二个卷。
pub(crate) fn seed_holy_docs(disk: MemoryDisk) -> Result<MemoryDisk> {
    let shared = Rc::new(RefCell::new(disk));
    let total = shared.borrow().capacity_blocks();
    let half = total / 2;
    if half == 0 {
        return Err(Failure::new("HolyFS 镜像容量不足以划分两个卷"));
    }
    {
        let mut filesystem = mount_or_format(SharedDisk::new(shared.clone(), 0, half), VOLUME_H)?;
        write_holy_docs(&mut filesystem)?;
        filesystem
            .sync()
            .map_err(|error| Failure::new(format!("提交 H: 文档失败：{error:?}")))?;
    }
    {
        let mut filesystem = mount_or_format(
            SharedDisk::new(shared.clone(), half, total - half),
            VOLUME_I,
        )?;
        filesystem
            .sync()
            .map_err(|error| Failure::new(format!("提交 I: 卷失败：{error:?}")))?;
    }
    let disk = Rc::try_unwrap(shared)
        .map_err(|_| Failure::new("文档播种后仍持有镜像引用"))?
        .into_inner();
    Ok(disk)
}

fn mount_or_format(disk: SharedDisk, volume: VolumeId) -> Result<HolyFs<SharedDisk>> {
    let disk = if is_formatted(&disk) {
        disk
    } else {
        format_with_volume_id(disk, volume)
            .map_err(|error| Failure::new(format!("格式化 HolyFS 卷失败：{error}")))?
    };
    HolyFs::mount(disk).map_err(|error| Failure::new(format!("挂载 HolyFS 卷失败：{error}")))
}

/// 写入 H: 卷文档；集合按需创建，文档按名字覆盖。
fn write_holy_docs(filesystem: &mut HolyFs<SharedDisk>) -> Result<()> {
    let root = filesystem.root();
    for (name, source, collection) in HOLY_DOCS {
        let parent = match collection {
            None => root,
            Some(collection) => match filesystem.lookup(root, collection) {
                Ok(document) => document,
                Err(HolyError::NotFound) => filesystem
                    .create(root, collection, DocumentKind::Collection)
                    .map_err(|error| {
                        Failure::new(format!("创建集合 {collection} 失败：{error:?}"))
                    })?,
                Err(error) => {
                    return Err(Failure::new(format!(
                        "查找集合 {collection} 失败：{error:?}"
                    )));
                }
            },
        };
        let bytes = read_source(source)?;
        let document = match filesystem.lookup(parent, name) {
            Ok(document) => document,
            Err(HolyError::NotFound) => filesystem
                .create(parent, name, DocumentKind::Content)
                .map_err(|error| Failure::new(format!("创建文档 {name} 失败：{error:?}")))?,
            Err(error) => {
                return Err(Failure::new(format!("查找文档 {name} 失败：{error:?}")));
            }
        };
        filesystem
            .write_document_id(document, &bytes)
            .map_err(|error| Failure::new(format!("写入文档 {name} 失败：{error:?}")))?;
    }
    Ok(())
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| Failure::new(format!("创建 {} 失败：{error}", parent.display())))?;
    }
    Ok(())
}

fn write_image(path: &Path, disk: MemoryDisk) -> Result<()> {
    fs::write(path, disk.into_data())
        .map_err(|error| Failure::new(format!("写入 {} 失败：{error}", path.display())))
}

/// 读取仓库内的文档源文件。
fn read_source(source: &str) -> Result<Vec<u8>> {
    let path = workspace::root().join(source);
    fs::read(&path).map_err(|error| Failure::new(format!("读取 {} 失败：{error}", path.display())))
}

/// 与挂载中的后端共享同一个内存盘，并暴露其中一个卷区间。
struct SharedDisk {
    disk: Rc<RefCell<MemoryDisk>>,
    start: u64,
    blocks: u64,
}

impl SharedDisk {
    fn new(disk: Rc<RefCell<MemoryDisk>>, start: u64, blocks: u64) -> Self {
        Self {
            disk,
            start,
            blocks,
        }
    }

    fn check(&self, block: u64, length: usize) -> core::result::Result<(), BlockError> {
        if length == 0 || !length.is_multiple_of(SECTOR_SIZE) {
            return Err(BlockError::InvalidLength);
        }
        let end = block
            .checked_add((length / SECTOR_SIZE) as u64)
            .ok_or(BlockError::OutOfRange)?;
        if end > self.blocks {
            return Err(BlockError::OutOfRange);
        }
        Ok(())
    }
}

impl BlockDevice for SharedDisk {
    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn capacity_blocks(&self) -> u64 {
        self.blocks
    }

    fn read_blocks(
        &self,
        block: u64,
        buffer: &mut [u8],
    ) -> core::result::Result<usize, BlockError> {
        self.check(block, buffer.len())?;
        self.disk.borrow().read_blocks(self.start + block, buffer)
    }

    fn write_blocks(&mut self, block: u64, data: &[u8]) -> core::result::Result<usize, BlockError> {
        self.check(block, data.len())?;
        self.disk
            .borrow_mut()
            .write_blocks(self.start + block, data)
    }

    fn flush(&mut self) -> core::result::Result<(), BlockError> {
        Ok(())
    }
}
