//! System Arena：内核内存根对象。
//!
//! 内核自用的地址映射与物理分配都以 Region 登记在这里：内核镜像、direct
//! map、内核堆、per-CPU 栈、MMIO 窗口、页表页与 IOMMU 表。物理页仍由
//! System Capacity/FrameBank 承载，System Arena 只维护对象面，使内核内存
//! 可以像任务 Arena 一样被枚举与报告。
//!
//! Region 只覆盖内核高半区中可表达的映射：可执行区间是 `TaskImage` 能力，
//! 其余是 `Anonymous` 能力；内核 Region 常驻不回收，统一带 `PINNED` 标志。
//! 同时可写可执行的映射按 W^X 契约登记为可写数据视图。

use alloc::vec::Vec;

use hk_hal::{PAGE_SIZE, PhysAddr, PhysRange, VirtAddr};
use hk_mem::hmp::{Arena, Capability, CapabilityKind, Region, RegionFlags, RegionId};
use hk_mem::{FrameBank, Rights};
use spin::Mutex;

use crate::paging::flags::{NX, PCD, PWT, WRITABLE};
use crate::system;

/// 内核高半区起始虚拟地址；System Arena 覆盖整个高半区。
pub const KERNEL_HALF_BASE: u64 = 0xFFFF_8000_0000_0000;

/// 内核高半区长度。
const KERNEL_HALF_SIZE: u64 = 0x0000_8000_0000_0000 - PAGE_SIZE as u64;

/// System Arena 中的一条 Region：虚拟区间、权利与标志。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemRegion {
    id: RegionId,
    virt: u64,
    len: u64,
    rights: Rights,
    flags: RegionFlags,
}

impl SystemRegion {
    /// 返回 Region 起始虚拟地址。
    pub const fn virt(&self) -> u64 {
        self.virt
    }

    /// 返回 Region 字节长度。
    pub const fn len(&self) -> u64 {
        self.len
    }

    /// 判断长度是否为零。
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 返回 Region 权利。
    pub const fn rights(&self) -> Rights {
        self.rights
    }

    /// 返回 Region 标志。
    pub const fn flags(&self) -> RegionFlags {
        self.flags
    }
}

/// System Arena 快照：地址窗口与 Region 统计。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemArenaSnapshot {
    /// Arena 窗口起始虚拟地址。
    pub base: u64,
    /// Arena 窗口长度。
    pub size: u64,
    /// 已登记 Region 的字节数。
    pub used: u64,
    /// Region 数。
    pub regions: u64,
}

struct SystemArenaState {
    arena: Arena,
    regions: Vec<SystemRegion>,
}

static STATE: Mutex<Option<SystemArenaState>> = Mutex::new(None);

/// 建立 System Arena；重复调用时返回 `false`。
pub fn establish() -> bool {
    let Ok(arena) = Arena::create(VirtAddr::new(KERNEL_HALF_BASE), KERNEL_HALF_SIZE) else {
        return false;
    };
    let mut state = STATE.lock();
    if state.is_some() {
        return false;
    }
    *state = Some(SystemArenaState {
        arena,
        regions: Vec::new(),
    });
    true
}

/// 判断 System Arena 是否已建立。
pub fn is_established() -> bool {
    STATE.lock().is_some()
}

/// 把一段已物化的内核映射登记为 Region；区间重叠、越界或违反 W^X 时返回 `false`。
pub fn register(virt: u64, len: u64, rights: Rights, flags: RegionFlags) -> bool {
    if len == 0 || !virt.is_multiple_of(PAGE_SIZE as u64) || !len.is_multiple_of(PAGE_SIZE as u64) {
        return false;
    }
    let kind = if rights.contains(Rights::EXECUTE) {
        CapabilityKind::TaskImage
    } else {
        CapabilityKind::Anonymous
    };
    let Ok(capability) = Capability::new(kind, len, PAGE_SIZE, None) else {
        return false;
    };
    let Ok(mut region) = Region::new(&capability, 0, len as usize, rights, flags) else {
        return false;
    };
    if region.map(VirtAddr::new(virt)).is_err() {
        return false;
    }
    let mut state = STATE.lock();
    let Some(state) = state.as_mut() else {
        return false;
    };
    if state.arena.insert(&region).is_err() {
        return false;
    }
    state.regions.push(SystemRegion {
        id: region.id(),
        virt,
        len,
        rights,
        flags,
    });
    true
}

/// 移除起始地址与长度完全匹配的 Region；不存在时返回 `false`。
///
/// 内核自用页归还 FrameBank 时同步移除对应登记。
pub fn unregister(virt: u64, len: u64) -> bool {
    let mut state = STATE.lock();
    let Some(state) = state.as_mut() else {
        return false;
    };
    let Some(index) = state
        .regions
        .iter()
        .position(|region| region.virt == virt && region.len == len)
    else {
        return false;
    };
    let region = state.regions.remove(index);
    state.arena.remove(region.id).is_ok()
}

