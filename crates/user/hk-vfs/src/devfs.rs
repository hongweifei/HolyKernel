//! 设备文件系统：最小的只读/丢弃写设备集合。

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::vfs::{DirEntry, FileType, Metadata, NodeId, PathBackend, VfsError};

const ROOT: NodeId = 0;
const NULL: NodeId = 1;
const CONSOLE: NodeId = 2;
const ZERO: NodeId = 3;

/// 设备文件系统：`/dev/null`、`/dev/console`。
pub struct Devfs {
    names: BTreeMap<String, NodeId>,
}

impl Devfs {
    /// 构造设备文件系统。
    pub fn new() -> Self {
        let mut names = BTreeMap::new();
        names.insert("console".to_string(), CONSOLE);
        names.insert("null".to_string(), NULL);
        names.insert("zero".to_string(), ZERO);
        Self { names }
    }
}

impl Default for Devfs {
    fn default() -> Self {
        Self::new()
    }
}

impl PathBackend for Devfs {
    fn root(&self) -> NodeId {
        ROOT
    }

    fn lookup(&self, parent: NodeId, name: &str) -> Result<NodeId, VfsError> {
        if parent != ROOT {
            return Err(VfsError::NotDirectory);
        }
        self.names.get(name).copied().ok_or(VfsError::NotFound)
    }

    fn metadata(&self, node: NodeId) -> Result<Metadata, VfsError> {
        match node {
            ROOT => Ok(Metadata {
                kind: FileType::Directory,
                size: 0,
            }),
            NULL | CONSOLE | ZERO => Ok(Metadata {
                kind: FileType::File,
                size: 0,
            }),
            _ => Err(VfsError::NotFound),
        }
    }

    fn read(&self, node: NodeId, _offset: u64, buffer: &mut [u8]) -> Result<usize, VfsError> {
        match node {
            ZERO => {
                buffer.fill(0);
                Ok(buffer.len())
            }
            NULL | CONSOLE => Ok(0),
            _ => Err(VfsError::NotFound),
        }
    }

    fn write(&mut self, _node: NodeId, _offset: u64, data: &[u8]) -> Result<usize, VfsError> {
        Ok(data.len())
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
        Ok(self
            .names
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

    fn cacheable(&self, _node: NodeId) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_devices() {
        let mut fs = Devfs::new();
        let null = fs.lookup(0, "null").expect("null");
        assert_eq!(fs.metadata(null).expect("元数据").kind, FileType::File);
        assert_eq!(fs.write(null, 0, b"discard"), Ok(7));
        assert_eq!(fs.readdir(0).expect("目录").len(), 3);
        let zero = fs.lookup(0, "zero").expect("zero");
        let mut buffer = [1_u8; 4];
        assert_eq!(fs.read(zero, 0, &mut buffer), Ok(4));
        assert_eq!(buffer, [0; 4]);
        assert_eq!(fs.create(0, "x", FileType::File), Err(VfsError::ReadOnly));
    }
}
