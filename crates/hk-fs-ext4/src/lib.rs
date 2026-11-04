//! ext4 文件系统后端：把 rsext4 接到 VFS 的 [`Backend`] 契约上。
//!
//! 本 crate 用 [`hk_block::BlockDevice`] 作为底层设备，用 rsext4 处理 ext4
//! 元数据、extent、日志与缓存；VFS 只看到节点、目录项与按偏移读写。

#![no_std]

#[cfg(test)]
extern crate std;

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use hk_block::BlockDevice;
use hk_vfs::{Backend, DirEntry, FileType, Metadata, NodeId, VfsError};
use rsext4::{
    BlockIo, Clock, DeviceCapabilities, DeviceGeometry, DirectoryCursor, DirectoryEntryType,
    EntropySource, Ext4, Ext4Error, Ext4ErrorKind, Ext4Result, Ext4Timestamp, FileName,
    FilePermissions, MkfsOptions, MountOptions, MountServices, MountedServices, MutationContext,
    NoopObserver, SectorId, WriteFlags,
};

/// rsext4 需要的挂载服务。
type Services = MountedServices<SimpleEntropy, NoopObserver, ()>;

/// 固定时钟：系统尚无 RTC 接口时以 Unix 纪元记录 inode 时间。
#[derive(Clone, Copy, Debug, Default)]
pub struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> Ext4Result<Ext4Timestamp> {
        Ok(Ext4Timestamp::UNIX_EPOCH)
    }
}

/// 确定性熵源：ext4 只在启用 MMP 时需要。
#[derive(Clone, Copy, Debug)]
pub struct SimpleEntropy(u64);

impl Default for SimpleEntropy {
    fn default() -> Self {
        Self(0x9E37_79B9_7F4A_7C15)
    }
}

impl EntropySource for SimpleEntropy {
    fn fill_bytes(&mut self, output: &mut [u8]) -> Ext4Result<()> {
        for chunk in output.chunks_mut(8) {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            let bytes = self.0.to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
        Ok(())
    }
}

/// 把 [`BlockDevice`] 适配为 rsext4 的扇区 I/O。
pub struct Disk<D> {
    device: RefCell<D>,
}

impl<D: BlockDevice> Disk<D> {
    /// 包装块设备。
    pub fn new(device: D) -> Self {
        Self {
            device: RefCell::new(device),
        }
    }

    /// 取回块设备。
    pub fn into_inner(self) -> D {
        self.device.into_inner()
    }
}

impl<D: BlockDevice> BlockIo for Disk<D> {
    fn write(&mut self, buffer: &[u8], sector: SectorId, count: u32) -> Ext4Result<()> {
        let device = self.device.get_mut();
        let expected = (count as usize)
            .checked_mul(device.block_size())
            .ok_or_else(Ext4Error::overflow)?;
        if buffer.len() != expected {
            return Err(Ext4Error::invalid_block_size(buffer.len(), expected));
        }
        device
            .write_blocks(sector.raw(), buffer)
            .map(|_| ())
            .map_err(|_| Ext4Error::io())
    }

    fn read(&mut self, buffer: &mut [u8], sector: SectorId, count: u32) -> Ext4Result<()> {
        let device = self.device.get_mut();
        let expected = (count as usize)
            .checked_mul(device.block_size())
            .ok_or_else(Ext4Error::overflow)?;
        if buffer.len() != expected {
            return Err(Ext4Error::invalid_block_size(buffer.len(), expected));
        }
        device
            .read_blocks(sector.raw(), buffer)
            .map(|_| ())
            .map_err(|_| Ext4Error::io())
    }

    fn write_with_flags(
        &mut self,
        buffer: &[u8],
        sector: SectorId,
        count: u32,
        flags: WriteFlags,
    ) -> Ext4Result<()> {
        self.write(buffer, sector, count)?;
        if flags.contains(WriteFlags::FUA) {
            self.flush()?;
        }
        Ok(())
    }

    fn geometry(&self) -> DeviceGeometry {
        let device = self.device.borrow();
        DeviceGeometry::new(device.block_size() as u32, device.capacity_blocks())
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            flush: true,
            ..DeviceCapabilities::default()
        }
    }

    fn flush(&mut self) -> Ext4Result<()> {
        self.device.get_mut().flush().map_err(|_| Ext4Error::io())
    }
}

/// 格式化块设备为 ext4。
pub fn format<D: BlockDevice>(device: D) -> Result<D, Ext4Error> {
    rsext4::format(Disk::new(device), FixedClock, MkfsOptions::default()).map(Disk::into_inner)
}

/// ext4 VFS 后端。
pub struct Ext4Backend<D: BlockDevice> {
    filesystem: RefCell<Ext4<Disk<D>, Services>>,
}