/// 返回 System Arena 快照；未建立时返回 `None`。
pub fn snapshot() -> Option<SystemArenaSnapshot> {
    let state = STATE.lock();
    let state = state.as_ref()?;
    Some(SystemArenaSnapshot {
        base: state.arena.base().get(),
        size: state.arena.size(),
        used: state.arena.used(),
        regions: state.arena.regions().len() as u64,
    })
}

/// 按索引返回 Region 记录；越界或未建立时返回 `None`。
pub fn region(index: usize) -> Option<SystemRegion> {
    let state = STATE.lock();
    let state = state.as_ref()?;
    state.regions.get(index).copied()
}

/// 把内核镜像登记为 Region。
///
/// `virt` 是镜像的虚拟基址（链接地址），`image` 是物理区间；长度向上取整
/// 到页。镜像可执行且常驻，带 `SEALED | PINNED` 标志。
pub fn register_image(virt: u64, image: PhysRange) -> bool {
    let len = image.len().div_ceil(PAGE_SIZE as u64) * PAGE_SIZE as u64;
    register(
        virt,
        len,
        Rights::READ | Rights::EXECUTE,
        RegionFlags::SEALED | RegionFlags::PINNED,
    )
}

/// 把 direct map 中不与既有 Region 重叠的空洞登记为可写 Region。
///
/// direct map 覆盖全部物理内存且同时可写可执行；W^X 契约下它不能整体成为
/// 一个 Region。内核镜像段已按可执行登记，这里只补上其余部分的数据视图。
pub fn register_direct_map(virt: u64, len: u64) -> usize {
    let Some(end) = virt.checked_add(len) else {
        return 0;
    };
    let existing: Vec<(u64, u64)> = {
        let state = STATE.lock();
        let Some(state) = state.as_ref() else {
            return 0;
        };
        state
            .regions
            .iter()
            .filter(|region| region.virt < end && virt < region.virt + region.len)
            .map(|region| (region.virt, region.len))
            .collect()
    };
    let mut holes: Vec<(u64, u64)> = Vec::new();
    let mut cursor = virt;
    for (start, region_len) in existing {
        if start > cursor {
            holes.push((cursor, start - cursor));
        }
        cursor = cursor.max(start + region_len);
    }
    if cursor < end {
        holes.push((cursor, end - cursor));
    }
    let mut count = 0;
    for (start, hole_len) in holes {
        if register(
            start,
            hole_len,
            Rights::READ | Rights::WRITE,
            RegionFlags::PINNED,
        ) {
            count += 1;
        }
    }
    count
}

/// 把 FrameBank 中归属 System Capacity 的内核自用分配登记为 Region。
///
/// 内核堆、页表页与 IOMMU 表都经 direct map 访问，虚拟地址为
/// `direct_map_base + 物理起始`。用户地址空间的页（镜像/COW/恢复）不属于
/// 内核高半区，跳过。
pub fn register_bank_allocations(bank: &FrameBank, direct_map_base: u64) -> usize {
    let Some(capacity) = system::capacity() else {
        return 0;
    };
    let owner = capacity.lock().id();
    let mut count = 0;
    for (range, kind) in bank.allocations_of(Some(owner)) {
        if kind.is_user_space() {
            continue;
        }
        let virt = direct_map_base.saturating_add(range.start().get());
        if register(
            virt,
            range.len(),
            Rights::READ | Rights::WRITE,
            RegionFlags::PINNED,
        ) {
            count += 1;
        }
    }
    count
}

/// 把内核页表高半区中尚未登记的叶子映射登记为 Region。
///
/// 相邻同属性页合并为一段；MMIO（PWT/PCD）页带 `GUARD` 标志。已有 Region
/// 覆盖的页跳过，重叠冲突的段也跳过。返回新登记的 Region 数。
pub fn register_kernel_mappings(table: &crate::paging::PageTable) -> usize {
    let mut runs: Vec<(u64, u64, u64)> = Vec::new();
    table.kernel_half_regions(|virt, _phys, len, flags| {
        if let Some(last) = runs.last_mut()
            && last.0 + last.1 == virt
            && last.2 == flags
        {
            last.1 += len;
            return;
        }
        runs.push((virt, len, flags));
    });
    let mut count = 0;
    for (virt, len, flags) in runs {
        let mut rights = Rights::READ;
        if flags & WRITABLE != 0 {
            rights |= Rights::WRITE;
        }
        if flags & NX == 0 {
            rights |= Rights::EXECUTE;
        }
        // W^X：同时可写可执行的段按可写数据视图登记。
        if rights.contains(Rights::WRITE) {
            rights.remove(Rights::EXECUTE);
        }
        let mut region_flags = RegionFlags::PINNED;
        if flags & (PWT | PCD) != 0 {
            region_flags |= RegionFlags::GUARD;
        }
        if register(virt, len, rights, region_flags) {
            count += 1;
        }
    }
    count
}

/// 把 per-CPU 内核栈窗口登记为 Region；guard 页保持未映射，不登记。
pub fn register_stack(virt: u64, len: u64) -> bool {
    register(virt, len, Rights::READ | Rights::WRITE, RegionFlags::PINNED)
}

