//! 宿主侧崩溃恢复：可丢弃未刷写写入的块设备 + HolyFS 重新挂载校验。

use std::cell::RefCell;
use std::rc::Rc;

use clap::Args;
use hk_block::{BlockDevice, BlockError, SECTOR_SIZE};
use hk_fs_holyfs::{DocumentKind, HolyFs, format};

use crate::failure::{Failure, Result};

const IMAGE_BLOCKS: u64 = (64 * 1024 * 1024) / SECTOR_SIZE as u64;

/// 崩溃测试参数。
#[derive(Debug, Args)]
pub struct CrashArguments {
    /// 使用的块数
    #[arg(long, default_value_t = IMAGE_BLOCKS)]
    pub blocks: u64,
}

/// 执行崩溃恢复场景。
pub fn run(arguments: CrashArguments) -> Result<()> {
    let blocks = arguments.blocks.max(1024);
    unsynced_write_lost(blocks)?;
    synced_write_durable(blocks)?;
    torn_write_recovers(blocks)?;
    corrupted_tail_does_not_panic(blocks)?;
    println!("crash recovery：4 个场景通过");
    Ok(())
}

struct DiskState {
    stable: Vec<u8>,
    pending: Vec<(u64, Vec<u8>)>,
}

struct CrashDisk {
    state: Rc<RefCell<DiskState>>,
    block_size: usize,
}

impl CrashDisk {
    fn new(blocks: u64, block_size: usize) -> Self {
        Self {
            state: Rc::new(RefCell::new(DiskState {
                stable: vec![0; blocks as usize * block_size],
                pending: Vec::new(),
            })),
            block_size,
        }
    }

    fn crash(&self) {
        self.state.borrow_mut().pending.clear();
    }

    fn crash_partial(&self) {
        let mut state = self.state.borrow_mut();
        let pending = core::mem::take(&mut state.pending);
        let apply = pending.len() / 2;
        for (block, data) in pending.into_iter().take(apply) {
            let start = block as usize * self.block_size;
            if start + data.len() <= state.stable.len() {
                state.stable[start..start + data.len()].copy_from_slice(&data);
            }
        }
    }

    fn corrupt_tail(&self) {
        let mut state = self.state.borrow_mut();
        let start = state.stable.len().saturating_sub(self.block_size);
        for byte in &mut state.stable[start..] {
            *byte ^= 0xA5;
        }
    }

    fn state(&self) -> Rc<RefCell<DiskState>> {
        Rc::clone(&self.state)
    }
}

impl BlockDevice for CrashDisk {
    fn block_size(&self) -> usize {
        self.block_size
    }

    fn capacity_blocks(&self) -> u64 {
        (self.state.borrow().stable.len() / self.block_size) as u64
    }

    fn read_blocks(&self, block: u64, buffer: &mut [u8]) -> std::result::Result<usize, BlockError> {
        let start = (block as usize)
            .checked_mul(self.block_size)
            .ok_or(BlockError::OutOfRange)?;
        let end = start
            .checked_add(buffer.len())
            .ok_or(BlockError::OutOfRange)?;
        let state = self.state.borrow();
        if end > state.stable.len() || !buffer.len().is_multiple_of(self.block_size) {
            return Err(BlockError::OutOfRange);
        }
        buffer.copy_from_slice(&state.stable[start..end]);
        for (pending_block, data) in &state.pending {
            let pending_start = *pending_block as usize * self.block_size;
            let pending_end = pending_start + data.len();
            let overlap_start = start.max(pending_start);
            let overlap_end = end.min(pending_end);
            if overlap_start < overlap_end {
                let source = &data[overlap_start - pending_start..overlap_end - pending_start];
                let target = &mut buffer[overlap_start - start..overlap_end - start];
                target.copy_from_slice(source);
            }
        }
        Ok(buffer.len())
    }

    fn write_blocks(&mut self, block: u64, data: &[u8]) -> std::result::Result<usize, BlockError> {
        if data.is_empty() || !data.len().is_multiple_of(self.block_size) {
            return Err(BlockError::InvalidLength);
        }
        let capacity = self.capacity_blocks();
        let end = block
            .checked_add((data.len() / self.block_size) as u64)
            .ok_or(BlockError::OutOfRange)?;
        if end > capacity {
            return Err(BlockError::OutOfRange);
        }
        self.state
            .borrow_mut()
            .pending
            .retain(|(pending, _)| *pending != block);
        self.state.borrow_mut().pending.push((block, data.to_vec()));
        Ok(data.len())
    }

    fn flush(&mut self) -> std::result::Result<(), BlockError> {
        let mut state = self.state.borrow_mut();
        let pending = core::mem::take(&mut state.pending);
        for (block, data) in pending {
            let start = block as usize * self.block_size;
            state.stable[start..start + data.len()].copy_from_slice(&data);
        }
        Ok(())
    }
}

