//! VFS 适配层：把 HolyFS 文档与集合映射到路径与文件句柄。

use alloc::vec::Vec;
use core::cell::RefCell;

use hk_block::BlockDevice;
use hk_vfs::{Backend, DirEntry, FileType, Metadata, NodeId, VfsError};

use crate::error::{HolyError, map_vfs};
use crate::volume::Volume;

/// HolyFS 后端。
pub struct HolyFs<D: BlockDevice> {
    state: RefCell<Volume<D>>,
}

impl<D: BlockDevice> HolyFs<D> {
    /// 挂载 HolyFS，选择 generation 最新的有效超级块。
    pub fn mount(device: D) -> Result<Self, HolyError> {
        Ok(Self {
            state: RefCell::new(Volume::mount(device)?),
        })
    }
}

impl<D: BlockDevice> Backend for HolyFs<D> {
    fn root(&self) -> NodeId {
        self.state.borrow().root_node()
    }

    fn lookup(&self, parent: NodeId, name: &str) -> Result<NodeId, VfsError> {
        self.state
            .borrow_mut()
            .lookup(parent, name)
            .map_err(map_vfs)
    }

    fn metadata(&self, node: NodeId) -> Result<Metadata, VfsError> {
        self.state.borrow().metadata(node).map_err(map_vfs)
    }

    fn read(&self, node: NodeId, offset: u64, buffer: &mut [u8]) -> Result<usize, VfsError> {
        self.state
            .borrow()
            .read(node, offset, buffer)
            .map_err(map_vfs)
    }

    fn write(&mut self, node: NodeId, offset: u64, data: &[u8]) -> Result<usize, VfsError> {
        self.state
            .borrow_mut()
            .write(node, offset, data)
            .map_err(map_vfs)
    }

    fn create(&mut self, parent: NodeId, name: &str, kind: FileType) -> Result<NodeId, VfsError> {
        self.state
            .borrow_mut()
            .create(parent, name, kind)
            .map_err(map_vfs)
    }

    fn readdir(&self, node: NodeId) -> Result<Vec<DirEntry>, VfsError> {
        self.state.borrow().readdir(node).map_err(map_vfs)
    }

    fn truncate(&mut self, node: NodeId, size: u64) -> Result<(), VfsError> {
        self.state
            .borrow_mut()
            .truncate(node, size)
            .map_err(map_vfs)
    }

    fn unlink(&mut self, parent: NodeId, name: &str) -> Result<NodeId, VfsError> {
        self.state
            .borrow_mut()
            .unlink(parent, name)
            .map_err(map_vfs)
    }

    fn sync(&mut self) -> Result<(), VfsError> {
        self.state.borrow_mut().sync().map_err(map_vfs)
    }
}
