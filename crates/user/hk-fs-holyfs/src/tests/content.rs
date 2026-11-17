//! 内容读写、压缩与集合测试。

use super::*;

#[test]
fn writes_compressed_documents_after_sync() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    for index in 0..4u64 {
        let name = alloc::format!("sync{index}");
        let document = fs.create(root, &name, DocumentKind::Content).expect("创建");
        let mut data = Vec::with_capacity(16 * 1024);
        for offset in 0..16 * 1024 {
            if offset % 103 < 50 {
                data.push(0x5A);
            } else {
                data.push((((offset as u64 + index * 17) * 37) % 251) as u8);
            }
        }
        fs.write_range(document, 0, &data).expect("写");
        fs.sync().expect("提交");
        let mut buffer = vec![0_u8; data.len()];
        fs.read_range(document, 0, &mut buffer).expect("读");
        assert_eq!(buffer, data);
    }
}

#[test]
fn persists_compressed_multiblock_content() {
    let disk = Rc::new(RefCell::new(formatted()));
    let mut data = Vec::with_capacity(16 * 1024);
    for index in 0..16 * 1024 {
        if index % 103 < 50 {
            data.push(0x5A);
        } else {
            data.push(((index as u64 * 37) % 251) as u8);
        }
    }
    {
        let mut fs = HolyFs::mount(Shared(disk.clone())).expect("首次挂载");
        let root = fs.root();
        let document = fs
            .create(root, "compressed", DocumentKind::Content)
            .expect("创建");
        fs.write_range(document, 0, &data).expect("写");
        fs.sync().expect("提交");
    }
    let fs = HolyFs::mount(Shared(disk)).expect("重新挂载");
    let root = fs.root();
    let document = fs.lookup(root, "compressed").expect("查找");
    let mut buffer = vec![0_u8; data.len()];
    fs.read_range(document, 0, &mut buffer).expect("读");
    assert_eq!(buffer, data);
}

#[test]
fn reads_compressed_multiblock_content() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let document = fs
        .create(root, "compressed", DocumentKind::Content)
        .expect("创建");
    let mut data = Vec::with_capacity(16 * 1024);
    for index in 0..16 * 1024 {
        if index % 103 < 50 {
            data.push(0x5A);
        } else {
            data.push(((index as u64 * 37) % 251) as u8);
        }
    }
    fs.write_range(document, 0, &data).expect("写");
    let mut buffer = vec![0_u8; data.len()];
    fs.read_range(document, 0, &mut buffer).expect("读");
    assert_eq!(buffer, data);
}

#[test]
fn writes_multiple_binary_chunks() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let mut documents = Vec::new();
    for index in 0..8u64 {
        let name = alloc::format!("bin{index}");
        let document = fs.create(root, &name, DocumentKind::Content).expect("创建");
        let data: Vec<u8> = (0..16 * 1024)
            .map(|offset| (((offset as u64 + index * 17) * 37) % 251) as u8)
            .collect();
        fs.write_range(document, 0, &data).expect("写");
        documents.push((document, data));
    }
    for (document, data) in documents {
        let mut buffer = vec![0_u8; data.len()];
        fs.read_range(document, 0, &mut buffer).expect("读");
        assert_eq!(buffer, data);
    }
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
