//! HolyFS 的 VFS 后端适配。
//!
//! 原生实现已经以 `DocId` 暴露文档操作；本 crate 只把 [`hk_fs::Backend`]
//! 契约接到 [`HolyFs`] 上，不引入路径或偏移语义。

#![no_std]

#[cfg(test)]
extern crate std;

extern crate alloc;

use alloc::vec::Vec;

use hk_block::BlockDevice;
use hk_fs::{
    Backend, Caps, DocId, DocumentInfo, DocumentKind, EntryInfo, Error, RevisionId, RevisionInfo,
};
use hk_fs_holyfs::{HolyError, HolyFs};

/// HolyFS 卷的 VFS 后端。
pub struct HolyFsVfs<D: BlockDevice> {
    volume: HolyFs<D>,
}

impl<D: BlockDevice> HolyFsVfs<D> {
    /// 挂载块设备并构造后端。
    pub fn mount(device: D) -> Result<Self, HolyError> {
        Ok(Self::new(HolyFs::mount(device)?))
    }

    /// 以已挂载的卷构造后端。
    pub fn new(volume: HolyFs<D>) -> Self {
        Self { volume }
    }

    /// 返回共享同一份 Volume 的原生句柄。
    pub fn volume(&self) -> HolyFs<D> {
        self.volume.clone()
    }
}

impl<D: BlockDevice> Backend for HolyFsVfs<D> {
    fn root(&self) -> DocId {
        self.volume.root()
    }

    fn caps(&self) -> Caps {
        Caps::NATIVE
    }

    fn document(&self, document: DocId) -> Result<DocumentInfo, Error> {
        self.volume.document_info(document)
    }

    fn resolve(&self, collection: DocId, name: &str) -> Result<DocId, Error> {
        self.volume.lookup(collection, name)
    }

    fn enumerate(&self, collection: DocId) -> Result<Vec<EntryInfo>, Error> {
        self.volume.enumerate_document(collection)
    }

    fn create(
        &mut self,
        collection: DocId,
        name: &str,
        kind: DocumentKind,
    ) -> Result<DocId, Error> {
        self.volume.create(collection, name, kind)
    }

    fn unbind(&mut self, collection: DocId, name: &str) -> Result<DocId, Error> {
        self.volume.unlink(collection, name)
    }

    fn read(
        &self,
        document: DocId,
        revision: Option<RevisionId>,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, Error> {
        if let Some(revision) = revision {
            let head = self.volume.document_info(document)?.head;
            if revision != head {
                return Err(Error::Unsupported);
            }
        }
        self.volume.read_range(document, offset, buffer)
    }

    fn write(&mut self, document: DocId, offset: u64, data: &[u8]) -> Result<usize, Error> {
        self.volume.write_range(document, offset, data)
    }

    fn truncate(&mut self, document: DocId, size: u64) -> Result<(), Error> {
        self.volume.truncate(document, size)
    }

    fn revisions(&self, document: DocId) -> Result<Vec<RevisionInfo>, Error> {
        self.volume.revisions_of(document)
    }

    fn sync(&mut self) -> Result<(), Error> {
        self.volume.sync()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hk_block::{MemoryDisk, SECTOR_SIZE};
    use hk_fs::DocumentKind;
    use hk_fs_holyfs::format;

    const IMAGE_BLOCKS: u64 = (8 * 1024 * 1024) / SECTOR_SIZE as u64;

    #[test]
    fn exposes_documents_through_the_contract() {
        let disk = format(MemoryDisk::new(IMAGE_BLOCKS, SECTOR_SIZE)).expect("格式化");
        let mut backend = HolyFsVfs::mount(disk).expect("挂载");
        assert!(backend.caps().contains(Caps::REVISIONS));

        let root = backend.root();
        let document = backend
            .create(root, "hello", DocumentKind::Content)
            .expect("创建");
        backend.write(document, 0, b"holyfs").expect("写");
        assert_eq!(backend.document(document).expect("视图").size, 6);
        assert_eq!(backend.resolve(root, "hello").expect("查找"), document);

        let mut buffer = [0_u8; 16];
        let read = backend.read(document, None, 0, &mut buffer).expect("读");
        assert_eq!(&buffer[..read], b"holyfs");
        assert_eq!(backend.revisions(document).expect("修订").len(), 2);

        let entries = backend.enumerate(root).expect("枚举");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "hello");

        assert_eq!(backend.unbind(root, "hello").expect("删除"), document);
        assert_eq!(
            backend.resolve(root, "hello").expect_err("已删除"),
            Error::NotFound
        );
    }

    #[test]
    fn rejects_unknown_revisions() {
        let disk = format(MemoryDisk::new(IMAGE_BLOCKS, SECTOR_SIZE)).expect("格式化");
        let mut backend = HolyFsVfs::mount(disk).expect("挂载");
        let root = backend.root();
        let document = backend
            .create(root, "doc", DocumentKind::Content)
            .expect("创建");
        backend.write(document, 0, b"first").expect("写");
        let head = backend.document(document).expect("视图").head;
        let mut buffer = [0_u8; 8];
        assert_eq!(backend.read(document, Some(head), 0, &mut buffer), Ok(5));
        assert_eq!(
            backend.read(document, Some(RevisionId::ROOT), 0, &mut buffer),
            Err(Error::Unsupported)
        );
    }
}
