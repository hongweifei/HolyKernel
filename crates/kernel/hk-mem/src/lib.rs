//! HMP 对象、物理容量与平台内存基础设施。
//!
//! HMP 对象模型位于 [`hmp`]；页分配与用户地址空间登记位于 [`page`]；
//! DMA 分配器与软件 IOMMU 位于 [`platform`]。

#![no_std]

extern crate alloc;

pub mod hmp;
pub mod page;
pub mod platform;

pub use page::{
    AddressSpaceRegistry, FrameBank, FrameBankStats, FrameKind, LedgerStats, PageAllocator,
    UserAddressSpace,
};
pub use platform::{DmaAllocator, SoftwareIommu};

/// HMP 使用的句柄权利类型；映射路径按它决定页表写权限。
pub use hk_handle::Rights;
