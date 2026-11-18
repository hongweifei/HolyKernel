//! AMD-Vi v1 主机页表。
//!
//! 与 VT-d 的第二级表类似，但页目录项携带 `NextLevel`，叶子项使用 AMD 的
//! `IR/IW` 权限位。表页同样由 [`TableMemory`] 提供。

use amd_iommu::pagetables::{Format, Perm, index_at, v1_page, v1_table, walk};
use hk_hal::{IommuError, IommuFlags};

use super::second_stage::{PAGE_SIZE, TableMemory};

const PA_MASK: u64 = 0x000f_ffff_ffff_f000;

/// 一棵 AMD-Vi v1 四层主机页表。
pub struct V1PageTable {
    root: u64,
}

impl V1PageTable {
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
        if !flags.contains(IommuFlags::READ) {
            return Err(IommuError::Unsupported);
        }
        let write = flags.contains(IommuFlags::WRITE);
        for page in 0..len / PAGE_SIZE {
            self.map_page(
                memory,
                iova + page * PAGE_SIZE,
                physical + page * PAGE_SIZE,
                write,
            )?;
        }
        Ok(())
    }

    fn map_page(
        &self,
        memory: &mut impl TableMemory,
        iova: u64,
        physical: u64,
        write: bool,
    ) -> Result<(), IommuError> {
        let mut table = self.root;
        for level in (2..=4).rev() {
            let index = index_at(iova, level);
            let entry = table + index as u64 * 8;
            let value = memory.read(entry);
            if value & 1 == 0 {
                let next = memory.allocate_table().ok_or(IommuError::OutOfMemory)?;
                memory.write(entry, v1_table(next, level - 1));
                table = next;
            } else {
                table = value & PA_MASK;
            }
        }
        let index = index_at(iova, 1);
        let entry = table + index as u64 * 8;
        if memory.read(entry) & 1 != 0 {
            return Err(IommuError::AlreadyMapped);
        }
        memory.write(entry, v1_page(physical, true, write));
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
        for page in 0..len / PAGE_SIZE {
            let va = iova + page * PAGE_SIZE;
            let mut table = self.root;
            for level in (2..=4).rev() {
                let index = index_at(va, level);
                let entry = table + index as u64 * 8;
                let value = memory.read(entry);
                if value & 1 == 0 {
                    return Err(IommuError::NotMapped);
                }
                table = value & PA_MASK;
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
    pub fn translate(&self, memory: &impl TableMemory, iova: u64) -> Option<(u64, Perm)> {
        walk(Format::V1, 4, self.root, iova, &|entry| memory.read(entry))
            .ok()
            .map(|result| (result.physical & !(PAGE_SIZE - 1), result.perms))
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
    fn maps_two_pages_with_v1_entries() {
        let mut memory = TestMemory::new();
        let table = V1PageTable::new(&mut memory).expect("root");
        table
            .map(
                &mut memory,
                0x1_0000,
                0x20_0000,
                0x2000,
                IommuFlags::READ | IommuFlags::WRITE,
            )
            .expect("map");
        let result = table.translate(&memory, 0x1_1100).expect("translate");
        assert_eq!(result.0, 0x20_1000);
        assert!(result.1.contains(Perm::IR));
        assert!(result.1.contains(Perm::IW));
        table.unmap(&mut memory, 0x1_0000, 0x2000).expect("unmap");
        assert!(table.translate(&memory, 0x1_0000).is_none());
    }
}
