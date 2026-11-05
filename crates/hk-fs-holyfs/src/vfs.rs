//! 卷句柄：原生文档操作与 VFS 适配层共享同一份 Volume。

use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use hk_block::BlockDevice;
use hk_vfs::{Backend, DirEntry, FileType, Metadata, NodeId, VfsError};

use crate::error::{HolyError, map_vfs};
use crate::ids::{DocId, RevisionId};
use crate::types::{DocumentInfo, EntryInfo, RevisionInfo};
use crate::volume::Volume;

/// HolyFS 卷句柄。
///
/// 通过 `Clone` 派生的句柄共享同一个 Volume：VFS 适配层与原生文档操作看到
/// 同一份文档与修订状态。
pub struct HolyFs<D: BlockDevice> {
    state: Rc<RefCell<Volume<D>>>,
}

impl<D: BlockDevice> Clone for HolyFs<D> {
    fn clone(&self) -> Self {
        Self {
            state: Rc::clone(&self.state),
        }
    }
}

impl<D: BlockDevice> HolyFs<D> {
    /// 挂载 HolyFS，选择 generation 最新的有效超级块。
    pub fn mount(device: D) -> Result<Self, HolyError> {
        Ok(Self {
            state: Rc::new(RefCell::new(Volume::mount(device)?)),
        })
    }

    /// 按 DocPath 解析文档身份。
    pub fn resolve(&self, path: &str) -> Result<DocId, HolyError> {
        self.state.borrow().resolve(path)
    }

    /// 枚举集合文档。
    pub fn enumerate(&self, path: &str) -> Result<Vec<EntryInfo>, HolyError> {
        let document = self.resolve(path)?;
        self.state.borrow().enumerate(document)
    }

    /// 读取文档身份、类型与当前修订。
    pub fn document(&self, path: &str) -> Result<DocumentInfo, HolyError> {
        let document = self.resolve(path)?;
        self.state.borrow().document_info(document)
    }

    /// 整文档读取。
    pub fn read_document(&self, path: &str) -> Result<Vec<u8>, HolyError> {
        let document = self.resolve(path)?;
        self.state.borrow().read_document(document)
    }

    /// 枚举文档修订链。
    pub fn revisions(&self, path: &str) -> Result<Vec<RevisionInfo>, HolyError> {
        let document = self.resolve(path)?;
        self.state.borrow().revisions(document)
    }

    /// 整文档写入，返回新的 head 修订。
    pub fn write_document(&mut self, path: &str, content: &[u8]) -> Result<RevisionId, HolyError> {
        let document = self.resolve(path)?;
        self.state.borrow_mut().write_document(document, content)
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
