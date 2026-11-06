//! 块设备适配、位图分配器与字节区间 I/O。

use alloc::vec;
use alloc::vec::Vec;

use hk_block::BlockDevice;

use crate::error::{HolyError, map_io};
use crate::superblock::{BITMAP_START, BLOCK_SIZE, SUPERBLOCK_A, SUPERBLOCK_B, Superblock};

const IO_CHUNK: usize = 60 * 1024;
/// 每个分配组的块数。
pub(crate) const GROUP_BLOCKS: u64 = 1024;

struct Group {
    start: u64,
    end: u64,
    free: u64,
    hint: u64,
}

pub(crate) struct Store<D: BlockDevice> {
    device: D,
    total_blocks: u64,
    bitmap_start: u64,
    bitmap_blocks: u32,
    data_start: u64,
    bitmap: Vec<u8>,
    next_hint: u64,
    groups: Vec<Group>,
    group_hint: usize,
}

impl<D: BlockDevice> Store<D> {
    pub(crate) fn format(device: D) -> Result<Self, HolyError> {
        let block_size = device.block_size();
        if block_size == 0 || !BLOCK_SIZE.is_multiple_of(block_size) {
            return Err(HolyError::Invalid);
        }
        let sectors = (BLOCK_SIZE / block_size) as u64;
        let total_blocks = device.capacity_blocks() / sectors;
        let bitmap_blocks = bitmap_blocks_for(total_blocks)?;
        let data_start = BITMAP_START + u64::from(bitmap_blocks);
        if total_blocks <= data_start {
            return Err(HolyError::NoSpace);
        }
        let mut store = Self {
            device,
            total_blocks,
            bitmap_start: BITMAP_START,
            bitmap_blocks,
            data_start,
            bitmap: vec![0_u8; bitmap_blocks as usize * BLOCK_SIZE],
            next_hint: data_start,
            groups: Vec::new(),
            group_hint: 0,
        };
        let zero = [0_u8; BLOCK_SIZE];
        store.write_block(SUPERBLOCK_A, &zero)?;
        store.write_block(SUPERBLOCK_B, &zero)?;
        for block in BITMAP_START..data_start {
            store.write_block(block, &zero)?;
        }
        for block in 0..data_start {
            store.mark(block, true);
        }
        store.rebuild_groups();
        store.write_bitmap()?;
        Ok(store)
    }

    pub(crate) fn load(device: D, superblock: &Superblock) -> Result<Self, HolyError> {
        let block_size = device.block_size();
        if block_size == 0 || !BLOCK_SIZE.is_multiple_of(block_size) {
            return Err(HolyError::Invalid);
        }
        let sectors = (BLOCK_SIZE / block_size) as u64;
        let capacity = device.capacity_blocks() / sectors;
        if superblock.total_blocks == 0 || superblock.total_blocks > capacity {
            return Err(HolyError::Corrupt);
        }
        let expected = bitmap_blocks_for(superblock.total_blocks)?;
        if superblock.bitmap_blocks != expected {
            return Err(HolyError::Corrupt);
        }
        let mut store = Self {
            device,
            total_blocks: superblock.total_blocks,
            bitmap_start: superblock.bitmap_start,
            bitmap_blocks: superblock.bitmap_blocks,
            data_start: superblock.bitmap_start + u64::from(superblock.bitmap_blocks),
            bitmap: vec![0_u8; superblock.bitmap_blocks as usize * BLOCK_SIZE],
            next_hint: superblock.bitmap_start + u64::from(superblock.bitmap_blocks),
            groups: Vec::new(),
            group_hint: 0,
        };
        if store.data_start > store.total_blocks {
            return Err(HolyError::Corrupt);
        }
        for index in 0..store.bitmap_blocks as usize {
            let mut block = [0_u8; BLOCK_SIZE];
            store.read_block(store.bitmap_start + index as u64, &mut block)?;
            store.bitmap[index * BLOCK_SIZE..(index + 1) * BLOCK_SIZE].copy_from_slice(&block);
        }
        store.rebuild_groups();
        Ok(store)
    }

    pub(crate) fn total_blocks(&self) -> u64 {
        self.total_blocks
    }

    pub(crate) fn data_start(&self) -> u64 {
        self.data_start
    }

    pub(crate) fn bitmap_blocks(&self) -> Result<u32, HolyError> {
        bitmap_blocks_for(self.total_blocks)
    }

    pub(crate) fn read_block(
        &self,
        block: u64,
        buffer: &mut [u8; BLOCK_SIZE],
    ) -> Result<(), HolyError> {
        if block >= self.total_blocks {
            return Err(HolyError::Corrupt);
        }
        let sectors = (BLOCK_SIZE / self.device.block_size()) as u64;
        self.device
            .read_blocks(block * sectors, buffer)
            .map(|_| ())
            .map_err(map_io)
    }

