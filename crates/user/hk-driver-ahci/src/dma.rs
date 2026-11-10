//! DMA 布局与 Frame 映射表。

use alloc::vec::Vec;

use hk_server_api::{
    CapabilityKind, capability_create, capability_phys, close_handle, dma_map, map_attach,
};

/// FIS 接收区偏移。
pub const FIS_OFFSET: usize = 0x400;
/// 命令表区偏移。
pub const TABLE_OFFSET: usize = 0x800;
/// 每个命令槽的命令表大小。
pub const SLOT_BYTES: usize = 512;
/// 命令槽数量。
pub const SLOTS: usize = 32;
/// IDENTIFY 数据区偏移。
pub const DATA_OFFSET: usize = 0x5000;
/// IDENTIFY 数据区大小。
pub const DATA_BYTES: usize = 0x1000;
/// DMA 区总大小。
pub const DMA_BYTES: usize = DATA_OFFSET + DATA_BYTES;

/// 从 DeviceGrant 分配并映射的连续 DMA 区。
pub struct DmaArena {
    virt: u64,
    phys: u64,
    _capability: u64,
    _region: u64,
}

impl DmaArena {
    /// 分配并登记到设备 IOMMU 域。
    pub fn allocate(grant: u64, bytes: usize) -> Option<Self> {
        let capability = capability_create(CapabilityKind::DeviceAllocation, bytes)?;
        let (region, virt) = map_attach(capability)?;
        let (phys, length) = capability_phys(capability)?;
        if length < bytes as u64 || dma_map(grant, capability, bytes) == u64::MAX {
            let _ = close_handle(capability);
            return None;
        }
        // SAFETY: map_attach 返回当前任务可写的连续能力视图。
        unsafe {
            core::ptr::write_bytes(virt as *mut u8, 0, bytes);
        }
        Some(Self {
            virt,
            phys,
            _capability: capability,
            _region: region,
        })
    }

    /// 命令列表虚拟地址。
    pub fn command_list_virt(&self) -> u64 {
        self.virt
    }

    /// 命令列表物理地址。
    pub fn command_list_phys(&self) -> u64 {
        self.phys
    }

    /// FIS 接收区物理地址。
    pub fn fis_phys(&self) -> u64 {
        self.phys + FIS_OFFSET as u64
    }

    /// 指定命令槽的命令表虚拟地址。
    pub fn table_virt(&self, slot: usize) -> u64 {
        self.virt + TABLE_OFFSET as u64 + (slot * SLOT_BYTES) as u64
    }

    /// 指定命令槽的命令表物理地址。
    pub fn table_phys(&self, slot: usize) -> u64 {
        self.phys + TABLE_OFFSET as u64 + (slot * SLOT_BYTES) as u64
    }

    /// IDENTIFY 数据虚拟地址。
    pub fn data_virt(&self) -> u64 {
        self.virt + DATA_OFFSET as u64
    }

    /// IDENTIFY 数据物理地址。
    pub fn data_phys(&self) -> u64 {
        self.phys + DATA_OFFSET as u64
    }
}

struct Mapping {
    virt: u64,
    phys: u64,
    len: usize,
}

static MAPPINGS: spin::Mutex<Vec<Mapping>> = spin::Mutex::new(Vec::new());

/// 登记 Block 服务映射的 Frame，供 PRDT 直接引用。
pub fn register_mapping(virt: u64, phys: u64, len: usize) {
    if len == 0 {
        return;
    }
    let mut mappings = MAPPINGS.lock();
    if let Some(entry) = mappings.iter_mut().find(|entry| entry.virt == virt) {
        entry.phys = phys;
        entry.len = len;
    } else {
        mappings.push(Mapping { virt, phys, len });
    }
}

/// 查找虚拟区间对应的物理地址。
pub fn translate(virt: u64, len: usize) -> Option<u64> {
    let end = virt.checked_add(len as u64)?;
    let mappings = MAPPINGS.lock();
    mappings.iter().find_map(|mapping| {
        let map_end = mapping.virt.checked_add(mapping.len as u64)?;
        (virt >= mapping.virt && end <= map_end).then_some(mapping.phys + (virt - mapping.virt))
    })
}
