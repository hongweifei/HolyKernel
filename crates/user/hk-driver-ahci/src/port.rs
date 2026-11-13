//! AHCI 端口发现、命令槽与块传输。

use hk_block::{BlockError, SECTOR_SIZE};

use crate::command::{self, ATA_IDENTIFY, ATA_READ_DMA_EXT, ATA_WRITE_DMA_EXT};
use crate::dma::{self, DmaArena, SLOTS};
use crate::hba::Hba;

#[derive(Clone, Copy)]
pub struct Slot {
    pub token: u64,
    pub len: usize,
    pub in_use: bool,
}

const EMPTY_SLOT: Slot = Slot {
    token: 0,
    len: 0,
    in_use: false,
};

/// 已初始化的 AHCI 端口。
pub struct Port {
    index: u16,
    capacity: u64,
    slots: [Slot; SLOTS],
    next_token: u64,
}

impl Port {
    /// 枚举并初始化第一个 SATA 磁盘端口。
    pub fn probe(hba: &Hba, arena: &DmaArena) -> Option<Self> {
        let index = Self::find_disk(hba)?;
        hba.start_port(index, arena.command_list_phys(), arena.fis_phys())?;
        let mut port = Self {
            index,
            capacity: 0,
            slots: [EMPTY_SLOT; SLOTS],
            next_token: 1,
        };
        port.capacity = port.identify(hba, arena)?;
        (port.capacity > 0).then_some(port)
    }

    fn find_disk(hba: &Hba) -> Option<u16> {
        let implemented = hba.implemented_ports();
        (0..=hba.max_port()).find(|&index| implemented & (1 << index) != 0 && hba.is_disk(index))
    }

    /// 端口号。
    pub fn index(&self) -> u16 {
        self.index
    }

    /// 磁盘容量（512 字节块数）。
    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    fn identify(&mut self, hba: &Hba, arena: &DmaArena) -> Option<u64> {
        let fis = command::cfis(ATA_IDENTIFY, 0, 1);
        command::install(
            arena.command_list_virt(),
            arena.table_virt(0),
            arena.table_phys(0),
            &fis,
            arena.data_phys(),
            512,
        )?;
        hba.issue(self.index, 0);
        self.wait_slot(hba, 0)?;
        // SAFETY: IDENTIFY 数据位于已映射的 DMA 数据区。
        let data = unsafe { core::slice::from_raw_parts(arena.data_virt() as *const u8, 512) };
        let word = |index: usize| u16::from_le_bytes([data[index * 2], data[index * 2 + 1]]) as u64;
        let lba48 = word(100) | (word(101) << 16) | (word(102) << 32) | (word(103) << 48);
        let lba28 = word(60) | (word(61) << 16);
        self.slots[0] = EMPTY_SLOT;
        Some(if lba48 != 0 { lba48 } else { lba28 })
    }

    fn wait_slot(&self, hba: &Hba, slot: usize) -> Option<()> {
        for _ in 0..10_000_000 {
            if hba.commands_issued(self.index) & (1 << slot) == 0 {
                return (!hba.port_error(self.index)).then_some(());
            }
            core::hint::spin_loop();
        }
        None
    }

    /// 提交一次读取或写入；返回异步 token。
    pub fn submit(
        &mut self,
        hba: &Hba,
        arena: &DmaArena,
        block: u64,
        buffer: &[u8],
        write: bool,
    ) -> Result<u64, BlockError> {
        if buffer.is_empty() || !buffer.len().is_multiple_of(SECTOR_SIZE) {
            return Err(BlockError::InvalidLength);
        }
        let sectors = buffer.len() / SECTOR_SIZE;
        if sectors > u16::MAX as usize
            || block
                .checked_add(sectors as u64)
                .is_none_or(|end| end > self.capacity)
        {
            return Err(BlockError::OutOfRange);
        }
        let Some(phys) = dma::translate(buffer.as_ptr() as u64, buffer.len()) else {
            return Err(BlockError::Io);
        };
        let Some(slot) = self.slots.iter().position(|slot| !slot.in_use) else {
            return Err(BlockError::Busy);
        };
        let command = if write {
            ATA_WRITE_DMA_EXT
        } else {
            ATA_READ_DMA_EXT
        };
        let fis = command::cfis(command, block, sectors as u16);
        command::install(
            arena.command_list_virt() + (slot * 32) as u64,
            arena.table_virt(slot),
            arena.table_phys(slot),
            &fis,
            phys,
            buffer.len(),
        )
        .ok_or(BlockError::Io)?;
        let token = self.next_token;
        self.next_token = self.next_token.wrapping_add(1);
        self.slots[slot] = Slot {
            token,
            len: buffer.len(),
            in_use: true,
        };
        hba.issue(self.index, slot);
        Ok(token)
    }

    /// 回收一个已完成命令；没有完成时返回 `None`。
    pub fn poll_complete(&mut self, hba: &Hba) -> Option<(u64, Result<usize, BlockError>)> {
        let issued = hba.commands_issued(self.index);
        let failed = hba.port_error(self.index);
        for (slot, cell) in self.slots.iter_mut().enumerate() {
            if !cell.in_use || issued & (1 << slot) != 0 {
                continue;
            }
            let entry = *cell;
            *cell = EMPTY_SLOT;
            let result = if failed {
                Err(BlockError::Io)
            } else {
                Ok(entry.len)
            };
            return Some((entry.token, result));
        }
        None
    }
}
