//! 内存管理单元与地址空间映射。

use core::fmt;

use crate::addr::{PhysAddr, VirtAddr};

bitflags::bitflags! {
    /// 映射属性。
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct MapFlags: u32 {
        /// 可读。
        const READ = 1 << 0;
        /// 可写。
        const WRITE = 1 << 1;
        /// 可执行。
        const EXECUTE = 1 << 2;
        /// 设备内存，使用非缓存属性。
        const DEVICE = 1 << 3;
        /// 保护页，不映射物理内存。
        const GUARD = 1 << 4;
        /// 与其他地址空间共享。
        const SHARED = 1 << 5;
    }
}

impl MapFlags {
    /// 判断是否同时要求可写与可执行。
    ///
    /// W^X 要求可写与可执行互斥，映射前必须检查该条件。
    pub const fn violates_wx(&self) -> bool {
        self.bits() & Self::WRITE.bits() != 0 && self.bits() & Self::EXECUTE.bits() != 0
    }
}

/// 地址空间标识，由平台分配。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VSpaceId(u64);

impl VSpaceId {
    /// 构造地址空间标识。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 返回标识数值。
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// 映射操作错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmuError {
    /// 请求同时可写与可执行，违反 W^X。
    WxConflict,
    /// 地址或长度未按页对齐。
    Unaligned,
    /// 目标虚拟地址已有映射。
    AlreadyMapped,
    /// 目标虚拟地址没有映射。
    NotMapped,
    /// 地址空间标识无效。
    InvalidAddressSpace,
    /// 页表内存不足。
    OutOfMemory,
}

impl fmt::Display for MmuError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WxConflict => formatter.write_str("映射同时要求可写与可执行"),
            Self::Unaligned => formatter.write_str("地址或长度未按页对齐"),
            Self::AlreadyMapped => formatter.write_str("虚拟地址已有映射"),
            Self::NotMapped => formatter.write_str("虚拟地址没有映射"),
            Self::InvalidAddressSpace => formatter.write_str("地址空间标识无效"),
            Self::OutOfMemory => formatter.write_str("页表内存不足"),
        }
    }
}

impl core::error::Error for MmuError {}

/// 内存管理单元。
///
/// 实现方负责页表布局、TLB 维护与 SMP 下的 TLB 同步；调用方负责按页对齐
/// 传入地址，并通过 [`Mmu::shootdown`] 通知其他 CPU。
pub trait Mmu: Send + Sync {
    /// 返回页大小，单位为字节。
    fn page_size(&self) -> usize;

    /// 创建新的地址空间。
    fn create_vspace(&self) -> VSpaceId;

    /// 销毁地址空间以及其中全部映射。
    fn destroy_vspace(&self, vspace: VSpaceId);

    /// 把物理区间映射到虚拟地址。
    fn map(
        &self,
        vspace: VSpaceId,
        frame: PhysAddr,
        virt: VirtAddr,
        len: usize,
        flags: MapFlags,
    ) -> Result<(), MmuError>;

    /// 解除虚拟地址的映射，返回原物理地址。
    fn unmap(&self, vspace: VSpaceId, virt: VirtAddr, len: usize) -> Result<PhysAddr, MmuError>;

    /// 修改已有映射的属性。
    fn protect(
        &self,
        vspace: VSpaceId,
        virt: VirtAddr,
        len: usize,
        flags: MapFlags,
    ) -> Result<(), MmuError>;

    /// 查询虚拟地址对应的物理地址与属性。
    fn translate(&self, vspace: VSpaceId, virt: VirtAddr) -> Option<(PhysAddr, MapFlags)>;

    /// 刷新地址空间在全部 CPU 上的 TLB 条目。
    fn shootdown(&self, vspace: VSpaceId);
}
