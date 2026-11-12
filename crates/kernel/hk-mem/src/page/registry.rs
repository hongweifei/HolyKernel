//! 用户地址空间登记表：任务槽位到页表根。

use alloc::sync::Arc;
use alloc::vec::Vec;

use hk_handle::Rights;
use spin::Mutex;

use crate::hmp::{Capacity, RegionFlags};

/// 可登记的用户地址空间。
pub trait UserAddressSpace: Sized {
    /// 切换到该地址空间。
    fn activate(&self);
    /// 返回页表根。
    fn root(&self) -> u64;
    /// 复制为子地址空间；`cow_ranges` 指定需要标 COW 的虚拟区间，
    /// 其余可写页必须复制为私有页。
    /// 复制为写时复制副本；`capacity` 为新私有页的物理来源。
    /// 返回副本与为子地址空间复制私有页的字节数。
    fn fork_user(
        &self,
        cow_ranges: &[(u64, u64)],
        capacity: Option<&Arc<Mutex<Capacity>>>,
    ) -> Option<(Self, u64)>;
    /// 返回同一地址空间的共享句柄；不支持时返回 `None`。
    fn share_user(&self) -> Option<Self> {
        None
    }
    /// 按 Region 权利与标志把一段物理区间映射进地址空间。
    ///
    /// 只读 Region 映射为不可写页，不可执行 Region 映射为 NX 页；能力层
    /// 已校验 W^X，这里只负责把结果落到页表。
    fn map_user(
        &self,
        _virt: u64,
        _phys: u64,
        _len: usize,
        _rights: Rights,
        _flags: RegionFlags,
    ) -> bool {
        false
    }
    /// 在地址空间中映射一段设备 MMIO；不支持时返回 `false`。
    fn map_mmio(&self, _virt: u64, _phys: u64, _len: usize) -> bool {
        false
    }

    /// 解除一段用户映射；不支持时返回 `false`。
    fn unmap_user(&self, _virt: u64, _len: usize) -> bool {
        false
    }
}

/// 任务槽位到用户地址空间的映射。
pub struct AddressSpaceRegistry<T> {
    tables: Mutex<Vec<Option<T>>>,
    /// 每个页表根的持有者数量；共享地址空间的线程共享同一份页表。
    roots: Mutex<Vec<(u64, u32)>>,
}

fn acquire_root(roots: &mut Vec<(u64, u32)>, root: u64) {
    if let Some(entry) = roots.iter_mut().find(|entry| entry.0 == root) {
        entry.1 = entry.1.saturating_add(1);
    } else {
        roots.push((root, 1));
    }
}

fn release_root(roots: &mut Vec<(u64, u32)>, root: u64) -> u32 {
    let Some(index) = roots.iter().position(|entry| entry.0 == root) else {
        return 0;
    };
    let remaining = roots[index].1.saturating_sub(1);
    if remaining == 0 {
        roots.remove(index);
    } else {
        roots[index].1 = remaining;
    }
    remaining
}

impl<T> AddressSpaceRegistry<T> {
    /// 构造空登记表。
    pub const fn new() -> Self {
        Self {
            tables: Mutex::new(Vec::new()),
            roots: Mutex::new(Vec::new()),
        }
    }
}

impl<T: UserAddressSpace> AddressSpaceRegistry<T> {
    /// 登记任务槽位的地址空间。
    pub fn install(&self, slot: usize, table: T) {
        let root = table.root();
        let mut tables = self.tables.lock();
        let old_root = tables
            .get(slot)
            .and_then(Option::as_ref)
            .map(UserAddressSpace::root);
        if tables.len() <= slot {
            tables.resize_with(slot + 1, || None);
        }
        tables[slot] = Some(table);
        drop(tables);
        let mut roots = self.roots.lock();
        if let Some(old_root) = old_root {
            release_root(&mut roots, old_root);
        }
        acquire_root(&mut roots, root);
    }

    /// 切换到任务槽位的地址空间。
    pub fn activate(&self, slot: usize) {
        let tables = self.tables.lock();
        if let Some(table) = tables.get(slot).and_then(Option::as_ref) {
            table.activate();
        }
    }

    /// 取走任务槽位的地址空间；只有最后一个根持有者才返回地址空间本身。
    pub fn take(&self, slot: usize) -> Option<T> {
        let mut tables = self.tables.lock();
        let table = tables.get_mut(slot)?.take()?;
        let root = table.root();
        drop(tables);
        let mut roots = self.roots.lock();
        (release_root(&mut roots, root) == 0).then_some(table)
    }

    /// 借用任务槽位的地址空间。
    pub fn with<R>(&self, slot: usize, inspect: impl FnOnce(&T) -> R) -> Option<R> {
        let tables = self.tables.lock();
        tables.get(slot).and_then(Option::as_ref).map(inspect)
    }

