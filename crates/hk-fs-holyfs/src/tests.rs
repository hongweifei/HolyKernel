//! HolyFS 单元与持久化测试。

use alloc::rc::Rc;
use alloc::vec;
use core::cell::RefCell;

use hk_block::{BlockDevice, BlockError, MemoryDisk, SECTOR_SIZE};

use crate::error::HolyError;
use crate::types::{DocumentKind, RevisionFlags};
use crate::{HolyFs, Quota, VOLUME_H, VOLUME_I, format, format_with_volume_id};

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
fn verifies_empty_content_hash() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let document = fs
        .create(root, "empty", DocumentKind::Content)
        .expect("创建");
    assert!(fs.read_document_id(document).expect("读空文档").is_empty());
    assert_eq!(fs.check().expect("校验").leaked_blocks, 0);
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
fn check_and_scrub_clean_volume() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let document = fs.create(root, "doc", DocumentKind::Content).expect("创建");
    fs.write_range(document, 0, b"integrity").expect("写");
    fs.sync().expect("提交");
    let report = fs.check().expect("校验");
    assert_eq!(report.documents, 2);
    assert_eq!(report.entries, 1);
    assert_eq!(report.leaked_blocks, 0);
    let report = fs.scrub().expect("清理");
    assert_eq!(report.leaked_blocks, 0);
    assert_eq!(report.repaired_blocks, 0);
}

#[test]
fn scrub_reclaims_leaked_blocks() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let block = fs.allocate_leak(2).expect("泄漏块");
    assert!(block > 0);
    let report = fs.check().expect("校验");
    assert_eq!(report.leaked_blocks, 2);
    let report = fs.scrub().expect("清理");
    assert_eq!(report.repaired_blocks, 2);
    assert_eq!(fs.check().expect("复检").leaked_blocks, 0);
}

#[test]
fn prune_keeps_recent_and_sealed_revisions() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let document = fs.create(root, "doc", DocumentKind::Content).expect("创建");
    fs.write_range(document, 0, b"one").expect("写一");
    let first = fs.document_info(document).expect("元数据").head;
    fs.write_range(document, 0, b"two").expect("写二");
    fs.write_range(document, 0, b"three").expect("写三");
    let third = fs.document_info(document).expect("元数据").head;
    fs.seal(first).expect("封存");
    let report = fs.prune(document, 1).expect("裁剪");
    assert!(report.repaired_blocks > 0);
    assert_eq!(fs.document_info(document).expect("元数据").head, third);
    let revisions = fs.revisions_of(document).expect("修订链");
    assert_eq!(revisions.len(), 1);
    assert_eq!(revisions[0].id, third);
    let mut buffer = [0_u8; 8];
    assert_eq!(
        fs.read_range_at(document, first, 0, &mut buffer)
            .expect("旧修订"),
        3
    );
    assert_eq!(&buffer[..3], b"one");
    assert_eq!(fs.check().expect("复检").leaked_blocks, 0);
}

#[test]
fn purge_removes_document_and_rejects_snapshot_pins() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let document = fs.create(root, "doc", DocumentKind::Content).expect("创建");
    fs.write_range(document, 0, b"data").expect("写");
    let snapshot = fs.snapshot("pin").expect("快照");
    assert_eq!(fs.purge(document), Err(HolyError::Pinned));
    fs.forget(snapshot).expect("释放快照");
    fs.purge(document).expect("销毁");
    assert_eq!(fs.lookup(root, "doc"), Err(HolyError::NotFound));
    assert!(fs.read_range(document, 0, &mut [0_u8; 4]).is_err());
    assert_eq!(fs.check().expect("复检").leaked_blocks, 0);
}

#[test]
fn enforces_quota_limits() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    fs.set_quota(Quota {
        documents: 2,
        bytes: 4,
        revisions: 100,
        snapshots: 1,
        tags: 1,
    })
    .expect("配额");
    let document = fs.create(root, "doc", DocumentKind::Content).expect("创建");
    assert_eq!(
        fs.create(root, "other", DocumentKind::Content),
        Err(HolyError::Quota)
    );
    assert_eq!(fs.write_range(document, 0, b"12345"), Err(HolyError::Quota));
    assert_eq!(fs.write_range(document, 0, b"1234").expect("写"), 4);
    assert_eq!(fs.create_tag("one").expect("标签"), 1);
    assert_eq!(fs.create_tag("two"), Err(HolyError::Quota));
    assert!(fs.snapshot("one").is_ok());
    assert_eq!(fs.snapshot("two"), Err(HolyError::Quota));
}

#[test]
fn quota_enforces_revision_limit() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    fs.set_quota(Quota {
        revisions: 3,
        ..Quota::default()
    })
    .expect("配额");
    let document = fs.create(root, "doc", DocumentKind::Content).expect("创建");
    assert_eq!(fs.write_range(document, 0, b"x"), Err(HolyError::Quota));
}

