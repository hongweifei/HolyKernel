//! HolyFS 单元与持久化测试。

use alloc::rc::Rc;
use alloc::vec;
use core::cell::RefCell;

use hk_block::{BlockDevice, BlockError, MemoryDisk, SECTOR_SIZE};

use crate::error::HolyError;
use crate::types::DocumentKind;
use crate::{HolyFs, format};

const IMAGE_BLOCKS: u64 = (64 * 1024 * 1024) / SECTOR_SIZE as u64;

fn formatted() -> MemoryDisk {
    format(MemoryDisk::new(IMAGE_BLOCKS, SECTOR_SIZE)).expect("格式化")
}

#[test]
fn creates_writes_and_reads_files() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let document = fs
        .create(root, "hello", DocumentKind::Content)
        .expect("创建");
    assert_eq!(fs.write_range(document, 0, b"holyfs").expect("写"), 6);
    let mut buffer = [0_u8; 16];
    assert_eq!(fs.read_range(document, 0, &mut buffer).expect("读"), 6);
    assert_eq!(&buffer[..6], b"holyfs");
    assert_eq!(fs.document_info(document).expect("元数据").size, 6);
    let entries = fs.enumerate_document(root).expect("目录");
    assert!(entries.iter().any(|entry| entry.name == "hello"));
}

#[test]
fn appends_and_reads_large_file() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let document = fs
        .create(root, "large", DocumentKind::Content)
        .expect("创建");
    let chunk = [0x5A_u8; 60 * 1024];
    for index in 0..17 {
        fs.write_range(document, index * chunk.len() as u64, &chunk)
            .expect("追加");
    }
    let size = fs.document_info(document).expect("元数据").size;
    assert_eq!(size, 17 * chunk.len() as u64);
    let mut buffer = [0_u8; 4096];
    let read = fs
        .read_range(document, size - 4096, &mut buffer)
        .expect("读");
    assert_eq!(read, 4096);
    assert_eq!(buffer, [0x5A; 4096]);
}

#[test]
fn supports_nested_collections() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let docs = fs
        .create(root, "docs", DocumentKind::Collection)
        .expect("集合");
    let readme = fs
        .create(docs, "readme", DocumentKind::Content)
        .expect("文档");
    fs.write_range(readme, 0, b"nested").expect("写");
    assert_eq!(fs.lookup(root, "docs").expect("查找集合"), docs);
    assert_eq!(fs.lookup(docs, "readme").expect("查找文档"), readme);
    let mut buffer = [0_u8; 8];
    assert_eq!(fs.read_range(readme, 0, &mut buffer).expect("读"), 6);
    assert_eq!(&buffer[..6], b"nested");
    let entries = fs.enumerate_document(docs).expect("枚举");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "readme");
}

#[test]
fn rejects_duplicate_names() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    fs.create(root, "same", DocumentKind::Content)
        .expect("创建");
    assert_eq!(
        fs.create(root, "same", DocumentKind::Content),
        Err(HolyError::Exists)
    );
}

#[test]
fn truncates_and_unlinks() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let document = fs
        .create(root, "file", DocumentKind::Content)
        .expect("创建");
    fs.write_range(document, 0, b"0123456789").expect("写");
    fs.truncate(document, 4).expect("截断");
    assert_eq!(fs.document_info(document).expect("元数据").size, 4);
    assert_eq!(fs.unlink(root, "file").expect("删除"), document);
    assert_eq!(fs.lookup(root, "file"), Err(HolyError::NotFound));
}

struct Shared(Rc<RefCell<MemoryDisk>>);

impl BlockDevice for Shared {
    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn capacity_blocks(&self) -> u64 {
        self.0.borrow().capacity_blocks()
    }

    fn read_blocks(&self, block: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        self.0.borrow().read_blocks(block, buffer)
    }

    fn write_blocks(&mut self, block: u64, data: &[u8]) -> Result<usize, BlockError> {
        self.0.borrow_mut().write_blocks(block, data)
    }

    fn flush(&mut self) -> Result<(), BlockError> {
        Ok(())
    }
}