/// 把一段新分配的内核自用物理内存登记为 Region（经 direct map 访问）。
///
/// 页表页与 IOMMU 表在分配时调用；System Arena 未建立时跳过。
pub fn register_kernel_range(phys: PhysAddr, len: u64, direct_map_base: u64) {
    if !is_established() {
        return;
    }
    let _ = register(
        direct_map_base.saturating_add(phys.get()),
        len,
        Rights::READ | Rights::WRITE,
        RegionFlags::PINNED,
    );
}

/// 把一页新分配的内核自用物理页登记为 Region（经 direct map 访问）。
pub fn register_kernel_page(phys: PhysAddr, direct_map_base: u64) {
    register_kernel_range(phys, PAGE_SIZE as u64, direct_map_base);
}

/// 移除一段经 direct map 访问的内核自用物理内存的 Region 登记。
pub fn unregister_kernel_range(phys: PhysAddr, len: u64, direct_map_base: u64) {
    if !is_established() {
        return;
    }
    let _ = unregister(direct_map_base.saturating_add(phys.get()), len);
}

/// 移除一页经 direct map 访问的内核自用物理页的 Region 登记。
pub fn unregister_kernel_page(phys: PhysAddr, direct_map_base: u64) {
    unregister_kernel_range(phys, PAGE_SIZE as u64, direct_map_base);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个测试使用独立的地址段，避免全局状态相互干扰。
    fn segment(index: u64) -> u64 {
        KERNEL_HALF_BASE + index * 0x100_0000
    }

    /// 宿主测试并行运行；首个测试建立 Arena，其余复用。
    fn ensure_established() {
        if !is_established() {
            assert!(establish());
        }
    }

    fn regions_containing(virt: u64) -> Vec<SystemRegion> {
        let state = STATE.lock();
        state
            .as_ref()
            .map(|state| {
                state
                    .regions
                    .iter()
                    .filter(|region| region.virt <= virt && virt < region.virt + region.len)
                    .copied()
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn registers_and_enumerates_kernel_regions() {
        ensure_established();
        let base = segment(1);
        assert!(register(
            base,
            0x2000,
            Rights::READ | Rights::WRITE,
            RegionFlags::PINNED,
        ));
        assert!(register(
            base + 0x2000,
            0x1000,
            Rights::READ | Rights::EXECUTE,
            RegionFlags::SEALED | RegionFlags::PINNED,
        ));
        let snapshot = snapshot().expect("快照");
        assert_eq!(snapshot.base, KERNEL_HALF_BASE);
        assert_eq!(snapshot.size, KERNEL_HALF_SIZE);
        assert!(snapshot.used >= 0x3000);
        let executable = regions_containing(base + 0x2000);
        assert_eq!(executable.len(), 1);
        assert_eq!(executable[0].rights(), Rights::READ | Rights::EXECUTE);
        assert!(executable[0].flags().contains(RegionFlags::SEALED));
    }

    #[test]
    fn rejects_overlap_and_unaligned_regions() {
        ensure_established();
        let base = segment(2);
        assert!(register(base, 0x1000, Rights::READ, RegionFlags::PINNED));
        assert!(!register(base, 0x1000, Rights::READ, RegionFlags::PINNED));
        assert!(!register(
            base + 0x10_0000 + 0x100,
            0x1000,
            Rights::READ,
            RegionFlags::PINNED,
        ));
        assert!(!register(
            base + 0x20_0000,
            0x1001,
            Rights::READ,
            RegionFlags::PINNED,
        ));
        // W^X：可写可执行的组合被拒绝。
        assert!(!register(
            base + 0x30_0000,
            0x1000,
            Rights::READ | Rights::WRITE | Rights::EXECUTE,
            RegionFlags::PINNED,
        ));
    }

    #[test]
    fn unregisters_returned_pages() {
        ensure_established();
        let virt = segment(3);
        assert!(register(
            virt,
            PAGE_SIZE as u64,
            Rights::READ | Rights::WRITE,
            RegionFlags::PINNED,
        ));
        assert_eq!(regions_containing(virt).len(), 1);
        assert!(unregister(virt, PAGE_SIZE as u64));
        assert!(regions_containing(virt).is_empty());
        assert!(!unregister(virt, PAGE_SIZE as u64));
    }

    #[test]
    fn direct_map_fills_holes_around_image() {
        ensure_established();
        let base = segment(4);
        assert!(register_image(
            base + 0x1000,
            PhysRange::new(PhysAddr::new(0x20_0000), 0x2000),
        ));
        let count = register_direct_map(base, 0x8000);
        assert_eq!(count, 2);
        let before = regions_containing(base);
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].len(), 0x1000);
        assert_eq!(before[0].rights(), Rights::READ | Rights::WRITE);
        let image = regions_containing(base + 0x1000);
        assert_eq!(image.len(), 1);
        assert_eq!(image[0].rights(), Rights::READ | Rights::EXECUTE);
        let after = regions_containing(base + 0x3000);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].len(), 0x5000);
    }
}
