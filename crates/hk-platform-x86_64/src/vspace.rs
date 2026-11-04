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
}

/// 全局用户页表登记表。
pub static USER_VSPACES: VspaceRegistry<PageTable> = VspaceRegistry::new();
