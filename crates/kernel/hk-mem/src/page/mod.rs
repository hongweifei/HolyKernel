//! 早期物理页分配、页表页池与用户地址空间登记。

pub mod frame_bank;
pub mod registry;

pub use frame_bank::{FrameBank, FrameBankStats, FrameKind};
pub use registry::{UserVspace, VspaceRegistry};
