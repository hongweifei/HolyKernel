//! AHCI/SATA 块设备驱动。
//!
//! 代码按 HBA 寄存器、DMA 布局、命令构造与端口状态拆分；命令完成支持中断
//! 与轮询两种方式，PRDT 直接引用 Block 服务登记的 Frame。

#![cfg_attr(target_os = "none", no_std)]

extern crate alloc;

mod command;
mod dma;
mod hba;
mod port;

use hk_block::{AsyncBlockDevice, BlockDevice, BlockError, SECTOR_SIZE};
use hk_device::DeviceGrant;
use hk_server_api::device_map;

pub use dma::register_mapping;

use hba::Hba;
use port::Port;

/// AHCI 块设备。
pub struct Ahci {
    hba: Hba,
    arena: dma::DmaArena,
    port: spin::Mutex<Port>,
}

impl Ahci {
    /// 从 DeviceGrant 探测 AHCI 控制器与第一个 SATA 磁盘。
    pub fn probe(grant: u64, descriptor: &DeviceGrant) -> Option<Self> {
        let (index, _) = descriptor
            .mmio
            .iter()
            .enumerate()
            .find(|(_, range)| range.is_some())?;
        let mmio = device_map(grant, index as u64)?;
        let arena = dma::DmaArena::allocate(grant, dma::DMA_BYTES)?;
        let hba = Hba::new(mmio);
        hba.reset();
        let port = Port::probe(&hba, &arena)?;
        Some(Self {
            hba,
            arena,
            port: spin::Mutex::new(port),
        })
    }

    /// 打开端口中断；中断线由服务通过 `irq_bind` 绑定。
    pub fn enable_interrupts(&self) {
        let port = self.port.lock();
        self.hba.enable_interrupts(port.index());
    }

    /// 返回磁盘容量（512 字节块）。
    pub fn capacity_blocks(&self) -> u64 {
        self.port.lock().capacity()
    }

    fn submit(&self, block: u64, buffer: &[u8], write: bool) -> Result<u64, BlockError> {
        self.port
            .lock()
            .submit(&self.hba, &self.arena, block, buffer, write)
    }

    fn poll(&self) -> Option<(u64, Result<usize, BlockError>)> {
        self.port.lock().poll_complete(&self.hba)
    }

    fn wait(&self, token: u64) -> Result<usize, BlockError> {
        for _ in 0..10_000_000 {
            if let Some((done, result)) = self.poll()
                && done == token
            {
                return result;
            }
            core::hint::spin_loop();
        }
        Err(BlockError::Io)
    }
}

impl BlockDevice for Ahci {
    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn capacity_blocks(&self) -> u64 {
        self.port.lock().capacity()
    }

    fn read_blocks(&self, block: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        let token = self.submit(block, buffer, false)?;
        self.wait(token)
    }

    fn write_blocks(&mut self, block: u64, data: &[u8]) -> Result<usize, BlockError> {
        let token = self.submit(block, data, true)?;
        self.wait(token)
    }

    fn flush(&mut self) -> Result<(), BlockError> {
        Ok(())
    }
}

impl AsyncBlockDevice for Ahci {
    fn submit_read(&mut self, block: u64, buffer: &mut [u8]) -> Result<u64, BlockError> {
        self.submit(block, buffer, false)
    }

    fn submit_write(&mut self, block: u64, data: &[u8]) -> Result<u64, BlockError> {
        self.submit(block, data, true)
    }

    fn poll_complete(&mut self) -> Option<(u64, Result<usize, BlockError>)> {
        self.poll()
    }

    fn ack_interrupt(&mut self) {
        let index = self.port.lock().index();
        self.hba.acknowledge(index);
    }
}
