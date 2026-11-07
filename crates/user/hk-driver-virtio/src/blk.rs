//! virtio-blk 设备封装。
//!
//! 提交走 `read_blocks_nb` / `write_blocks_nb`，完成由 [`Block::poll_complete`]
//! 从 used ring 回收；驱动不在等待设备上自旋，同一队列可以同时挂多条请求。

use alloc::boxed::Box;
use alloc::vec::Vec;

use hk_block::{AsyncBlockDevice, BlockDevice, BlockError};
use hk_device::DeviceGrant;
use spin::Mutex;
use virtio_drivers::device::blk::{BlkReq, BlkResp, SECTOR_SIZE, VirtIOBlk};
use virtio_drivers::transport::pci::PciTransport;

use crate::hal::{self, HalImpl};

/// 一条已提交、等待设备完成的请求。
struct Pending {
    token: u16,
    request: Box<BlkReq>,
    response: Box<BlkResp>,
    buffer: *mut u8,
    length: usize,
    write: bool,
}

/// virtio 块设备。
pub struct Block {
    block: Mutex<VirtIOBlk<HalImpl, PciTransport>>,
    pending: Mutex<Vec<Pending>>,
    grant: u64,
    descriptor: DeviceGrant,
}

impl Block {
    pub(crate) fn new(block: VirtIOBlk<HalImpl, PciTransport>) -> Self {
        Self {
            block: Mutex::new(block),
            pending: Mutex::new(Vec::new()),
            grant: hal::grant_handle(),
            descriptor: hal::grant_descriptor().expect("probe 未绑定设备授权"),
        }
    }

    /// 切回本设备的授权；同一进程里的多个设备共享 HAL 状态。
    fn activate(&self) {
        hal::bind(self.grant, self.descriptor);
    }

    /// 启用设备中断；完成时设备会向授权 IRQ 线发信号。
    pub fn enable_interrupts(&self) {
        self.activate();
        self.block.lock().enable_interrupts();
    }

    fn submit(
        &mut self,
        block: u64,
        buffer: *mut u8,
        length: usize,
        write: bool,
    ) -> Result<u64, BlockError> {
        if length == 0 || !length.is_multiple_of(SECTOR_SIZE) {
            return Err(BlockError::InvalidLength);
        }
        self.activate();
        let mut request = Box::new(BlkReq::default());
        let mut response = Box::new(BlkResp::default());
        let token = {
            let mut device = self.block.lock();
            // SAFETY: 缓冲区在对应完成被回收前由调用方保持有效且不被访问。
            let result = unsafe {
                let data = core::slice::from_raw_parts_mut(buffer, length);
                if write {
                    device.write_blocks_nb(block as usize, &mut request, data, &mut response)
                } else {
                    device.read_blocks_nb(block as usize, &mut request, data, &mut response)
                }
            };
            result.map_err(map_error)?
        };
        self.pending.lock().push(Pending {
            token,
            request,
            response,
            buffer,
            length,
            write,
        });
        Ok(u64::from(token))
    }
}

impl BlockDevice for Block {
    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn capacity_blocks(&self) -> u64 {
        self.activate();
        self.block.lock().capacity()
    }

    fn read_blocks(&self, block: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        self.activate();
        self.block
            .lock()
            .read_blocks(block as usize, buffer)
            .map_err(map_error)?;
        Ok(buffer.len())
    }

    fn write_blocks(&mut self, block: u64, data: &[u8]) -> Result<usize, BlockError> {
        self.activate();
        self.block
            .lock()
            .write_blocks(block as usize, data)
            .map_err(map_error)?;
        Ok(data.len())
    }

    fn flush(&mut self) -> Result<(), BlockError> {
        self.activate();
        self.block.lock().flush().map_err(map_error)
    }
}

impl AsyncBlockDevice for Block {
    fn submit_read(&mut self, block: u64, buffer: &mut [u8]) -> Result<u64, BlockError> {
        let pointer = buffer.as_mut_ptr();
        let length = buffer.len();
        self.submit(block, pointer, length, false)
    }

    fn submit_write(&mut self, block: u64, data: &[u8]) -> Result<u64, BlockError> {
        let pointer = data.as_ptr().cast_mut();
        let length = data.len();
        self.submit(block, pointer, length, true)
    }

    fn poll_complete(&mut self) -> Option<(u64, Result<usize, BlockError>)> {
        self.activate();
        let token = self.block.lock().peek_used()?;
        let mut pending = self.pending.lock();
        let index = pending.iter().position(|entry| entry.token == token)?;
        let mut entry = pending.swap_remove(index);
        drop(pending);

        let mut device = self.block.lock();
        // SAFETY: `entry.buffer` 与 `entry.request`/`entry.response` 正是提交时传入的同一组
        // 缓冲区，长度一致，且在本次完成回收前没有被访问。
        let result = unsafe {
            let data = core::slice::from_raw_parts_mut(entry.buffer, entry.length);
            if entry.write {
                device.complete_write_blocks(token, &entry.request, data, &mut entry.response)
            } else {
                device.complete_read_blocks(token, &entry.request, data, &mut entry.response)
            }
        };
        let result = match result {
            Ok(()) => Ok(entry.length),
            Err(error) => Err(map_error(error)),
        };
        Some((u64::from(token), result))
    }

    fn ack_interrupt(&mut self) {
        self.activate();
        self.block.lock().ack_interrupt();
    }
}

fn map_error(error: virtio_drivers::Error) -> BlockError {
    match error {
        virtio_drivers::Error::QueueFull => BlockError::Busy,
        _ => BlockError::Io,
    }
}
