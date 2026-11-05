//! 运行与冒烟共用的磁盘镜像生成与系统文档播种。

use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use hk_block::{BlockDevice, BlockError, MemoryDisk, SECTOR_SIZE};
use hk_fs_ext4::Ext4Backend;
use hk_hal::CivilTime;
use hk_vfs::{Backend, FileType};

use crate::failure::{Failure, Result};
use crate::workspace;

/// 测试/运行磁盘大小。
pub(crate) const DISK_BYTES: u64 = 64 * 1024 * 1024;

/// 播种到磁盘的系统文档：目标文件名与仓库内源文件。
const SYSTEM_DOCS: &[(&str, &str)] = &[
    ("architecture.md", "docs/architecture.md"),
    ("crates.md", "docs/crates.md"),
    ("shell.md", "docs/shell.md"),
    ("ui.md", "docs/ui.md"),
    ("roadmap.md", "docs/roadmap.md"),
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

/// 确保运行磁盘存在；缺失时格式化为空 ext4，并补齐系统文档。
pub(crate) fn ensure_ext4(path: &Path) -> Result<()> {
    if path.exists() {
        return ensure_docs(path);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| Failure::new(format!("创建 {} 失败：{error}", parent.display())))?;
    }
    let blocks = DISK_BYTES / SECTOR_SIZE as u64;
    let disk = MemoryDisk::new(blocks, SECTOR_SIZE);
    let disk = hk_fs_ext4::format_with_wall_clock(disk, HostClock)
        .map_err(|error| Failure::new(format!("格式化 ext4 运行磁盘失败：{error}")))?;
    let disk = seed_docs(disk)?;
    fs::write(path, disk.into_data())
        .map_err(|error| Failure::new(format!("写入 {} 失败：{error}", path.display())))?;
    Ok(())
}

/// 把系统文档写入 ext4 镜像的 `/docs`。
pub(crate) fn seed_docs(disk: MemoryDisk) -> Result<MemoryDisk> {
    let shared = Rc::new(RefCell::new(disk));
    {
        let mut backend = Ext4Backend::mount_with_wall_clock(SharedDisk(shared.clone()), HostClock)
            .map_err(|error| Failure::new(format!("挂载 ext4 以播种文档失败：{error}")))?;
        write_docs(&mut backend)?;
        backend
            .sync()
            .map_err(|error| Failure::new(format!("刷写文档失败：{error:?}")))?;
    }
    let shared = Rc::try_unwrap(shared)
        .map_err(|_| Failure::new("文档播种后仍持有镜像引用"))?
        .into_inner();
    Ok(shared)
}

/// 已有镜像缺少文档或文档过期时，重写 `/docs` 并保存。
fn ensure_docs(path: &Path) -> Result<()> {
    let bytes = fs::read(path)
        .map_err(|error| Failure::new(format!("读取 {} 失败：{error}", path.display())))?;
    let disk = MemoryDisk::from_data(bytes, SECTOR_SIZE)
        .ok_or_else(|| Failure::new("运行磁盘长度不是扇区整数倍"))?;
    let shared = Rc::new(RefCell::new(disk));
    let changed = {
        let mut backend = Ext4Backend::mount_with_wall_clock(SharedDisk(shared.clone()), HostClock)
            .map_err(|error| Failure::new(format!("挂载运行磁盘失败：{error}")))?;
        if docs_current(&backend)? {
            false
        } else {
            write_docs(&mut backend)?;
            backend
                .sync()
                .map_err(|error| Failure::new(format!("刷写文档失败：{error:?}")))?;
            true
        }
    };
    if !changed {
        return Ok(());
    }
    let disk = Rc::try_unwrap(shared)
        .map_err(|_| Failure::new("文档播种后仍持有镜像引用"))?
        .into_inner();
    fs::write(path, disk.into_data())
        .map_err(|error| Failure::new(format!("写入 {} 失败：{error}", path.display())))?;
    Ok(())
}

/// 检查磁盘上的系统文档是否与仓库一致。
fn docs_current<D: BlockDevice>(backend: &Ext4Backend<D>) -> Result<bool> {
    let root = backend.root();
    let Ok(docs) = backend.lookup(root, "docs") else {
        return Ok(false);
    };
    for (name, source) in SYSTEM_DOCS {
        let expected = read_source(source)?;
        let Ok(node) = backend.lookup(docs, name) else {
            return Ok(false);
        };
        let size = backend
            .metadata(node)
            .map_err(|error| Failure::new(format!("读取 {name} 元数据失败：{error:?}")))?
            .size;
        if size != expected.len() as u64 {
            return Ok(false);
        }
        let mut buffer = vec![0_u8; expected.len()];
        let read = backend
            .read(node, 0, &mut buffer)
            .map_err(|error| Failure::new(format!("读取 {name} 失败：{error:?}")))?;
        if buffer[..read] != expected[..] {
            return Ok(false);
        }
    }
    Ok(true)
}

/// 写入全部系统文档，覆盖已有内容。
fn write_docs<D: BlockDevice>(backend: &mut Ext4Backend<D>) -> Result<()> {
    let root = backend.root();
    let docs = match backend.lookup(root, "docs") {
        Ok(node) => node,
        Err(_) => backend
            .create(root, "docs", FileType::Directory)
            .map_err(|error| Failure::new(format!("创建 /docs 失败：{error:?}")))?,
    };
    for (name, source) in SYSTEM_DOCS {
        let bytes = read_source(source)?;
        let node = match backend.lookup(docs, name) {
            Ok(node) => node,
            Err(_) => backend
                .create(docs, name, FileType::File)
                .map_err(|error| Failure::new(format!("创建 /docs/{name} 失败：{error:?}")))?,
        };
        backend
            .truncate(node, 0)
            .map_err(|error| Failure::new(format!("清空 /docs/{name} 失败：{error:?}")))?;
        backend
            .write(node, 0, &bytes)
            .map_err(|error| Failure::new(format!("写入 /docs/{name} 失败：{error:?}")))?;
    }
    Ok(())
}

/// 读取仓库内的文档源文件。
fn read_source(source: &str) -> Result<Vec<u8>> {
    let path = workspace::root().join(source);
    fs::read(&path).map_err(|error| Failure::new(format!("读取 {} 失败：{error}", path.display())))
}

/// 与挂载中的后端共享同一个内存盘，便于写回镜像文件。
struct SharedDisk(Rc<RefCell<MemoryDisk>>);

impl BlockDevice for SharedDisk {
    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn capacity_blocks(&self) -> u64 {
        self.0.borrow().capacity_blocks()
    }

    fn read_blocks(
        &self,
        block: u64,
        buffer: &mut [u8],
    ) -> core::result::Result<usize, BlockError> {
        self.0.borrow().read_blocks(block, buffer)
    }

    fn write_blocks(&mut self, block: u64, data: &[u8]) -> core::result::Result<usize, BlockError> {
        self.0.borrow_mut().write_blocks(block, data)
    }

    fn flush(&mut self) -> core::result::Result<(), BlockError> {
        Ok(())
    }
}