#[test]
fn persists_documents_and_collections() {
    let disk = Rc::new(RefCell::new(formatted()));
    {
        let mut fs = HolyFs::mount(Shared(disk.clone())).expect("首次挂载");
        let root = fs.root();
        let docs = fs
            .create(root, "docs", DocumentKind::Collection)
            .expect("集合");
        let document = fs.create(docs, "doc", DocumentKind::Content).expect("文档");
        fs.write_range(document, 0, b"HolyFS document").expect("写");
        fs.sync().expect("提交");
    }
    let fs = HolyFs::mount(Shared(disk)).expect("重新挂载");
    let root = fs.root();
    let docs = fs.lookup(root, "docs").expect("集合");
    let document = fs.lookup(docs, "doc").expect("文档");
    let mut buffer = [0_u8; 32];
    let read = fs.read_range(document, 0, &mut buffer).expect("读");
    assert_eq!(&buffer[..read], b"HolyFS document");
    assert_eq!(fs.enumerate_document(docs).expect("枚举").len(), 1);
}

#[test]
fn reclaims_old_metadata_trees() {
    let disk = Rc::new(RefCell::new(formatted()));
    {
        let mut fs = HolyFs::mount(Shared(disk.clone())).expect("首次挂载");
        let root = fs.root();
        let document = fs.create(root, "doc", DocumentKind::Content).expect("创建");
        fs.write_range(document, 0, b"first").expect("写一");
        fs.sync().expect("提交一");
        fs.write_range(document, 0, b"second").expect("写二");
        fs.sync().expect("提交二");
        fs.write_range(document, 0, b"third!").expect("写三");
        fs.sync().expect("提交三");
    }
    let fs = HolyFs::mount(Shared(disk)).expect("重新挂载");
    let root = fs.root();
    let document = fs.lookup(root, "doc").expect("查找");
    let mut buffer = [0_u8; 8];
    let read = fs.read_range(document, 0, &mut buffer).expect("读");
    assert_eq!(&buffer[..read], b"third!");
}

#[test]
fn revision_head_moves_on_write() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let document = fs.create(root, "doc", DocumentKind::Content).expect("创建");
    fs.write_range(document, 0, b"one").expect("写一");
    let first = fs.document_info(document).expect("元数据").size;
    fs.write_range(document, 0, b"two").expect("写二");
    assert_eq!(first, 3);
    let mut buffer = [0_u8; 8];
    assert_eq!(fs.read_range(document, 0, &mut buffer).expect("读"), 3);
    assert_eq!(&buffer[..3], b"two");
}

#[test]
fn native_document_api_reads_whole_documents() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let system = fs
        .create(root, "System", DocumentKind::Collection)
        .expect("集合");
    let document = fs
        .create(system, "Welcome", DocumentKind::Content)
        .expect("文档");
    fs.write_range(document, 0, b"@title(\"Welcome\")")
        .expect("写");
    fs.sync().expect("提交");

    assert_eq!(
        fs.resolve("H:/System/Welcome").expect("解析"),
        fs.resolve("/System/Welcome").expect("无盘符")
    );
    assert_eq!(
        fs.read_document("H:/System/Welcome").expect("整文档"),
        b"@title(\"Welcome\")"
    );
    let info = fs.document("H:/System/Welcome").expect("文档视图");
    assert_eq!(info.size, 17);
    assert_eq!(fs.revisions("H:/System/Welcome").expect("修订").len(), 2);
    let entries = fs.enumerate("H:/System").expect("集合");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "Welcome");
}

#[test]
fn native_document_write_moves_head() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let document = fs.create(root, "Doc", DocumentKind::Content).expect("文档");
    fs.write_range(document, 0, b"first").expect("写一");
    fs.sync().expect("提交");
    let head = fs.document("H:/Doc").expect("视图").head;
    let next = fs.write_document("H:/Doc", b"second").expect("整文档写");
    assert_ne!(next, head);
    assert_eq!(fs.read_document("H:/Doc").expect("读"), b"second");
    assert_eq!(fs.revisions("H:/Doc").expect("修订").len(), 3);
}

