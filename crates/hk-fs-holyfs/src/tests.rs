//! HolyFS 单元与持久化测试。

use alloc::rc::Rc;
use core::cell::RefCell;

use hk_block::{BlockDevice, BlockError, MemoryDisk, SECTOR_SIZE};
use hk_vfs::{Backend, FileType, VfsError};

use crate::{HolyFs, format};

const IMAGE_BLOCKS: u64 = (64 * 1024 * 1024) / SECTOR_SIZE as u64;

fn formatted() -> MemoryDisk {
    format(MemoryDisk::new(IMAGE_BLOCKS, SECTOR_SIZE)).expect("格式化")
}

#[test]
fn creates_writes_and_reads_files() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let node = fs.create(root, "hello", FileType::File).expect("创建");
    assert_eq!(fs.write(node, 0, b"holyfs").expect("写"), 6);
    let mut buffer = [0_u8; 16];
    assert_eq!(fs.read(node, 0, &mut buffer).expect("读"), 6);
    assert_eq!(&buffer[..6], b"holyfs");
    assert_eq!(fs.metadata(node).expect("元数据").size, 6);
    let entries = fs.readdir(root).expect("目录");
    assert!(entries.iter().any(|entry| entry.name == "hello"));
}

#[test]
fn appends_and_reads_large_file() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let node = fs.create(root, "large", FileType::File).expect("创建");
    let chunk = [0x5A_u8; 60 * 1024];
    for index in 0..17 {
        fs.write(node, index * chunk.len() as u64, &chunk)
            .expect("追加");
    }
    let size = fs.metadata(node).expect("元数据").size;
    assert_eq!(size, 17 * chunk.len() as u64);
    let mut buffer = [0_u8; 4096];
    let read = fs.read(node, size - 4096, &mut buffer).expect("读");
    assert_eq!(read, 4096);
    assert_eq!(buffer, [0x5A; 4096]);
}

#[test]
fn supports_nested_collections() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let docs = fs.create(root, "docs", FileType::Directory).expect("集合");
    let readme = fs.create(docs, "readme", FileType::File).expect("文档");
    fs.write(readme, 0, b"nested").expect("写");
    assert_eq!(fs.lookup(root, "docs").expect("查找集合"), docs);
    assert_eq!(fs.lookup(docs, "readme").expect("查找文档"), readme);
    let mut buffer = [0_u8; 8];
    assert_eq!(fs.read(readme, 0, &mut buffer).expect("读"), 6);
    assert_eq!(&buffer[..6], b"nested");
    let entries = fs.readdir(docs).expect("枚举");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "readme");
}

#[test]
fn rejects_duplicate_names() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    fs.create(root, "same", FileType::File).expect("创建");
    assert_eq!(
        fs.create(root, "same", FileType::File),
        Err(VfsError::AlreadyExists)
    );
}

#[test]
fn truncates_and_unlinks() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let node = fs.create(root, "file", FileType::File).expect("创建");
    fs.write(node, 0, b"0123456789").expect("写");
    fs.truncate(node, 4).expect("截断");
    assert_eq!(fs.metadata(node).expect("元数据").size, 4);
    assert_eq!(fs.unlink(root, "file").expect("删除"), node);
    assert_eq!(fs.lookup(root, "file"), Err(VfsError::NotFound));
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
        let docs = fs.create(root, "docs", FileType::Directory).expect("集合");
        let node = fs.create(docs, "doc", FileType::File).expect("文档");
        fs.write(node, 0, b"HolyFS document").expect("写");
        fs.sync().expect("提交");
    }
    let fs = HolyFs::mount(Shared(disk)).expect("重新挂载");
    let root = fs.root();
    let docs = fs.lookup(root, "docs").expect("集合");
    let node = fs.lookup(docs, "doc").expect("文档");
    let mut buffer = [0_u8; 32];
    let read = fs.read(node, 0, &mut buffer).expect("读");
    assert_eq!(&buffer[..read], b"HolyFS document");
    assert_eq!(fs.readdir(docs).expect("枚举").len(), 1);
}

#[test]
fn reclaims_old_metadata_trees() {
    let disk = Rc::new(RefCell::new(formatted()));
    {
        let mut fs = HolyFs::mount(Shared(disk.clone())).expect("首次挂载");
        let root = fs.root();
        let node = fs.create(root, "doc", FileType::File).expect("创建");
        fs.write(node, 0, b"first").expect("写一");
        fs.sync().expect("提交一");
        fs.write(node, 0, b"second").expect("写二");
        fs.sync().expect("提交二");
        fs.write(node, 0, b"third!").expect("写三");
        fs.sync().expect("提交三");
    }
    let fs = HolyFs::mount(Shared(disk)).expect("重新挂载");
    let root = fs.root();
    let node = fs.lookup(root, "doc").expect("查找");
    let mut buffer = [0_u8; 8];
    let read = fs.read(node, 0, &mut buffer).expect("读");
    assert_eq!(&buffer[..read], b"third!");
}

#[test]
fn revision_head_moves_on_write() {
    let mut fs = HolyFs::mount(formatted()).expect("挂载");
    let root = fs.root();
    let node = fs.create(root, "doc", FileType::File).expect("创建");
    fs.write(node, 0, b"one").expect("写一");
    let first = fs.metadata(node).expect("元数据").size;
    fs.write(node, 0, b"two").expect("写二");
    assert_eq!(first, 3);
    let mut buffer = [0_u8; 8];
    assert_eq!(fs.read(node, 0, &mut buffer).expect("读"), 3);
    assert_eq!(&buffer[..3], b"two");
}
