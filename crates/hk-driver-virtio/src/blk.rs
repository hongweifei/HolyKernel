//! virtio-blk 设备封装。

use hk_block::{BlockDevice, BlockError};
use spin::Mutex;
use virtio_drivers::device::blk::{SECTOR_SIZE, VirtIOBlk};
use virtio_drivers::transport::pci::PciTransport;

use crate::hal::HalImpl;

/// virtio 块设备。
pub struct Block {
    block: Mutex<VirtIOBlk<HalImpl, PciTransport>>,
}

impl Block {
    pub(crate) fn new(block: VirtIOBlk<HalImpl, PciTransport>) -> Self {
        Self {
            block: Mutex::new(block),
        }
    }
}

impl BlockDevice for Block {
    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn capacity_blocks(&self) -> u64 {
        self.block.lock().capacity()
    }

    fn read_blocks(&self, block: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        self.block
            .lock()
            .read_blocks(block as usize, buffer)
            .map_err(|_| BlockError::Io)?;
        Ok(buffer.len())
    }

    fn write_blocks(&mut self, block: u64, data: &[u8]) -> Result<usize, BlockError> {
        self.block
            .lock()
            .write_blocks(block as usize, data)
            .map_err(|_| BlockError::Io)?;
        Ok(data.len())
    }

    fn flush(&mut self) -> Result<(), BlockError> {
        self.block.lock().flush().map_err(|_| BlockError::Io)
    }
}
