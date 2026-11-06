//! 物理容量、HMP 对象与用户地址空间基础设施。
//!
//! HMP 对象模型位于 [`hmp`]；`Capacity` 管理物理容量，`backing` 承载
//! 类型化能力的常驻物理页，`frames`/`frame_pool`/`vspace_registry` 提供页表与
//! 用户地址空间基础设施。

#![no_std]

extern crate alloc;

pub mod backing;
pub mod dma;
pub mod frame_pool;
pub mod frames;
pub mod hmp;
pub mod iommu;
pub mod vspace_registry;

pub use backing::{Backing, set_backing_releaser};
pub use dma::DmaPool;
pub use frame_pool::FramePool;
pub use frames::LinearFrames;
pub use iommu::SoftwareIommu;
pub use vspace_registry::{UserVspace, VspaceRegistry};
