//! 每任务用户页表登记、切换与写时复制分叉。

use alloc::vec::Vec;

use spin::Mutex;

use crate::paging::PageTable;

/// 任务槽位到用户页表的映射。
pub struct UserVspaces {
    tables: Mutex<Vec<Option<PageTable>>>,
}

impl UserVspaces {
    /// 构造空映射。
    pub const fn new() -> Self {
        Self {
            tables: Mutex::new(Vec::new()),
        }
    }

    /// 登记任务槽位的页表。
    pub fn install(&self, slot: usize, table: PageTable) {
        let mut tables = self.tables.lock();
        if tables.len() <= slot {
            tables.resize_with(slot + 1, || None);
        }
        tables[slot] = Some(table);
    }

    /// 切换到任务槽位的页表。
    pub fn activate(&self, slot: usize) {
        let tables = self.tables.lock();
        if let Some(table) = tables.get(slot).and_then(Option::as_ref) {
            table.activate();
        }
    }

    /// 返回任务槽位的页表根；不存在时为 0。
    pub fn root(&self, slot: usize) -> u64 {
        self.tables
            .lock()
            .get(slot)
            .and_then(Option::as_ref)
            .map_or(0, |table| table.root().get())
    }

    /// 把父任务页表分叉为子任务写时复制副本。
    pub fn fork(&self, parent: usize, child: usize) -> bool {
        let mut tables = self.tables.lock();
        let child_table = {
            let Some(table) = tables.get(parent).and_then(Option::as_ref) else {
                return false;
            };
            let Some(table) = table.fork_user() else {
                return false;
            };
            table
        };

        if tables.len() <= child {
            tables.resize_with(child + 1, || None);
        }
        tables[child] = Some(child_table);
        true
    }
}

impl Default for UserVspaces {
    fn default() -> Self {
        Self::new()
    }
}

/// 全局用户页表登记表。
pub static USER_VSPACES: UserVspaces = UserVspaces::new();
