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

/// 任务槽位到用户页表的映射。
pub type UserVspaces = VspaceRegistry<PageTable>;

/// 全局用户页表登记表。
pub static USER_VSPACES: UserVspaces = VspaceRegistry::new();
