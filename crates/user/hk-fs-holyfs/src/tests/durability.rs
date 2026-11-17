//! 崩溃恢复、完整性检查、配额与维护测试。

use super::*;

#[test]
fn boot_survives_write_and_flush_faults() {
    let base = crash_base();
    let raw = MemoryDisk::from_data(base.clone(), SECTOR_SIZE).expect("内存盘");
    let inner = Rc::new(RefCell::new(raw));
    let (disk, state) = FaultDisk::new(inner, None, None, None);
    let mut fs = HolyFs::mount(disk).expect("计数挂载");
    let root = fs.root();
    let document = fs
        .create(root, "probe", DocumentKind::Content)
        .expect("创建");
    fs.write_range(document, 0, b"crash-data").expect("写");
    fs.sync().expect("提交");
    let writes = state.borrow().writes;
    let flushes = state.borrow().flushes;
    drop(fs);
    for index in 0..writes {
        crash_case(&base, Some(index), None, None);
        crash_case(&base, None, Some(index), None);
    }
    for index in 0..flushes {
        crash_case(&base, None, None, Some(index));
    }
}

#[test]
fn read_fault_marks_damaged_and_repair_clears() {
    let base = crash_base();
    let raw = MemoryDisk::from_data(base, SECTOR_SIZE).expect("内存盘");
    let inner = Rc::new(RefCell::new(raw));
    let (disk, _state) = FaultDisk::new(inner.clone(), None, None, None);
    let mut fs = HolyFs::mount(disk).expect("挂载");
    let root = fs.root();
    let document = fs
        .create(root, "damaged", DocumentKind::Content)
        .expect("创建");
    fs.write_range(document, 0, b"read-fault").expect("写");
    fs.sync().expect("提交");
    drop(fs);

    let data = Rc::try_unwrap(inner)
        .ok()
        .expect("故障盘仍被引用")
        .into_inner()
        .into_data();
    let raw = MemoryDisk::from_data(data, SECTOR_SIZE).expect("内存盘");
    let inner = Rc::new(RefCell::new(raw));
    let (disk, state) = FaultDisk::new(inner, None, None, None);
    let mut fs = HolyFs::mount(disk).expect("重新挂载");
    let reads = state.borrow().reads;
    state.borrow_mut().fail_read = Some(reads);
    assert!(fs.read_document_id(document).is_err());
    assert!(fs.is_damaged());
    assert_eq!(
        fs.write_range(document, 0, b"blocked"),
        Err(HolyError::Damaged)
    );

    state.borrow_mut().fail_read = None;
    state.borrow_mut().dead = false;
    fs.repair().expect("修复");
    assert!(!fs.is_damaged());
    fs.write_range(document, 0, b"recovered").expect("恢复写");

    let reads = state.borrow().reads;
    state.borrow_mut().short_read = Some(reads);
    state.borrow_mut().dead = false;
    assert!(fs.read_document_id(document).is_err());
    assert!(fs.is_damaged());
    state.borrow_mut().short_read = None;
    state.borrow_mut().dead = false;
    fs.repair().expect("短读修复");
    assert!(!fs.is_damaged());
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
