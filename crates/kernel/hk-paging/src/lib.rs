//! 通用分页契约。
//!
//! 架构后端只负责页表编码、地址翻译与 TLB；COW 的共享状态由本 crate 的
//! [`FrameRefs`] 统一维护，不依赖任何架构专用 PTE 位。

#![no_std]

extern crate alloc;

pub mod cow;
pub mod space;

pub use cow::FrameRefs;
pub use space::UserSpace;