    pub(crate) fn write_block(
        &mut self,
        block: u64,
        buffer: &[u8; BLOCK_SIZE],
    ) -> Result<(), HolyError> {
        if block >= self.total_blocks {
            return Err(HolyError::NoSpace);
        }
        let sectors = (BLOCK_SIZE / self.device.block_size()) as u64;
        self.device
            .write_blocks(block * sectors, buffer)
            .map(|_| ())
            .map_err(map_io)
    }

    pub(crate) fn read_bytes(&self, offset: u64, buffer: &mut [u8]) -> Result<(), HolyError> {
        let block_size = self.device.block_size();
        let mut done = 0;
        let mut position = offset;
        let mut temporary = [0_u8; BLOCK_SIZE];
        while done < buffer.len() {
            let within = (position % BLOCK_SIZE as u64) as usize;
            let remaining = buffer.len() - done;
            let direct = within == 0 && remaining >= BLOCK_SIZE;
            if direct {
                let chunk = (remaining.min(IO_CHUNK) / block_size) * block_size;
                let sectors = position / block_size as u64;
                self.device
                    .read_blocks(sectors, &mut buffer[done..done + chunk])
                    .map_err(map_io)?;
                done += chunk;
                position += chunk as u64;
            } else {
                let block = position / BLOCK_SIZE as u64;
                self.read_block(block, &mut temporary)?;
                let take = (BLOCK_SIZE - within).min(remaining);
                buffer[done..done + take].copy_from_slice(&temporary[within..within + take]);
                done += take;
                position += take as u64;
            }
        }
        Ok(())
    }

    pub(crate) fn write_bytes(&mut self, offset: u64, data: &[u8]) -> Result<(), HolyError> {
        let block_size = self.device.block_size();
        let mut done = 0;
        let mut position = offset;
        let mut temporary = [0_u8; BLOCK_SIZE];
        while done < data.len() {
            let within = (position % BLOCK_SIZE as u64) as usize;
            let remaining = data.len() - done;
            let direct = within == 0 && remaining >= BLOCK_SIZE;
            if direct {
                let chunk = (remaining.min(IO_CHUNK) / block_size) * block_size;
                let sectors = position / block_size as u64;
                self.device
                    .write_blocks(sectors, &data[done..done + chunk])
                    .map_err(map_io)?;
                done += chunk;
                position += chunk as u64;
            } else {
                let block = position / BLOCK_SIZE as u64;
                self.read_block(block, &mut temporary)?;
                let take = (BLOCK_SIZE - within).min(remaining);
                temporary[within..within + take].copy_from_slice(&data[done..done + take]);
                self.write_block(block, &temporary)?;
                done += take;
                position += take as u64;
            }
        }
        Ok(())
    }

    pub(crate) fn allocate_blocks(&mut self, count: u64) -> Result<u64, HolyError> {
        if count == 0 {
            return Ok(0);
        }
        if count <= GROUP_BLOCKS {
            for offset in 0..self.groups.len() {
                let index = (self.group_hint + offset) % self.groups.len();
                if self.groups[index].free < count {
                    continue;
                }
                if let Some(start) = self.find_run(index, count) {
                    self.allocate_run(start, count);
                    self.group_hint = (index + 1) % self.groups.len();
                    return Ok(start);
                }
            }
        }
        let mut scan = self.data_start;
        while scan < self.total_blocks {
            let mut run = 0;
            while scan + run < self.total_blocks && !self.block_allocated(scan + run) {
                run += 1;
                if run == count {
                    self.allocate_run(scan, count);
                    return Ok(scan);
                }
            }
            scan += run.max(1);
        }
        Err(HolyError::NoSpace)
    }

    pub(crate) fn free_blocks(&mut self, start: u64, count: u64) {
        if count == 0 {
            return;
        }
        for block in start..start.saturating_add(count).min(self.total_blocks) {
            if !self.block_allocated(block) {
                continue;
            }
            self.mark(block, false);
            let index = self.group_index(block);
            if let Some(group) = self.group_for(block) {
                group.free = group.free.saturating_add(1);
                group.hint = group.hint.min(block);
            }
            if let Some(index) = index {
                self.group_hint = index;
            }
            self.next_hint = self.next_hint.min(block);
        }
    }

    pub(crate) fn block_allocated(&self, block: u64) -> bool {
        if block >= self.total_blocks {
            return false;
        }
        let index = block as usize;
        self.bitmap[index / 8] & (1 << (index % 8)) != 0
    }

