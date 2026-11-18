//! VT-d 第二级页表。
//!
//! 只负责 IOVA 到物理页的 4 级表构建；表页由 [`TableMemory`] 提供，便于在
//! 单元测试中使用独立内存，在平台上使用物理帧银行。

use hk_hal::{IommuError, IommuFlags};
use intel_iommu::pagetables::{Perm, SecondStagePte, Stage, index_at, walk};

/// VT-d 第二级页表的 4 KiB 页大小。
pub const PAGE_SIZE: u64 = 4096;

/// 页表所在的物理内存访问接口。
pub trait TableMemory {
    /// 分配一页已清零的物理页。
    fn allocate_table(&mut self) -> Option<u64>;

    /// 读取物理地址处的一个 64 位表项。
    fn read(&self, physical: u64) -> u64;

    /// 写入物理地址处的一个 64 位表项。
    fn write(&mut self, physical: u64, value: u64);
}

/// 一棵第二级页表。
pub struct SecondStage {
    root: u64,
}

impl SecondStage {
    /// 分配根表。
    pub fn new(memory: &mut impl TableMemory) -> Option<Self> {
        Some(Self {
            root: memory.allocate_table()?,
        })
    }

    /// 根表物理地址。
    pub fn root(&self) -> u64 {
        self.root
    }

    /// 映射一个页对齐区间。
    pub fn map(
        &self,
        memory: &mut impl TableMemory,
        iova: u64,
        physical: u64,
        len: u64,
        flags: IommuFlags,
    ) -> Result<(), IommuError> {
        if len == 0
            || !iova.is_multiple_of(PAGE_SIZE)
            || !physical.is_multiple_of(PAGE_SIZE)
            || !len.is_multiple_of(PAGE_SIZE)
        {
            return Err(IommuError::Unaligned);
        }
        let perms = match flags {
            flags if flags.contains(IommuFlags::READ | IommuFlags::WRITE) => Perm::RW,
            flags if flags.contains(IommuFlags::READ) => Perm::R,
            _ => return Err(IommuError::Unsupported),
        };
        let pages = len / PAGE_SIZE;
        for page in 0..pages {
            let va = iova + page * PAGE_SIZE;
            let pa = physical + page * PAGE_SIZE;
            self.map_page(memory, va, pa, perms)?;
        }
        Ok(())
    }

    fn map_page(
        &self,
        memory: &mut impl TableMemory,
        iova: u64,
        physical: u64,
        perms: Perm,
    ) -> Result<(), IommuError> {
        let mut table = self.root;
        for level in (2..=4).rev() {
            let index = index_at(iova, level);
            let entry = table + index as u64 * 8;
            let value = memory.read(entry);
            if value & 1 == 0 {
                let next = memory.allocate_table().ok_or(IommuError::OutOfMemory)?;
                memory.write(entry, SecondStagePte::table(next).bits());
                table = next;
            } else if value & (1 << 7) != 0 {
                return Err(IommuError::AlreadyMapped);
            } else {
                table = value & 0x000f_ffff_ffff_f000;
            }
        }
        let index = index_at(iova, 1);
        let entry = table + index as u64 * 8;
        if memory.read(entry) & 1 != 0 {
            return Err(IommuError::AlreadyMapped);
        }
        memory.write(entry, SecondStagePte::page(physical, perms).bits());
        Ok(())
    }

    /// 解除一个页对齐区间的映射。
    pub fn unmap(
        &self,
        memory: &mut impl TableMemory,
        iova: u64,
        len: u64,
    ) -> Result<(), IommuError> {
        if len == 0 || !iova.is_multiple_of(PAGE_SIZE) || !len.is_multiple_of(PAGE_SIZE) {
            return Err(IommuError::Unaligned);
        }
        let pages = len / PAGE_SIZE;
        for page in 0..pages {
            let va = iova + page * PAGE_SIZE;
            let mut table = self.root;
            let mut present = true;
            for level in (2..=4).rev() {
                let index = index_at(va, level);
                let entry = table + index as u64 * 8;
                let value = memory.read(entry);
                if value & 1 == 0 {
                    present = false;
                    break;
                }
                table = value & 0x000f_ffff_ffff_f000;
            }
            if !present {
                return Err(IommuError::NotMapped);
            }
            let index = index_at(va, 1);
            let entry = table + index as u64 * 8;
            if memory.read(entry) & 1 == 0 {
                return Err(IommuError::NotMapped);
            }
            memory.write(entry, 0);
        }
        Ok(())
    }

    /// 查询一个 IOVA 的物理页与权限。
    #[allow(dead_code)]
    pub fn translate(&self, memory: &impl TableMemory, iova: u64) -> Option<(u64, IommuFlags)> {
        let result = walk(Stage::Second, 4, self.root, iova, &|entry| {
            memory.read(entry)
        })
        .ok()?;
        let flags = if result.perms.contains(Perm::W) {
            IommuFlags::READ | IommuFlags::WRITE
        } else if result.perms.contains(Perm::R) {
            IommuFlags::READ
        } else {
            return None;
        };
        Some((result.physical & !(PAGE_SIZE - 1), flags))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    struct TestMemory {
        pages: Vec<Vec<u64>>,
    }

    impl TestMemory {
        fn new() -> Self {
            Self { pages: Vec::new() }
        }

        fn physical(&self, index: usize) -> u64 {
            (index as u64 + 1) * PAGE_SIZE
        }

        fn index(&self, physical: u64) -> usize {
            (physical / PAGE_SIZE) as usize - 1
        }
    }

    impl TableMemory for TestMemory {
        fn allocate_table(&mut self) -> Option<u64> {
            self.pages.push(vec![0; 512]);
            Some(self.physical(self.pages.len() - 1))
        }

        fn read(&self, physical: u64) -> u64 {
            let page = self.index(physical & !(PAGE_SIZE - 1));
            self.pages[page][((physical & (PAGE_SIZE - 1)) / 8) as usize]
        }

        fn write(&mut self, physical: u64, value: u64) {
            let page = self.index(physical & !(PAGE_SIZE - 1));
            self.pages[page][((physical & (PAGE_SIZE - 1)) / 8) as usize] = value;
        }
    }

    #[test]
    fn maps_translates_and_unmaps() {
        let mut memory = TestMemory::new();
        let table = SecondStage::new(&mut memory).expect("root");
        table
            .map(
                &mut memory,
                0x1_0000,
                0x20_0000,
                0x2000,
                IommuFlags::READ | IommuFlags::WRITE,
            )
            .expect("map");
        assert_eq!(
            table.translate(&memory, 0x1_0100),
            Some((0x20_0000, IommuFlags::READ | IommuFlags::WRITE))
        );
        assert_eq!(
            table.translate(&memory, 0x1_1100),
            Some((0x20_1000, IommuFlags::READ | IommuFlags::WRITE))
        );
        assert_eq!(
            table.map(
                &mut memory,
                0x1_0000,
                0x30_0000,
                PAGE_SIZE,
                IommuFlags::READ
            ),
            Err(IommuError::AlreadyMapped)
        );
        table.unmap(&mut memory, 0x1_0000, 0x2000).expect("unmap");
        assert_eq!(table.translate(&memory, 0x1_0000), None);
        assert_eq!(
            table.unmap(&mut memory, 0x1_0000, PAGE_SIZE),
            Err(IommuError::NotMapped)
        );
    }
}
