//! 卷句柄：原生文档操作。
//!
//! 通过 `Clone` 派生的句柄共享同一个 Volume；句柄只暴露文档与卷操作。

use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use hk_block::BlockDevice;

use crate::error::HolyError;
use crate::ids::{DocId, RevisionId, VolumeId};
use crate::types::{DocumentInfo, DocumentKind, EntryInfo, RevisionInfo, SnapshotInfo};
use crate::volume::Volume;

/// HolyFS 卷句柄。
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

    /// 返回根集合文档。
    pub fn root(&self) -> DocId {
        self.state.borrow().root_document()
    }

    /// 返回卷身份。
    pub fn volume_id(&self) -> VolumeId {
        self.state.borrow().volume_id()
    }

    /// 返回卷代；恢复推进，普通提交不变。
    pub fn epoch(&self) -> u64 {
        self.state.borrow().epoch()
    }

    /// 按 DocPath 解析文档身份。
    pub fn resolve(&self, path: &str) -> Result<DocId, HolyError> {
        self.state.borrow().resolve(path)
    }

    /// 在集合中按名字查找文档身份。
    pub fn lookup(&self, parent: DocId, name: &str) -> Result<DocId, HolyError> {
        self.state.borrow().lookup(parent, name)
    }

    /// 在集合中创建文档，返回新文档身份。
    pub fn create(
        &mut self,
        parent: DocId,
        name: &str,
        kind: DocumentKind,
    ) -> Result<DocId, HolyError> {
        self.state.borrow_mut().create(parent, name, kind)
    }

    /// 移除集合项，返回被移除的文档身份。
    pub fn unlink(&mut self, parent: DocId, name: &str) -> Result<DocId, HolyError> {
        self.state.borrow_mut().unlink(parent, name)
    }

    /// 枚举集合文档。
    pub fn enumerate(&self, path: &str) -> Result<Vec<EntryInfo>, HolyError> {
        let document = self.resolve(path)?;
        self.enumerate_document(document)
    }

    /// 按文档身份枚举集合。
    pub fn enumerate_document(&self, document: DocId) -> Result<Vec<EntryInfo>, HolyError> {
        self.state.borrow().enumerate(document)
    }

    /// 读取文档身份、类型与当前修订。
    pub fn document(&self, path: &str) -> Result<DocumentInfo, HolyError> {
        let document = self.resolve(path)?;
        self.document_info(document)
    }

    /// 按文档身份读取视图。
    pub fn document_info(&self, document: DocId) -> Result<DocumentInfo, HolyError> {
        self.state.borrow().document_info(document)
    }

    /// 整文档读取。
    pub fn read_document(&self, path: &str) -> Result<Vec<u8>, HolyError> {
        let document = self.resolve(path)?;
        self.read_document_id(document)
    }

    /// 按文档身份整文档读取。
    pub fn read_document_id(&self, document: DocId) -> Result<Vec<u8>, HolyError> {
        self.state.borrow().read_document(document)
    }

    /// 从文档偏移读取。
    pub fn read_range(
        &self,
        document: DocId,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, HolyError> {
        self.state.borrow().read_range(document, offset, buffer)
    }

    /// 从指定修订的偏移读取。
    pub fn read_range_at(
        &self,
        document: DocId,
        revision: RevisionId,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, HolyError> {
        self.state
            .borrow()
            .read_range_at(document, revision, offset, buffer)
    }

    /// 整文档写入，返回新的 head 修订。
    pub fn write_document(&mut self, path: &str, content: &[u8]) -> Result<RevisionId, HolyError> {
        let document = self.resolve(path)?;
        self.write_document_id(document, content)
    }

    /// 按文档身份整文档写入。
    pub fn write_document_id(
        &mut self,
        document: DocId,
        content: &[u8],
    ) -> Result<RevisionId, HolyError> {
        self.state.borrow_mut().write_document(document, content)
    }

    /// 从文档偏移写入。
    pub fn write_range(
        &mut self,
        document: DocId,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, HolyError> {
        self.state.borrow_mut().write_range(document, offset, data)
    }

    /// 调整文档大小。
    pub fn truncate(&mut self, document: DocId, size: u64) -> Result<(), HolyError> {
        self.state.borrow_mut().truncate(document, size)
    }

    /// 枚举文档修订链。
    pub fn revisions(&self, path: &str) -> Result<Vec<RevisionInfo>, HolyError> {
        let document = self.resolve(path)?;
        self.revisions_of(document)
    }

    /// 按文档身份枚举修订链。
    pub fn revisions_of(&self, document: DocId) -> Result<Vec<RevisionInfo>, HolyError> {
        self.state.borrow().revisions(document)
    }

    /// 创建标签。
    pub fn create_tag(&mut self, name: &str) -> Result<u32, HolyError> {
        self.state.borrow_mut().create_tag(name)
    }

    /// 删除标签及其成员关系。
    pub fn drop_tag(&mut self, tag: u32) -> Result<(), HolyError> {
        self.state.borrow_mut().drop_tag(tag)
    }

    /// 把文档加入标签。
    pub fn tag(&mut self, document: DocId, tag: u32) -> Result<(), HolyError> {
        self.state.borrow_mut().tag(document, tag)
    }

    /// 把文档从标签移除。
    pub fn untag(&mut self, document: DocId, tag: u32) -> Result<(), HolyError> {
        self.state.borrow_mut().untag(document, tag)
    }

    /// 枚举标签。
    pub fn tags(&self) -> Vec<hk_fs::TagInfo> {
        self.state.borrow().tags()
    }

    /// 枚举标签成员。
    pub fn tag_members(&self, tag: u32) -> Result<Vec<DocId>, HolyError> {
        self.state.borrow().tag_members(tag)
    }

    /// 枚举文档所属标签。
    pub fn tags_of(&self, document: DocId) -> Result<Vec<u32>, HolyError> {
        self.state.borrow().tags_of(document)
    }

    /// 创建快照并提交。
    pub fn snapshot(&mut self, name: &str) -> Result<u64, HolyError> {
        self.state.borrow_mut().snapshot(name)
    }

    /// 枚举快照。
    pub fn snapshots(&self) -> Vec<SnapshotInfo> {
        self.state.borrow().snapshots()
    }

    /// 把卷恢复到快照，返回新的卷代数。
    pub fn restore(&mut self, snapshot: u64) -> Result<u64, HolyError> {
        self.state.borrow_mut().restore(snapshot)
    }

    /// 释放快照固定。
    pub fn forget(&mut self, snapshot: u64) -> Result<(), HolyError> {
        self.state.borrow_mut().forget(snapshot)
    }

    /// 返回当前卷代数。
    pub fn generation(&self) -> u64 {
        self.state.borrow().generation()
    }

    /// 把挂起写入提交到稳定存储。
    pub fn sync(&mut self) -> Result<(), HolyError> {
        self.state.borrow_mut().sync()
    }

    /// 只读校验对象图、引用与内容哈希。
    pub fn check(&self) -> Result<crate::MaintenanceReport, HolyError> {
        self.state.borrow().check()
    }

    /// 校验并回收泄漏块。
    pub fn scrub(&mut self) -> Result<crate::MaintenanceReport, HolyError> {
        self.state.borrow_mut().scrub()
    }

    /// 校验并修复分配位图。
    pub fn repair(&mut self) -> Result<crate::MaintenanceReport, HolyError> {
        self.state.borrow_mut().repair()
    }

    /// 封存修订。
    pub fn seal(&mut self, revision: RevisionId) -> Result<(), HolyError> {
        self.state.borrow_mut().seal(revision)
    }

    /// 裁剪文档修订链并回收旧块。
    pub fn prune(
        &mut self,
        document: DocId,
        keep_last: u64,
    ) -> Result<crate::MaintenanceReport, HolyError> {
        self.state.borrow_mut().prune(document, keep_last)
    }

    /// 显式销毁文档及其修订链。
    pub fn purge(&mut self, document: DocId) -> Result<(), HolyError> {
        self.state.borrow_mut().purge(document)
    }

    /// 内容去重命中次数。
    pub fn dedup_hits(&self) -> u64 {
        self.state.borrow().dedup_hits()
    }

    /// 返回卷配额。
    pub fn quota(&self) -> crate::Quota {
        self.state.borrow().quota()
    }

    /// 设置卷配额并提交。
    pub fn set_quota(&mut self, quota: crate::Quota) -> Result<(), HolyError> {
        self.state.borrow_mut().set_quota(quota)
    }

    /// 返回当前配额使用量。
    pub fn quota_used(&self) -> crate::Quota {
        self.state.borrow().quota_used()
    }

    #[cfg(test)]
    pub(crate) fn allocate_leak(&mut self, count: u64) -> Result<u64, HolyError> {
        self.state.borrow_mut().allocate_leak(count)
    }
}
