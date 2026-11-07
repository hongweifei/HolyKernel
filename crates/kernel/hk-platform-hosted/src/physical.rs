//! 模拟物理内存。

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use hk_hal::{PAGE_SIZE, PhysAddr, PhysRange, PhysicalAccess, PhysicalError};

/// Hosted 平台的物理内存。
///
/// 页按需分配并初始化为 0，模拟上电后的 RAM；越界访问返回
/// [`PhysicalError::OutOfRange`]。
#[derive(Debug)]
pub struct HostedMemory {
    region: PhysRange,
    pages: Mutex<BTreeMap<u64, Box<[u8; PAGE_SIZE]>>>,
}

impl HostedMemory {
    /// 以物理区域创建模拟内存。
    pub fn new(region: PhysRange) -> Self {
        Self {
            region,
            pages: Mutex::new(BTreeMap::new()),
        }
    }

    /// 返回模拟内存覆盖的物理区域。
    pub fn region(&self) -> PhysRange {
        self.region
    }

    /// 返回已实体化的页数。
    pub fn materialized_pages(&self) -> usize {
        self.pages.lock().expect("物理内存锁中毒").len()
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<u64, Box<[u8; PAGE_SIZE]>>> {
        self.pages.lock().expect("物理内存锁中毒")
    }

    fn check(&self, address: PhysAddr, len: usize) -> Result<(), PhysicalError> {
        let end = address
            .get()
            .checked_add(len as u64)
            .ok_or(PhysicalError::OutOfRange)?;
        if len == 0 || end > self.region.end().get() {
            return Err(PhysicalError::OutOfRange);
        }
        Ok(())
    }
}

impl PhysicalAccess for HostedMemory {
    fn read(&self, address: PhysAddr, buffer: &mut [u8]) -> Result<(), PhysicalError> {
        self.check(address, buffer.len())?;

        let mut done = 0;
        while done < buffer.len() {
            let current = address.get() + done as u64;
            let page = current / PAGE_SIZE as u64;
            let offset = (current % PAGE_SIZE as u64) as usize;
            let chunk = core::cmp::min(buffer.len() - done, PAGE_SIZE - offset);

            let mut pages = self.lock();
            let data = pages
                .entry(page)
                .or_insert_with(|| Box::new([0_u8; PAGE_SIZE]));
            buffer[done..done + chunk].copy_from_slice(&data[offset..offset + chunk]);
            done += chunk;
        }
        Ok(())
    }

    fn write(&self, address: PhysAddr, bytes: &[u8]) -> Result<(), PhysicalError> {
        self.check(address, bytes.len())?;

        let mut done = 0;
        while done < bytes.len() {
            let current = address.get() + done as u64;
            let page = current / PAGE_SIZE as u64;
            let offset = (current % PAGE_SIZE as u64) as usize;
            let chunk = core::cmp::min(bytes.len() - done, PAGE_SIZE - offset);

            let mut pages = self.lock();
            let data = pages
                .entry(page)
                .or_insert_with(|| Box::new([0_u8; PAGE_SIZE]));
            data[offset..offset + chunk].copy_from_slice(&bytes[done..done + chunk]);
            done += chunk;
        }
        Ok(())
    }

    fn copy(&self, from: PhysAddr, to: PhysAddr, len: usize) -> Result<(), PhysicalError> {
        self.check(from, len)?;
        self.check(to, len)?;

        let mut buffer = [0_u8; PAGE_SIZE];
        let mut done = 0;
        while done < len {
            let chunk = core::cmp::min(PAGE_SIZE, len - done);
            self.read(from.offset(done as u64), &mut buffer[..chunk])?;
            self.write(to.offset(done as u64), &buffer[..chunk])?;
            done += chunk;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> HostedMemory {
        HostedMemory::new(PhysRange::new(
            PhysAddr::new(0x10_0000),
            4 * PAGE_SIZE as u64,
        ))
    }

    #[test]
    fn reads_zero_filled_memory() {
        let memory = memory();
        let mut buffer = [0xFF_u8; 16];

        memory
            .read(PhysAddr::new(0x10_0000), &mut buffer)
            .expect("读取成功");
        assert_eq!(buffer, [0_u8; 16]);
        assert_eq!(memory.materialized_pages(), 1);
    }

    #[test]
    fn writes_across_pages() {
        let memory = memory();
        let bytes = [0x5A_u8; 16];
        let address = PhysAddr::new(0x10_0FF8);

        memory.write(address, &bytes).expect("写入成功");

        let mut buffer = [0_u8; 16];
        memory.read(address, &mut buffer).expect("读取成功");
        assert_eq!(buffer, bytes);
        assert_eq!(memory.materialized_pages(), 2);
    }

    #[test]
    fn copies_between_pages() {
        let memory = memory();
        memory
            .write(PhysAddr::new(0x10_0000), &[0x42; 64])
            .expect("写入成功");

        memory
            .copy(PhysAddr::new(0x10_0000), PhysAddr::new(0x10_2000), 64)
            .expect("复制成功");

        let mut buffer = [0_u8; 64];
        memory
            .read(PhysAddr::new(0x10_2000), &mut buffer)
            .expect("读取成功");
        assert_eq!(buffer, [0x42; 64]);
    }

    #[test]
    fn rejects_out_of_range_access() {
        let memory = memory();
        let mut buffer = [0_u8; 16];

        assert_eq!(
            memory.read(PhysAddr::new(0x20_0000), &mut buffer),
            Err(PhysicalError::OutOfRange)
        );
        assert_eq!(
            memory.write(PhysAddr::new(0x10_3FF8), &[0_u8; 64]),
            Err(PhysicalError::OutOfRange)
        );
    }
}
