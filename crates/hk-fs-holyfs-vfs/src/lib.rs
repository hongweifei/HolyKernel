//! HolyFS 的 VFS 后端适配。
//!
//! 原生实现只暴露 `DocId` 与文档操作；本 crate 把 `NodeId`、路径无关的
//! 文件句柄契约映射到原生接口，是 HolyFS 唯一的 VFS 感知层。

#![no_std]

#[cfg(test)]
extern crate std;

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::cell::RefCell;

use hk_block::BlockDevice;
use hk_fs_holyfs::{DocId, DocumentInfo, DocumentKind, HolyError, HolyFs};
use hk_vfs::{Backend, DirEntry, FileType, Metadata, NodeId, VfsError};

struct Nodes {
    nodes: BTreeMap<DocId, NodeId>,
    documents: BTreeMap<NodeId, DocId>,
    next: NodeId,
}

impl Nodes {
    fn new(root: DocId) -> Self {
        let mut nodes = BTreeMap::new();
        nodes.insert(root, 1);
        let mut documents = BTreeMap::new();
        documents.insert(1, root);
        Self {
            nodes,
            documents,
            next: 2,
        }
    }

    fn node_for(&mut self, document: DocId) -> NodeId {
        if let Some(node) = self.nodes.get(&document) {
            return *node;
        }
        let node = self.next;
        self.next = self.next.saturating_add(1);
        self.nodes.insert(document, node);
        self.documents.insert(node, document);
        node
    }

    fn document_for(&self, node: NodeId) -> Result<DocId, VfsError> {
        self.documents.get(&node).copied().ok_or(VfsError::NotFound)
    }
}

/// HolyFS 卷的 VFS 后端。
pub struct HolyFsVfs<D: BlockDevice> {
    volume: HolyFs<D>,
    root: NodeId,
    nodes: RefCell<Nodes>,
}

impl<D: BlockDevice> HolyFsVfs<D> {
    /// 挂载块设备并构造后端。
    pub fn mount(device: D) -> Result<Self, HolyError> {
        Ok(Self::new(HolyFs::mount(device)?))
    }

    /// 以已挂载的卷构造后端。
    pub fn new(volume: HolyFs<D>) -> Self {
        let root_document = volume.root();
        Self {
            volume,
            root: 1,
            nodes: RefCell::new(Nodes::new(root_document)),
        }
    }

    /// 返回共享同一份 Volume 的原生句柄。
    pub fn volume(&self) -> HolyFs<D> {
        self.volume.clone()
    }

    fn node_for(&self, document: DocId) -> NodeId {
        self.nodes.borrow_mut().node_for(document)
    }

    fn document_for(&self, node: NodeId) -> Result<DocId, VfsError> {
        self.nodes.borrow().document_for(node)
    }
}

impl<D: BlockDevice> Backend for HolyFsVfs<D> {
    fn root(&self) -> NodeId {
        self.root
    }

    fn lookup(&self, parent: NodeId, name: &str) -> Result<NodeId, VfsError> {
        let parent = self.document_for(parent)?;
        let document = self.volume.lookup(parent, name).map_err(map_error)?;
        Ok(self.node_for(document))
    }

    fn metadata(&self, node: NodeId) -> Result<Metadata, VfsError> {
        let document = self.document_for(node)?;
        let info = self.volume.document_info(document).map_err(map_error)?;
        Ok(metadata(info))
    }

    fn read(&self, node: NodeId, offset: u64, buffer: &mut [u8]) -> Result<usize, VfsError> {
        let document = self.document_for(node)?;
        self.volume
            .read_range(document, offset, buffer)
            .map_err(map_error)
    }

    fn write(&mut self, node: NodeId, offset: u64, data: &[u8]) -> Result<usize, VfsError> {
        let document = self.document_for(node)?;
        self.volume
            .write_range(document, offset, data)
            .map_err(map_error)
    }

    fn create(&mut self, parent: NodeId, name: &str, kind: FileType) -> Result<NodeId, VfsError> {
        let parent = self.document_for(parent)?;
        let document = self
            .volume
            .create(parent, name, document_kind(kind))
            .map_err(map_error)?;
        Ok(self.node_for(document))
    }

    fn readdir(&self, node: NodeId) -> Result<Vec<DirEntry>, VfsError> {
        let document = self.document_for(node)?;
        let entries = self
            .volume
            .enumerate_document(document)
            .map_err(map_error)?;
        Ok(entries
            .into_iter()
            .map(|entry| DirEntry {
                name: entry.name,
                kind: file_type(entry.kind),
            })
            .collect())
    }