#[test]
fn tags_documents_and_persists() {
    let disk = Rc::new(RefCell::new(formatted()));
    let tag;
    {
        let mut fs = HolyFs::mount(Shared(disk.clone())).expect("首次挂载");
        let root = fs.root();
        let document = fs.create(root, "Doc", DocumentKind::Content).expect("创建");
        tag = fs.create_tag("work").expect("标签");
        assert_eq!(fs.create_tag("work"), Err(HolyError::Exists));
        fs.tag(document, tag).expect("加入标签");
        assert_eq!(fs.tags_of(document).expect("所属标签"), vec![tag]);
        assert_eq!(fs.tag_members(tag).expect("成员"), vec![document]);
        assert_eq!(fs.tags().len(), 1);
        fs.sync().expect("提交");
    }
    let mut fs = HolyFs::mount(Shared(disk)).expect("重新挂载");
    let root = fs.root();
    let document = fs.lookup(root, "Doc").expect("查找");
    let tags = fs.tags();
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].name, "work");
    assert_eq!(fs.tag_members(tags[0].id).expect("成员"), vec![document]);
    assert_eq!(fs.tags_of(document).expect("所属标签"), vec![tags[0].id]);
    fs.drop_tag(tags[0].id).expect("删除标签");
    assert!(fs.tags().is_empty());
    assert_eq!(fs.tag_members(tag), Err(HolyError::NotFound));
    fs.sync().expect("提交删除");
}

#[test]
fn snapshots_restore_documents_and_tags() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let document = fs.create(root, "Doc", DocumentKind::Content).expect("文档");
    fs.write_document("H:/Doc", b"before").expect("写一");
    let tag = fs.create_tag("keep").expect("标签");
    fs.tag(document, tag).expect("加入标签");
    let snapshot = fs.snapshot("checkpoint").expect("快照");
    assert_eq!(fs.snapshots().len(), 1);
    assert_eq!(fs.snapshots()[0].name, "checkpoint");

    fs.write_document("H:/Doc", b"after!").expect("写二");
    let extra = fs
        .create(root, "Extra", DocumentKind::Content)
        .expect("新增");
    let late = fs.create_tag("late").expect("新标签");
    fs.tag(extra, late).expect("新标签成员");
    let generation = fs.generation();

    let restored = fs.restore(snapshot).expect("恢复");
    assert!(restored > generation);
    assert_eq!(fs.read_document("H:/Doc").expect("读"), b"before");
    assert_eq!(fs.lookup(root, "Extra"), Err(HolyError::NotFound));
    assert_eq!(fs.document_info(extra), Err(HolyError::NotFound));
    let tags = fs.tags();
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].name, "keep");
    assert_eq!(fs.tag_members(tag).expect("成员"), vec![document]);
    assert_eq!(fs.tag_members(late), Err(HolyError::NotFound));
}

#[test]
fn snapshots_survive_remount() {
    let disk = Rc::new(RefCell::new(formatted()));
    let snapshot;
    {
        let mut fs = HolyFs::mount(Shared(disk.clone())).expect("首次挂载");
        let root = fs.root();
        fs.create(root, "Doc", DocumentKind::Content).expect("文档");
        fs.write_document("H:/Doc", b"before").expect("写一");
        snapshot = fs.snapshot("persisted").expect("快照");
        fs.write_document("H:/Doc", b"after!").expect("写二");
        fs.sync().expect("提交");
    }
    {
        let mut fs = HolyFs::mount(Shared(disk.clone())).expect("重新挂载");
        let snapshots = fs.snapshots();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, snapshot);
        assert_eq!(snapshots[0].name, "persisted");
        assert_eq!(snapshots[0].documents, 2);
        assert_eq!(fs.read_document("H:/Doc").expect("读"), b"after!");
        fs.restore(snapshot).expect("恢复");
        assert_eq!(fs.read_document("H:/Doc").expect("读"), b"before");
        assert_eq!(fs.snapshots().len(), 1);
    }
    let fs = HolyFs::mount(Shared(disk)).expect("再次挂载");
    assert_eq!(fs.read_document("H:/Doc").expect("读"), b"before");
    assert_eq!(fs.snapshots().len(), 1);
}

#[test]
fn restore_advances_epoch_only_on_restore() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let epoch = fs.epoch();
    let snapshot = fs.snapshot("epoch").expect("快照");
    assert_eq!(fs.epoch(), epoch);
    fs.sync().expect("提交");
    assert_eq!(fs.epoch(), epoch);
    fs.restore(snapshot).expect("恢复");
    assert!(fs.epoch() > epoch);
}

#[test]
fn forget_releases_snapshot() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let snapshot = fs.snapshot("temporary").expect("快照");
    fs.forget(snapshot).expect("释放");
    assert!(fs.snapshots().is_empty());
    assert_eq!(fs.restore(snapshot), Err(HolyError::NotFound));
}

#[test]
fn native_paths_have_no_parent_traversal() {
    let fs = HolyFs::mount(formatted()).expect("挂载");
    assert!(fs.resolve("H:/..").is_err());
    assert!(fs.resolve("H:/Missing").is_err());
}
