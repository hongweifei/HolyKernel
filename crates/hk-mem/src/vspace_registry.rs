//! 用户地址空间登记表：任务槽位到页表根。

use alloc::vec::Vec;

use spin::Mutex;

/// 可登记的用户地址空间。
pub trait UserVspace: Sized {
    /// 切换到该地址空间。
    fn activate(&self);
    /// 返回页表根。
    fn root(&self) -> u64;
    /// 复制为写时复制子地址空间。
    fn fork_user(&self) -> Option<Self>;
    /// 在地址空间中映射一段设备 MMIO；不支持时返回 `false`。
    fn map_mmio(&self, _virt: u64, _phys: u64, _len: usize) -> bool {
        false
    }
    /// 在地址空间中映射一段 DMA 内存；不支持时返回 `false`。
    fn map_dma(&self, _virt: u64, _phys: u64, _len: usize) -> bool {
        false
    }
}

/// 任务槽位到用户地址空间的映射。
pub struct VspaceRegistry<T> {
    tables: Mutex<Vec<Option<T>>>,
}

impl<T> VspaceRegistry<T> {
    /// 构造空登记表。
    pub const fn new() -> Self {
        Self {
            tables: Mutex::new(Vec::new()),
        }
    }
}

impl<T: UserVspace> VspaceRegistry<T> {
    /// 登记任务槽位的地址空间。
    pub fn install(&self, slot: usize, table: T) {
        let mut tables = self.tables.lock();
        if tables.len() <= slot {
            tables.resize_with(slot + 1, || None);
        }
        tables[slot] = Some(table);
    }

    /// 切换到任务槽位的地址空间。
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
            .map_or(0, UserVspace::root)
    }

    /// 把设备 MMIO 映射到任务地址空间。
    pub fn map_mmio(&self, slot: usize, virt: u64, phys: u64, len: usize) -> bool {
        self.tables
            .lock()
            .get(slot)
            .and_then(Option::as_ref)
            .is_some_and(|table| table.map_mmio(virt, phys, len))
    }

    /// 把 DMA 内存映射到任务地址空间。
    pub fn map_dma(&self, slot: usize, virt: u64, phys: u64, len: usize) -> bool {
        self.tables
            .lock()
            .get(slot)
            .and_then(Option::as_ref)
            .is_some_and(|table| table.map_dma(virt, phys, len))
    }

    /// 把父任务地址空间分叉为子任务写时复制副本。
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

impl<T> Default for VspaceRegistry<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockTable(u64);

    impl UserVspace for MockTable {
        fn activate(&self) {}

        fn root(&self) -> u64 {
            self.0
        }

        fn fork_user(&self) -> Option<Self> {
            Some(Self(self.0 + 1))
        }
    }

    #[test]
    fn installs_roots_and_forks() {
        let registry = VspaceRegistry::new();
        registry.install(1, MockTable(0x1000));
        assert_eq!(registry.root(1), 0x1000);
        assert_eq!(registry.root(2), 0);

        assert!(registry.fork(1, 2));
        assert_eq!(registry.root(2), 0x1001);
        assert!(!registry.fork(3, 4));
    }
}