impl<D: BlockDevice> Ext4Backend<D> {
    /// 挂载 ext4。
    pub fn mount(device: D) -> Result<Self, Ext4Error> {
        let filesystem = Ext4::mount(
            Disk::new(device),
            MountServices::new(FixedClock, SimpleEntropy::default(), NoopObserver),
            MountOptions::read_write(),
        )?;
        Ok(Self {
            filesystem: RefCell::new(filesystem),
        })
    }
}

fn map_node(node: NodeId) -> Result<rsext4::InodeNumber, VfsError> {
    rsext4::InodeNumber::from_u64(node).map_err(|_| VfsError::NotFound)
}

fn map_name(name: &str) -> Result<FileName<'_>, VfsError> {
    FileName::new(name.as_bytes()).map_err(|_| VfsError::InvalidPath)
}

fn map_error(error: Ext4Error) -> VfsError {
    match error.kind() {
        Ext4ErrorKind::NotFound => VfsError::NotFound,
        Ext4ErrorKind::NotDirectory => VfsError::NotDirectory,
        Ext4ErrorKind::IsDirectory => VfsError::IsDirectory,
        Ext4ErrorKind::AlreadyExists => VfsError::AlreadyExists,
        Ext4ErrorKind::ReadOnly => VfsError::ReadOnly,
        Ext4ErrorKind::PermissionDenied => VfsError::PermissionDenied,
        Ext4ErrorKind::InvalidInput => VfsError::InvalidPath,
        Ext4ErrorKind::NoSpace | Ext4ErrorKind::NoMemory => VfsError::Backend,
        _ => VfsError::Backend,
    }
}

fn map_entry_type(kind: DirectoryEntryType) -> FileType {
    match kind {
        DirectoryEntryType::Directory => FileType::Directory,
        _ => FileType::File,
    }
}

impl<D: BlockDevice> Backend for Ext4Backend<D> {
    fn root(&self) -> NodeId {
        self.filesystem.borrow().root_inode().raw() as u64
    }

    fn lookup(&self, parent: NodeId, name: &str) -> Result<NodeId, VfsError> {
        let parent = map_node(parent)?;
        let name = map_name(name)?;
        match self.filesystem.borrow_mut().lookup_child(parent, name) {
            Ok(Some(info)) => Ok(info.number.raw() as u64),
            Ok(None) => Err(VfsError::NotFound),
            Err(error) => Err(map_error(error)),
        }
    }

    fn metadata(&self, node: NodeId) -> Result<Metadata, VfsError> {
        let node = map_node(node)?;
        let info = self
            .filesystem
            .borrow_mut()
            .inode(node)
            .map_err(map_error)?;
        Ok(Metadata {
            kind: map_entry_type(info.file_type()),
            size: info.size,
        })
    }

    fn read(&self, node: NodeId, offset: u64, buffer: &mut [u8]) -> Result<usize, VfsError> {
        let node = map_node(node)?;
        self.filesystem
            .borrow_mut()
            .read_inode(node, offset, buffer)
            .map_err(map_error)
    }

    fn write(&mut self, node: NodeId, offset: u64, data: &[u8]) -> Result<usize, VfsError> {
        let node = map_node(node)?;
        self.filesystem
            .borrow_mut()
            .write_inode(node, offset, data)
            .map_err(map_error)?;
        Ok(data.len())
    }

    fn create(&mut self, parent: NodeId, name: &str, kind: FileType) -> Result<NodeId, VfsError> {
        let parent = map_node(parent)?;
        let name = map_name(name)?;
        let context = MutationContext::new(0, 0, 0, 0o022);
        let permissions = FilePermissions::new(0o644).map_err(|_| VfsError::PermissionDenied)?;
        let info = match kind {
            FileType::File => {
                self.filesystem
                    .borrow_mut()
                    .create_regular_file(context, parent, name, permissions)
            }
            FileType::Directory => {
                let permissions =
                    FilePermissions::new(0o755).map_err(|_| VfsError::PermissionDenied)?;
                self.filesystem
                    .borrow_mut()
                    .create_directory(context, parent, name, permissions)
            }
        }
        .map_err(map_error)?;
        Ok(info.number.raw() as u64)
    }

    fn readdir(&self, node: NodeId) -> Result<Vec<DirEntry>, VfsError> {
        let node = map_node(node)?;
        let mut filesystem = self.filesystem.borrow_mut();
        let mut cursor = DirectoryCursor::Start;
        let mut entries = Vec::new();
        loop {
            let batch = filesystem
                .read_directory(node, cursor, 64)
                .map_err(map_error)?;
            if batch.is_empty() {
                break;
            }
            for entry in &batch {
                if let Ok(name) = core::str::from_utf8(&entry.name) {
                    if name == "." || name == ".." {
                        continue;
                    }
                    entries.push(DirEntry {
                        name: String::from(name),
                        kind: map_entry_type(entry.file_type),
                    });
                }
            }
            let next = batch
                .last()
                .map(|entry| entry.next_cursor)
                .unwrap_or(DirectoryCursor::End);
            if next == cursor || next == DirectoryCursor::End {
                break;
            }
            cursor = next;
        }
        Ok(entries)
    }