#[test]
fn quota_survives_remount() {
    let disk = Rc::new(RefCell::new(formatted()));
    {
        let mut fs = HolyFs::mount(Shared(disk.clone())).expect("挂载");
        fs.set_quota(Quota {
            documents: 8,
            bytes: 4096,
            revisions: 64,
            snapshots: 2,
            tags: 3,
        })
        .expect("配额");
    }
    let fs = HolyFs::mount(Shared(disk)).expect("重新挂载");
    assert_eq!(fs.quota().documents, 8);
    assert_eq!(fs.quota().bytes, 4096);
}

#[test]
fn compresses_repetitive_content() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let document = fs
        .create(root, "compressed", DocumentKind::Content)
        .expect("创建");
    let data = vec![0x5A_u8; 8192];
    fs.write_range(document, 0, &data).expect("写");
    let revisions = fs.revisions_of(document).expect("修订");
    assert!(revisions[0].flags.contains(RevisionFlags::COMPRESSED));
    assert_eq!(fs.read_document_id(document).expect("读"), data);
    assert_eq!(fs.check().expect("校验").leaked_blocks, 0);
}

#[test]
fn compresses_after_append() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let document = fs
        .create(root, "append", DocumentKind::Content)
        .expect("创建");
    fs.write_range(document, 0, &vec![0x41_u8; 4096])
        .expect("写一");
    fs.write_range(document, 4096, &vec![0x41_u8; 4096])
        .expect("写二");
    let revisions = fs.revisions_of(document).expect("修订");
    assert!(revisions[0].flags.contains(RevisionFlags::COMPRESSED));
    let content = fs.read_document_id(document).expect("读");
    assert_eq!(content.len(), 8192);
    assert!(content.iter().all(|byte| *byte == 0x41));
}

#[test]
fn dedups_identical_content() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let first = fs
        .create(root, "first", DocumentKind::Content)
        .expect("创建");
    let second = fs
        .create(root, "second", DocumentKind::Content)
        .expect("创建");
    fs.write_range(first, 0, b"same-content").expect("写一");
    fs.write_range(second, 0, b"same-content").expect("写二");
    assert!(fs.dedup_hits() >= 1);
    let mut buffer = [0_u8; 16];
    assert_eq!(fs.read_range(second, 0, &mut buffer).expect("读"), 12);
    assert_eq!(&buffer[..12], b"same-content");
    assert_eq!(fs.check().expect("复检").leaked_blocks, 0);
}

#[test]
fn rebuilds_dedup_index_on_mount() {
    let disk = Rc::new(RefCell::new(formatted()));
    {
        let mut fs = HolyFs::mount(Shared(disk.clone())).expect("挂载");
        let root = fs.root();
        let document = fs
            .create(root, "first", DocumentKind::Content)
            .expect("创建");
        fs.write_range(document, 0, b"persisted-content")
            .expect("写");
        fs.sync().expect("提交");
    }
    let mut fs = HolyFs::mount(Shared(disk)).expect("重新挂载");
    let root = fs.root();
    let document = fs
        .create(root, "second", DocumentKind::Content)
        .expect("创建");
    fs.write_range(document, 0, b"persisted-content")
        .expect("写");
    assert!(fs.dedup_hits() >= 1);
}

#[test]
fn detects_content_hash_damage() {
    let disk = Rc::new(RefCell::new(formatted()));
    {
        let mut fs = HolyFs::mount(Shared(disk.clone())).expect("挂载");
        let root = fs.root();
        let document = fs.create(root, "doc", DocumentKind::Content).expect("创建");
        fs.write_range(document, 0, b"integrity-check-payload")
            .expect("写");
        fs.sync().expect("提交");
    }
    {
        let mut raw = disk.borrow_mut();
        let sectors = raw.capacity_blocks();
        let mut buffer = [0_u8; SECTOR_SIZE];
        for sector in 0..sectors {
            raw.read_blocks(sector, &mut buffer).expect("读盘");
            if let Some(position) = buffer.windows(7).position(|window| window == b"check-p") {
                buffer[position] ^= 0x40;
                raw.write_blocks(sector, &buffer).expect("写盘");
                break;
            }
        }
    }
    let fs = HolyFs::mount(Shared(disk)).expect("重新挂载");
    assert_eq!(fs.check(), Err(HolyError::Damaged));
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
fn formats_distinct_volume_ids() {
    let blocks = (8 * 1024 * 1024) / SECTOR_SIZE as u64;
    let h =
        format_with_volume_id(MemoryDisk::new(blocks, SECTOR_SIZE), VOLUME_H).expect("格式化 H");
    let h = HolyFs::mount(h).expect("挂载 H");
    assert_eq!(h.volume_id(), VOLUME_H);
    let i =
        format_with_volume_id(MemoryDisk::new(blocks, SECTOR_SIZE), VOLUME_I).expect("格式化 I");
    let i = HolyFs::mount(i).expect("挂载 I");
    assert_eq!(i.volume_id(), VOLUME_I);
    assert_ne!(h.volume_id(), i.volume_id());
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