    fn truncate(&mut self, node: NodeId, size: u64) -> Result<(), VfsError> {
        let document = self.document_for(node)?;
        self.volume.truncate(document, size).map_err(map_error)
    }

    fn unlink(&mut self, parent: NodeId, name: &str) -> Result<NodeId, VfsError> {
        let parent = self.document_for(parent)?;
        let document = self.volume.unlink(parent, name).map_err(map_error)?;
        Ok(self.node_for(document))
    }

    fn sync(&mut self) -> Result<(), VfsError> {
        self.volume.sync().map_err(map_error)
    }
}

fn metadata(info: DocumentInfo) -> Metadata {
    Metadata {
        kind: file_type(info.kind),
        size: if info.kind == DocumentKind::Content {
            info.size
        } else {
            0
        },
    }
}

const fn file_type(kind: DocumentKind) -> FileType {
    match kind {
        DocumentKind::Content => FileType::File,
        DocumentKind::Collection => FileType::Directory,
    }
}

const fn document_kind(kind: FileType) -> DocumentKind {
    match kind {
        FileType::File => DocumentKind::Content,
        FileType::Directory => DocumentKind::Collection,
    }
}

fn map_error(error: HolyError) -> VfsError {
    match error {
        HolyError::NotFound => VfsError::NotFound,
        HolyError::NotDirectory => VfsError::NotDirectory,
        HolyError::IsDirectory => VfsError::IsDirectory,
        HolyError::Exists => VfsError::AlreadyExists,
        HolyError::ReadOnly | HolyError::Refused => VfsError::PermissionDenied,
        HolyError::Invalid | HolyError::Stale => VfsError::InvalidOffset,
        HolyError::Io
        | HolyError::Corrupt
        | HolyError::NoSpace
        | HolyError::Unsupported
        | HolyError::Damaged
        | HolyError::Busy
        | HolyError::Pinned
        | HolyError::Quota => VfsError::Backend,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use hk_block::{MemoryDisk, SECTOR_SIZE};
    use hk_fs_holyfs::format;
    use hk_vfs::{OpenFlags, Vfs};

    const IMAGE_BLOCKS: u64 = (8 * 1024 * 1024) / SECTOR_SIZE as u64;

    #[test]
    fn maps_documents_to_vfs_nodes() {
        let disk = format(MemoryDisk::new(IMAGE_BLOCKS, SECTOR_SIZE)).expect("格式化");
        let backend = HolyFsVfs::mount(disk).expect("挂载");
        let mut vfs = Vfs::new(Box::new(backend));

        let handle = vfs.open("/hello", OpenFlags::CREATE).expect("创建");
        vfs.write(handle, b"holyfs").expect("写");
        vfs.close(handle).expect("关闭");
        vfs.sync_all().expect("提交");

        let handle = vfs.open("/hello", OpenFlags::READ_ONLY).expect("打开");
        let mut buffer = [0_u8; 16];
        let read = vfs.read(handle, &mut buffer).expect("读");
        assert_eq!(&buffer[..read], b"holyfs");
        vfs.close(handle).expect("关闭");

        let entries = vfs.readdir("/").expect("枚举");
        assert!(entries.iter().any(|entry| entry.name == "hello"));
        let metadata = vfs.metadata("/hello").expect("元数据");
        assert_eq!(metadata.kind, FileType::File);
        assert_eq!(metadata.size, 6);
    }

    #[test]
    fn maps_collections_to_directories() {
        let disk = format(MemoryDisk::new(IMAGE_BLOCKS, SECTOR_SIZE)).expect("格式化");
        let backend = HolyFsVfs::mount(disk).expect("挂载");
        let mut vfs = Vfs::new(Box::new(backend));

        vfs.create_dir("/docs").expect("集合");
        let handle = vfs.open("/docs/readme", OpenFlags::CREATE).expect("文档");
        vfs.write(handle, b"nested").expect("写");
        vfs.close(handle).expect("关闭");

        let entries = vfs.readdir("/docs").expect("枚举");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "readme");
        assert_eq!(
            vfs.metadata("/docs").expect("集合元数据").kind,
            FileType::Directory
        );
    }

    #[test]
    fn propagates_holy_errors() {
        let disk = format(MemoryDisk::new(IMAGE_BLOCKS, SECTOR_SIZE)).expect("格式化");
        let backend = HolyFsVfs::mount(disk).expect("挂载");
        let mut vfs = Vfs::new(Box::new(backend));

        assert_eq!(
            vfs.metadata("/missing").expect_err("缺失"),
            VfsError::NotFound
        );
        vfs.create_dir("/docs").expect("集合");
        assert_eq!(
            vfs.create_dir("/docs").expect_err("重复"),
            VfsError::AlreadyExists
        );
    }
}