    fn truncate(&mut self, node: NodeId, size: u64) -> Result<(), VfsError> {
        let node = map_node(node)?;
        self.filesystem
            .borrow_mut()
            .truncate_inode(node, size)
            .map_err(map_error)
    }

    fn unlink(&mut self, parent: NodeId, name: &str) -> Result<NodeId, VfsError> {
        let parent = map_node(parent)?;
        let name = map_name(name)?;
        let mut filesystem = self.filesystem.borrow_mut();
        let info = filesystem
            .lookup_child(parent, name)
            .map_err(map_error)?
            .ok_or(VfsError::NotFound)?;
        let outcome = if info.is_directory() {
            filesystem
                .remove_empty_directory(parent, name)
                .map_err(map_error)?
        } else {
            filesystem.unlink(parent, name).map_err(map_error)?
        };
        if outcome.requires_reap() {
            filesystem
                .reap_unlinked_inode(outcome.inode)
                .map_err(map_error)?;
        }
        Ok(outcome.inode.raw() as u64)
    }

    fn sync(&mut self) -> Result<(), VfsError> {
        self.filesystem.borrow_mut().sync().map_err(map_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::rc::Rc;
    use hk_block::{MemoryDisk, SECTOR_SIZE};

    const IMAGE_BLOCKS: u64 = (64 * 1024 * 1024) / SECTOR_SIZE as u64;

    fn formatted() -> MemoryDisk {
        format(MemoryDisk::new(IMAGE_BLOCKS, SECTOR_SIZE)).expect("格式化")
    }

    #[test]
    fn creates_reads_and_lists_files() {
        let mut backend = Ext4Backend::mount(formatted()).expect("挂载");
        let root = backend.root();
        let node = backend
            .create(root, "hello", FileType::File)
            .expect("创建文件");
        assert_eq!(backend.write(node, 0, b"hello ext4").expect("写入"), 10);

        let mut buffer = [0_u8; 16];
        let read = backend.read(node, 0, &mut buffer).expect("读取");
        assert_eq!(&buffer[..read], b"hello ext4");
        assert_eq!(backend.metadata(node).expect("元数据").size, 10);

        let entries = backend.readdir(root).expect("目录");
        assert!(entries.iter().any(|entry| entry.name == "hello"));
        assert_eq!(backend.lookup(root, "hello").expect("查找"), node);
        assert_eq!(backend.lookup(root, "missing"), Err(VfsError::NotFound));
    }

    #[test]
    fn truncates_and_unlinks() {
        let mut backend = Ext4Backend::mount(formatted()).expect("挂载");
        let root = backend.root();
        let node = backend
            .create(root, "file", FileType::File)
            .expect("创建文件");
        backend.write(node, 0, b"0123456789").expect("写入");
        backend.truncate(node, 4).expect("截断");
        assert_eq!(backend.metadata(node).expect("元数据").size, 4);
        assert_eq!(backend.unlink(root, "file").expect("删除"), node);
        assert_eq!(backend.lookup(root, "file"), Err(VfsError::NotFound));
    }

    struct Shared(Rc<RefCell<MemoryDisk>>);

    impl BlockDevice for Shared {
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
        ) -> Result<usize, hk_block::BlockError> {
            self.0.borrow().read_blocks(block, buffer)
        }

        fn write_blocks(&mut self, block: u64, data: &[u8]) -> Result<usize, hk_block::BlockError> {
            self.0.borrow_mut().write_blocks(block, data)
        }

        fn flush(&mut self) -> Result<(), hk_block::BlockError> {
            Ok(())
        }
    }

    #[test]
    fn persists_across_remount() {
        let disk = Rc::new(RefCell::new(formatted()));
        {
            let mut backend = Ext4Backend::mount(Shared(disk.clone())).expect("首次挂载");
            let root = backend.root();
            let node = backend
                .create(root, "marker", FileType::File)
                .expect("创建");
            backend.write(node, 0, b"HolyKernel ext4").expect("写入");
            backend.sync().expect("刷写");
        }

        let backend = Ext4Backend::mount(Shared(disk)).expect("重新挂载");
        let root = backend.root();
        let node = backend.lookup(root, "marker").expect("查找");
        let mut buffer = [0_u8; 32];
        let read = backend.read(node, 0, &mut buffer).expect("读取");
        assert_eq!(&buffer[..read], b"HolyKernel ext4");
    }
}
