//! 内存文件系统后端。

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::vfs::{DirEntry, FileType, Metadata, NodeId, PathBackend, VfsError};

/// 内存文件系统。
pub struct Tmpfs {
    nodes: Vec<Node>,
}

struct Node {
    kind: FileType,
    children: BTreeMap<String, NodeId>,
    data: Vec<u8>,
}

impl Tmpfs {
    /// 创建带根目录的空文件系统。
    pub fn new() -> Self {
        Self {
            nodes: alloc::vec![Node {
                kind: FileType::Directory,
                children: BTreeMap::new(),
                data: Vec::new(),
            }],
        }
    }

    fn node(&self, node: NodeId) -> Result<&Node, VfsError> {
        self.nodes.get(node as usize).ok_or(VfsError::NotFound)
    }

    fn node_mut(&mut self, node: NodeId) -> Result<&mut Node, VfsError> {
        self.nodes.get_mut(node as usize).ok_or(VfsError::NotFound)
    }

    fn check_name(name: &str) -> Result<(), VfsError> {
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            return Err(VfsError::InvalidPath);
        }
        Ok(())
    }
}

impl Default for Tmpfs {
    fn default() -> Self {
        Self::new()
    }
}

impl PathBackend for Tmpfs {
    fn root(&self) -> NodeId {
        0
    }

    fn lookup(&self, parent: NodeId, name: &str) -> Result<NodeId, VfsError> {
        Self::check_name(name)?;
        let node = self.node(parent)?;
        if node.kind != FileType::Directory {
            return Err(VfsError::NotDirectory);
        }
        node.children.get(name).copied().ok_or(VfsError::NotFound)
    }

    fn metadata(&self, node: NodeId) -> Result<Metadata, VfsError> {
        let node = self.node(node)?;
        Ok(Metadata {
            kind: node.kind,
            size: node.data.len() as u64,
        })
    }

    fn read(&self, node: NodeId, offset: u64, buffer: &mut [u8]) -> Result<usize, VfsError> {
        let node = self.node(node)?;
        if node.kind != FileType::File {
            return Err(VfsError::IsDirectory);
        }
        let offset = usize::try_from(offset).map_err(|_| VfsError::InvalidOffset)?;
        if offset >= node.data.len() {
            return Ok(0);
        }
        let length = buffer.len().min(node.data.len() - offset);
        buffer[..length].copy_from_slice(&node.data[offset..offset + length]);
        Ok(length)
    }

    fn write(&mut self, node: NodeId, offset: u64, data: &[u8]) -> Result<usize, VfsError> {
        let node = self.node_mut(node)?;
        if node.kind != FileType::File {
            return Err(VfsError::IsDirectory);
        }
        let offset = usize::try_from(offset).map_err(|_| VfsError::InvalidOffset)?;
        let end = offset
            .checked_add(data.len())
            .ok_or(VfsError::InvalidOffset)?;
        if node.data.len() < end {
            node.data.resize(end, 0);
        }
        node.data[offset..end].copy_from_slice(data);
        Ok(data.len())
    }

    fn create(&mut self, parent: NodeId, name: &str, kind: FileType) -> Result<NodeId, VfsError> {
        Self::check_name(name)?;
        let parent_node = self.node(parent)?;
        if parent_node.kind != FileType::Directory {
            return Err(VfsError::NotDirectory);
        }
        if parent_node.children.contains_key(name) {
            return Err(VfsError::AlreadyExists);
        }

        let id = self.nodes.len() as NodeId;
        self.nodes.push(Node {
            kind,
            children: BTreeMap::new(),
            data: Vec::new(),
        });
        self.node_mut(parent)?.children.insert(name.to_string(), id);
        Ok(id)
    }

    fn truncate(&mut self, node: NodeId, size: u64) -> Result<(), VfsError> {
        let node = self.node_mut(node)?;
        if node.kind != FileType::File {
            return Err(VfsError::IsDirectory);
        }
        let size = usize::try_from(size).map_err(|_| VfsError::InvalidOffset)?;
        node.data.resize(size, 0);
        Ok(())
    }

    fn unlink(&mut self, parent: NodeId, name: &str) -> Result<NodeId, VfsError> {
        Self::check_name(name)?;
        let node = {
            let parent_node = self.node_mut(parent)?;
            if parent_node.kind != FileType::Directory {
                return Err(VfsError::NotDirectory);
            }
            parent_node
                .children
                .remove(name)
                .ok_or(VfsError::NotFound)?
        };
        let non_empty_directory = self
            .nodes
            .get(node as usize)
            .is_some_and(|child| child.kind == FileType::Directory && !child.children.is_empty());
        if non_empty_directory {
            self.node_mut(parent)?
                .children
                .insert(name.to_string(), node);
            return Err(VfsError::IsDirectory);
        }
        Ok(node)
    }

    fn readdir(&self, node: NodeId) -> Result<Vec<DirEntry>, VfsError> {
        let node = self.node(node)?;
        if node.kind != FileType::Directory {
            return Err(VfsError::NotDirectory);
        }
        Ok(node
            .children
            .iter()
            .filter_map(|(name, id)| {
                self.nodes.get(*id as usize).map(|child| DirEntry {
                    name: name.clone(),
                    kind: child.kind,
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_reads_and_writes_files() {
        let mut fs = Tmpfs::new();
        let file = fs.create(0, "hello", FileType::File).expect("创建文件");
        assert_eq!(fs.write(file, 0, b"hello").expect("写入"), 5);
        assert_eq!(fs.write(file, 5, b" world").expect("追加"), 6);

        let mut buffer = [0_u8; 16];
        let read = fs.read(file, 0, &mut buffer).expect("读取");
        assert_eq!(&buffer[..read], b"hello world");
        assert_eq!(fs.metadata(file).expect("元数据").size, 11);
    }

    #[test]
    fn maintains_directories_and_errors() {
        let mut fs = Tmpfs::new();
        let dir = fs.create(0, "tmp", FileType::Directory).expect("创建目录");
        assert_eq!(fs.lookup(0, "tmp").expect("查找"), dir);
        assert_eq!(
            fs.create(0, "tmp", FileType::File),
            Err(VfsError::AlreadyExists)
        );
        assert_eq!(fs.lookup(dir, "missing"), Err(VfsError::NotFound));
        assert_eq!(fs.readdir(dir).expect("读取目录").len(), 0);
        assert_eq!(fs.write(dir, 0, b"x"), Err(VfsError::IsDirectory));
    }
}
