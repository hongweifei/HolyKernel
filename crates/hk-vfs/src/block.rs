//! 块设备后端：把原始设备暴露为根目录下的 `disk` 文件。
//!
//! 真正的文件系统后端后续复用它作为底层 BlockDevice；当前用于打通
//! BlockDevice → VFS 的数据路径。

use alloc::vec;
use alloc::vec::Vec;

use hk_block::{BlockDevice, BlockError};

use crate::vfs::NodeId;
use crate::{Backend, DirEntry, FileType, Metadata, VfsError};

const ROOT: NodeId = 0;
const DISK: NodeId = 1;

/// 原始块设备 VFS 后端。
pub struct BlockBackend<D> {
    device: D,
}

impl<D: BlockDevice> BlockBackend<D> {
    /// 以块设备构造后端。
    pub const fn new(device: D) -> Self {
        Self { device }
    }

    fn translate(error: BlockError) -> VfsError {
        match error {
            BlockError::OutOfRange => VfsError::InvalidOffset,
            BlockError::InvalidLength | BlockError::Io => VfsError::Backend,
            BlockError::ReadOnly => VfsError::ReadOnly,
        }
    }
}

impl<D: BlockDevice> Backend for BlockBackend<D> {
    fn root(&self) -> NodeId {
        ROOT
    }

    fn lookup(&self, parent: NodeId, name: &str) -> Result<NodeId, VfsError> {
        if parent != ROOT {
            return Err(VfsError::NotDirectory);
        }
        if name == "disk" {
            Ok(DISK)
        } else {
            Err(VfsError::NotFound)
        }
    }

    fn metadata(&self, node: NodeId) -> Result<Metadata, VfsError> {
        match node {
            ROOT => Ok(Metadata {
                kind: FileType::Directory,
                size: 0,
            }),
            DISK => Ok(Metadata {
                kind: FileType::File,
                size: self.device.capacity_bytes(),
            }),
            _ => Err(VfsError::NotFound),
        }
    }

    fn read(&self, node: NodeId, offset: u64, buffer: &mut [u8]) -> Result<usize, VfsError> {
        if node != DISK {
            return Err(VfsError::IsDirectory);
        }
        let size = self.device.capacity_bytes();
        if offset >= size || buffer.is_empty() {
            return Ok(0);
        }
        let end = (offset as usize)
            .saturating_add(buffer.len())
            .min(size as usize);
        let length = end - offset as usize;
        let block_size = self.device.block_size();
        let mut block = vec![0_u8; block_size];
        let mut copied = 0;
        while copied < length {
            let position = offset as usize + copied;
            let sector = (position / block_size) as u64;
            let within = position % block_size;
            let take = (block_size - within).min(length - copied);
            self.device
                .read_blocks(sector, &mut block)
                .map_err(Self::translate)?;
            buffer[copied..copied + take].copy_from_slice(&block[within..within + take]);
            copied += take;
        }
        Ok(copied)
    }

    fn write(&mut self, node: NodeId, offset: u64, data: &[u8]) -> Result<usize, VfsError> {
        if node != DISK {
            return Err(VfsError::IsDirectory);
        }
        let size = self.device.capacity_bytes();
        let end = (offset as usize)
            .checked_add(data.len())
            .ok_or(VfsError::InvalidOffset)?;
        if end > size as usize {
            return Err(VfsError::InvalidOffset);
        }
        let block_size = self.device.block_size();
        let mut block = vec![0_u8; block_size];
        let mut written = 0;
        while written < data.len() {
            let position = offset as usize + written;
            let sector = (position / block_size) as u64;
            let within = position % block_size;
            let take = (block_size - within).min(data.len() - written);
            self.device
                .read_blocks(sector, &mut block)
                .map_err(Self::translate)?;
            block[within..within + take].copy_from_slice(&data[written..written + take]);
            self.device
                .write_blocks(sector, &block)
                .map_err(Self::translate)?;
            written += take;
        }
        Ok(written)
    }

    fn create(
        &mut self,
        _parent: NodeId,
        _name: &str,
        _kind: FileType,
    ) -> Result<NodeId, VfsError> {
        Err(VfsError::ReadOnly)
    }

    fn readdir(&self, node: NodeId) -> Result<Vec<DirEntry>, VfsError> {
        if node != ROOT {
            return Err(VfsError::NotDirectory);
        }
        Ok(vec![DirEntry {
            name: alloc::string::String::from("disk"),
            kind: FileType::File,
        }])
    }

    fn truncate(&mut self, _node: NodeId, _size: u64) -> Result<(), VfsError> {
        Err(VfsError::ReadOnly)
    }

    fn unlink(&mut self, _parent: NodeId, _name: &str) -> Result<NodeId, VfsError> {
        Err(VfsError::ReadOnly)
    }

    fn cacheable(&self, _node: NodeId) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hk_block::{MemoryDisk, SECTOR_SIZE};

    #[test]
    fn mounts_as_vfs_backend() {
        use crate::{OpenFlags, Tmpfs, Vfs};
        use alloc::boxed::Box;

        let disk = MemoryDisk::new(4, SECTOR_SIZE);
        let mut vfs = Vfs::new(Box::new(Tmpfs::new()));
        vfs.create_dir("/disk").expect("目录");
        vfs.mount("/disk", Box::new(BlockBackend::new(disk)))
            .expect("挂载");
        let handle = vfs.open("/disk/disk", OpenFlags::READ_WRITE).expect("打开");
        vfs.write_at(handle, 0, &[0x33; 16]).expect("写");
        let mut bytes = [0_u8; 16];
        vfs.read_at(handle, 0, &mut bytes).expect("读");
        assert_eq!(bytes, [0x33; 16]);
    }

    #[test]
    fn exposes_raw_disk_file() {
        let mut disk = MemoryDisk::new(4, SECTOR_SIZE);
        disk.write_blocks(0, &[0x5A; SECTOR_SIZE]).unwrap();
        let mut backend = BlockBackend::new(disk);
        assert_eq!(backend.lookup(ROOT, "disk"), Ok(DISK));
        assert_eq!(backend.metadata(DISK).unwrap().size, 4 * SECTOR_SIZE as u64);
        let mut buffer = [0_u8; 8];
        assert_eq!(backend.read(DISK, 0, &mut buffer), Ok(8));
        assert_eq!(buffer, [0x5A; 8]);
        assert_eq!(backend.write(DISK, 4, &[0x11; 8]), Ok(8));
        assert_eq!(backend.read(DISK, 4, &mut buffer), Ok(8));
        assert_eq!(buffer, [0x11; 8]);
    }
}