    pub(crate) fn set_allocated(&mut self, block: u64, allocated: bool) {
        if self.block_allocated(block) == allocated {
            return;
        }
        self.mark(block, allocated);
        if let Some(group) = self.group_for(block) {
            if allocated {
                group.free = group.free.saturating_sub(1);
            } else {
                group.free = group.free.saturating_add(1);
                group.hint = group.hint.min(block);
            }
        }
    }

    fn rebuild_groups(&mut self) {
        self.groups.clear();
        let mut start = self.data_start;
        while start < self.total_blocks {
            let end = (start + GROUP_BLOCKS).min(self.total_blocks);
            let mut free = 0;
            let mut hint = end;
            for block in start..end {
                if !self.block_allocated(block) {
                    free += 1;
                    if hint == end {
                        hint = block;
                    }
                }
            }
            self.groups.push(Group {
                start,
                end,
                free,
                hint,
            });
            start = end;
        }
        self.group_hint = 0;
    }

    fn find_run(&self, index: usize, count: u64) -> Option<u64> {
        let group = self.groups.get(index)?;
        let mut run = 0;
        let mut start = 0;
        for block in group.start..group.end {
            if self.block_allocated(block) {
                run = 0;
            } else {
                if run == 0 {
                    start = block;
                }
                run += 1;
                if run == count {
                    return Some(start);
                }
            }
        }
        None
    }

    fn allocate_run(&mut self, start: u64, count: u64) {
        let end = start.saturating_add(count).min(self.total_blocks);
        for block in start..end {
            if self.block_allocated(block) {
                continue;
            }
            self.mark(block, true);
            if let Some(group) = self.group_for(block) {
                group.free = group.free.saturating_sub(1);
                if group.hint == block {
                    group.hint = block.saturating_add(1).min(group.end);
                }
            }
        }
        self.next_hint = end;
    }

    fn group_for(&mut self, block: u64) -> Option<&mut Group> {
        let index = self.group_index(block)?;
        self.groups.get_mut(index)
    }

    fn group_index(&self, block: u64) -> Option<usize> {
        let offset = block.checked_sub(self.data_start)?;
        let index = usize::try_from(offset / GROUP_BLOCKS).ok()?;
        (index < self.groups.len()).then_some(index)
    }

    fn mark(&mut self, block: u64, allocated: bool) {
        if block >= self.total_blocks {
            return;
        }
        let index = block as usize;
        let mask = 1 << (index % 8);
        if allocated {
            self.bitmap[index / 8] |= mask;
        } else {
            self.bitmap[index / 8] &= !mask;
        }
    }

    pub(crate) fn write_bitmap(&mut self) -> Result<(), HolyError> {
        for index in 0..self.bitmap_blocks as usize {
            let mut block = [0_u8; BLOCK_SIZE];
            let start = index * BLOCK_SIZE;
            let end = start + BLOCK_SIZE;
            if end <= self.bitmap.len() {
                block.copy_from_slice(&self.bitmap[start..end]);
            } else {
                let available = self.bitmap.len().saturating_sub(start);
                block[..available].copy_from_slice(&self.bitmap[start..start + available]);
            }
            self.write_block(self.bitmap_start + index as u64, &block)?;
        }
        Ok(())
    }

    pub(crate) fn flush(&mut self) -> Result<(), HolyError> {
        self.device.flush().map_err(map_io)
    }

    pub(crate) fn into_device(self) -> D {
        self.device
    }
}

pub(crate) fn bitmap_blocks_for(total_blocks: u64) -> Result<u32, HolyError> {
    let blocks = total_blocks.div_ceil((BLOCK_SIZE * 8) as u64);
    u32::try_from(blocks.max(1)).map_err(|_| HolyError::NoSpace)
}

#[cfg(test)]
mod tests {
    use super::{GROUP_BLOCKS, Store};
    use hk_block::{MemoryDisk, SECTOR_SIZE};

    #[test]
    fn allocates_contiguous_runs_and_reuses_freed_space() {
        let mut store = Store::format(MemoryDisk::new(32768, SECTOR_SIZE)).expect("格式");
        let start = store.allocate_blocks(8).expect("分配");
        for block in start..start + 8 {
            assert!(store.block_allocated(block));
        }
        store.free_blocks(start, 8);
        assert_eq!(store.allocate_blocks(8).expect("复用"), start);
    }

    #[test]
    fn spans_groups_when_current_group_is_full() {
        let mut store = Store::format(MemoryDisk::new(32768, SECTOR_SIZE)).expect("格式");
        let first = store.allocate_blocks(GROUP_BLOCKS - 2).expect("第一组");
        let second = store.allocate_blocks(4).expect("跨组");
        assert!(second >= first + GROUP_BLOCKS - 2);
    }
}
