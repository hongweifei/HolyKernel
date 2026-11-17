//! HolyFS 单元与持久化测试。

use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use hk_block::{BlockDevice, BlockError, MemoryDisk, SECTOR_SIZE};

use crate::error::HolyError;
use crate::types::{DocumentKind, RevisionFlags};
use crate::{HolyFs, Quota, VOLUME_H, VOLUME_I, format, format_with_volume_id};

const IMAGE_BLOCKS: u64 = (64 * 1024 * 1024) / SECTOR_SIZE as u64;

fn formatted() -> MemoryDisk {
    format(MemoryDisk::new(IMAGE_BLOCKS, SECTOR_SIZE)).expect("格式化")
}

mod content;
mod documents;
mod durability;
mod staging;

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

struct FaultState {
    fail_write: Option<usize>,
    short_write: Option<usize>,
    fail_read: Option<usize>,
    short_read: Option<usize>,
    fail_flush: Option<usize>,
    writes: usize,
    reads: usize,
    flushes: usize,
    dead: bool,
}

struct FaultDisk {
    inner: Rc<RefCell<MemoryDisk>>,
    state: Rc<RefCell<FaultState>>,
}

impl FaultDisk {
    fn new(
        inner: Rc<RefCell<MemoryDisk>>,
        fail_write: Option<usize>,
        short_write: Option<usize>,
        fail_flush: Option<usize>,
    ) -> (Self, Rc<RefCell<FaultState>>) {
        let state = Rc::new(RefCell::new(FaultState {
            fail_write,
            short_write,
            fail_read: None,
            short_read: None,
            fail_flush,
            writes: 0,
            reads: 0,
            flushes: 0,
            dead: false,
        }));
        (
            Self {
                inner,
                state: Rc::clone(&state),
            },
            state,
        )
    }
}

impl BlockDevice for FaultDisk {
    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn capacity_blocks(&self) -> u64 {
        self.inner.borrow().capacity_blocks()
    }

    fn read_blocks(&self, block: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        let (dead, index) = {
            let mut state = self.state.borrow_mut();
            if state.dead {
                return Err(BlockError::Io);
            }
            let index = state.reads;
            state.reads += 1;
            (state.dead, index)
        };
        if dead {
            return Err(BlockError::Io);
        }
        let mut state = self.state.borrow_mut();
        if state.fail_read == Some(index) {
            state.dead = true;
            return Err(BlockError::Io);
        }
        if state.short_read == Some(index) {
            state.dead = true;
            let half = (buffer.len() / 2 / SECTOR_SIZE) * SECTOR_SIZE;
            if half == 0 {
                return Err(BlockError::Io);
            }
            let read = self
                .inner
                .borrow()
                .read_blocks(block, &mut buffer[..half])?;
            return Ok(read);
        }
        drop(state);
        self.inner.borrow().read_blocks(block, buffer)
    }

    fn write_blocks(&mut self, block: u64, data: &[u8]) -> Result<usize, BlockError> {
        let index = {
            let mut state = self.state.borrow_mut();
            if state.dead {
                return Err(BlockError::Io);
            }
            let index = state.writes;
            state.writes += 1;
            index
        };
        let mut state = self.state.borrow_mut();
        if state.fail_write == Some(index) {
            state.dead = true;
            return Err(BlockError::Io);
        }
        if state.short_write == Some(index) {
            state.dead = true;
            let half = (data.len() / 2 / SECTOR_SIZE) * SECTOR_SIZE;
            if half == 0 {
                return Err(BlockError::Io);
            }
            let written = self.inner.borrow_mut().write_blocks(block, &data[..half])?;
            return Ok(written);
        }
        drop(state);
        self.inner.borrow_mut().write_blocks(block, data)
    }

    fn flush(&mut self) -> Result<(), BlockError> {
        let mut state = self.state.borrow_mut();
        if state.dead {
            return Err(BlockError::Io);
        }
        let index = state.flushes;
        state.flushes += 1;
        if state.fail_flush == Some(index) {
            state.dead = true;
            return Err(BlockError::Io);
        }
        drop(state);
        self.inner.borrow_mut().flush()
    }
}

fn crash_base() -> Vec<u8> {
    format(MemoryDisk::new(IMAGE_BLOCKS, SECTOR_SIZE))
        .expect("格式")
        .into_data()
}

fn crash_case(
    base: &[u8],
    fail_write: Option<usize>,
    short_write: Option<usize>,
    fail_flush: Option<usize>,
) {
    let raw = MemoryDisk::from_data(base.to_vec(), SECTOR_SIZE).expect("内存盘");
    let inner = Rc::new(RefCell::new(raw));
    let (disk, state) = FaultDisk::new(inner.clone(), fail_write, short_write, fail_flush);
    let result = {
        let mut fs = HolyFs::mount(disk).expect("挂载");
        let root = fs.root();
        let document = fs
            .create(root, "crash", DocumentKind::Content)
            .expect("创建");
        fs.write_range(document, 0, b"crash-data")
            .and_then(|_| fs.sync())
    };
    assert!(result.is_err(), "故障点未触发失败");
    assert!(state.borrow().dead);
    let data = Rc::try_unwrap(inner)
        .ok()
        .expect("故障盘仍被引用")
        .into_inner()
        .into_data();
    let fs = HolyFs::mount(MemoryDisk::from_data(data, SECTOR_SIZE).expect("内存盘"))
        .expect("故障后重新挂载");
    let root = fs.root();
    match fs.lookup(root, "crash") {
        Ok(document) => {
            assert_eq!(fs.read_document_id(document).expect("读"), b"crash-data");
        }
        Err(HolyError::NotFound) => {}
        Err(error) => panic!("故障恢复后引用损坏：{error:?}"),
    }
    fs.check().expect("故障恢复后校验");
}