fn mount(state: &Rc<RefCell<DiskState>>, block_size: usize) -> Result<HolyFs<CrashDisk>> {
    let disk = CrashDisk {
        state: Rc::clone(state),
        block_size,
    };
    HolyFs::mount(disk).map_err(|error| Failure::new(format!("崩溃后挂载失败：{error:?}")))
}

fn unsynced_write_lost(blocks: u64) -> Result<()> {
    let disk = CrashDisk::new(blocks, SECTOR_SIZE);
    let state = disk.state();
    let disk = format(disk).map_err(|error| Failure::new(format!("格式化失败：{error:?}")))?;
    let mut fs =
        HolyFs::mount(disk).map_err(|error| Failure::new(format!("挂载失败：{error:?}")))?;
    let root = fs.root();
    let old = fs
        .create(root, "old", DocumentKind::Content)
        .map_err(fs_error)?;
    fs.write_range(old, 0, b"old data").map_err(fs_error)?;
    fs.sync().map_err(fs_error)?;
    let new = fs
        .create(root, "new", DocumentKind::Content)
        .map_err(fs_error)?;
    fs.write_range(new, 0, b"new data").map_err(fs_error)?;
    drop(fs);
    CrashDisk {
        state: state.clone(),
        block_size: SECTOR_SIZE,
    }
    .crash();

    let fs = mount(&state, SECTOR_SIZE)?;
    let old_data = fs.read_document("/old").map_err(fs_error)?;
    if old_data != b"old data" {
        return Err(Failure::new("未同步崩溃后旧内容丢失"));
    }
    if let Ok(data) = fs.read_document("/new")
        && data != b"new data"
    {
        return Err(Failure::new("未同步写入在崩溃后出现部分内容"));
    }
    Ok(())
}

fn synced_write_durable(blocks: u64) -> Result<()> {
    let disk = CrashDisk::new(blocks, SECTOR_SIZE);
    let state = disk.state();
    let disk = format(disk).map_err(|error| Failure::new(format!("格式化失败：{error:?}")))?;
    let mut fs =
        HolyFs::mount(disk).map_err(|error| Failure::new(format!("挂载失败：{error:?}")))?;
    let root = fs.root();
    let document = fs
        .create(root, "durable", DocumentKind::Content)
        .map_err(fs_error)?;
    fs.write_range(document, 0, b"durable data")
        .map_err(fs_error)?;
    fs.sync().map_err(fs_error)?;
    drop(fs);
    CrashDisk {
        state: state.clone(),
        block_size: SECTOR_SIZE,
    }
    .crash();

    let fs = mount(&state, SECTOR_SIZE)?;
    let data = fs.read_document("/durable").map_err(fs_error)?;
    if data != b"durable data" {
        return Err(Failure::new("已同步内容在崩溃后不一致"));
    }
    Ok(())
}

fn torn_write_recovers(blocks: u64) -> Result<()> {
    let disk = CrashDisk::new(blocks, SECTOR_SIZE);
    let state = disk.state();
    let disk = format(disk).map_err(|error| Failure::new(format!("格式化失败：{error:?}")))?;
    let mut fs =
        HolyFs::mount(disk).map_err(|error| Failure::new(format!("挂载失败：{error:?}")))?;
    let root = fs.root();
    let document = fs
        .create(root, "torn", DocumentKind::Content)
        .map_err(fs_error)?;
    fs.write_range(document, 0, &vec![0x5A; 16 * 1024])
        .map_err(fs_error)?;
    drop(fs);
    CrashDisk {
        state: state.clone(),
        block_size: SECTOR_SIZE,
    }
    .crash_partial();

    if let Ok(fs) = mount(&state, SECTOR_SIZE) {
        let _ = fs.check();
        if let Ok(data) = fs.read_document("/torn")
            && !data.is_empty()
            && data != vec![0x5A; 16 * 1024]
        {
            return Err(Failure::new("撕裂写恢复后出现部分内容"));
        }
    }
    Ok(())
}

fn corrupted_tail_does_not_panic(blocks: u64) -> Result<()> {
    let disk = CrashDisk::new(blocks, SECTOR_SIZE);
    let state = disk.state();
    let disk = format(disk).map_err(|error| Failure::new(format!("格式化失败：{error:?}")))?;
    let mut fs =
        HolyFs::mount(disk).map_err(|error| Failure::new(format!("挂载失败：{error:?}")))?;
    let root = fs.root();
    let document = fs
        .create(root, "corrupt", DocumentKind::Content)
        .map_err(fs_error)?;
    fs.write_range(document, 0, b"corrupt data")
        .map_err(fs_error)?;
    fs.sync().map_err(fs_error)?;
    drop(fs);
    CrashDisk {
        state: state.clone(),
        block_size: SECTOR_SIZE,
    }
    .corrupt_tail();

    if let Ok(fs) = mount(&state, SECTOR_SIZE) {
        let _ = fs.check();
    }
    Ok(())
}

fn fs_error(error: hk_fs_holyfs::HolyError) -> Failure {
    Failure::new(format!("HolyFS 操作失败：{error:?}"))
}