    /// 返回任务槽位的页表根；不存在时为 0。
    pub fn root(&self, slot: usize) -> u64 {
        self.tables
            .lock()
            .get(slot)
            .and_then(Option::as_ref)
            .map_or(0, UserAddressSpace::root)
    }

    /// 把设备 MMIO 映射到任务地址空间。
    pub fn map_mmio(&self, slot: usize, virt: u64, phys: u64, len: usize) -> bool {
        self.tables
            .lock()
            .get(slot)
            .and_then(Option::as_ref)
            .is_some_and(|table| table.map_mmio(virt, phys, len))
    }

    /// 按 Region 权利与标志把一段物理区间映射到任务地址空间。
    pub fn map_user(
        &self,
        slot: usize,
        virt: u64,
        phys: u64,
        len: usize,
        rights: Rights,
        flags: RegionFlags,
    ) -> bool {
        self.tables
            .lock()
            .get(slot)
            .and_then(Option::as_ref)
            .is_some_and(|table| table.map_user(virt, phys, len, rights, flags))
    }

    /// 解除任务地址空间中的一段映射。
    pub fn unmap_user(&self, slot: usize, virt: u64, len: usize) -> bool {
        self.tables
            .lock()
            .get(slot)
            .and_then(Option::as_ref)
            .is_some_and(|table| table.unmap_user(virt, len))
    }

    /// 让子任务与父任务共享同一地址空间。
    pub fn share(&self, parent: usize, child: usize) -> bool {
        let mut tables = self.tables.lock();
        let shared = {
            let Some(table) = tables.get(parent).and_then(Option::as_ref) else {
                return false;
            };
            let Some(shared) = table.share_user() else {
                return false;
            };
            shared
        };
        if tables.len() <= child {
            tables.resize_with(child + 1, || None);
        }
        let root = shared.root();
        let old_root = tables
            .get(child)
            .and_then(Option::as_ref)
            .map(UserAddressSpace::root);
        tables[child] = Some(shared);
        drop(tables);
        let mut roots = self.roots.lock();
        if let Some(old_root) = old_root {
            release_root(&mut roots, old_root);
        }
        acquire_root(&mut roots, root);
        true
    }

    /// 把父任务地址空间分叉为子任务副本。
    ///
    /// `cow_ranges` 来自父任务的 Region 表；范围外的可写页由机制层私有复制。
    pub fn fork(
        &self,
        parent: usize,
        child: usize,
        cow_ranges: &[(u64, u64)],
        capacity: Option<&Arc<Mutex<Capacity>>>,
    ) -> Option<u64> {
        let mut tables = self.tables.lock();
        let (child_table, copied) = {
            let table = tables.get(parent).and_then(Option::as_ref)?;
            table.fork_user(cow_ranges, capacity)?
        };

        if tables.len() <= child {
            tables.resize_with(child + 1, || None);
        }
        let root = child_table.root();
        let old_root = tables
            .get(child)
            .and_then(Option::as_ref)
            .map(UserAddressSpace::root);
        tables[child] = Some(child_table);
        drop(tables);
        let mut roots = self.roots.lock();
        if let Some(old_root) = old_root {
            release_root(&mut roots, old_root);
        }
        acquire_root(&mut roots, root);
        Some(copied)
    }
}

impl<T> Default for AddressSpaceRegistry<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockTable(u64);

    impl UserAddressSpace for MockTable {
        fn activate(&self) {}

        fn root(&self) -> u64 {
            self.0
        }

        fn fork_user(
            &self,
            _cow_ranges: &[(u64, u64)],
            _capacity: Option<&Arc<Mutex<Capacity>>>,
        ) -> Option<(Self, u64)> {
            Some((Self(self.0 + 1), 0))
        }

        fn share_user(&self) -> Option<Self> {
            Some(Self(self.0))
        }
    }

    #[test]
    fn shared_root_is_released_only_by_last_holder() {
        let registry = AddressSpaceRegistry::new();
        registry.install(0, MockTable(0x1000));
        assert!(registry.share(0, 1));
        assert!(registry.take(1).is_none(), "共享根不能提前释放");
        assert!(registry.take(0).is_some(), "最后一个持有者释放根");
    }

    #[test]
    fn installs_roots_and_forks() {
        let registry = AddressSpaceRegistry::new();
        registry.install(1, MockTable(0x1000));
        assert_eq!(registry.root(1), 0x1000);
        assert_eq!(registry.root(2), 0);

        assert!(registry.fork(1, 2, &[], None).is_some());
        assert_eq!(registry.root(2), 0x1001);
        assert!(registry.fork(3, 4, &[], None).is_none());
    }
}
