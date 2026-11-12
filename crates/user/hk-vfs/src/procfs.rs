//! 进程文件系统：只读的静态系统信息文件。

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::vfs::{DirEntry, FileType, Metadata, NodeId, PathBackend, VfsError};

/// 进程文件系统。
pub struct Procfs {
    files: BTreeMap<String, Vec<u8>>,
}

impl Procfs {
    /// 构造默认的只读系统信息视图。
    pub fn new() -> Self {
        let mut files = BTreeMap::new();
        files.insert(
            "version".to_string(),
            concat!("HolyKernel ", env!("CARGO_PKG_VERSION"), "\n")
                .as_bytes()
                .to_vec(),
        );
        files.insert("uptime".to_string(), b"0.00 0.00\n".to_vec());
        files.insert("mounts".to_string(), b"/\n/tmp\n/dev\n/proc\n".to_vec());
        Self { files }
    }

    fn node(name: &str) -> Option<NodeId> {
        let names = ["mounts", "uptime", "version"];
        names
            .iter()
            .position(|candidate| *candidate == name)
            .map(|index| index as NodeId + 1)
    }

    fn name(node: NodeId) -> Option<&'static str> {
        match node {
            1 => Some("mounts"),
            2 => Some("uptime"),
            3 => Some("version"),
            _ => None,
        }
    }
}

impl Default for Procfs {
    fn default() -> Self {
        Self::new()
    }
}

impl PathBackend for Procfs {
    fn root(&self) -> NodeId {
        0
    }

    fn lookup(&self, parent: NodeId, name: &str) -> Result<NodeId, VfsError> {
        if parent != 0 {
            return Err(VfsError::NotDirectory);
        }
        Self::node(name).ok_or(VfsError::NotFound)
    }

    fn metadata(&self, node: NodeId) -> Result<Metadata, VfsError> {
        if node == 0 {
            return Ok(Metadata {
                kind: FileType::Directory,
                size: 0,
            });
        }
        let name = Self::name(node).ok_or(VfsError::NotFound)?;
        let data = self.files.get(name).ok_or(VfsError::NotFound)?;
        Ok(Metadata {
            kind: FileType::File,
            size: data.len() as u64,
        })
    }

    fn read(&self, node: NodeId, offset: u64, buffer: &mut [u8]) -> Result<usize, VfsError> {
        let name = Self::name(node).ok_or(VfsError::NotFound)?;
        let data = self.files.get(name).ok_or(VfsError::NotFound)?;
        let offset = usize::try_from(offset).map_err(|_| VfsError::InvalidOffset)?;
        if offset >= data.len() {
            return Ok(0);
        }
        let length = buffer.len().min(data.len() - offset);
        buffer[..length].copy_from_slice(&data[offset..offset + length]);
        Ok(length)
    }

    fn write(&mut self, _node: NodeId, _offset: u64, _data: &[u8]) -> Result<usize, VfsError> {
        Err(VfsError::ReadOnly)
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
        if node != 0 {
            return Err(VfsError::NotDirectory);
        }
        Ok(self
            .files
            .keys()
            .map(|name| DirEntry {
                name: name.clone(),
                kind: FileType::File,
            })
            .collect())
    }

    fn truncate(&mut self, _node: NodeId, _size: u64) -> Result<(), VfsError> {
        Err(VfsError::ReadOnly)
    }

    fn unlink(&mut self, _parent: NodeId, _name: &str) -> Result<NodeId, VfsError> {
        Err(VfsError::ReadOnly)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_static_system_files() {
        let fs = Procfs::new();
        let version = fs.lookup(0, "version").expect("version");
        assert_eq!(fs.metadata(version).expect("元数据").kind, FileType::File);
        let mut buffer = [0_u8; 32];
        let expected = concat!("HolyKernel ", env!("CARGO_PKG_VERSION"), "\n");
        assert_eq!(
            fs.read(version, 0, &mut buffer).expect("读取"),
            expected.len()
        );
        assert!(buffer.starts_with(b"HolyKernel"));
        assert_eq!(fs.readdir(0).expect("目录").len(), 3);
        assert_eq!(fs.lookup(0, "missing"), Err(VfsError::NotFound));
    }
}
