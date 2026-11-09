//! 每任务用户页表登记、切换与写时复制分叉。

use core::ops::Deref;
use hk_mem::Rights;
use hk_mem::hmp::RegionFlags;

use hk_mem::{UserVspace, VspaceRegistry};
use hk_paging::UserSpace as PagingUserSpace;

use crate::paging::PageTable;

/// 平台侧用户地址空间：在通用包装上实现 `hk-mem` 的登记契约。
pub struct UserSpace(PagingUserSpace<PageTable>);

impl UserSpace {
    /// 包装一个 x86_64 用户页表。
    pub const fn new(table: PageTable) -> Self {
        Self(PagingUserSpace::new(table))
    }
}

impl Deref for UserSpace {
    type Target = PageTable;

    fn deref(&self) -> &Self::Target {
        self.0.backend()
    }
}

impl UserVspace for UserSpace {
    fn activate(&self) {
        PageTable::activate(self.0.backend());
    }

    fn root(&self) -> u64 {
        PageTable::root(self.0.backend()).get()
    }

    fn fork_user(&self) -> Option<Self> {
        PageTable::fork_user(self.0.backend()).map(UserSpace::new)
    }

    fn share_user(&self) -> Option<Self> {
        Some(UserSpace::new(PageTable::share(self.0.backend())))
    }

    fn map_user(
        &self,
        virt: u64,
        phys: u64,
        len: usize,
        rights: Rights,
        flags: RegionFlags,
    ) -> bool {
        PageTable::map_user_range(
            self.0.backend(),
            virt,
            phys,
            len,
            crate::paging::PageFlags::user(rights, flags),
            flags.contains(RegionFlags::COW),
        )
    }

    fn map_mmio(&self, virt: u64, phys: u64, len: usize) -> bool {
        PageTable::map_user_range(
            self.0.backend(),
            virt,
            phys,
            len,
            crate::paging::PageFlags::USER_MMIO,
            false,
        )
    }

    fn unmap_user(&self, virt: u64, len: usize) -> bool {
        PageTable::unmap_user_range(self.0.backend(), hk_hal::VirtAddr::new(virt), len)
    }
}

impl PageTable {
    fn map_user_range(
        &self,
        virt: u64,
        phys: u64,
        len: usize,
        flags: crate::paging::PageFlags,
        cow: bool,
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
            let size = if !cow {
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
pub static USER_VSPACES: VspaceRegistry<UserSpace> = VspaceRegistry::new();
