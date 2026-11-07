//! 每任务用户页表登记、切换与写时复制分叉。

use hk_mem::{UserVspace, VspaceRegistry};

use crate::paging::PageTable;

impl UserVspace for PageTable {
    fn activate(&self) {
        PageTable::activate(self);
    }

    fn root(&self) -> u64 {
        PageTable::root(self).get()
    }

    fn fork_user(&self) -> Option<Self> {
        PageTable::fork_user(self)
    }

    fn share_user(&self) -> Option<Self> {
        Some(PageTable::share(self))
    }

    fn map_mmio(&self, virt: u64, phys: u64, len: usize) -> bool {
        self.map_user_range(virt, phys, len, crate::paging::PageFlags::USER_MMIO)
    }

    fn map_dma(&self, virt: u64, phys: u64, len: usize) -> bool {
        self.map_user_range(virt, phys, len, crate::paging::PageFlags::USER_RW)
    }

    fn unmap_dma(&self, virt: u64, len: usize) -> bool {
        self.unmap_user_range(hk_hal::VirtAddr::new(virt), len)
    }
}

impl PageTable {
    fn map_user_range(
        &self,
        virt: u64,
        phys: u64,
        len: usize,
        flags: crate::paging::PageFlags,
    ) -> bool {
        if len == 0
            || !virt.is_multiple_of(hk_hal::PAGE_SIZE as u64)
            || !phys.is_multiple_of(hk_hal::PAGE_SIZE as u64)
        {
            return false;
        }
        let mut offset = 0;
        while offset < len {
            if self
                .map_mmio(
                    hk_hal::VirtAddr::new(virt + offset as u64),
                    hk_hal::PhysAddr::new(phys + offset as u64),
                    flags,
                )
                .is_err()
            {
                return false;
            }
            offset += hk_hal::PAGE_SIZE;
        }
        true
    }
}

/// 全局用户页表登记表。
pub static USER_VSPACES: VspaceRegistry<PageTable> = VspaceRegistry::new();
