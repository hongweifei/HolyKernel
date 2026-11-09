//! 每任务用户页表登记、切换与写时复制分叉。

use hk_mem::Rights;
use hk_mem::hmp::RegionFlags;
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

    fn map_user(
        &self,
        virt: u64,
        phys: u64,
        len: usize,
        rights: Rights,
        flags: RegionFlags,
    ) -> bool {
        self.map_user_range(
            virt,
            phys,
            len,
            crate::paging::PageFlags::user(rights, flags),
        )
    }

    fn map_mmio(&self, virt: u64, phys: u64, len: usize) -> bool {
        self.map_user_range(virt, phys, len, crate::paging::PageFlags::USER_MMIO)
    }

    fn unmap_user(&self, virt: u64, len: usize) -> bool {
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
            let virt = virt + offset as u64;
            let phys = phys + offset as u64;
            let remaining = len - offset;
            let size = if !flags.is_cow() {
                if remaining >= (1 << 30)
                    && virt.is_multiple_of(1 << 30)
                    && phys.is_multiple_of(1 << 30)
                    && crate::paging::has_1g_pages()
                {
                    1 << 30
                } else if remaining >= (1 << 21)
                    && virt.is_multiple_of(1 << 21)
                    && phys.is_multiple_of(1 << 21)
                {
                    1 << 21
                } else {
                    hk_hal::PAGE_SIZE as u64
                }
            } else {
                hk_hal::PAGE_SIZE as u64
            };
            if size > hk_hal::PAGE_SIZE as u64
                && self
                    .map_huge(
                        hk_hal::VirtAddr::new(virt),
                        hk_hal::PhysAddr::new(phys),
                        flags,
                        size,
                        &crate::paging::FRAME_BANK,
                    )
                    .is_ok()
            {
                offset += size as usize;
                continue;
            }
            if self
                .map_mmio(
                    hk_hal::VirtAddr::new(virt),
                    hk_hal::PhysAddr::new(phys),
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
