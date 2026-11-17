//! staging、压缩、去重与内容哈希测试。

use super::*;

#[test]
fn streams_staged_writes() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let document = fs
        .create(root, "stream", DocumentKind::Content)
        .expect("创建");
    let staging = fs.stage(document, None).expect("staging");
    fs.write_staging(staging, 0, b"hello").expect("写一");
    fs.write_staging(staging, 5, b"!").expect("写二");
    fs.commit_staging(staging, None).expect("提交");
    assert_eq!(fs.read_document_id(document).expect("读"), b"hello!");
    assert_eq!(fs.check().expect("校验").leaked_blocks, 0);
}

#[test]
fn rejects_stale_or_aborted_staging() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let document = fs
        .create(root, "stream", DocumentKind::Content)
        .expect("创建");
    let staging = fs.stage(document, None).expect("staging");
    fs.write_range(document, 0, b"changed").expect("外部写");
    assert_eq!(fs.commit_staging(staging, None), Err(HolyError::Stale));
    fs.abort_staging(staging).expect("中止");
    assert_eq!(fs.commit_staging(staging, None), Err(HolyError::NotFound));
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
