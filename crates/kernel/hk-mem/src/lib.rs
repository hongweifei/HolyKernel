//! HMP 对象、物理容量与平台内存基础设施。
//!
//! HMP 对象模型位于 [`hmp`]；页分配与用户地址空间登记位于 [`page`]；
//! DMA 池与软件 IOMMU 位于 [`platform`]。

#![no_std]

extern crate alloc;

pub mod hmp;
pub mod page;
pub mod platform;

pub use page::{FrameBank, FrameBankStats, FrameKind, UserVspace, VspaceRegistry};
pub use platform::{DmaPool, SoftwareIommu};
